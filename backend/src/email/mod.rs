//! Email to transactions: reads the mail you label in Gmail, asks an AI what transaction each
//! one describes, and puts the answers in a review queue. Nothing becomes a transaction until
//! a person accepts it in the app.
//!
//! How it fits together:
//! * [`gmail`]: the Gmail REST calls (OAuth refresh, `users.watch`, `history.list`, `messages.get`).
//! * [`ai`]: the Gemini call that turns one email into zero or more suggested transactions.
//! * [`push`]: checking the Google Pub/Sub push request that tells us "something changed".
//! * [`Service`] (this file): connection state, the sync that ties them together, and renewal.
//!
//! **What the app can read.** Gmail has no scope limited to one label, so the OAuth grant
//! (`gmail.readonly`) can read the whole mailbox. The limit is in this code: every Gmail call
//! that lists mail passes the label, the watch is created for that label only, and a message
//! is dropped unless it carries the label (see [`Service::fetch_labeled`]). Keep the label
//! on a dedicated account, or on mail a Gmail filter puts there, to keep that exposure small.
//!
//! **Push is only a nudge.** The notification's content is never trusted or used: a valid or a
//! forged push can only make the server run a sync, which reads Gmail itself. It is still
//! authenticated (see [`push`]), and a poll every few minutes covers dropped notifications.

pub mod ai;
pub mod gmail;
pub mod push;
#[cfg(test)]
mod tests;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex as StdMutex;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::api::Db;
use crate::storage::{Category, NewEmailSuggestion};

const STATE_KEY: &str = "gmail_state";
const OAUTH_KEY: &str = "gmail_oauth_state";
/// Messages handled per sync run; the rest wait for the next run (nothing is lost).
const MAX_PER_RUN: usize = 100;
/// How long a message needs to be unseen in a full scan before we give up on old mail.
const SCAN_DAYS_DEFAULT: u32 = 7;
const SCAN_DAYS_MAX: u32 = 90;

/// Settings from the environment. Without a client id and secret the feature is off.
#[derive(Clone, Debug)]
pub struct Config {
    pub client_id: String,
    pub client_secret: String,
    /// Where Google sends the browser after consent: `https://<host>/api/email/gmail/callback`.
    pub redirect_uri: String,
    /// The Gmail label to read (its name, e.g. `Budget`).
    pub label: String,
    /// `projects/<project>/topics/<topic>`; without it no watch is made and polling is used.
    pub topic: Option<String>,
    /// Secret in the push URL (`?token=`).
    pub push_token: Option<String>,
    /// Expected `aud` of the Pub/Sub OIDC token (the push endpoint URL).
    pub push_audience: Option<String>,
    /// Expected `email` of the OIDC token (the service account Pub/Sub signs as).
    pub push_service_account: Option<String>,
    pub ai_key: Option<String>,
    pub model: String,
    pub poll_minutes: u64,
    // Endpoints, overridden in tests.
    pub gmail_base: String,
    pub oauth_base: String,
    pub auth_url: String,
    pub ai_base: String,
    pub jwks_url: String,
}

impl Config {
    pub fn from_env() -> Option<Config> {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        Some(Config {
            client_id: var("GMAIL_CLIENT_ID")?,
            client_secret: var("GMAIL_CLIENT_SECRET")?,
            redirect_uri: var("GMAIL_REDIRECT_URI")?,
            label: var("GMAIL_LABEL").unwrap_or_else(|| "Budget".into()),
            topic: var("GMAIL_PUBSUB_TOPIC"),
            push_token: var("GMAIL_PUSH_TOKEN"),
            push_audience: var("GMAIL_PUSH_AUDIENCE"),
            push_service_account: var("GMAIL_PUSH_SERVICE_ACCOUNT"),
            ai_key: var("GEMINI_API_KEY"),
            model: var("EMAIL_AI_MODEL").unwrap_or_else(|| "gemini-3.5-flash-lite".into()),
            poll_minutes: var("GMAIL_POLL_MINUTES")
                .and_then(|m| m.parse().ok())
                .unwrap_or(24 * 60),
            gmail_base: var("GMAIL_API_BASE")
                .unwrap_or_else(|| "https://gmail.googleapis.com".into()),
            oauth_base: var("GOOGLE_OAUTH_BASE")
                .unwrap_or_else(|| "https://oauth2.googleapis.com".into()),
            auth_url: var("GOOGLE_AUTH_URL")
                .unwrap_or_else(|| "https://accounts.google.com/o/oauth2/v2/auth".into()),
            ai_base: var("GEMINI_BASE_URL")
                .unwrap_or_else(|| "https://generativelanguage.googleapis.com".into()),
            jwks_url: var("GOOGLE_JWKS_URL")
                .unwrap_or_else(|| "https://www.googleapis.com/oauth2/v3/certs".into()),
        })
    }
}

