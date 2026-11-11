//! Who deals with a forwarded message. Reading it with the AI is the default; a message that
//! needs its own treatment (for example one whose link must be followed to fetch a bill) gets a
//! [`Handler`] that claims it first.
//!
//! To add one: implement [`Handler`], and register it in [`registry`]. Handlers are asked in
//! order, the first whose [`Handler::matches`] says yes handles the message and the AI is not
//! used for it. A handler may read anything in the [`Mail`] (sender, subject, text, HTML, and
//! [`Mail::links`]), call other services, and must record an outcome with [`Ctx::finish`] so the
//! message is not handled twice (messages are remembered by their Message-ID). Anything it wants
//! a person to confirm goes in as suggestions, like the AI's answers do.

use anyhow::Result;
use async_trait::async_trait;

use super::inbound::Mail;
use super::{base_row, rows_for, Ingested};
use crate::api::Db;
use crate::storage::NewEmailSuggestion;

/// What a handler may use.
#[allow(dead_code)] // the extension point has no handler yet
pub struct Ctx {
    pub db: Db,
}

#[allow(dead_code)]
impl Ctx {
    /// Store the suggestions a handler found (any number; none means "nothing to add") and
    /// remember the message. Always call this, or the message would be handled again if resent.
    pub async fn finish(&self, mail: &Mail, rows: Vec<NewEmailSuggestion>) -> Result<Ingested> {
        let found = rows.len();
        if rows.is_empty() {
            self.db
                .add_email_suggestions(&rows_for(mail, &[], &[]))
                .await?;
            return Ok(Ingested {
                ignored: true,
                ..Default::default()
            });
        }
        self.db.add_email_suggestions(&rows).await?;
        Ok(Ingested {
            suggestions: found,
            ..Default::default()
        })
    }

    /// A pending suggestion row for this message, to fill in (date, amount, currency, ...).
    pub fn suggestion(&self, mail: &Mail, idx: i64) -> NewEmailSuggestion {
        NewEmailSuggestion {
            excerpt: super::truncate(&mail.text, 600),
            html: mail.html.clone(),
            ..base_row(mail, idx, "pending")
        }
    }
}

#[async_trait]
pub trait Handler: Send + Sync {
    /// Short name, for logs.
    fn name(&self) -> &'static str;
    /// Is this message yours? Decide from the sender, subject or content; no side effects.
    fn matches(&self, mail: &Mail) -> bool;
    /// Do the work for a message that matched.
    async fn handle(&self, ctx: &Ctx, mail: &Mail) -> Result<Ingested>;
}

/// The manual handlers, in the order they are asked. None yet: every message goes to the AI.
pub fn registry() -> Vec<Box<dyn Handler>> {
    Vec::new()
}
