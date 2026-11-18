//! Storage boundary. The app talks only to [`Storage`]; the one implementation is
//! [`crate::mysql::MySqlStorage`].

mod model;
#[cfg(test)]
mod tests;

pub use model::*;

use crate::oplog::{LogEntry, NewLogEntry};
use crate::recurring::{NewRecurring, Occurrence, RecurringSegment};

#[async_trait::async_trait]
pub trait Storage: Send + Sync {
    async fn list_payment_methods(&self) -> anyhow::Result<Vec<PaymentMethod>>;
    /// Set (or with None, clear) the foreign-currency details of a transaction.
    async fn set_tx_fx(&self, id: i64, fx: Option<&TxFx>) -> anyhow::Result<()>;
    /// Details of the transactions that still exist and were entered in a foreign currency.
    async fn list_tx_fx(&self) -> anyhow::Result<Vec<TxFx>>;
    /// Cached rate (shekels per unit) for a currency on a date (`latest` is the newest fetched).
    async fn get_fx_rate(&self, currency: &str, date: &str) -> anyhow::Result<Option<f64>>;
    async fn put_fx_rate(&self, currency: &str, date: &str, rate: f64) -> anyhow::Result<()>;
    /// App-wide settings (key/value text), synced across devices.
    async fn get_setting(&self, key: &str) -> anyhow::Result<Option<String>>;
    async fn set_setting(&self, key: &str, value: &str) -> anyhow::Result<()>;
    /// Store parsed suggestions. A (message, idx) pair that exists is left as it is, so
    /// parsing the same message again can never duplicate or undo a decision. Returns how
    /// many rows were new.
    async fn add_email_suggestions(&self, rows: &[NewEmailSuggestion]) -> anyhow::Result<usize>;
    /// Has any row for this email message been stored (whatever its status)?
    async fn email_message_seen(&self, message_id: &str) -> anyhow::Result<bool>;
    /// Suggestions with the given status (all of them when None), newest first.
    async fn list_email_suggestions(
        &self,
        status: Option<&str>,
    ) -> anyhow::Result<Vec<EmailSuggestion>>;
    /// Move a `pending` suggestion to `accepted` (with the transaction made from it) or
    /// `rejected`, and drop its excerpt. False if it was not pending.
    async fn resolve_email_suggestion(
        &self,
        id: i64,
        status: &str,
        transaction_id: Option<i64>,
    ) -> anyhow::Result<bool>;
    /// Ids of the transactions made from an email (accepted by hand or added automatically).
    async fn email_transaction_ids(&self) -> anyhow::Result<Vec<i64>>;
    /// Close `auto` and `added` rows whose mail was received before this day (YYYY-MM-DD), dropping their
    /// stored email. Returns how many.
    async fn expire_auto_emails(&self, before: &str) -> anyhow::Result<usize>;
    /// The stored HTML of an email, for showing it. None once the suggestion is decided.
    async fn email_html(&self, id: i64) -> anyhow::Result<Option<String>>;
    /// The raw message of a row that is waiting unread, if it was kept.
    async fn email_raw(&self, id: i64) -> anyhow::Result<Option<Vec<u8>>>;
    /// Remove a `queued` or `error` row (before its message is parsed again). False otherwise.
    async fn delete_email_suggestion(&self, id: i64) -> anyhow::Result<bool>;
    async fn list_businesses(&self) -> anyhow::Result<Vec<Business>>;
    async fn insert_business(&self, name: &str, is_general: bool) -> anyhow::Result<i64>;
    async fn rename_business(&self, id: i64, name: &str) -> anyhow::Result<()>;
    /// Also drops every tag and category link that pointed at it.
    async fn remove_business(&self, id: i64) -> anyhow::Result<()>;
    /// Tags of transactions that still exist, and the category links.
    async fn business_links(&self) -> anyhow::Result<BusinessLinks>;
    /// Make transactions of `category` count in `business` (None removes the link).
    async fn set_category_business(
        &self,
        category: i64,
        business: Option<i64>,
    ) -> anyhow::Result<()>;
    /// Tag transactions as `business`'s (None removes their tags).
    async fn set_transactions_business(
        &self,
        ids: &[i64],
        business: Option<i64>,
    ) -> anyhow::Result<()>;
    /// Make new transactions in `category` say which business they belong to (or none).
    async fn set_category_business_required(
        &self,
        category: i64,
        required: bool,
    ) -> anyhow::Result<()>;
    /// Raw rows for the full Google Sheets backup.
    async fn raw_backup(&self) -> anyhow::Result<RawBackup>;
    /// Transactions in state `pending`, `modified` or `verifying`, oldest first.
    async fn pending_mirrors(&self) -> anyhow::Result<Vec<PendingMirror>>;
    /// Set `sync_state` (e.g. `verifying` before a request, `error`, `local`).
    async fn set_mirror_state(&self, id: i64, state: SyncState) -> anyhow::Result<()>;
    /// Record that `posted` now exists in Paamonim as `line_id`. The line it replaces (if the
    /// transaction was already linked) is queued for deletion. If the transaction changed
    /// since `posted` was read it becomes `modified` instead of `synced`. Returns false when
    /// the transaction no longer exists (the caller then queues `line_id` for deletion).
    async fn mark_mirrored(&self, posted: &PendingMirror, line_id: &str) -> anyhow::Result<bool>;
    /// Give a category made here the Paamonim item it now has (`e:<id>` / `i:<id>`); its
    /// `skipped` transactions go back to `pending`. False if it already had one.
    async fn link_category(&self, id: i64, key: &str) -> anyhow::Result<bool>;
    /// Stop mirroring a transaction (e.g. moved to a category Paamonim doesn't have): queue
    /// its line for deletion, unlink it and mark it `skipped`.
    async fn detach_mirror(&self, id: i64) -> anyhow::Result<()>;
    /// `error` -> `pending`/`modified`, for a manual retry. False if it wasn't in `error`.
    async fn retry_mirror(&self, id: i64) -> anyhow::Result<bool>;
    /// Put back in the queue rows skipped (or put in error, when making the item failed) for
    /// having no Paamonim item although their category is a sub-category of a Paamonim
    /// category, which can get one. A row never sent has no line, and a category without an
    /// item means it never got as far as saving one, so sending it again can't duplicate.
    /// Called at start, so rows left by an earlier version that failed to make the item are
    /// sent. Returns how many.
    async fn requeue_skipped_sub_categories(&self) -> anyhow::Result<usize>;
    /// Lines to delete in Paamonim.
    async fn pending_line_deletes(&self) -> anyhow::Result<Vec<LineDelete>>;
    async fn queue_line_delete(&self, line_id: &str, date: &str) -> anyhow::Result<()>;
    async fn finish_line_delete(&self, line_id: &str) -> anyhow::Result<()>;
    /// Paamonim line ids already known locally, including ones queued for deletion.
    async fn known_line_ids(&self) -> anyhow::Result<Vec<String>>;
    /// Called once at start when mirroring is on: the first time, whatever is unsent or
    /// edited so far (made while mirroring was off) is excluded from mirroring, so turning it
    /// on never replays history. Returns how many transactions were set aside.
    async fn baseline_mirror(&self) -> anyhow::Result<usize>;
    /// Called at start when mirroring is off, so the next time it is turned on starts afresh.
    async fn clear_mirror_baseline(&self) -> anyhow::Result<()>;
    /// Idempotent: lookups are upserted by key, transactions skipped if their line id exists.
    async fn import_paamonim(&self, batch: &ImportBatch) -> anyhow::Result<ImportSummary>;
    async fn list_transactions(&self) -> anyhow::Result<Vec<Transaction>>;
    /// Transactions dated `from` (YYYY-MM-DD) or later, newest first. Backends should filter
    /// in the database; this fallback filters after reading everything.
    async fn list_transactions_from(&self, from: &str) -> anyhow::Result<Vec<Transaction>> {
        let mut all = self.list_transactions().await?;
        all.retain(|t| t.date.as_str() >= from);
        Ok(all)
    }
    async fn create_transaction(&self, t: &NewTransaction) -> anyhow::Result<i64>;
    async fn delete_transaction(&self, id: i64) -> anyhow::Result<()>;
    /// Local edit. A line already in Paamonim is flagged `modified` so mirroring replaces it.
    async fn update_transaction(&self, id: i64, t: &NewTransaction) -> anyhow::Result<()>;
    /// Live categories only; ones deleted locally are hidden.
    async fn list_categories(&self) -> anyhow::Result<Vec<Category>>;
    /// Raw insert; use [`Storage::create_category`], which validates.
    async fn insert_category(
        &self,
        name: &str,
        is_income: bool,
        parent_id: Option<i64>,
    ) -> anyhow::Result<i64>;
    /// Raw removal of `ids` (sub-categories before their parent), after moving their
    /// transactions to `move_to`, in one step where the backend can. Categories that came
    /// from Paamonim are kept as hidden tombstones (pointing at `move_to`) so that a
    /// re-import neither recreates them nor loses where their lines now belong; local-only
    /// ones are deleted outright. Use [`Storage::delete_category`], which validates.
    async fn remove_categories(&self, ids: &[i64], move_to: Option<i64>) -> anyhow::Result<()>;
    /// Raw: set the Paamonim category a top-level category made here stands in for. Use
    /// [`Storage::create_category`] or [`Storage::link_category_to`], which validate.
    async fn set_category_link(&self, id: i64, linked_to: Option<i64>) -> anyhow::Result<()>;
    /// Raw: put a sub-category under another top-level category; its transactions and rules
    /// stay on it. Use [`Storage::move_category`], which validates.
    async fn set_category_parent(&self, id: i64, parent_id: i64) -> anyhow::Result<()>;