/// What is remembered about the connection (the `gmail_state` setting).
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Conn {
    pub email: String,
    pub refresh_token: String,
    pub label_id: String,
    pub label_name: String,
    /// Where the next `history.list` starts.
    pub history_id: Option<String>,
    /// When the Pub/Sub watch ends (Unix ms).
    pub watch_expiration: Option<i64>,
    pub connected_at: String,
    pub last_sync: Option<String>,
    pub last_error: Option<String>,
}

/// What the settings screen shows. Never contains the token.
#[derive(Serialize, Debug, Default)]
pub struct Status {
    /// The server has the Google client settings.
    pub configured: bool,
    pub ai_configured: bool,
    pub connected: bool,
    pub email: String,
    pub label: String,
    pub push_enabled: bool,
    pub watch_expiration: Option<String>,
    pub last_sync: Option<String>,
    pub last_error: Option<String>,
    pub pending: usize,
}

#[derive(Serialize, Debug, Default, PartialEq)]
pub struct SyncSummary {
    /// Messages with the label that were new to us.
    pub messages: usize,
    /// Suggested transactions added to the review queue.
    pub suggestions: usize,
    /// Messages that described no transaction.
    pub ignored: usize,
    /// Messages the AI could not read (shown in the queue as failed).
    pub failed: usize,
}

pub struct Service {
    db: Db,
    pub cfg: Option<Config>,
    http: reqwest::Client,
    /// One sync at a time.
    running: Mutex<()>,
    /// A push arrived while a sync was running: run once more.
    rerun: AtomicBool,
    access: StdMutex<Option<(String, Instant)>>,
    jwks: StdMutex<Option<(push::Jwks, Instant)>>,
}

