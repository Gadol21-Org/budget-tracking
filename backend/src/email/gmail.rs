//! The Gmail and Google OAuth calls, over plain REST.
//!
//! Every call that lists mail takes the label id, so there is no function here that reads the
//! mailbox as a whole.

use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use serde::Deserialize;
use serde_json::{json, Value};

use super::Config;

/// The only scope asked for: read mail (Gmail has nothing narrower that includes bodies).
pub const SCOPE: &str = "https://www.googleapis.com/auth/gmail.readonly";

/// 24 random bytes from the OS, hex encoded.
pub fn random_token() -> String {
    let mut b = [0u8; 24];
    getrandom::fill(&mut b).expect("the OS random number generator");
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn consent_url(cfg: &Config, state: &str) -> Result<String> {
    let mut u = reqwest::Url::parse(&cfg.auth_url)?;
    u.query_pairs_mut()
        .append_pair("client_id", &cfg.client_id)
        .append_pair("redirect_uri", &cfg.redirect_uri)
        .append_pair("response_type", "code")
        .append_pair("scope", SCOPE)
        // A refresh token is only given with offline access, and again only with a fresh consent.
        .append_pair("access_type", "offline")
        .append_pair("prompt", "consent")
        .append_pair("include_granted_scopes", "false")
        .append_pair("state", state);
    Ok(u.to_string())
}

#[derive(Deserialize, Debug)]
pub struct Tokens {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default = "one_hour")]
    pub expires_in: u64,
}

fn one_hour() -> u64 {
    3600
}

async fn token_request(
    http: &reqwest::Client,
    cfg: &Config,
    form: &[(&str, &str)],
) -> Result<Tokens> {
    let resp = http
        .post(format!("{}/token", cfg.oauth_base))
        .form(form)
        .send()
        .await?;
    let status = resp.status();
    let body = resp.text().await?;
    if !status.is_success() {
        let v: Value = serde_json::from_str(&body).unwrap_or_default();
        let code = v["error"].as_str().unwrap_or("error");
        if code == "invalid_grant" {
            bail!("Google no longer accepts the saved sign-in (revoked, expired or password changed): connect Gmail again");
        }
        bail!(
            "Google sign-in failed: {code} {}",
            v["error_description"].as_str().unwrap_or("")
        );
    }
    serde_json::from_str(&body).context("unexpected answer from Google's token endpoint")
}

pub async fn exchange_code(http: &reqwest::Client, cfg: &Config, code: &str) -> Result<Tokens> {
    token_request(
        http,
        cfg,
        &[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("client_id", &cfg.client_id),
            ("client_secret", &cfg.client_secret),
            ("redirect_uri", &cfg.redirect_uri),
        ],
    )
    .await
}

pub async fn refresh(http: &reqwest::Client, cfg: &Config, refresh_token: &str) -> Result<Tokens> {
    token_request(
        http,
        cfg,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", &cfg.client_id),
            ("client_secret", &cfg.client_secret),
        ],
    )
    .await
}

pub async fn revoke(http: &reqwest::Client, cfg: &Config, token: &str) -> Result<()> {
    http.post(format!("{}/revoke", cfg.oauth_base))
        .form(&[("token", token)])
        .send()
        .await?;
    Ok(())
}

pub struct Profile {
    pub email: String,
    pub history_id: String,
}

pub struct Label {
    pub id: String,
    pub name: String,
}

pub struct Watch {
    pub history_id: String,
    /// Unix ms, as text.
    pub expiration: String,
}

pub enum History {
    /// Ids of messages that were added to the label or got it, and where to continue from.
    Changes {
        ids: Vec<String>,
        history_id: String,
    },
    /// Gmail no longer has records from that point (HTTP 404): start over.
    Expired,
}

/// A message, reduced to what the parser needs.
#[derive(Debug, Clone)]
pub struct Mail {
    pub id: String,
    pub label_ids: Vec<String>,
    pub from: String,
    pub subject: String,
    /// Day it arrived (YYYY-MM-DD, UTC).
    pub received: String,
    /// Plain text of the body (HTML is stripped), cut to a size the AI call can take.
    pub text: String,
}

pub struct Client<'a> {
    http: &'a reqwest::Client,
    base: &'a str,
    token: &'a str,
}

impl<'a> Client<'a> {
    pub fn new(http: &'a reqwest::Client, cfg: &'a Config, token: &'a str) -> Self {
        Client {
            http,
            base: &cfg.gmail_base,
            token,
        }
    }