    /// Check `linked_to` for a top-level category of kind `is_income` (other than `own`).
    /// Required when there is a Paamonim top-level category of that kind to link to.
    fn check_link(
        &self,
        all: &[Category],
        own: Option<i64>,
        is_income: bool,
        linked_to: Option<i64>,
    ) -> Result<(), CategoryError> {
        let candidate = |c: &&Category| {
            c.parent_id.is_none()
                && c.from_paamonim
                && c.is_income == is_income
                && Some(c.id) != own
        };
        match linked_to {
            Some(l) => {
                if !all.iter().filter(candidate).any(|c| c.id == l) {
                    return Err(CategoryError::Invalid(
                        "link to a Paamonim top-level category of the same kind".into(),
                    ));
                }
            }
            None => {
                if all.iter().any(|c| candidate(&c)) {
                    return Err(CategoryError::Invalid(
                        "choose the Paamonim category this category belongs to".into(),
                    ));
                }
            }
        }
        Ok(())
    }

    /// Change the Paamonim category a top-level category made here stands in for. Refused
    /// while one of its sub-categories is an item in Paamonim, whose place there is fixed.
    async fn link_category_to(&self, id: i64, linked_to: Option<i64>) -> anyhow::Result<()> {
        let all = self.list_categories().await?;
        let cat = all
            .iter()
            .find(|c| c.id == id)
            .ok_or(CategoryError::NotFound)?;
        if cat.parent_id.is_some() || cat.from_paamonim {
            return Err(CategoryError::Invalid(
                "only a top-level category made here can be linked".into(),
            )
            .into());
        }
        if cat.linked_to == linked_to {
            return Ok(());
        }
        self.check_link(&all, Some(id), cat.is_income, linked_to)?;
        if all
            .iter()
            .any(|c| c.parent_id == Some(id) && c.from_paamonim)
        {
            return Err(CategoryError::Invalid(
                "a sub-category of this category is an item in Paamonim; move it out first".into(),
            )
            .into());
        }
        self.set_category_link(id, linked_to).await
    }

