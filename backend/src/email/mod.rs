//! Email to transactions: mail you forward to an address on your own domain is read by an AI,
//! which says what transaction each message describes. The answers go in a review queue, and
//! nothing becomes a transaction until a person accepts it in the app.
//!
//! How it fits together:
//! * A Gmail filter forwards the mail you choose (receipts, card and bank notices) to an address
//!   handled by Cloudflare Email Routing.
//! * A Cloudflare Email Worker (`deploy/email-worker.js`) posts the raw message to
//!   `POST /api/email/inbound`, with a shared secret.
//! * [`Service::ingest`] parses it ([`inbound`]), asks the model ([`ai`]) and stores the answers.
//!
//! The app holds no credential for any mailbox, and only ever sees what the filter forwards.
//! If the AI service is unavailable or the daily cap is reached, the message is kept as `queued`
//! (its text stored until it is read) and retried each day or when asked.

pub mod ai;
pub mod handlers;
pub mod inbound;
#[cfg(test)]
mod tests;

use std::time::Duration;

use anyhow::{anyhow, Result};
use chrono::Utc;
use serde::Serialize;
use tokio::sync::Mutex;

use crate::api::Db;
use crate::storage::{Category, NewEmailSuggestion};
use inbound::Mail;

const LAST_RECEIVED_KEY: &str = "email_last_received";
const LAST_ERROR_KEY: &str = "email_last_error";
const AI_DAY_KEY: &str = "email_ai_day";

/// Settings from the environment. Without `EMAIL_INBOUND_SECRET` the feature is off.
#[derive(Clone, Debug)]
pub struct Config {
    /// Shared with the Cloudflare Worker; the inbound endpoint refuses requests without it.
    pub inbound_secret: String,
    /// When set, a message must carry an `X-Forwarded-For` header naming this address.
    pub forwarder: Option<String>,
    pub ai_key: Option<String>,
    pub model: String,
    /// AI calls per day at most (a guard against a flood of mail to the routed address).
    pub max_per_day: u32,
    pub ai_base: String,
}

impl Config {
    pub fn from_env() -> Option<Config> {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        Some(Config {
            inbound_secret: var("EMAIL_INBOUND_SECRET")?,
            forwarder: var("EMAIL_FORWARDER"),
            ai_key: var("GEMINI_API_KEY"),
            model: var("EMAIL_AI_MODEL").unwrap_or_else(|| "gemini-3.5-flash-lite".into()),
            max_per_day: var("EMAIL_MAX_PER_DAY")
                .and_then(|m| m.parse().ok())
                .unwrap_or(100),
            ai_base: var("GEMINI_BASE_URL")
                .unwrap_or_else(|| "https://generativelanguage.googleapis.com".into()),
        })
    }
}

/// What the settings screen shows. Never contains a secret.
#[derive(Serialize, Debug, Default)]
pub struct Status {
    /// The server has `EMAIL_INBOUND_SECRET`.
    pub configured: bool,
    pub ai_configured: bool,
    /// When a message last arrived (UTC, ISO 8601).
    pub last_received: Option<String>,
    pub last_error: Option<String>,
    /// Suggestions waiting for a decision.
    pub pending: usize,
    /// Messages the AI has not read yet.
    pub queued: usize,
    /// VAPID public key for subscribing to web push; absent when push is not set up.
    pub push_public_key: Option<String>,
}

/// What became of one message.
#[derive(Serialize, Debug, Default, PartialEq)]
pub struct Ingested {
    /// Suggested transactions added to the review queue.
    pub suggestions: usize,
    /// The message was already handled.
    pub duplicate: bool,
    /// The message described no transaction.
    pub ignored: bool,
    /// The AI could not read it now; it will be tried again.
    pub queued: bool,
    /// The AI answered with something unusable.
    pub failed: bool,
}

#[derive(Debug)]
pub enum IngestError {
    /// Not something we accept (not an email, not forwarded by the expected address).
    Refused(String),
    Internal(anyhow::Error),
}

impl From<anyhow::Error> for IngestError {
    fn from(e: anyhow::Error) -> Self {
        IngestError::Internal(e)
    }
}

pub struct Service {
    db: Db,
    pub cfg: Option<Config>,
    http: reqwest::Client,
    /// Parsing happens one message at a time, so the daily count and duplicates are exact.
    running: Mutex<()>,
    /// Manual handlers, asked before the AI (see [`handlers`]).
    handlers: Vec<Box<dyn handlers::Handler>>,
}

/// Equal strings, compared without stopping at the first difference.
pub fn same(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut diff = (a.len() != b.len()) as u8;
    for i in 0..a.len().max(b.len()) {
        diff |= a.get(i).copied().unwrap_or(0) ^ b.get(i).copied().unwrap_or(0);
    }
    diff == 0
}