    async fn call(&self, req: reqwest::RequestBuilder) -> Result<(reqwest::StatusCode, Value)> {
        let resp = req.bearer_auth(self.token).send().await?;
        let status = resp.status();
        let text = resp.text().await?;
        let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if !status.is_success() && status != reqwest::StatusCode::NOT_FOUND {
            let msg = v["error"]["message"].as_str().unwrap_or(&text);
            bail!("Gmail answered {status}: {}", super::truncate(msg, 200));
        }
        Ok((status, v))
    }

    fn url(&self, path: &str) -> String {
        format!("{}/gmail/v1/users/me/{path}", self.base)
    }

    pub async fn profile(&self) -> Result<Profile> {
        let (_, v) = self.call(self.http.get(self.url("profile"))).await?;
        Ok(Profile {
            email: v["emailAddress"]
                .as_str()
                .ok_or_else(|| anyhow!("no address in the Gmail profile"))?
                .into(),
            history_id: text_of(&v["historyId"])
                .ok_or_else(|| anyhow!("no historyId in the Gmail profile"))?,
        })
    }

    pub async fn find_label(&self, name: &str) -> Result<Option<Label>> {
        let (_, v) = self.call(self.http.get(self.url("labels"))).await?;
        Ok(v["labels"].as_array().into_iter().flatten().find_map(|l| {
            let n = l["name"].as_str()?;
            n.eq_ignore_ascii_case(name).then(|| Label {
                id: l["id"].as_str().unwrap_or_default().into(),
                name: n.into(),
            })
        }))
    }

    /// Ask Gmail to notify the topic about changes to mail with this label only.
    pub async fn watch(&self, topic: &str, label_id: &str) -> Result<Watch> {
        let body =
            json!({ "topicName": topic, "labelIds": [label_id], "labelFilterBehavior": "INCLUDE" });
        let (_, v) = self
            .call(self.http.post(self.url("watch")).json(&body))
            .await?;
        Ok(Watch {
            history_id: text_of(&v["historyId"])
                .ok_or_else(|| anyhow!("watch gave no historyId"))?,
            expiration: text_of(&v["expiration"]).unwrap_or_default(),
        })
    }

    pub async fn stop(&self) -> Result<()> {
        self.call(self.http.post(self.url("stop"))).await?;
        Ok(())
    }

    /// Changes since `start` that involve the label.
    pub async fn history(&self, start: &str, label_id: &str) -> Result<History> {
        let mut ids: Vec<String> = Vec::new();
        let mut page: Option<String> = None;
        loop {
            let mut req = self.http.get(self.url("history")).query(&[
                ("startHistoryId", start),
                ("labelId", label_id),
                ("maxResults", "500"),
            ]);
            req = req.query(&[
                ("historyTypes", "messageAdded"),
                ("historyTypes", "labelAdded"),
            ]);
            if let Some(p) = &page {
                req = req.query(&[("pageToken", p.as_str())]);
            }
            let (status, v) = self.call(req).await?;
            if status == reqwest::StatusCode::NOT_FOUND {
                return Ok(History::Expired);
            }
            for h in v["history"].as_array().into_iter().flatten() {
                for key in ["messagesAdded", "labelsAdded"] {
                    for m in h[key].as_array().into_iter().flatten() {
                        if let Some(id) = m["message"]["id"].as_str() {
                            if !ids.iter().any(|i| i == id) {
                                ids.push(id.to_string());
                            }
                        }
                    }
                }
            }
            match v["nextPageToken"].as_str() {
                Some(t) => page = Some(t.to_string()),
                None => {
                    let history_id = text_of(&v["historyId"]).unwrap_or_else(|| start.to_string());
                    return Ok(History::Changes { ids, history_id });
                }
            }
        }
    }

    /// Messages that have the label and arrived in the last `days` days (newest first).
    pub async fn list_label_messages(&self, label_id: &str, days: u32) -> Result<Vec<String>> {
        let q = format!("newer_than:{days}d");
        let mut ids = Vec::new();
        let mut page: Option<String> = None;
        loop {
            let mut req = self.http.get(self.url("messages")).query(&[
                ("labelIds", label_id),
                ("q", q.as_str()),
                ("maxResults", "100"),
            ]);
            if let Some(p) = &page {
                req = req.query(&[("pageToken", p.as_str())]);
            }
            let (_, v) = self.call(req).await?;
            for m in v["messages"].as_array().into_iter().flatten() {
                if let Some(id) = m["id"].as_str() {
                    ids.push(id.to_string());
                }
            }
            match v["nextPageToken"].as_str() {
                Some(t) if ids.len() < 500 => page = Some(t.to_string()),
                _ => return Ok(ids),
            }
        }
    }