    /// Put sub-category `id` under top-level category `to`, with its transactions and rules.
    /// A sub-category that is an item in Paamonim can only move to a category that goes under
    /// the same Paamonim category, so nothing changes there.
    async fn move_category(&self, id: i64, to: i64) -> anyhow::Result<()> {
        let all = self.list_categories().await?;
        let cat = all
            .iter()
            .find(|c| c.id == id)
            .ok_or(CategoryError::NotFound)?;
        let Some(from) = cat.parent_id.and_then(|p| all.iter().find(|c| c.id == p)) else {
            return Err(CategoryError::Invalid("only a sub-category can be moved".into()).into());
        };
        let target = all.iter().find(|c| c.id == to).ok_or_else(|| {
            CategoryError::Invalid("the category to move to was not found".into())
        })?;
        if target.parent_id.is_some() {
            return Err(CategoryError::Invalid("move it under a top-level category".into()).into());
        }
        if target.is_income != cat.is_income {
            return Err(CategoryError::Invalid(
                "cannot move between income and expense categories".into(),
            )
            .into());
        }
        if target.id == from.id {
            return Ok(());
        }
        if cat.from_paamonim && target.paamonim_home() != from.paamonim_home() {
            return Err(CategoryError::Invalid(
                "this sub-category is an item in Paamonim; it can only move to a category \
                 linked to the same Paamonim category"
                    .into(),
            )
            .into());
        }
        if target.paamonim_home() != from.paamonim_home()
            && self.list_transactions().await?.iter().any(|t| {
                t.category_id == Some(id)
                    && matches!(
                        t.sync_state,
                        SyncState::Synced | SyncState::Modified | SyncState::Verifying
                    )
            })
        {
            return Err(CategoryError::Invalid(
                "some of its lines are already in Paamonim under another category; it can only \
                 move to a category linked to the same Paamonim category"
                    .into(),
            )
            .into());
        }
        if all
            .iter()
            .any(|c| c.parent_id == Some(to) && c.name == cat.name)
        {
            return Err(CategoryError::Invalid(
                "the target category already has a sub-category with this name".into(),
            )
            .into());
        }
        self.set_category_parent(id, to).await
    }

