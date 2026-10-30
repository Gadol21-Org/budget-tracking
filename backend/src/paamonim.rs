//! Client for Paamonim (Rishumit). There is no public API: we log in through the
//! HTML form and call the cookie-authenticated `/rishumit/*` endpoints.
//!
//! NOTE: written from the site's public JS only; response shapes and the
//! `save_item_line` field names still need verifying against a real account.

use anyhow::{bail, Context, Result};
use scraper::{Html, Selector};
use serde::Deserialize;

pub struct Client {
    http: reqwest::Client,
    base: String,
}

#[derive(Debug, Clone)]
pub struct HistoryLine {
    pub line_id: String,
    pub text: String,
}

#[derive(Deserialize)]
struct HtmlEnvelope {
    html: String,
}

impl Client {
    pub async fn login(base: &str, username: &str, password: &str) -> Result<Self> {
        let http = reqwest::Client::builder().cookie_store(true).build()?;
        let base = base.trim_end_matches('/').to_string();

        let page = http.get(format!("{base}/")).send().await?.text().await?;
        let token = {
            let doc = Html::parse_document(&page);
            let sel = Selector::parse("input[name=authenticity_token]").unwrap();
            doc.select(&sel)
                .next()
                .and_then(|e| e.value().attr("value"))
                .context("authenticity_token not found on login page")?
                .to_string()
        };

        let resp = http
            .post(format!("{base}/login"))
            .form(&[
                ("utf8", "\u{2713}"),
                ("authenticity_token", token.as_str()),
                ("login", username),
                ("password", password),
                ("save[login]", "0"),
            ])
            .send()
            .await?;
        let body = resp.text().await?;
        // Still on the login form => credentials rejected.
        if body.contains("id=\"frm_login\"") {
            bail!("Paamonim login failed (still on login form)");
        }
        Ok(Self { http, base })
    }

    /// Fetch raw activity-history HTML for a date range (YYYY-MM-DD).
    pub async fn activity_history_html(&self, from: &str, to: &str) -> Result<String> {
        let resp = self
            .http
            .get(format!("{}/rishumit/activity_history", self.base))
            .header("X-Requested-With", "XMLHttpRequest")
            .header("Accept", "application/json")
            .query(&[("start_date", from), ("end_date", to)])
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        // The endpoint returns JSON wrapping an HTML fragment.
        Ok(match serde_json::from_str::<HtmlEnvelope>(&resp) {
            Ok(env) => env.html,
            Err(_) => resp,
        })
    }

    pub fn parse_history(html: &str) -> Vec<HistoryLine> {
        let doc = Html::parse_fragment(html);
        let sel = Selector::parse(".historyLine").unwrap();
        doc.select(&sel)
            .filter_map(|e| {
                let line_id = e.value().attr("line_id")?.to_string();
                let text = e.text().collect::<Vec<_>>().join(" ");
                Some(HistoryLine {
                    line_id,
                    text: text.split_whitespace().collect::<Vec<_>>().join(" "),
                })
            })
            .collect()
    }

    /// Mirror a line to Paamonim. `amount_shekels` must already be rounded.
    pub async fn save_item_line(&self, fields: &[(&str, String)]) -> Result<()> {
        self.http
            .get(format!("{}/rishumit/save_item_line", self.base))
            .header("X-Requested-With", "XMLHttpRequest")
            .query(fields)
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }

    pub async fn delete_history_line(&self, line_id: &str) -> Result<()> {
        self.http
            .post(format!("{}/rishumit/delete_history_line", self.base))
            .header("X-Requested-With", "XMLHttpRequest")
            .form(&[("line_id", line_id)])
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }
}