impl Service {
    pub fn new(db: Db, cfg: Option<Config>) -> std::sync::Arc<Service> {
        std::sync::Arc::new(Service {
            db,
            cfg,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(60))
                .build()
                .expect("http client"),
            running: Mutex::new(()),
            rerun: AtomicBool::new(false),
            access: StdMutex::new(None),
            jwks: StdMutex::new(None),
        })
    }

    pub fn from_env(db: Db) -> std::sync::Arc<Service> {
        Service::new(db, Config::from_env())
    }

    pub(crate) fn cfg(&self) -> Result<&Config> {
        self.cfg
            .as_ref()
            .ok_or_else(|| anyhow!("Gmail is not configured on the server"))
    }

    // ---- State ------------------------------------------------------------------------------

    pub async fn conn(&self) -> Result<Option<Conn>> {
        Ok(match self.db.get_setting(STATE_KEY).await? {
            Some(s) => serde_json::from_str::<Option<Conn>>(&s)
                .context("stored Gmail state is unreadable")?,
            None => None,
        })
    }

    async fn save(&self, c: &Conn) -> Result<()> {
        self.db
            .set_setting(STATE_KEY, &serde_json::to_string(c)?)
            .await
    }

    async fn note(&self, error: Option<String>, synced: bool) {
        if let Ok(Some(mut c)) = self.conn().await {
            c.last_error = error;
            if synced {
                c.last_sync = Some(Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string());
            }
            let _ = self.save(&c).await;
        }
    }

    pub async fn status(&self) -> Result<Status> {
        let pending = self.db.list_email_suggestions(Some("pending")).await?.len();
        let c = self.conn().await?;
        let cfg = self.cfg.as_ref();
        Ok(Status {
            configured: cfg.is_some(),
            ai_configured: cfg.is_some_and(|c| c.ai_key.is_some()),
            connected: c.is_some(),
            email: c.as_ref().map(|c| c.email.clone()).unwrap_or_default(),
            label: c
                .as_ref()
                .map(|c| c.label_name.clone())
                .or_else(|| cfg.map(|c| c.label.clone()))
                .unwrap_or_default(),
            push_enabled: cfg.is_some_and(|c| c.topic.is_some()),
            watch_expiration: c
                .as_ref()
                .and_then(|c| c.watch_expiration)
                .and_then(chrono::DateTime::from_timestamp_millis)
                .map(|d| d.format("%Y-%m-%dT%H:%M:%SZ").to_string()),
            last_sync: c.as_ref().and_then(|c| c.last_sync.clone()),
            last_error: c.and_then(|c| c.last_error),
            pending,
        })
    }

    // ---- Connecting -------------------------------------------------------------------------

    /// The Google consent URL. `state` is remembered so the callback can be checked.
    pub async fn consent_url(&self) -> Result<String> {
        let cfg = self.cfg()?;
        let nonce = gmail::random_token();
        let expires = Utc::now().timestamp() + 600;
        self.db
            .set_setting(OAUTH_KEY, &format!("{nonce}:{expires}"))
            .await?;
        gmail::consent_url(cfg, &nonce)
    }

    /// Finish the consent: swap the code for a refresh token, find the label, start the watch.
    pub async fn finish_connect(&self, code: &str, state: &str) -> Result<()> {
        let cfg = self.cfg()?;
        let saved = self.db.get_setting(OAUTH_KEY).await?.unwrap_or_default();
        // One use only.
        self.db.set_setting(OAUTH_KEY, "").await?;
        let (nonce, expires) = saved.split_once(':').unwrap_or(("", "0"));
        let fresh = expires.parse::<i64>().unwrap_or(0) > Utc::now().timestamp();
        if nonce.is_empty() || !fresh || !push::same(nonce, state) {
            return Err(anyhow!(
                "the sign-in link expired or was not started here; start again"
            ));
        }
        let tokens = gmail::exchange_code(&self.http, cfg, code).await?;
        let refresh = tokens
            .refresh_token
            .ok_or_else(|| anyhow!("Google gave no refresh token; remove the app at myaccount.google.com/permissions and connect again"))?;
        let g = gmail::Client::new(&self.http, cfg, &tokens.access_token);
        let profile = g.profile().await?;
        let label = g
            .find_label(&cfg.label)
            .await?
            .ok_or_else(|| anyhow!("the Gmail account {} has no label named \"{}\"; create it (and the filter that applies it) and connect again", profile.email, cfg.label))?;
        let mut conn = Conn {
            email: profile.email,
            refresh_token: refresh,
            label_id: label.id,
            label_name: label.name,
            history_id: Some(profile.history_id),
            watch_expiration: None,
            connected_at: Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            last_sync: None,
            last_error: None,
        };
        if let Some(topic) = &cfg.topic {
            let w = g.watch(topic, &conn.label_id).await?;
            conn.history_id = Some(w.history_id);
            conn.watch_expiration = w.expiration.parse().ok();
        }
        *self.access.lock().unwrap() = Some((
            tokens.access_token,
            Instant::now() + Duration::from_secs(tokens.expires_in.saturating_sub(60)),
        ));
        self.save(&conn).await
    }

    /// Stop the watch, revoke the token at Google and forget the connection. Suggestions stay.
    pub async fn disconnect(&self) -> Result<()> {
        let Some(conn) = self.conn().await? else {
            return Ok(());
        };
        if let Ok(cfg) = self.cfg() {
            if let Ok(token) = self.access_token(&conn).await {
                let _ = gmail::Client::new(&self.http, cfg, &token).stop().await;
            }
            let _ = gmail::revoke(&self.http, cfg, &conn.refresh_token).await;
        }
        *self.access.lock().unwrap() = None;
        self.db.set_setting(STATE_KEY, "null").await?;
        Ok(())
    }

    async fn access_token(&self, conn: &Conn) -> Result<String> {
        if let Some((t, until)) = &*self.access.lock().unwrap() {
            if *until > Instant::now() {
                return Ok(t.clone());
            }
        }
        let cfg = self.cfg()?;
        let t = gmail::refresh(&self.http, cfg, &conn.refresh_token).await?;
        *self.access.lock().unwrap() = Some((
            t.access_token.clone(),
            Instant::now() + Duration::from_secs(t.expires_in.saturating_sub(60)),
        ));
        Ok(t.access_token)
    }

    // ---- Watch ------------------------------------------------------------------------------

    /// Start (or renew) the Pub/Sub watch when it is missing or ends within `within_hours`.
    pub async fn renew_watch(&self, within_hours: i64) -> Result<bool> {
        let cfg = self.cfg()?;
        let (Some(topic), Some(mut conn)) = (&cfg.topic, self.conn().await?) else {
            return Ok(false);
        };
        let soon = Utc::now().timestamp_millis() + within_hours * 3_600_000;
        if conn.watch_expiration.is_some_and(|e| e > soon) {
            return Ok(false);
        }
        let token = self.access_token(&conn).await?;
        let w = gmail::Client::new(&self.http, cfg, &token)
            .watch(topic, &conn.label_id)
            .await?;
        conn.watch_expiration = w.expiration.parse().ok();
        // Keep our own place in the history: the watch's historyId is only for the first start.
        self.save(&conn).await?;
        Ok(true)
    }

    // ---- Sync -------------------------------------------------------------------------------

    /// Read what is new under the label and queue suggestions. Safe to call any time and from
    /// several places: runs one at a time, and a call made during a run makes it go round again.
    pub async fn sync(&self) -> Result<SyncSummary> {
        let mut total = SyncSummary::default();
        loop {
            let Ok(_guard) = self.running.try_lock() else {
                self.rerun.store(true, Ordering::SeqCst);
                return Ok(total);
            };
            self.rerun.store(false, Ordering::SeqCst);
            let r = self.sync_once(None).await;
            match &r {
                Ok(_) => self.note(None, true).await,
                Err(e) => self.note(Some(format!("{e:#}")), false).await,
            }
            let s = r?;
            total.messages += s.messages;
            total.suggestions += s.suggestions;
            total.ignored += s.ignored;
            total.failed += s.failed;
            if !self.rerun.load(Ordering::SeqCst) {
                return Ok(total);
            }
        }
    }

    /// Look at the labelled mail of the last `days` days, whether or not a push said so.
    /// Mail already handled is skipped, so this can be run any number of times.
    pub async fn scan(&self, days: Option<u32>) -> Result<SyncSummary> {
        let _guard = self.running.lock().await;
        let days = days.unwrap_or(SCAN_DAYS_DEFAULT).clamp(1, SCAN_DAYS_MAX);
        let r = self.sync_once(Some(days)).await;
        match &r {
            Ok(_) => self.note(None, true).await,
            Err(e) => self.note(Some(format!("{e:#}")), false).await,
        }
        r
    }

    async fn sync_once(&self, scan_days: Option<u32>) -> Result<SyncSummary> {
        let cfg = self.cfg()?;
        let Some(mut conn) = self.conn().await? else {
            return Ok(SyncSummary::default());
        };
        let key = cfg
            .ai_key
            .as_deref()
            .ok_or_else(|| anyhow!("GEMINI_API_KEY is not set, so emails cannot be read"))?;
        let token = self.access_token(&conn).await?;
        let g = gmail::Client::new(&self.http, cfg, &token);

        // Which messages to look at, and where the history stands afterwards.
        let (ids, next_history) = match scan_days {
            Some(days) => (g.list_label_messages(&conn.label_id, days).await?, None),
            None => match conn.history_id.clone() {
                Some(start) => match g.history(&start, &conn.label_id).await? {
                    gmail::History::Changes { ids, history_id } => (ids, Some(history_id)),
                    // Too old: start from now, and catch up on the recent labelled mail.
                    gmail::History::Expired => {
                        let now = g.profile().await?.history_id;
                        (
                            g.list_label_messages(&conn.label_id, SCAN_DAYS_DEFAULT)
                                .await?,
                            Some(now),
                        )
                    }
                },
                None => (Vec::new(), Some(g.profile().await?.history_id)),
            },
        };

        let categories = self.db.list_categories().await?;
        let mut summary = SyncSummary::default();
        let mut complete = true;
        let mut transient: Option<String> = None;
        for id in ids {
            if self.db.email_message_seen(&id).await? {
                continue;
            }
            if summary.messages >= MAX_PER_RUN {
                complete = false;
                break;
            }
            let Some(mail) = self.fetch_labeled(&g, &conn.label_id, &id).await? else {
                continue;
            };
            summary.messages += 1;
            match ai::parse(&self.http, cfg, key, &mail, &categories).await {
                Ok(found) => {
                    let rows = rows_for(&mail, &found, &categories);
                    if found.is_empty() {
                        summary.ignored += 1;
                    } else {
                        summary.suggestions += found.len();
                    }
                    self.db.add_email_suggestions(&rows).await?;
                }
                Err(ai::Failure::Transient(e)) => {
                    // Leave it (and the history position) for the next run.
                    complete = false;
                    tracing::warn!("email AI call failed, will retry: {e:#}");
                    transient = Some(format!("{e:#}"));
                }
                Err(ai::Failure::Unreadable(e)) => {
                    summary.failed += 1;
                    self.db
                        .add_email_suggestions(&[error_row(&mail, &e)])
                        .await?;
                }
            }
        }
        if complete {
            if let Some(h) = next_history {
                conn.history_id = Some(h);
                // Re-read in case a renewal saved meanwhile, then write only what we changed.
                if let Some(mut latest) = self.conn().await? {
                    latest.history_id = conn.history_id;
                    self.save(&latest).await?;
                }
            }
        }
        if let Some(e) = transient {
            return Err(anyhow!(
                "some emails could not be read yet and will be tried again: {e}"
            ));
        }
        Ok(summary)
    }

    /// The one place a message's content is fetched. Its labels are looked at first (without
    /// the content), and it is only read if it carries the label: everything else in the
    /// mailbox stays unread, whatever id we were handed.
    pub(crate) async fn fetch_labeled(
        &self,
        g: &gmail::Client<'_>,
        label_id: &str,
        id: &str,
    ) -> Result<Option<gmail::Mail>> {
        match g.message_labels(id).await? {
            Some(labels) if labels.iter().any(|l| l == label_id) => Ok(g
                .message(id)
                .await?
                .filter(|m| m.label_ids.iter().any(|l| l == label_id))),
            _ => Ok(None),
        }
    }

    // ---- Background -------------------------------------------------------------------------

    /// Poll as a fallback for missed pushes, and renew the watch. Runs forever.
    pub async fn run_background(self: std::sync::Arc<Self>) {
        let Some(cfg) = self.cfg.as_ref() else { return };
        let mut tick = tokio::time::interval(Duration::from_secs(cfg.poll_minutes.max(1) * 60));
        loop {
            tick.tick().await;
            if self.conn().await.ok().flatten().is_none() {
                continue;
            }
            if let Err(e) = self.renew_watch(48).await {
                tracing::warn!("renewing the Gmail watch failed: {e:#}");
            }
            if let Err(e) = self.sync().await {
                tracing::warn!("email sync failed: {e:#}");
            }
        }
    }

    /// A push arrived (already authenticated): sync in the background.
    pub fn nudge(self: &std::sync::Arc<Self>) {
        let me = self.clone();
        tokio::spawn(async move {
            if let Err(e) = me.sync().await {
                tracing::warn!("email sync after push failed: {e:#}");
            }
        });
    }

    /// Does the push's message belong to the connected mailbox? (The content is not used for
    /// anything else.)
    pub async fn is_our_mailbox(&self, email: &str) -> bool {
        self.conn()
            .await
            .ok()
            .flatten()
            .is_some_and(|c| c.email.eq_ignore_ascii_case(email))
    }

    pub(crate) fn http(&self) -> &reqwest::Client {
        &self.http
    }

    pub(crate) fn jwks_cache(&self) -> &StdMutex<Option<(push::Jwks, Instant)>> {
        &self.jwks
    }
}