    async fn create_category(&self, c: &NewCategory) -> anyhow::Result<i64> {
        let name = c.name.trim();
        if name.is_empty() || name.chars().count() > 100 {
            return Err(CategoryError::Invalid("name must be 1-100 characters".into()).into());
        }
        let all = self.list_categories().await?;
        let is_income = match c.parent_id {
            Some(pid) => {
                let parent = all
                    .iter()
                    .find(|p| p.id == pid)
                    .ok_or_else(|| CategoryError::Invalid("parent category not found".into()))?;
                if parent.parent_id.is_some() {
                    return Err(CategoryError::Invalid(
                        "categories have two levels; a sub-category cannot have its own".into(),
                    )
                    .into());
                }
                parent.is_income
            }
            None => {
                self.check_link(&all, None, c.is_income, c.linked_to)?;
                c.is_income
            }
        };
        if c.parent_id.is_some() && c.linked_to.is_some() {
            return Err(CategoryError::Invalid(
                "a sub-category takes its parent's Paamonim category".into(),
            )
            .into());
        }
        if all
            .iter()
            .any(|o| o.parent_id == c.parent_id && o.is_income == is_income && o.name == name)
        {
            return Err(
                CategoryError::Invalid("a category with this name already exists".into()).into(),
            );
        }
        let id = self.insert_category(name, is_income, c.parent_id).await?;
        if c.linked_to.is_some() {
            self.set_category_link(id, c.linked_to).await?;
        }
        Ok(id)
    }

    /// Delete a category together with its sub-categories. Their transactions go to
    /// `move_to` along with the repeating rules that use them; it is required when there are
    /// any. Returns how many transactions were moved.
    async fn delete_category(&self, id: i64, move_to: Option<i64>) -> anyhow::Result<usize> {
        let all = self.list_categories().await?;
        let cat = all
            .iter()
            .find(|c| c.id == id)
            .ok_or(CategoryError::NotFound)?;
        let linking: Vec<&str> = all
            .iter()
            .filter(|c| c.linked_to == Some(id))
            .map(|c| c.name.as_str())
            .collect();
        if !linking.is_empty() {
            return Err(CategoryError::Invalid(format!(
                "categories linked to this one in Paamonim: {}; link them elsewhere first",
                linking.join(", ")
            ))
            .into());
        }
        // Sub-categories first, so a parent is never removed while children point at it.
        let mut ids: Vec<i64> = all
            .iter()
            .filter(|c| c.parent_id == Some(id))
            .map(|c| c.id)
            .collect();
        ids.push(id);
        let moved = self
            .list_transactions()
            .await?
            .iter()
            .filter(|t| t.category_id.is_some_and(|c| ids.contains(&c)))
            .count();
        // Segments of one series count once.
        let rules = self
            .list_recurring()
            .await?
            .iter()
            .filter(|r| r.category_id.is_some_and(|c| ids.contains(&c)))
            .map(|r| r.series_id)
            .collect::<std::collections::HashSet<_>>()
            .len();
        if let Some(target) = move_to {
            let t = all.iter().find(|c| c.id == target).ok_or_else(|| {
                CategoryError::Invalid("the category to move to was not found".into())
            })?;
            if ids.contains(&target) {
                return Err(CategoryError::Invalid(
                    "cannot move into the category being deleted".into(),
                )
                .into());
            }
            if t.is_income != cat.is_income {
                return Err(CategoryError::Invalid(
                    "cannot move between income and expense categories".into(),
                )
                .into());
            }
        } else if moved > 0 || rules > 0 {
            return Err(CategoryError::NeedsReplacement {
                transactions: moved,
                rules,
            }
            .into());
        }
        self.remove_categories(&ids, move_to).await?;
        Ok(moved)
    }