    /// The labels of a message, without any of its content. None if it no longer exists.
    pub async fn message_labels(&self, id: &str) -> Result<Option<Vec<String>>> {
        let (status, v) = self
            .call(
                self.http
                    .get(self.url(&format!("messages/{id}")))
                    .query(&[("format", "minimal")]),
            )
            .await?;
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        Ok(Some(
            v["labelIds"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|l| l.as_str().map(String::from))
                .collect(),
        ))
    }

    /// One message in full. None if it no longer exists (deleted since the history said so).
    pub async fn message(&self, id: &str) -> Result<Option<Mail>> {
        let (status, v) = self
            .call(
                self.http
                    .get(self.url(&format!("messages/{id}")))
                    .query(&[("format", "full")]),
            )
            .await?;
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        Ok(Some(mail_from(&v)))
    }
}

fn text_of(v: &Value) -> Option<String> {
    v.as_str()
        .map(String::from)
        .or_else(|| v.as_u64().map(|n| n.to_string()))
}

/// Biggest piece of a body passed on to the AI.
const MAX_TEXT: usize = 12_000;

pub fn mail_from(v: &Value) -> Mail {
    let header = |name: &str| {
        v["payload"]["headers"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|h| {
                h["name"]
                    .as_str()
                    .is_some_and(|n| n.eq_ignore_ascii_case(name))
            })
            .and_then(|h| h["value"].as_str())
            .unwrap_or("")
            .to_string()
    };
    let received = v["internalDate"]
        .as_str()
        .and_then(|s| s.parse::<i64>().ok())
        .and_then(chrono::DateTime::from_timestamp_millis)
        .map(|d| d.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| chrono::Utc::now().format("%Y-%m-%d").to_string());
    let (mut plain, mut html) = (String::new(), String::new());
    collect_text(&v["payload"], &mut plain, &mut html);
    let text = if plain.trim().is_empty() {
        strip_html(&html)
    } else {
        plain
    };
    Mail {
        id: v["id"].as_str().unwrap_or_default().into(),
        label_ids: v["labelIds"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|l| l.as_str().map(String::from))
            .collect(),
        from: header("From"),
        subject: header("Subject"),
        received,
        text: collapse(&text).chars().take(MAX_TEXT).collect(),
    }
}

/// Gather the text/plain and text/html parts (attachments are ignored).
fn collect_text(part: &Value, plain: &mut String, html: &mut String) {
    if part["filename"].as_str().is_some_and(|f| !f.is_empty()) {
        return;
    }
    let mime = part["mimeType"].as_str().unwrap_or("");
    if let Some(data) = part["body"]["data"].as_str() {
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(data.trim_end_matches('='))
            .unwrap_or_default();
        let s = String::from_utf8_lossy(&bytes);
        match mime {
            "text/plain" => {
                plain.push_str(&s);
                plain.push('\n');
            }
            "text/html" => {
                html.push_str(&s);
                html.push('\n');
            }
            _ => {}
        }
    }
    for p in part["parts"].as_array().into_iter().flatten() {
        collect_text(p, plain, html);
    }
}

/// Tags out, style/script blocks out, the common entities decoded.
pub fn strip_html(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 2);
    let lower = html.to_ascii_lowercase();
    let (mut i, bytes) = (0, html.as_bytes());
    while i < bytes.len() {
        if bytes[i] == b'<' {
            // Skip whole style and script elements.
            let skipped = ["style", "script"].iter().find_map(|t| {
                lower[i..].starts_with(&format!("<{t}")).then(|| {
                    lower[i..]
                        .find(&format!("</{t}>"))
                        .map(|e| i + e + t.len() + 3)
                })
            });
            if let Some(Some(end)) = skipped {
                i = end;
                continue;
            }
            match html[i..].find('>') {
                Some(e) => {
                    let tag = &lower[i + 1..i + e];
                    if tag.starts_with("br")
                        || tag.starts_with("/p")
                        || tag.starts_with("/div")
                        || tag.starts_with("/tr")
                        || tag.starts_with("/li")
                    {
                        out.push('\n');
                    } else {
                        out.push(' ');
                    }
                    i += e + 1;
                }
                None => break,
            }
        } else {
            let ch = html[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out.replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

/// Runs of blanks become one space, runs of blank lines one line break.
fn collapse(s: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    for l in s.lines() {
        let l = l.split_whitespace().collect::<Vec<_>>().join(" ");
        if l.is_empty() && lines.last().is_none_or(|p| p.is_empty()) {
            continue;
        }
        lines.push(l);
    }
    lines.join("\n").trim().to_string()
}
