//! Email to transactions: mail you forward to an address on your own domain is read by an AI,
//! which says what transaction each message describes. The answers go in a review queue, and
//! nothing becomes a transaction until a person accepts it in the app, except mail whose subject
//! has a standing rule (see [`rules`]) or that the AI is sure about and gave a category: that is
//! added at once, marked `auto`, and the person is told.
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
pub mod rules;
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
const LATEST_KEY: &str = "email_latest_batch";
/// Mail added without asking stays listed this many days, then goes (with its stored email).
const AUTO_KEEP_DAYS: i64 = 30;
const AI_DAY_KEY: &str = "email_ai_day";

/// Settings from the environment. Without `EMAIL_INBOUND_SECRET` the feature is off.
#[derive(Clone, Debug)]
pub struct Config {
    /// Shared with the Cloudflare Worker; the inbound endpoint refuses requests without it.
    pub inbound_secret: String,
    pub ai_key: Option<String>,
    pub model: String,
    /// AI calls per day at most (a guard against a flood of mail to the routed address).
    pub max_per_day: u32,
    pub ai_base: String,
}

impl Config {
    pub fn from_env() -> Option<Config> {
        let var = |k: &str| {
            std::env::var(k)
                .ok()
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        Some(Config {
            inbound_secret: var("EMAIL_INBOUND_SECRET")?,
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
    /// What the latest message brought (up to 5 lines), for the push text.
    pub latest: Vec<Latest>,
    /// Messages the AI has not read yet.
    pub queued: usize,
    /// VAPID public key for subscribing to web push; absent when push is not set up.
    pub push_public_key: Option<String>,
}

/// One transaction the latest message brought: added at once, or waiting for review.
#[derive(Serialize, serde::Deserialize, Debug, Default, Clone, PartialEq)]
pub struct Latest {
    pub auto: bool,
    /// The transaction's day (YYYY-MM-DD).
    pub date: String,
    pub note: String,
    pub amount_minor: i64,
    pub currency: String,
}

/// The pending and `auto` rows as push lines.
fn latest_of(rows: &[NewEmailSuggestion]) -> Vec<Latest> {
    rows.iter()
        .filter(|r| r.status == "pending" || r.status == "auto")
        .map(|r| Latest {
            auto: r.status == "auto",
            date: r.date.clone(),
            note: if r.note.is_empty() {
                truncate(&r.subject, 35)
            } else {
                r.note.clone()
            },
            amount_minor: r.amount_minor,
            currency: r.currency.clone(),
        })
        .collect()
}

/// What became of one message.
#[derive(Serialize, Debug, Default, PartialEq)]
pub struct Ingested {
    /// Suggested transactions added to the review queue.
    pub suggestions: usize,
    /// Transactions added at once, without review.
    pub auto: usize,
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
    /// Not something we accept (not an email).
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
        self.expire_auto().await;
        let latest = self
            .db
            .get_setting(LATEST_KEY)
            .await?
            .and_then(|v| serde_json::from_str(&v).ok())
            .unwrap_or_default();
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
            latest,
            queued,
            push_public_key: crate::push::public_key(),
        })
    }

    async fn note_error(&self, e: Option<&str>) {
        let _ = self.db.set_setting(LAST_ERROR_KEY, e.unwrap_or("")).await;
    }

    /// Close the mail that became a transaction (added at once or approved) once it is a month old.
    async fn expire_auto(&self) {
        let before = (Utc::now() - chrono::Duration::days(AUTO_KEEP_DAYS))
            .format("%Y-%m-%d")
            .to_string();
        if let Err(e) = self.db.expire_auto_emails(&before).await {
            tracing::warn!("expiring automatically added mail failed: {e:#}");
        }
    }

    /// Tell the devices what just arrived (the push itself carries no text; the app asks).
    async fn announce(&self, mut items: Vec<Latest>) {
        if items.is_empty() {
            return;
        }
        items.truncate(5);
        if let Ok(json) = serde_json::to_string(&items) {
            let _ = self.db.set_setting(LATEST_KEY, &json).await;
        }
        crate::push::notify_all(self.db.as_ref(), "email").await;
    }

    /// Record one paid AI call in the operation log (kind `ai_email`), with what it cost.
    async fn log_cost(
        &self,
        cfg: &Config,
        mail: &Mail,
        u: &ai::Usage,
        parsed: &Result<Vec<ai::Found>, ai::Failure>,
    ) {
        let result = match parsed {
            Ok(f) if f.is_empty() => "ignored".to_string(),
            Ok(f) => format!("{} suggestions", f.len()),
            Err(_) => "unreadable".to_string(),
        };
        crate::oplog::record(
            self.db.as_ref(),
            crate::oplog::kind::AI_EMAIL,
            None,
            None,
            Some(serde_json::json!({
                "sender": truncate(&mail.from, 120),
                "subject": truncate(&mail.subject, 120),
                "model": cfg.model,
                "input_tokens": u.input,
                "output_tokens": u.output,
                "cost_micro_usd": u.cost_micro_usd(),
                "result": result,
            })),
        )
        .await;
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
        if self.cfg.is_none() {
            return Err(IngestError::Refused("email is not configured".into()));
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
        if mail.text.is_empty() && mail.pdfs.is_empty() {
            // Nothing to read. With attachments the AI cannot read, a person should look.
            if !mail.unread.is_empty() {
                self.db.add_email_suggestions(&[unread_row(&mail)]).await?;
                return Ok(Ingested {
                    failed: true,
                    ..Default::default()
                });
            }
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
        self.announce(latest_of(&out.rows)).await;
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
            if !mail.pdfs.is_empty() {
                row.raw = mail.raw.to_vec();
            }
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
        // A rule for this subject fixes the category (and business): the model only reads the amounts.
        let rules = rules::list(self.db.as_ref()).await?;
        let rule = rules::find(&rules, &mail.subject)
            .filter(|r| {
                categories
                    .iter()
                    .any(|c| c.id == r.category_id && !c.is_income)
            })
            .cloned();
        let for_ai: &[Category] = if rule.is_some() { &[] } else { categories };
        let mut usage = None;
        let parsed = ai::parse(&self.http, cfg, key, mail, for_ai, &mut usage).await;
        if let Some(u) = usage {
            self.log_cost(cfg, mail, &u, &parsed).await;
        }
        Ok(match parsed {
            Ok(found) if found.is_empty() && !mail.unread.is_empty() => {
                self.note_error(None).await;
                Read {
                    rows: vec![unread_row(mail)],
                    summary: Ingested {
                        failed: true,
                        ..Default::default()
                    },
                }
            }
            Ok(found) => {
                self.note_error(None).await;
                let mut rows = rows_for(mail, &found, categories);
                let auto = self
                    .add_at_once(rule.as_ref(), &mut rows, categories)
                    .await?;
                Read {
                    summary: Ingested {
                        suggestions: found.len() - auto,
                        auto,
                        ignored: found.is_empty(),
                        ..Default::default()
                    },
                    rows,
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
        })
    }

    /// Make transactions of the pending rows that need no review: a category is known (from the
    /// subject's rule, else the AI's own pick) and the AI is sure about the rest. Such a row
    /// becomes `auto`. A row stays pending when the category needs a business that nothing
    /// names, or when the transaction cannot be made now. Returns how many were added.
    async fn add_at_once(
        &self,
        rule: Option<&rules::Rule>,
        rows: &mut [NewEmailSuggestion],
        cats: &[Category],
    ) -> Result<usize> {
        let links = self.db.business_links().await?;
        let mut added = 0;
        for row in rows.iter_mut().filter(|r| r.status == "pending") {
            if let Some(r) = rule {
                row.category_id = Some(r.category_id);
            }
            let (Some(cid), "high") = (row.category_id, row.confidence.as_str()) else {
                continue;
            };
            let Some(cat) = cats.iter().find(|c| c.id == cid) else {
                continue;
            };
            let ids = [Some(cid), cat.parent_id];
            let business = rule.and_then(|r| r.business_id);
            let linked = links
                .categories
                .iter()
                .any(|(c, _)| ids.contains(&Some(*c)));
            let required = links.required.iter().any(|c| ids.contains(&Some(*c)));
            if required && business.is_none() && !linked {
                continue;
            }
            let Ok(tx) = crate::api::add_for_email(
                &self.db,
                &row.date,
                row.amount_minor,
                &row.currency,
                cid,
                &row.note,
            )
            .await
            else {
                tracing::warn!("a mail matching a rule could not be added, left for review");
                continue;
            };
            if let Some(b) = business {
                self.db.set_transactions_business(&[tx], Some(b)).await?;
            }
            row.status = "auto".into();
            row.transaction_id = Some(tx);
            added += 1;
        }
        Ok(added)
    }

    /// Read the messages the AI could not read earlier. Stops at the first one that still
    /// cannot be read (the service is probably still down). Returns how many were done.
    pub async fn retry_queued(&self, only: Option<i64>) -> Result<usize> {
        let _guard = self.running.lock().await;
        let categories = self.db.list_categories().await?;
        let mut done = 0;
        let mut new_items = Vec::new();
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
            // A message kept whole (it had attachments) is parsed again; otherwise rebuilt from its text.
            let kept = match self.db.email_raw(row.id).await? {
                Some(raw) => inbound::parse_raw(&raw).ok(),
                None => None,
            };
            let mail = match kept {
                Some(m) => m,
                None => Mail {
                    id: row.message_id.clone(),
                    from: row.sender.clone(),
                    subject: row.subject.clone(),
                    received: row.received.clone(),
                    html: self.db.email_html(row.id).await?.unwrap_or_default(),
                    text: row.excerpt.clone(),
                    pdfs: Vec::new(),
                    unread: Vec::new(),
                    raw: Default::default(),
                },
            };
            let out = self.read(&mail, &categories).await?;
            if out.summary.queued {
                break;
            }
            // The old row goes first: the new ones take its place under the same message id.
            if self.db.delete_email_suggestion(row.id).await? {
                self.db.add_email_suggestions(&out.rows).await?;
                done += 1;
                new_items.extend(latest_of(&out.rows));
            }
        }
        self.announce(new_items).await;
        Ok(done)
    }

    /// Once a day: retry what is waiting. Runs forever.
    pub async fn run_background(self: std::sync::Arc<Self>) {
        let mut tick = tokio::time::interval(Duration::from_secs(24 * 3600));
        loop {
            tick.tick().await;
            self.expire_auto().await;
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

/// A message with attachments the AI cannot read and nothing found in the body: shown for a person to look at.
fn unread_row(mail: &Mail) -> NewEmailSuggestion {
    let names = mail.unread.join(", ");
    error_row(
        mail,
        &anyhow!("attachment not read by the AI: {}", truncate(&names, 200)),
    )
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