    /// Repeating-transaction rules (see `recurring.rs`). Backends without support have none.
    async fn list_recurring(&self) -> anyhow::Result<Vec<RecurringSegment>> {
        Ok(Vec::new())
    }
    /// Add a segment and return its id; `series_id` None starts a new series, whose id is
    /// that of its first segment.
    async fn insert_recurring(
        &self,
        _series_id: Option<i64>,
        _r: &NewRecurring,
    ) -> anyhow::Result<i64> {
        anyhow::bail!("repeating transactions are not supported by this storage backend")
    }
    async fn update_recurring(&self, _id: i64, _r: &NewRecurring) -> anyhow::Result<()> {
        anyhow::bail!("repeating transactions are not supported by this storage backend")
    }
    async fn delete_recurring(&self, _id: i64) -> anyhow::Result<()> {
        anyhow::bail!("repeating transactions are not supported by this storage backend")
    }
    /// Turn occurrences that have come due into real transactions (sync state `local`, so
    /// they are not mirrored). Each (series, month) is settled once, even if the transaction
    /// is later deleted. Returns how many were added.
    async fn settle_recurring(&self, _due: &[Occurrence]) -> anyhow::Result<usize> {
        Ok(0)
    }
    /// (series id, YYYY-MM) of every month that has settled.
    async fn settled_recurring(&self) -> anyhow::Result<Vec<(i64, String)>> {
        Ok(Vec::new())
    }
    /// Settle one month of a series early, with these values instead of the rule's (a salary
    /// that came early, a bill that differed), or with `None` to skip the month. False if the
    /// month had already settled.
    async fn settle_recurring_month(
        &self,
        _series_id: i64,
        _month: &str,
        _t: Option<&NewTransaction>,
        _fx: Option<&TxFx>,
    ) -> anyhow::Result<bool> {
        anyhow::bail!("repeating transactions are not supported by this storage backend")
    }
    /// Operation log (see `oplog.rs`). Backends without support keep none.
    async fn log_append(&self, _e: &NewLogEntry) -> anyhow::Result<()> {
        Ok(())
    }
    /// Newest first; with `before_id`, only entries older than that one.
    async fn list_log(
        &self,
        _limit: usize,
        _before_id: Option<i64>,
    ) -> anyhow::Result<Vec<LogEntry>> {
        Ok(Vec::new())
    }
    /// Entries of one kind stamped from `from` up to (not including) `before` (UTC timestamps),
    /// newest first.
    async fn list_log_kind(
        &self,
        _kind: &str,
        _from: &str,
        _before: &str,
    ) -> anyhow::Result<Vec<LogEntry>> {
        Ok(Vec::new())
    }
    /// Change-log entries matching a filter, newest first, one page (see [`LogFilter`]).
    async fn list_log_filtered(
        &self,
        _f: &LogFilter,
        _limit: usize,
        _before_id: Option<i64>,
    ) -> anyhow::Result<Vec<LogEntry>> {
        Ok(Vec::new())
    }
    /// The Paamonim link of each given transaction (only those that have one).
    async fn mirror_links(&self, _ids: &[i64]) -> anyhow::Result<Vec<MirrorLink>> {
        Ok(Vec::new())
    }
    /// The email rows that made each given transaction, without the stored mail text.
    async fn emails_for_transactions(&self, _ids: &[i64]) -> anyhow::Result<Vec<EmailSuggestion>> {
        Ok(Vec::new())
    }
    /// Distinct emails that appear on recorded changes, sorted.
    async fn log_users(&self) -> anyhow::Result<Vec<String>> {
        Ok(Vec::new())
    }
    /// Delete entries stamped before `before` (UTC, `YYYY-MM-DDTHH:MM:SSZ`); returns how many.
    async fn prune_log(&self, _before: &str) -> anyhow::Result<usize> {
        Ok(0)
    }
    /// Computed on demand from transactions. Fast enough at this data size; add
    /// pre-aggregation (or a date index) if a backend gets slow with a lot of history.
    async fn monthly_totals(&self) -> anyhow::Result<Vec<MonthlyTotal>> {
        let mut by_key: std::collections::BTreeMap<(String, Option<i64>), (i64, i64)> =
            Default::default();
        for t in self.list_transactions().await? {
            let e = by_key
                .entry((t.date[..7].to_string(), t.category_id))
                .or_default();
            e.0 += t.amount_agorot;
            e.1 += 1;
        }
        let mut totals: Vec<MonthlyTotal> = by_key
            .into_iter()
            .map(
                |((month, category_id), (total_agorot, count))| MonthlyTotal {
                    month,
                    category_id,
                    total_agorot,
                    count,
                },
            )
            .collect();
        totals.sort_by(|a, b| {
            b.month
                .cmp(&a.month)
                .then(a.category_id.cmp(&b.category_id))
        });
        Ok(totals)
    }
}