impl Service {
    pub fn new(db: Db, cfg: Option<Config>) -> std::sync::Arc<Service> {
        Service::with_handlers(db, cfg, handlers::registry())
    }

    pub fn with_handlers(
        db: Db,
        cfg: Option<Config>,
        handlers: Vec<Box<dyn handlers::Handler>>,
    ) -> std::sync::Arc<Service> {
        std::sync::Arc::new(Service {
            handlers,
            db,
            cfg,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(60))
                .build()
                .expect("http client"),
            running: Mutex::new(()),
        })
    }

    pub fn from_env(db: Db) -> std::sync::Arc<Service> {
        Service::new(db, Config::from_env())
    }

    /// Is this the shared secret? (Off, or wrong, is a no.)
    pub fn authorized(&self, secret: Option<&str>) -> bool {
        match (&self.cfg, secret) {
            (Some(c), Some(s)) => same(&c.inbound_secret, s),
            _ => false,
        }
    }

    pub async fn status(&self) -> Result<Status> {
        let pending = self.db.list_email_suggestions(Some("pending")).await?.len();
        let queued = self.db.list_email_suggestions(Some("queued")).await?.len();
        Ok(Status {
            configured: self.cfg.is_some(),
            ai_configured: self.cfg.as_ref().is_some_and(|c| c.ai_key.is_some()),
            last_received: self
                .db
                .get_setting(LAST_RECEIVED_KEY)
                .await?
                .filter(|s| !s.is_empty()),
            last_error: self
                .db
                .get_setting(LAST_ERROR_KEY)
                .await?
                .filter(|s| !s.is_empty()),
            pending,
            queued,
            push_public_key: crate::push::public_key(),
        })
    }

    async fn note_error(&self, e: Option<&str>) {
        let _ = self.db.set_setting(LAST_ERROR_KEY, e.unwrap_or("")).await;
    }

    /// Take one more AI call from today's allowance. False when it is used up.
    async fn ai_allowed(&self) -> Result<bool> {
        let cfg = self
            .cfg
            .as_ref()
            .ok_or_else(|| anyhow!("email is not configured"))?;
        let today = Utc::now().format("%Y-%m-%d").to_string();
        let used = match self
            .db
            .get_setting(AI_DAY_KEY)
            .await?
            .as_deref()
            .and_then(|s| s.split_once(':'))
        {
            Some((d, n)) if d == today => n.parse::<u32>().unwrap_or(0),
            _ => 0,
        };
        if used >= cfg.max_per_day {
            return Ok(false);
        }
        self.db
            .set_setting(AI_DAY_KEY, &format!("{today}:{}", used + 1))
            .await?;
        Ok(true)
    }

    /// Handle one raw email from the inbound endpoint.
    pub async fn ingest(&self, raw: &[u8]) -> Result<Ingested, IngestError> {
        let cfg = self
            .cfg
            .as_ref()
            .ok_or_else(|| IngestError::Refused("email is not configured".into()))?;
        if let Some(f) = &cfg.forwarder {
            if !inbound::forwarded_by(raw, f) {
                return Err(IngestError::Refused(
                    "not forwarded by the expected address".into(),
                ));
            }
        }
        let mail = inbound::parse_raw(raw).map_err(|e| IngestError::Refused(format!("{e:#}")))?;
        let _guard = self.running.lock().await;
        let _ = self
            .db
            .set_setting(
                LAST_RECEIVED_KEY,
                &Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            )
            .await;
        if self.db.email_message_seen(&mail.id).await? {
            return Ok(Ingested {
                duplicate: true,
                ..Default::default()
            });
        }
        if let Some(h) = self.handlers.iter().find(|h| h.matches(&mail)) {
            let ctx = handlers::Ctx {
                db: self.db.clone(),
            };
            return h.handle(&ctx, &mail).await.map_err(|e| {
                tracing::warn!("email handler {} failed: {e:#}", h.name());
                IngestError::from(e)
            });
        }
        if mail.text.is_empty() {
            self.db
                .add_email_suggestions(&rows_for(&mail, &[], &[]))
                .await?;
            return Ok(Ingested {
                ignored: true,
                ..Default::default()
            });
        }
        let categories = self.db.list_categories().await?;
        let out = self.read(&mail, &categories).await?;
        self.db.add_email_suggestions(&out.rows).await?;
        if out.summary.suggestions > 0 {
            crate::push::notify_all(self.db.as_ref()).await;
        }
        Ok(out.summary)
    }

    /// Ask the AI about one message and say what to store (nothing is written here).
    async fn read(&self, mail: &Mail, categories: &[Category]) -> Result<Read> {
        let cfg = self
            .cfg
            .as_ref()
            .ok_or_else(|| anyhow!("email is not configured"))?;
        let wait = |why: String| {
            let mut row = queued_row(mail, &why);
            row.excerpt = mail.text.clone();
            Read {
                rows: vec![row],
                summary: Ingested {
                    queued: true,
                    ..Default::default()
                },
            }
        };
        let Some(key) = cfg.ai_key.as_deref() else {
            self.note_error(Some("GEMINI_API_KEY is not set, so emails wait unread"))
                .await;
            return Ok(wait("GEMINI_API_KEY is not set".into()));
        };
        if !self.ai_allowed().await? {
            return Ok(wait(format!(
                "the daily limit of {} AI calls was reached",
                cfg.max_per_day
            )));
        }
        Ok(
            match ai::parse(&self.http, cfg, key, mail, categories).await {
                Ok(found) => {
                    self.note_error(None).await;
                    Read {
                        summary: Ingested {
                            suggestions: found.len(),
                            ignored: found.is_empty(),
                            ..Default::default()
                        },
                        rows: rows_for(mail, &found, categories),
                    }
                }
                Err(ai::Failure::Transient(e)) => {
                    tracing::warn!("email AI call failed, kept for a retry: {e:#}");
                    self.note_error(Some(&format!("{e:#}"))).await;
                    wait(format!("{e:#}"))
                }
                Err(ai::Failure::Unreadable(e)) => Read {
                    rows: vec![error_row(mail, &e)],
                    summary: Ingested {
                        failed: true,
                        ..Default::default()
                    },
                },
            },
        )
    }

    /// Read the messages the AI could not read earlier. Stops at the first one that still
    /// cannot be read (the service is probably still down). Returns how many were done.
    pub async fn retry_queued(&self, only: Option<i64>) -> Result<usize> {
        let _guard = self.running.lock().await;
        let categories = self.db.list_categories().await?;
        let mut done = 0;
        let mut found = false;
        for row in self
            .db
            .list_email_suggestions(Some("queued"))
            .await?
            .into_iter()
            .rev()
        {
            if only.is_some_and(|id| id != row.id) {
                continue;
            }
            let html = self.db.email_html(row.id).await?.unwrap_or_default();
            let mail = Mail {
                id: row.message_id.clone(),
                from: row.sender.clone(),
                subject: row.subject.clone(),
                received: row.received.clone(),
                html,
                text: row.excerpt.clone(),
            };
            let out = self.read(&mail, &categories).await?;
            if out.summary.queued {
                break;
            }
            // The old row goes first: the new ones take its place under the same message id.
            if self.db.delete_email_suggestion(row.id).await? {
                self.db.add_email_suggestions(&out.rows).await?;
                done += 1;
                found |= out.summary.suggestions > 0;
            }
        }
        if found {
            crate::push::notify_all(self.db.as_ref()).await;
        }
        Ok(done)
    }

    /// Once a day: retry what is waiting. Runs forever.
    pub async fn run_background(self: std::sync::Arc<Self>) {
        let mut tick = tokio::time::interval(Duration::from_secs(24 * 3600));
        loop {
            tick.tick().await;
            if let Err(e) = self.retry_queued(None).await {
                tracing::warn!("retrying queued emails failed: {e:#}");
            }
        }
    }
}