/// The rows for one parsed message: a row per transaction found, or one `ignored` row.
fn rows_for(mail: &gmail::Mail, found: &[ai::Found], cats: &[Category]) -> Vec<NewEmailSuggestion> {
    let base = |idx: i64, status: &str| NewEmailSuggestion {
        gmail_id: mail.id.clone(),
        idx,
        status: status.into(),
        received: mail.received.clone(),
        sender: truncate(&mail.from, 250),
        subject: truncate(&mail.subject, 250),
        ..Default::default()
    };
    if found.is_empty() {
        return vec![base(0, "ignored")];
    }
    found
        .iter()
        .enumerate()
        .map(|(i, f)| NewEmailSuggestion {
            excerpt: truncate(&mail.text, 600),
            date: f.date.clone(),
            amount_minor: f.amount_minor,
            currency: f.currency.clone(),
            category_id: f.category_id.filter(|id| cats.iter().any(|c| c.id == *id)),
            note: truncate(&f.note, 35),
            confidence: f.confidence.clone(),
            ..base(i as i64, "pending")
        })
        .collect()
}

fn error_row(mail: &gmail::Mail, why: &anyhow::Error) -> NewEmailSuggestion {
    NewEmailSuggestion {
        gmail_id: mail.id.clone(),
        idx: 0,
        status: "error".into(),
        received: mail.received.clone(),
        sender: truncate(&mail.from, 250),
        subject: truncate(&mail.subject, 250),
        excerpt: truncate(&mail.text, 600),
        error: truncate(&format!("{why:#}"), 300),
        ..Default::default()
    }
}

pub(crate) fn truncate(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect()
    }
}