struct Read {
    rows: Vec<NewEmailSuggestion>,
    summary: Ingested,
}

fn base_row(mail: &Mail, idx: i64, status: &str) -> NewEmailSuggestion {
    NewEmailSuggestion {
        message_id: mail.id.clone(),
        idx,
        status: status.into(),
        received: mail.received.clone(),
        sender: truncate(&mail.from, 250),
        subject: truncate(&mail.subject, 250),
        ..Default::default()
    }
}

/// The rows for one parsed message: a row per transaction found, or one `ignored` row.
fn rows_for(mail: &Mail, found: &[ai::Found], cats: &[Category]) -> Vec<NewEmailSuggestion> {
    if found.is_empty() {
        return vec![base_row(mail, 0, "ignored")];
    }
    found
        .iter()
        .enumerate()
        .map(|(i, f)| NewEmailSuggestion {
            excerpt: truncate(&mail.text, 600),
            html: mail.html.clone(),
            date: f.date.clone(),
            amount_minor: f.amount_minor,
            currency: f.currency.clone(),
            category_id: f.category_id.filter(|id| cats.iter().any(|c| c.id == *id)),
            note: truncate(&f.note, 35),
            confidence: f.confidence.clone(),
            ..base_row(mail, i as i64, "pending")
        })
        .collect()
}

fn queued_row(mail: &Mail, why: &str) -> NewEmailSuggestion {
    NewEmailSuggestion {
        html: mail.html.clone(),
        error: truncate(why, 300),
        ..base_row(mail, 0, "queued")
    }
}

fn error_row(mail: &Mail, why: &anyhow::Error) -> NewEmailSuggestion {
    NewEmailSuggestion {
        excerpt: truncate(&mail.text, 600),
        html: mail.html.clone(),
        error: truncate(&format!("{why:#}"), 300),
        ..base_row(mail, 0, "error")
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
