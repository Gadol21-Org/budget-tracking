//! Storage boundary. The app talks only to [`Storage`], so SQLite (local/tests)
//! can be swapped for Postgres on Oracle Cloud without touching the API.

use std::sync::Mutex;

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::oplog::{LogEntry, NewLogEntry};
use crate::recurring::{NewRecurring, Occurrence, RecurringSegment};

#[derive(Serialize, Clone)]
pub struct Transaction {
    pub id: i64,
    pub date: String,
    pub amount_agorot: i64,
    pub category_id: Option<i64>,
    pub payment_method_id: Option<i64>,
    pub note: String,
    pub sync_state: String,
    /// Set on a settled occurrence of a repeating transaction.
    pub recurring_series_id: Option<i64>,
}

#[derive(Deserialize)]
pub struct NewTransaction {
    pub date: String,
    pub amount_agorot: i64,
    pub category_id: Option<i64>,
    pub payment_method_id: Option<i64>,
    #[serde(default)]
    pub note: String,
}

/// The original foreign-currency amount of a transaction whose `amount_agorot` is the shekel
/// value. Local only: Paamonim and the totals only ever see the shekel amount.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct TxFx {
    pub transaction_id: i64,
    /// `USD` or `JPY`.
    pub currency: String,
    /// Signed like `amount_agorot`, in the currency's minor unit (cents; whole yen).
    pub orig_minor: i64,
    /// Shekels per one unit of the currency.
    pub rate: f64,
    /// The day the rate is for (the day it was set, for a manual one).
    pub rate_date: String,
    /// The rate is a stand-in (today's or fallback one); it is replaced once the day has closed.
    pub estimated: bool,
}

/// One business of the whole business. The `is_general` one holds shared costs.
#[derive(Serialize, Clone)]
pub struct Business {
    pub id: i64,
    pub name: String,
    pub is_general: bool,
}

/// What counts in which business: explicit tags on transactions, and categories whose
/// transactions count in a business unless tagged otherwise.
#[derive(Serialize, Default, Clone)]
pub struct BusinessLinks {
    /// (transaction id, business id)
    pub transactions: Vec<(i64, i64)>,
    /// (category id, business id)
    pub categories: Vec<(i64, i64)>,
    /// Categories whose transactions must say which business they belong to.
    pub required: Vec<i64>,
}

#[derive(Serialize, Clone)]
pub struct PaymentMethod {
    pub id: i64,
    pub name: String,
}

#[derive(Serialize, Clone)]
pub struct Category {
    pub id: i64,
    pub name: String,
    pub is_income: bool,
    /// Top-level categories have none; transactions are normally on the children.
    pub parent_id: Option<i64>,
    /// Came from Paamonim (a Paamonim category, or an item there for a sub-category).
    pub from_paamonim: bool,
    /// On a top-level category made here: the Paamonim top-level category it stands in for.
    /// Its sub-categories are mirrored as items under that one.
    pub linked_to: Option<i64>,
}

impl Category {
    /// The Paamonim top-level category this top-level category's lines go under, if any.
    pub fn paamonim_home(&self) -> Option<i64> {
        if self.parent_id.is_some() {
            return None;
        }
        if self.from_paamonim {
            Some(self.id)
        } else {
            self.linked_to
        }
    }
}

#[derive(Deserialize)]
pub struct NewCategory {
    pub name: String,
    /// Ignored for sub-categories, which take their parent's kind.
    #[serde(default)]
    pub is_income: bool,
    pub parent_id: Option<i64>,
    /// For a top-level category: the Paamonim top-level category it stands in for. Required
    /// when there are Paamonim categories of its kind, since Paamonim can't take new ones.
    #[serde(default)]
    pub linked_to: Option<i64>,
}

/// Why a category change was refused; the API maps these to 4xx responses.
#[derive(Debug, PartialEq, Eq)]
pub enum CategoryError {
    NotFound,
    Invalid(String),
    /// The category (or a sub-category) has transactions or repeating rules and no `move_to`
    /// was given.
    NeedsReplacement {
        transactions: usize,
        rules: usize,
    },
}

impl std::fmt::Display for CategoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(f, "category not found"),
            Self::Invalid(m) => write!(f, "{m}"),
            Self::NeedsReplacement {
                transactions,
                rules,
            } => write!(
                f,
                "category has {transactions} transactions and {rules} repeating rules; \
                 choose a category to move them to"
            ),
        }
    }
}

impl std::error::Error for CategoryError {}

/// Everything the plain lists leave out, for a full backup: deleted/merged categories, the
/// Paamonim ids, which Paamonim line each transaction is linked to, and the mirror metadata.
#[derive(Default)]
pub struct RawBackup {
    pub categories: Vec<RawCategory>,
    pub payment_methods: Vec<RawPaymentMethod>,
    /// (transaction id, Paamonim line id, mirror date)
    pub tx_links: Vec<(i64, Option<String>, Option<String>)>,
    /// `mirror_meta` key/value pairs.
    pub meta: Vec<(String, String)>,
}

pub struct RawCategory {
    pub id: i64,
    pub name: String,
    pub is_income: bool,
    pub paamonim_item_id: Option<String>,
    pub parent_id: Option<i64>,
    pub deleted: bool,
    pub merged_into: Option<i64>,
    pub linked_to: Option<i64>,
}

pub struct RawPaymentMethod {
    pub id: i64,
    pub name: String,
    pub paamonim_id: Option<String>,
}

/// Per-month, per-category totals, computed from transactions on demand.
#[derive(Serialize)]
pub struct MonthlyTotal {
    pub month: String, // YYYY-MM
    pub category_id: Option<i64>,
    pub total_agorot: i64,
    pub count: i64,
}

/// One Paamonim import run. Keys are Paamonim ids (items are `e:<id>` / `i:<id>` since
/// expense and income ids are separate spaces). Amounts are signed agorot: expenses negative.
pub struct ImportCategory {
    pub key: String,
    pub name: String,
    pub is_income: bool,
    /// Key of a parent that appears earlier in the batch.
    pub parent_key: Option<String>,
}

pub struct ImportPaymentMethod {
    pub key: String,
    pub name: String,
}

pub struct ImportTransaction {
    pub line_id: String,
    pub date: String,
    pub amount_agorot: i64,
    pub category_key: Option<String>,
    pub payment_key: Option<String>,
    pub note: String,
}

#[derive(Default)]
pub struct ImportBatch {
    pub categories: Vec<ImportCategory>,
    pub payment_methods: Vec<ImportPaymentMethod>,
    pub transactions: Vec<ImportTransaction>,
}

#[derive(Debug, Default, Serialize, PartialEq, Eq)]
pub struct ImportSummary {
    pub categories_added: usize,
    pub payment_methods_added: usize,
    pub transactions_added: usize,
    /// Already imported (same Paamonim line id); import is idempotent.
    pub transactions_skipped: usize,
    /// Lines we posted whose id was never recorded; linked to the local transaction (exact
    /// amount kept) instead of added again.
    pub transactions_adopted: usize,
}

/// A transaction whose Paamonim copy is missing or out of date.
///
/// `state` says which: `pending` (never sent), `modified` (edited after it was sent; the old
/// line in Paamonim has to be replaced) or `verifying` (a request may have been sent but its
/// outcome wasn't confirmed, so it must be checked before anything is posted again).
#[derive(Debug, Clone)]
pub struct PendingMirror {
    pub id: i64,
    pub state: String,
    pub date: String,
    pub amount_agorot: i64,
    pub note: String,
    /// `e:<id>` / `i:<id>` of the Paamonim budget item; None for categories made locally.
    pub item_key: Option<String>,
    /// Name of the local category, which is the Paamonim item name for imported ones.
    pub item_name: Option<String>,
    pub payment_key: Option<String>,
    /// The Paamonim line this transaction is linked to, if any.
    pub line_id: Option<String>,
    pub category_id: Option<i64>,
    /// Key of the category's parent (`ce:<id>` / `ci:<id>` for a Paamonim category), used to
    /// create a Paamonim item for a sub-category made here.
    pub parent_key: Option<String>,
    pub is_income: bool,
}

/// A line in Paamonim that no longer has a local transaction (deleted or replaced locally).
/// `date` is the day it has in Paamonim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineDelete {
    pub line_id: String,
    pub date: String,
}

/// True when `local` (exact agorot) is what Paamonim would hold for `imported` (whole shekels
/// as agorot, signed): lets a line we posted be recognised on import despite rounding.
pub fn rounds_to(local: i64, imported: i64) -> bool {
    (local < 0) == (imported < 0)
        && crate::money::to_paamonim_shekels(local.abs()) * 100 == imported.abs()
}

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
    async fn set_mirror_state(&self, id: i64, state: &str) -> anyhow::Result<()>;
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
                    && matches!(t.sync_state.as_str(), "synced" | "modified" | "verifying")
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

pub struct SqliteStorage(Mutex<Connection>);

/// Where lines of category `id` belong: itself, or what it was merged into when deleted
/// (None if it was deleted without a replacement).
/// See [`Storage::requeue_skipped_sub_categories`]. A category goes under a Paamonim category
/// directly or through the category its parent is linked to.
pub const REQUEUE_SKIPPED_SQL: &str = "UPDATE transactions SET sync_state = 'pending'
     WHERE sync_state IN ('skipped', 'error') AND paamonim_line_id IS NULL AND category_id IN (
       SELECT c.id FROM categories c JOIN categories pc ON pc.id = c.parent_id
       LEFT JOIN categories lc ON lc.id = pc.linked_to AND lc.deleted = 0
       WHERE c.paamonim_item_id IS NULL AND c.deleted = 0 AND pc.deleted = 0
         AND (pc.paamonim_item_id LIKE 'ce:%' OR pc.paamonim_item_id LIKE 'ci:%'
              OR lc.paamonim_item_id LIKE 'ce:%' OR lc.paamonim_item_id LIKE 'ci:%'))";

fn requeue_skipped(conn: &Connection) -> anyhow::Result<usize> {
    Ok(conn.execute(REQUEUE_SKIPPED_SQL, [])?)
}

fn resolve_category(conn: &Connection, mut id: i64) -> anyhow::Result<Option<i64>> {
    for _ in 0..16 {
        let (deleted, into): (i64, Option<i64>) = conn.query_row(
            "SELECT deleted, merged_into FROM categories WHERE id = ?1",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if deleted == 0 {
            return Ok(Some(id));
        }
        match into {
            Some(next) => id = next,
            None => return Ok(None),
        }
    }
    Ok(None)
}

impl SqliteStorage {
    pub fn open(path: &str) -> anyhow::Result<Self> {
        Ok(Self(Mutex::new(crate::db::open(path)?)))
    }
}

#[async_trait::async_trait]
impl Storage for SqliteStorage {
    async fn list_payment_methods(&self) -> anyhow::Result<Vec<PaymentMethod>> {
        let conn = self.0.lock().unwrap();
        let mut stmt = conn.prepare("SELECT id, name FROM payment_methods ORDER BY name")?;
        let rows = stmt
            .query_map([], |r| {
                Ok(PaymentMethod {
                    id: r.get(0)?,
                    name: r.get(1)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    async fn list_businesses(&self) -> anyhow::Result<Vec<Business>> {
        let conn = self.0.lock().unwrap();
        let mut stmt = conn.prepare("SELECT id, name, is_general FROM businesses ORDER BY id")?;
        let rows = stmt
            .query_map([], |r| {
                Ok(Business {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    is_general: r.get::<_, i64>(2)? != 0,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    async fn insert_business(&self, name: &str, is_general: bool) -> anyhow::Result<i64> {
        let conn = self.0.lock().unwrap();
        conn.execute(
            "INSERT INTO businesses (name, is_general) VALUES (?1, ?2)",
            params![name, is_general],
        )?;
        Ok(conn.last_insert_rowid())
    }

    async fn rename_business(&self, id: i64, name: &str) -> anyhow::Result<()> {
        self.0.lock().unwrap().execute(
            "UPDATE businesses SET name = ?2 WHERE id = ?1",
            params![id, name],
        )?;
        Ok(())
    }

    async fn remove_business(&self, id: i64) -> anyhow::Result<()> {
        let mut conn = self.0.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM transaction_business WHERE business_id = ?1",
            params![id],
        )?;
        tx.execute(
            "DELETE FROM category_business WHERE business_id = ?1",
            params![id],
        )?;
        tx.execute("DELETE FROM businesses WHERE id = ?1", params![id])?;
        tx.commit()?;
        Ok(())
    }

    async fn business_links(&self) -> anyhow::Result<BusinessLinks> {
        let conn = self.0.lock().unwrap();
        let mut out = BusinessLinks::default();
        let mut stmt = conn.prepare(
            "SELECT b.transaction_id, b.business_id FROM transaction_business b
             JOIN transactions t ON t.id = b.transaction_id",
        )?;
        out.transactions = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        let mut stmt = conn.prepare("SELECT category_id, business_id FROM category_business")?;
        out.categories = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        let mut stmt = conn.prepare("SELECT category_id FROM category_business_required")?;
        out.required = stmt
            .query_map([], |r| r.get(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(out)
    }

    async fn set_category_business_required(
        &self,
        category: i64,
        required: bool,
    ) -> anyhow::Result<()> {
        let conn = self.0.lock().unwrap();
        if required {
            conn.execute(
                "INSERT OR IGNORE INTO category_business_required (category_id) VALUES (?1)",
                params![category],
            )?;
        } else {
            conn.execute(
                "DELETE FROM category_business_required WHERE category_id = ?1",
                params![category],
            )?;
        }
        Ok(())
    }

    async fn set_category_business(
        &self,
        category: i64,
        business: Option<i64>,
    ) -> anyhow::Result<()> {
        let conn = self.0.lock().unwrap();
        match business {
            Some(b) => conn.execute(
                "INSERT INTO category_business (category_id, business_id) VALUES (?1, ?2)
                 ON CONFLICT(category_id) DO UPDATE SET business_id = excluded.business_id",
                params![category, b],
            )?,
            None => conn.execute(
                "DELETE FROM category_business WHERE category_id = ?1",
                params![category],
            )?,
        };
        Ok(())
    }

    async fn set_tx_fx(&self, id: i64, fx: Option<&TxFx>) -> anyhow::Result<()> {
        let conn = self.0.lock().unwrap();
        match fx {
            Some(f) => conn.execute(
                "INSERT INTO transaction_fx (transaction_id, currency, orig_minor, rate, rate_date, estimated)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(transaction_id) DO UPDATE SET currency = excluded.currency,
                   orig_minor = excluded.orig_minor, rate = excluded.rate,
                   rate_date = excluded.rate_date, estimated = excluded.estimated",
                params![id, f.currency, f.orig_minor, f.rate, f.rate_date, f.estimated],
            )?,
            None => conn.execute(
                "DELETE FROM transaction_fx WHERE transaction_id = ?1",
                params![id],
            )?,
        };
        Ok(())
    }

    async fn list_tx_fx(&self) -> anyhow::Result<Vec<TxFx>> {
        let conn = self.0.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT f.transaction_id, f.currency, f.orig_minor, f.rate, f.rate_date, f.estimated
             FROM transaction_fx f JOIN transactions t ON t.id = f.transaction_id
             ORDER BY f.transaction_id",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok(TxFx {
                    transaction_id: r.get(0)?,
                    currency: r.get(1)?,
                    orig_minor: r.get(2)?,
                    rate: r.get(3)?,
                    rate_date: r.get(4)?,
                    estimated: r.get(5)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    async fn get_fx_rate(&self, currency: &str, date: &str) -> anyhow::Result<Option<f64>> {
        let conn = self.0.lock().unwrap();
        Ok(conn
            .query_row(
                "SELECT rate FROM fx_rates WHERE currency = ?1 AND date = ?2",
                params![currency, date],
                |r| r.get(0),
            )
            .optional()?)
    }

    async fn put_fx_rate(&self, currency: &str, date: &str, rate: f64) -> anyhow::Result<()> {
        self.0.lock().unwrap().execute(
            "INSERT INTO fx_rates (currency, date, rate) VALUES (?1, ?2, ?3)
             ON CONFLICT(currency, date) DO UPDATE SET rate = excluded.rate",
            params![currency, date, rate],
        )?;
        Ok(())
    }

    async fn set_transactions_business(
        &self,
        ids: &[i64],
        business: Option<i64>,
    ) -> anyhow::Result<()> {
        let mut conn = self.0.lock().unwrap();
        let tx = conn.transaction()?;
        for id in ids {
            match business {
                Some(b) => tx.execute(
                    "INSERT INTO transaction_business (transaction_id, business_id) VALUES (?1, ?2)
                     ON CONFLICT(transaction_id) DO UPDATE SET business_id = excluded.business_id",
                    params![id, b],
                )?,
                None => tx.execute(
                    "DELETE FROM transaction_business WHERE transaction_id = ?1",
                    params![id],
                )?,
            };
        }
        tx.commit()?;
        Ok(())
    }

    async fn pending_mirrors(&self) -> anyhow::Result<Vec<PendingMirror>> {
        let conn = self.0.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT t.id, t.sync_state, t.date, t.amount_agorot, t.note,
                    COALESCE(c.paamonim_item_id, (SELECT s.paamonim_item_id FROM categories s
                       JOIN categories sp ON sp.id = s.parent_id AND sp.deleted = 0
                       WHERE s.deleted = 0 AND s.id <> c.id AND s.name = c.name
                         AND s.is_income = c.is_income
                         AND (s.paamonim_item_id LIKE 'e:%' OR s.paamonim_item_id LIKE 'i:%')
                         AND COALESCE(sp.linked_to, sp.id) = COALESCE(pc.linked_to, pc.id)
                       ORDER BY s.id LIMIT 1)),
                    c.name, p.paamonim_id, t.paamonim_line_id, t.category_id,
                    COALESCE(pc.paamonim_item_id, lc.paamonim_item_id),
                    COALESCE(c.is_income, 0)
             FROM transactions t
             LEFT JOIN categories c ON c.id = t.category_id
             LEFT JOIN categories pc ON pc.id = c.parent_id AND pc.deleted = 0
             LEFT JOIN categories lc ON lc.id = pc.linked_to AND lc.deleted = 0
             LEFT JOIN payment_methods p ON p.id = t.payment_method_id
             WHERE t.sync_state IN ('pending', 'modified', 'verifying') ORDER BY t.id",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok(PendingMirror {
                    id: r.get(0)?,
                    state: r.get(1)?,
                    date: r.get(2)?,
                    amount_agorot: r.get(3)?,
                    note: r.get(4)?,
                    item_key: r.get(5)?,
                    item_name: r.get(6)?,
                    payment_key: r.get(7)?,
                    line_id: r.get(8)?,
                    category_id: r.get(9)?,
                    parent_key: r.get(10)?,
                    is_income: r.get::<_, i64>(11)? != 0,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    async fn set_mirror_state(&self, id: i64, state: &str) -> anyhow::Result<()> {
        self.0.lock().unwrap().execute(
            "UPDATE transactions SET sync_state = ?2 WHERE id = ?1",
            params![id, state],
        )?;
        Ok(())
    }

    async fn mark_mirrored(&self, p: &PendingMirror, line_id: &str) -> anyhow::Result<bool> {
        let mut conn = self.0.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT OR IGNORE INTO mirror_deletes (line_id, date)
             SELECT paamonim_line_id, COALESCE(mirror_date, date) FROM transactions
             WHERE id = ?1 AND paamonim_line_id IS NOT NULL AND paamonim_line_id <> ?2",
            params![p.id, line_id],
        )?;
        let n = tx.execute(
            "UPDATE transactions SET paamonim_line_id = ?2, mirror_date = ?3,
               sync_state = CASE WHEN date = ?3 AND amount_agorot = ?4 AND note = ?5
                            THEN 'synced' ELSE 'modified' END
             WHERE id = ?1",
            params![p.id, line_id, p.date, p.amount_agorot, p.note],
        )?;
        tx.commit()?;
        Ok(n == 1)
    }

    async fn link_category(&self, id: i64, key: &str) -> anyhow::Result<bool> {
        let mut conn = self.0.lock().unwrap();
        let tx = conn.transaction()?;
        // An item another category already holds (one of the same name under a category
        // linked to the same Paamonim category) stays with it; lines are still sent there.
        let taken = tx
            .prepare("SELECT 1 FROM categories WHERE paamonim_item_id = ?1")?
            .exists(params![key])?;
        let n = if taken {
            0
        } else {
            tx.execute(
                "UPDATE categories SET paamonim_item_id = ?2 WHERE id = ?1 AND paamonim_item_id IS NULL",
                params![id, key],
            )?
        };
        if n == 1 {
            tx.execute(
                "UPDATE transactions SET sync_state = 'pending'
                 WHERE category_id = ?1 AND sync_state = 'skipped'",
                params![id],
            )?;
        }
        tx.commit()?;
        Ok(n == 1)
    }

    async fn detach_mirror(&self, id: i64) -> anyhow::Result<()> {
        let mut conn = self.0.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT OR IGNORE INTO mirror_deletes (line_id, date)
             SELECT paamonim_line_id, COALESCE(mirror_date, date) FROM transactions
             WHERE id = ?1 AND paamonim_line_id IS NOT NULL",
            params![id],
        )?;
        tx.execute(
            "UPDATE transactions SET paamonim_line_id = NULL, mirror_date = NULL,
               sync_state = 'skipped' WHERE id = ?1",
            params![id],
        )?;
        tx.commit()?;
        Ok(())
    }

    async fn retry_mirror(&self, id: i64) -> anyhow::Result<bool> {
        let n = self.0.lock().unwrap().execute(
            "UPDATE transactions SET sync_state =
               CASE WHEN paamonim_line_id IS NULL THEN 'pending' ELSE 'modified' END
             WHERE id = ?1 AND sync_state = 'error'",
            params![id],
        )?;
        Ok(n == 1)
    }

    async fn requeue_skipped_sub_categories(&self) -> anyhow::Result<usize> {
        requeue_skipped(&self.0.lock().unwrap())
    }

    async fn raw_backup(&self) -> anyhow::Result<RawBackup> {
        let conn = self.0.lock().unwrap();
        let categories = conn
            .prepare(
                "SELECT id, name, is_income, paamonim_item_id, parent_id, deleted, merged_into,
                        linked_to
                 FROM categories ORDER BY id",
            )?
            .query_map([], |r| {
                Ok(RawCategory {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    is_income: r.get::<_, i64>(2)? != 0,
                    paamonim_item_id: r.get(3)?,
                    parent_id: r.get(4)?,
                    deleted: r.get::<_, i64>(5)? != 0,
                    merged_into: r.get(6)?,
                    linked_to: r.get(7)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let payment_methods = conn
            .prepare("SELECT id, name, paamonim_id FROM payment_methods ORDER BY id")?
            .query_map([], |r| {
                Ok(RawPaymentMethod {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    paamonim_id: r.get(2)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let tx_links = conn
            .prepare("SELECT id, paamonim_line_id, mirror_date FROM transactions ORDER BY id")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        let meta = conn
            .prepare("SELECT k, v FROM mirror_meta ORDER BY k")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(RawBackup {
            categories,
            payment_methods,
            tx_links,
            meta,
        })
    }

    async fn pending_line_deletes(&self) -> anyhow::Result<Vec<LineDelete>> {
        let conn = self.0.lock().unwrap();
        let mut stmt = conn.prepare("SELECT line_id, date FROM mirror_deletes ORDER BY rowid")?;
        let rows = stmt
            .query_map([], |r| {
                Ok(LineDelete {
                    line_id: r.get(0)?,
                    date: r.get(1)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    async fn queue_line_delete(&self, line_id: &str, date: &str) -> anyhow::Result<()> {
        self.0.lock().unwrap().execute(
            "INSERT OR IGNORE INTO mirror_deletes (line_id, date) VALUES (?1, ?2)",
            params![line_id, date],
        )?;
        Ok(())
    }

    async fn finish_line_delete(&self, line_id: &str) -> anyhow::Result<()> {
        self.0.lock().unwrap().execute(
            "DELETE FROM mirror_deletes WHERE line_id = ?1",
            params![line_id],
        )?;
        Ok(())
    }

    async fn known_line_ids(&self) -> anyhow::Result<Vec<String>> {
        let conn = self.0.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT paamonim_line_id FROM transactions WHERE paamonim_line_id IS NOT NULL
             UNION SELECT line_id FROM mirror_deletes",
        )?;
        let ids = stmt
            .query_map([], |r| r.get(0))?
            .collect::<Result<Vec<String>, _>>()?;
        Ok(ids)
    }

    async fn baseline_mirror(&self) -> anyhow::Result<usize> {
        let mut conn = self.0.lock().unwrap();
        let tx = conn.transaction()?;
        let done = tx
            .prepare("SELECT 1 FROM mirror_meta WHERE k = 'baseline'")?
            .exists([])?;
        let mut n = 0;
        if !done {
            // Imported lines keep their Paamonim day from here on.
            tx.execute(
                "UPDATE transactions SET mirror_date = date
                 WHERE paamonim_line_id IS NOT NULL AND mirror_date IS NULL AND sync_state = 'synced'",
                [],
            )?;
            // Edited lines count as in step (their divergence is accepted); unsent ones are
            // set aside as `local` and never sent.
            tx.execute(
                "UPDATE transactions SET sync_state = 'synced'
                 WHERE sync_state = 'modified' AND paamonim_line_id IS NOT NULL",
                [],
            )?;
            n = tx.execute(
                "UPDATE transactions SET sync_state = 'local'
                 WHERE sync_state IN ('pending', 'modified', 'verifying', 'error', 'skipped')",
                [],
            )?;
            tx.execute("DELETE FROM mirror_deletes", [])?;
            tx.execute(
                "INSERT INTO mirror_meta (k, v) VALUES ('baseline', '1')",
                [],
            )?;
        }
        tx.commit()?;
        Ok(n)
    }

    async fn clear_mirror_baseline(&self) -> anyhow::Result<()> {
        self.0
            .lock()
            .unwrap()
            .execute("DELETE FROM mirror_meta WHERE k = 'baseline'", [])?;
        Ok(())
    }

    async fn import_paamonim(&self, batch: &ImportBatch) -> anyhow::Result<ImportSummary> {
        let mut conn = self.0.lock().unwrap();
        let tx = conn.transaction()?;
        let mut sum = ImportSummary::default();
        // `cats`: key -> row id, for linking parents. `live`: key -> where its lines go, which
        // differs for categories deleted locally (their replacement, or none).
        let mut cats = std::collections::HashMap::new();
        let mut live: std::collections::HashMap<String, Option<i64>> = Default::default();
        for c in &batch.categories {
            let existing: Option<(i64, bool)> = tx
                .query_row(
                    "SELECT id, deleted FROM categories WHERE paamonim_item_id = ?1",
                    params![c.key],
                    |r| Ok((r.get(0)?, r.get::<_, i64>(1)? != 0)),
                )
                .ok();
            let parent = c.parent_key.as_ref().and_then(|k| cats.get(k)).copied();
            let id = match existing {
                // Deleted here: stays deleted, and its name/parent are not refreshed.
                Some((id, true)) => id,
                Some((id, false)) => {
                    // A sub-category moved here under a category linked to its Paamonim
                    // category keeps its place.
                    tx.execute(
                        "UPDATE categories SET name = ?2, parent_id = CASE WHEN ?3 IS NOT NULL
                           AND parent_id IN (SELECT id FROM categories WHERE linked_to = ?3)
                           THEN parent_id ELSE ?3 END
                         WHERE id = ?1",
                        params![id, c.name, parent],
                    )?;
                    id
                }
                None => {
                    // New under a parent that was deleted here: deleted too, same destination.
                    let into = parent.map(|p| resolve_category(&tx, p)).transpose()?;
                    let parent_gone = parent.is_some() && into != Some(parent);
                    let into = into.flatten().filter(|_| parent_gone);
                    tx.execute(
                        "INSERT INTO categories (name, is_income, paamonim_item_id, parent_id, deleted, merged_into)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                        params![c.name, c.is_income, c.key, parent, parent_gone, into],
                    )?;
                    sum.categories_added += 1;
                    tx.last_insert_rowid()
                }
            };
            live.insert(c.key.clone(), resolve_category(&tx, id)?);
            cats.insert(c.key.clone(), id);
        }
        let mut pms = std::collections::HashMap::new();
        for p in &batch.payment_methods {
            let existed: Option<i64> = tx
                .query_row(
                    "SELECT id FROM payment_methods WHERE paamonim_id = ?1",
                    params![p.key],
                    |r| r.get(0),
                )
                .ok();
            tx.execute(
                "INSERT INTO payment_methods (name, paamonim_id) VALUES (?1, ?2)
                 ON CONFLICT(paamonim_id) DO UPDATE SET name = excluded.name",
                params![p.name, p.key],
            )?;
            let id: i64 = tx.query_row(
                "SELECT id FROM payment_methods WHERE paamonim_id = ?1",
                params![p.key],
                |r| r.get(0),
            )?;
            sum.payment_methods_added += existed.is_none() as usize;
            pms.insert(p.key.clone(), id);
        }
        for t in &batch.transactions {
            let cat = t
                .category_key
                .as_ref()
                .and_then(|k| live.get(k))
                .copied()
                .flatten();
            let pm = t.payment_key.as_ref().and_then(|k| pms.get(k)).copied();
            // Deleted here and waiting to be deleted there: don't bring it back.
            let queued = tx
                .prepare("SELECT 1 FROM mirror_deletes WHERE line_id = ?1")?
                .exists(params![t.line_id])?;
            if queued {
                sum.transactions_skipped += 1;
                continue;
            }
            let linked = tx
                .prepare("SELECT 1 FROM transactions WHERE paamonim_line_id = ?1")?
                .exists(params![t.line_id])?;
            if !linked {
                // A line we posted whose id we never recorded: link it, keeping the exact
                // local amount, rather than adding it a second time.
                let candidates: Vec<(i64, i64)> = {
                    let mut stmt = tx.prepare(
                        "SELECT id, amount_agorot FROM transactions
                         WHERE paamonim_line_id IS NULL AND sync_state IN ('verifying', 'error')
                           AND date = ?1 AND category_id IS ?2 ORDER BY id",
                    )?;
                    let rows = stmt
                        .query_map(params![t.date, cat], |r| Ok((r.get(0)?, r.get(1)?)))?
                        .collect::<Result<Vec<_>, _>>()?;
                    rows
                };
                if let Some((id, _)) = candidates
                    .into_iter()
                    .find(|(_, a)| rounds_to(*a, t.amount_agorot))
                {
                    tx.execute(
                        "UPDATE transactions SET paamonim_line_id = ?2, mirror_date = date,
                           sync_state = 'synced', payment_method_id = COALESCE(payment_method_id, ?3)
                         WHERE id = ?1",
                        params![id, t.line_id, pm],
                    )?;
                    sum.transactions_adopted += 1;
                    continue;
                }
            }
            let inserted = tx.execute(
                "INSERT OR IGNORE INTO transactions
                   (date, amount_agorot, category_id, payment_method_id, note, paamonim_line_id,
                    mirror_date, sync_state)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?1, 'synced')",
                params![t.date, t.amount_agorot, cat, pm, t.note, t.line_id],
            )?;
            if inserted == 1 {
                sum.transactions_added += 1;
            } else {
                // Already imported: only fill in links that were missing (e.g. before custom
                // items or sub-categories were understood); never overwrite local edits.
                tx.execute(
                    "UPDATE transactions SET category_id = COALESCE(category_id, ?2),
                       payment_method_id = COALESCE(payment_method_id, ?3)
                     WHERE paamonim_line_id = ?1",
                    params![t.line_id, cat, pm],
                )?;
                sum.transactions_skipped += 1;
            }
        }
        tx.commit()?;
        Ok(sum)
    }

    async fn list_transactions(&self) -> anyhow::Result<Vec<Transaction>> {
        let conn = self.0.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, date, amount_agorot, category_id, payment_method_id, note, sync_state,
                    recurring_series_id
             FROM transactions ORDER BY date DESC, id DESC",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok(Transaction {
                    id: r.get(0)?,
                    date: r.get(1)?,
                    amount_agorot: r.get(2)?,
                    category_id: r.get(3)?,
                    payment_method_id: r.get(4)?,
                    note: r.get(5)?,
                    sync_state: r.get(6)?,
                    recurring_series_id: r.get(7)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    async fn create_transaction(&self, t: &NewTransaction) -> anyhow::Result<i64> {
        let mut conn = self.0.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO transactions (date, amount_agorot, category_id, payment_method_id, note)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                t.date,
                t.amount_agorot,
                t.category_id,
                t.payment_method_id,
                t.note
            ],
        )?;
        let id = tx.last_insert_rowid();
        tx.commit()?;
        Ok(id)
    }

    async fn update_transaction(&self, id: i64, t: &NewTransaction) -> anyhow::Result<()> {
        self.0.lock().unwrap().execute(
            // mirror_date keeps the day the line has in Paamonim (SET reads the old `date`).
            "UPDATE transactions SET mirror_date = CASE WHEN paamonim_line_id IS NULL THEN NULL
                                                   ELSE COALESCE(mirror_date, date) END,
               date = ?2, amount_agorot = ?3, category_id = ?4, payment_method_id = ?5, note = ?6,
               sync_state = CASE WHEN sync_state = 'synced' THEN 'modified'
                                 WHEN sync_state = 'skipped' THEN 'pending' ELSE sync_state END
             WHERE id = ?1",
            params![
                id,
                t.date,
                t.amount_agorot,
                t.category_id,
                t.payment_method_id,
                t.note
            ],
        )?;
        Ok(())
    }

    async fn delete_transaction(&self, id: i64) -> anyhow::Result<()> {
        let mut conn = self.0.lock().unwrap();
        let tx = conn.transaction()?;
        // Queue its Paamonim line (if any) for deletion there.
        tx.execute(
            "INSERT OR IGNORE INTO mirror_deletes (line_id, date)
             SELECT paamonim_line_id, COALESCE(mirror_date, date) FROM transactions
             WHERE id = ?1 AND paamonim_line_id IS NOT NULL",
            params![id],
        )?;
        tx.execute("DELETE FROM transactions WHERE id = ?1", params![id])?;
        tx.execute(
            "DELETE FROM transaction_fx WHERE transaction_id = ?1",
            params![id],
        )?;
        tx.commit()?;
        Ok(())
    }

    async fn list_categories(&self) -> anyhow::Result<Vec<Category>> {
        let conn = self.0.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, is_income, parent_id, paamonim_item_id IS NOT NULL, linked_to
             FROM categories WHERE deleted = 0 ORDER BY name",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok(Category {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    is_income: r.get::<_, i64>(2)? != 0,
                    parent_id: r.get(3)?,
                    from_paamonim: r.get::<_, i64>(4)? != 0,
                    linked_to: r.get(5)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    async fn insert_category(
        &self,
        name: &str,
        is_income: bool,
        parent_id: Option<i64>,
    ) -> anyhow::Result<i64> {
        let conn = self.0.lock().unwrap();
        conn.execute(
            "INSERT INTO categories (name, is_income, parent_id) VALUES (?1, ?2, ?3)",
            params![name, is_income, parent_id],
        )?;
        Ok(conn.last_insert_rowid())
    }

    async fn set_category_link(&self, id: i64, linked_to: Option<i64>) -> anyhow::Result<()> {
        let mut conn = self.0.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE categories SET linked_to = ?2 WHERE id = ?1",
            params![id, linked_to],
        )?;
        requeue_skipped(&tx)?;
        tx.commit()?;
        Ok(())
    }

    async fn set_category_parent(&self, id: i64, parent_id: i64) -> anyhow::Result<()> {
        let mut conn = self.0.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE categories SET parent_id = ?2 WHERE id = ?1",
            params![id, parent_id],
        )?;
        requeue_skipped(&tx)?;
        tx.commit()?;
        Ok(())
    }

    async fn remove_categories(&self, ids: &[i64], move_to: Option<i64>) -> anyhow::Result<()> {
        let mut conn = self.0.lock().unwrap();
        let tx = conn.transaction()?;
        for id in ids {
            if let Some(to) = move_to {
                // Like any local edit: a line already in Paamonim now differs from it.
                tx.execute(
                    "UPDATE transactions SET category_id = ?2,
                       sync_state = CASE WHEN sync_state = 'synced' THEN 'modified'
                                 WHEN sync_state = 'skipped' THEN 'pending' ELSE sync_state END
                     WHERE category_id = ?1",
                    params![id, to],
                )?;
                tx.execute(
                    "UPDATE recurring SET category_id = ?2 WHERE category_id = ?1",
                    params![id, to],
                )?;
            }
            let from_paamonim: bool = tx.query_row(
                "SELECT paamonim_item_id IS NOT NULL FROM categories WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )?;
            if from_paamonim {
                tx.execute(
                    "UPDATE categories SET deleted = 1, merged_into = ?2 WHERE id = ?1",
                    params![id, move_to],
                )?;
            } else {
                tx.execute("DELETE FROM categories WHERE id = ?1", params![id])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    async fn list_recurring(&self) -> anyhow::Result<Vec<RecurringSegment>> {
        let conn = self.0.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, series_id, start_month, end_month, day, amount_agorot, category_id,
                    payment_method_id, note
             FROM recurring ORDER BY series_id, start_month",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok(RecurringSegment {
                    id: r.get(0)?,
                    series_id: r.get(1)?,
                    start_month: r.get(2)?,
                    end_month: r.get(3)?,
                    day: r.get(4)?,
                    amount_agorot: r.get(5)?,
                    category_id: r.get(6)?,
                    payment_method_id: r.get(7)?,
                    note: r.get(8)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    async fn insert_recurring(
        &self,
        series_id: Option<i64>,
        r: &NewRecurring,
    ) -> anyhow::Result<i64> {
        let mut conn = self.0.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO recurring (series_id, start_month, end_month, day, amount_agorot,
                                    category_id, payment_method_id, note)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                series_id.unwrap_or(0),
                r.start_month,
                r.end_month,
                r.day,
                r.amount_agorot,
                r.category_id,
                r.payment_method_id,
                r.note
            ],
        )?;
        let id = tx.last_insert_rowid();
        if series_id.is_none() {
            tx.execute(
                "UPDATE recurring SET series_id = ?1 WHERE id = ?1",
                params![id],
            )?;
        }
        tx.commit()?;
        Ok(id)
    }

    async fn update_recurring(&self, id: i64, r: &NewRecurring) -> anyhow::Result<()> {
        self.0.lock().unwrap().execute(
            "UPDATE recurring SET start_month = ?2, end_month = ?3, day = ?4, amount_agorot = ?5,
               category_id = ?6, payment_method_id = ?7, note = ?8
             WHERE id = ?1",
            params![
                id,
                r.start_month,
                r.end_month,
                r.day,
                r.amount_agorot,
                r.category_id,
                r.payment_method_id,
                r.note
            ],
        )?;
        Ok(())
    }

    async fn delete_recurring(&self, id: i64) -> anyhow::Result<()> {
        self.0
            .lock()
            .unwrap()
            .execute("DELETE FROM recurring WHERE id = ?1", params![id])?;
        Ok(())
    }

    async fn settled_recurring(&self) -> anyhow::Result<Vec<(i64, String)>> {
        let conn = self.0.lock().unwrap();
        let mut stmt = conn.prepare("SELECT series_id, month FROM recurring_settled")?;
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    async fn settle_recurring_month(
        &self,
        series_id: i64,
        month: &str,
        t: Option<&NewTransaction>,
    ) -> anyhow::Result<bool> {
        let mut conn = self.0.lock().unwrap();
        let tx = conn.transaction()?;
        let new = tx.execute(
            "INSERT OR IGNORE INTO recurring_settled (series_id, month) VALUES (?1, ?2)",
            params![series_id, month],
        )?;
        if let (1, Some(t)) = (new, t) {
            tx.execute(
                "INSERT INTO transactions (date, amount_agorot, category_id, payment_method_id,
                                           note, sync_state, recurring_series_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, 'pending', ?6)",
                params![
                    t.date,
                    t.amount_agorot,
                    t.category_id,
                    t.payment_method_id,
                    t.note,
                    series_id
                ],
            )?;
        }
        tx.commit()?;
        Ok(new == 1)
    }

    async fn settle_recurring(&self, due: &[Occurrence]) -> anyhow::Result<usize> {
        let mut conn = self.0.lock().unwrap();
        let tx = conn.transaction()?;
        let mut added = 0;
        for o in due {
            let new = tx.execute(
                "INSERT OR IGNORE INTO recurring_settled (series_id, month) VALUES (?1, ?2)",
                params![o.series_id, &o.date[..7]],
            )?;
            if new == 1 {
                tx.execute(
                    "INSERT INTO transactions (date, amount_agorot, category_id, payment_method_id,
                                               note, sync_state, recurring_series_id)
                     VALUES (?1, ?2, ?3, ?4, ?5, 'pending', ?6)",
                    params![
                        o.date,
                        o.amount_agorot,
                        o.category_id,
                        o.payment_method_id,
                        o.note,
                        o.series_id
                    ],
                )?;
                added += 1;
            }
        }
        tx.commit()?;
        Ok(added)
    }

    async fn log_append(&self, e: &NewLogEntry) -> anyhow::Result<()> {
        self.0.lock().unwrap().execute(
            "INSERT INTO change_log (at, kind, entity_id, old_value, new_value)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                e.at,
                e.kind,
                e.entity_id,
                e.old.as_ref().map(|v| v.to_string()),
                e.new.as_ref().map(|v| v.to_string())
            ],
        )?;
        Ok(())
    }

    async fn list_log(
        &self,
        limit: usize,
        before_id: Option<i64>,
    ) -> anyhow::Result<Vec<LogEntry>> {
        let conn = self.0.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, at, kind, entity_id, old_value, new_value FROM change_log
             WHERE id < ?1 ORDER BY id DESC LIMIT ?2",
        )?;
        let rows = stmt
            .query_map(params![before_id.unwrap_or(i64::MAX), limit as i64], |r| {
                Ok(LogEntry {
                    id: r.get(0)?,
                    at: r.get(1)?,
                    kind: r.get(2)?,
                    entity_id: r.get(3)?,
                    old: r.get::<_, Option<String>>(4)?.and_then(|s| s.parse().ok()),
                    new: r.get::<_, Option<String>>(5)?.and_then(|s| s.parse().ok()),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    async fn prune_log(&self, before: &str) -> anyhow::Result<usize> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .execute("DELETE FROM change_log WHERE at < ?1", params![before])?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Record a pending transaction as sent to Paamonim under `line`.
    async fn mark(s: &SqliteStorage, id: i64, line: &str) {
        let p = s
            .pending_mirrors()
            .await
            .unwrap()
            .into_iter()
            .find(|p| p.id == id)
            .unwrap();
        assert!(s.mark_mirrored(&p, line).await.unwrap());
    }

    fn tx(date: &str, amount: i64, cat: Option<i64>) -> NewTransaction {
        NewTransaction {
            date: date.into(),
            amount_agorot: amount,
            category_id: cat,
            payment_method_id: None,
            note: String::new(),
        }
    }

    #[tokio::test]
    async fn business_tags_and_links_follow_their_rows() {
        let s = SqliteStorage::open(":memory:").unwrap();
        let a = s.insert_business("A", false).await.unwrap();
        let g = s.insert_business("General", true).await.unwrap();
        let t1 = s
            .create_transaction(&tx("2026-10-03", -100, None))
            .await
            .unwrap();
        let t2 = s
            .create_transaction(&tx("2026-10-04", -200, None))
            .await
            .unwrap();
        s.set_transactions_business(&[t1, t2], Some(a))
            .await
            .unwrap();
        s.set_category_business(7, Some(g)).await.unwrap();
        let l = s.business_links().await.unwrap();
        assert_eq!(l.transactions.len(), 2);
        assert_eq!(l.categories, vec![(7, g)]);
        // A deleted transaction drops out of the tags.
        s.delete_transaction(t1).await.unwrap();
        assert_eq!(
            s.business_links().await.unwrap().transactions,
            vec![(t2, a)]
        );
        s.set_transactions_business(&[t2], None).await.unwrap();
        assert!(s.business_links().await.unwrap().transactions.is_empty());
        // Removing a business drops what pointed at it.
        s.set_transactions_business(&[t2], Some(a)).await.unwrap();
        s.remove_business(a).await.unwrap();
        s.remove_business(g).await.unwrap();
        let l = s.business_links().await.unwrap();
        assert!(l.transactions.is_empty() && l.categories.is_empty());
        assert!(s.list_businesses().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn editing_a_synced_line_flags_it_modified() {
        let s = SqliteStorage::open(":memory:").unwrap();
        let id = s
            .create_transaction(&tx("2026-10-03", -1000, None))
            .await
            .unwrap();
        s.update_transaction(id, &tx("2026-10-04", -2000, None))
            .await
            .unwrap();
        let t = &s.list_transactions().await.unwrap()[0];
        assert_eq!(
            (t.date.as_str(), t.amount_agorot, t.sync_state.as_str()),
            ("2026-10-04", -2000, "pending")
        );
        mark(&s, id, "77").await;
        s.update_transaction(id, &tx("2026-10-05", -2500, None))
            .await
            .unwrap();
        assert_eq!(
            s.list_transactions().await.unwrap()[0].sync_state,
            "modified"
        );
    }

    #[tokio::test]
    async fn import_links_parents_and_fills_missing_links_on_rerun() {
        let s = SqliteStorage::open(":memory:").unwrap();
        let line = |cat: Option<&str>| ImportTransaction {
            line_id: "1".into(),
            date: "2026-10-03".into(),
            amount_agorot: -100,
            category_key: cat.map(String::from),
            payment_key: None,
            note: String::new(),
        };
        let cat = |key: &str, parent: Option<&str>| ImportCategory {
            key: key.into(),
            name: key.into(),
            is_income: false,
            parent_key: parent.map(String::from),
        };
        // First run does not know the item yet, so the line has no category.
        let first = ImportBatch {
            transactions: vec![line(None)],
            ..Default::default()
        };
        s.import_paamonim(&first).await.unwrap();
        let second = ImportBatch {
            categories: vec![cat("ce:3", None), cat("e:1", Some("ce:3"))],
            transactions: vec![line(Some("e:1"))],
            ..Default::default()
        };
        s.import_paamonim(&second).await.unwrap();
        let cats = s.list_categories().await.unwrap();
        let child = cats.iter().find(|c| c.name == "e:1").unwrap();
        let parent = cats.iter().find(|c| c.name == "ce:3").unwrap();
        assert_eq!(child.parent_id, Some(parent.id));
        assert_eq!(
            s.list_transactions().await.unwrap()[0].category_id,
            Some(child.id)
        );
    }

    #[tokio::test]
    async fn import_is_idempotent() {
        let s = SqliteStorage::open(":memory:").unwrap();
        let batch = ImportBatch {
            categories: vec![ImportCategory {
                key: "e:1".into(),
                name: "Food".into(),
                is_income: false,
                parent_key: None,
            }],
            payment_methods: vec![ImportPaymentMethod {
                key: "7".into(),
                name: "Card".into(),
            }],
            transactions: vec![ImportTransaction {
                line_id: "100".into(),
                date: "2026-10-03".into(),
                amount_agorot: -12300,
                category_key: Some("e:1".into()),
                payment_key: Some("7".into()),
                note: String::new(),
            }],
        };
        let first = s.import_paamonim(&batch).await.unwrap();
        assert_eq!((first.categories_added, first.transactions_added), (1, 1));
        let second = s.import_paamonim(&batch).await.unwrap();
        assert_eq!(
            (
                second.categories_added,
                second.transactions_added,
                second.transactions_skipped
            ),
            (0, 0, 1)
        );
        let totals = s.monthly_totals().await.unwrap();
        assert_eq!((totals[0].total_agorot, totals[0].count), (-12300, 1));
    }

    #[tokio::test]
    async fn monthly_totals_follow_inserts_and_deletes() {
        let s = SqliteStorage::open(":memory:").unwrap();
        let a = s
            .create_transaction(&tx("2026-10-03", 1050, None))
            .await
            .unwrap();
        s.create_transaction(&tx("2026-10-20", 2025, None))
            .await
            .unwrap();
        let totals = s.monthly_totals().await.unwrap();
        assert_eq!(totals.len(), 1);
        assert_eq!(totals[0].total_agorot, 3075); // non-round amounts stay exact
        s.delete_transaction(a).await.unwrap();
        assert_eq!(s.monthly_totals().await.unwrap()[0].total_agorot, 2025);
    }

    /// Paamonim income category `ci:1` with items משכורת (`i:7`) and בונוס (`i:8`).
    async fn with_income() -> (SqliteStorage, i64, i64, i64) {
        let s = SqliteStorage::open(":memory:").unwrap();
        let cat = |key: &str, name: &str, parent: Option<&str>| ImportCategory {
            key: key.into(),
            name: name.into(),
            is_income: true,
            parent_key: parent.map(str::to_string),
        };
        let batch = ImportBatch {
            categories: vec![
                cat("ci:1", "הכנסות", None),
                cat("i:7", "משכורת", Some("ci:1")),
                cat("i:8", "בונוס", Some("ci:1")),
            ],
            ..Default::default()
        };
        s.import_paamonim(&batch).await.unwrap();
        let cats = s.list_categories().await.unwrap();
        let id = |n: &str| cats.iter().find(|c| c.name == n).unwrap().id;
        (s, id("הכנסות"), id("משכורת"), id("בונוס"))
    }

    fn top(name: &str, linked_to: Option<i64>) -> NewCategory {
        NewCategory {
            name: name.into(),
            is_income: true,
            parent_id: None,
            linked_to,
        }
    }

    #[tokio::test]
    async fn a_new_top_level_category_must_be_linked_to_a_paamonim_one() {
        let (s, income, salary, _) = with_income().await;
        let err = s.create_category(&top("עומר", None)).await.unwrap_err();
        assert!(err.to_string().contains("Paamonim category"));
        // Not to a sub-category, and not across kinds.
        assert!(s.create_category(&top("עומר", Some(salary))).await.is_err());
        let exp = NewCategory {
            is_income: false,
            ..top("x", Some(income))
        };
        // No Paamonim expense categories: an expense top-level needs no link, and can't
        // take an income one.
        assert!(s.create_category(&exp).await.is_err());
        let id = s.create_category(&top("עומר", Some(income))).await.unwrap();
        let made = s.list_categories().await.unwrap();
        let made = made.iter().find(|c| c.id == id).unwrap();
        assert_eq!((made.linked_to, made.from_paamonim), (Some(income), false));
        // Neither can be deleted from under the link.
        assert!(s.delete_category(income, None).await.is_err());
    }

    #[tokio::test]
    async fn a_sub_category_moves_with_its_lines_and_keeps_its_place_on_reimport() {
        let (s, income, salary, bonus) = with_income().await;
        let omer = s.create_category(&top("עומר", Some(income))).await.unwrap();
        let wife = s.create_category(&top("אשתי", Some(income))).await.unwrap();
        let t = s
            .create_transaction(&NewTransaction {
                date: "2026-10-01".into(),
                amount_agorot: 1_000_000,
                category_id: Some(salary),
                payment_method_id: None,
                note: String::new(),
            })
            .await
            .unwrap();
        s.move_category(salary, wife).await.unwrap();
        s.move_category(bonus, omer).await.unwrap();
        let parent =
            |cats: &[Category], id: i64| cats.iter().find(|c| c.id == id).unwrap().parent_id;
        let cats = s.list_categories().await.unwrap();
        assert_eq!(
            (parent(&cats, salary), parent(&cats, bonus)),
            (Some(wife), Some(omer))
        );
        let tx = s.list_transactions().await.unwrap();
        assert_eq!(
            tx.iter().find(|x| x.id == t).unwrap().category_id,
            Some(salary)
        );
        // A re-import leaves them where they were moved.
        let cat = |key: &str, name: &str, parent: Option<&str>| ImportCategory {
            key: key.into(),
            name: name.into(),
            is_income: true,
            parent_key: parent.map(str::to_string),
        };
        s.import_paamonim(&ImportBatch {
            categories: vec![
                cat("ci:1", "הכנסות", None),
                cat("i:7", "משכורת", Some("ci:1")),
                cat("i:8", "בונוס", Some("ci:1")),
            ],
            ..Default::default()
        })
        .await
        .unwrap();
        let cats = s.list_categories().await.unwrap();
        assert_eq!(
            (parent(&cats, salary), parent(&cats, bonus)),
            (Some(wife), Some(omer))
        );
        // Back under the Paamonim category is fine too.
        s.move_category(bonus, income).await.unwrap();
        // A Paamonim item can't move to a category that isn't linked to its Paamonim category.
        let other = s
            .create_category(&NewCategory {
                is_income: false,
                ..top("הוצאה", None)
            })
            .await
            .unwrap();
        assert!(s.move_category(salary, other).await.is_err());
        // The link can't change while a Paamonim item sits under it.
        assert!(s.link_category_to(wife, None).await.is_err());
        // A name the target already has is refused.
        let dup = s
            .create_category(&NewCategory {
                parent_id: Some(omer),
                ..top("משכורת", None)
            })
            .await
            .unwrap();
        assert!(dup > 0);
        assert!(s.move_category(salary, omer).await.is_err());
    }

    fn new_cat(name: &str, parent_id: Option<i64>, is_income: bool) -> NewCategory {
        NewCategory {
            name: name.into(),
            is_income,
            parent_id,
            linked_to: None,
        }
    }

    fn imported(key: &str, parent: Option<&str>) -> ImportCategory {
        ImportCategory {
            key: key.into(),
            name: key.into(),
            is_income: false,
            parent_key: parent.map(String::from),
        }
    }

    fn imported_line(line: &str, cat: &str) -> ImportTransaction {
        ImportTransaction {
            line_id: line.into(),
            date: "2026-10-03".into(),
            amount_agorot: -100,
            category_key: Some(cat.into()),
            payment_key: None,
            note: String::new(),
        }
    }

    #[tokio::test]
    async fn creating_categories_validates_and_nests_two_levels() {
        let s = SqliteStorage::open(":memory:").unwrap();
        let top = s
            .create_category(&new_cat(" Home ", None, true))
            .await
            .unwrap();
        let kid = s
            .create_category(&new_cat("Rent", Some(top), false))
            .await
            .unwrap();
        let cats = s.list_categories().await.unwrap();
        let kid = cats.iter().find(|c| c.id == kid).unwrap();
        // Trimmed, and a sub-category takes its parent's kind.
        assert_eq!((kid.parent_id, kid.is_income), (Some(top), true));
        assert!(cats.iter().any(|c| c.name == "Home"));
        let refused = |r: anyhow::Result<i64>| {
            matches!(
                r.unwrap_err().downcast_ref::<CategoryError>(),
                Some(CategoryError::Invalid(_))
            )
        };
        assert!(refused(
            s.create_category(&new_cat("  ", None, false)).await
        ));
        assert!(refused(
            s.create_category(&new_cat("Home", None, true)).await
        ));
        assert!(refused(
            s.create_category(&new_cat("x", Some(kid.id), false)).await
        ));
        assert!(refused(
            s.create_category(&new_cat("x", Some(999), false)).await
        ));
    }

    #[tokio::test]
    async fn deleting_a_used_category_needs_a_valid_replacement() {
        let s = SqliteStorage::open(":memory:").unwrap();
        let home = s
            .create_category(&new_cat("Home", None, false))
            .await
            .unwrap();
        let rent = s
            .create_category(&new_cat("Rent", Some(home), false))
            .await
            .unwrap();
        let food = s
            .create_category(&new_cat("Food", None, false))
            .await
            .unwrap();
        let pay = s
            .create_category(&new_cat("Pay", None, true))
            .await
            .unwrap();
        s.create_transaction(&tx("2026-10-03", -100, Some(rent)))
            .await
            .unwrap();
        let err = |r: anyhow::Result<usize>| r.unwrap_err().downcast::<CategoryError>().unwrap();
        // Used through a sub-category, so deleting the parent needs a replacement too.
        assert_eq!(
            err(s.delete_category(home, None).await),
            CategoryError::NeedsReplacement {
                transactions: 1,
                rules: 0
            }
        );
        assert!(matches!(
            err(s.delete_category(home, Some(rent)).await),
            CategoryError::Invalid(_)
        ));
        assert!(matches!(
            err(s.delete_category(home, Some(pay)).await),
            CategoryError::Invalid(_)
        ));
        assert_eq!(
            err(s.delete_category(999, None).await),
            CategoryError::NotFound
        );
        assert_eq!(s.delete_category(home, Some(food)).await.unwrap(), 1);
        let t = &s.list_transactions().await.unwrap()[0];
        assert_eq!(t.category_id, Some(food));
        let names: Vec<_> = s
            .list_categories()
            .await
            .unwrap()
            .into_iter()
            .map(|c| c.name)
            .collect();
        assert_eq!(names, ["Food", "Pay"]);
        // Unused categories go without a replacement.
        assert_eq!(s.delete_category(pay, None).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn moving_lines_flags_synced_ones_as_modified() {
        let s = SqliteStorage::open(":memory:").unwrap();
        let a = s.create_category(&new_cat("A", None, false)).await.unwrap();
        let b = s.create_category(&new_cat("B", None, false)).await.unwrap();
        let synced = s
            .create_transaction(&tx("2026-10-03", -100, Some(a)))
            .await
            .unwrap();
        mark(&s, synced, "5").await;
        s.create_transaction(&tx("2026-10-04", -100, Some(a)))
            .await
            .unwrap();
        s.delete_category(a, Some(b)).await.unwrap();
        let mut states: Vec<_> = s
            .list_transactions()
            .await
            .unwrap()
            .into_iter()
            .map(|t| (t.category_id, t.sync_state))
            .collect();
        states.sort();
        assert_eq!(
            states,
            [
                (Some(b), "modified".to_string()),
                (Some(b), "pending".to_string())
            ]
        );
    }

    #[tokio::test]
    async fn reimport_does_not_bring_back_deleted_categories() {
        let s = SqliteStorage::open(":memory:").unwrap();
        let batch = |extra: Vec<ImportCategory>, lines: Vec<ImportTransaction>| ImportBatch {
            categories: [
                imported("ce:1", None),
                imported("e:1", Some("ce:1")),
                imported("e:2", None),
            ]
            .into_iter()
            .chain(extra)
            .collect(),
            transactions: lines,
            ..Default::default()
        };
        s.import_paamonim(&batch(vec![], vec![imported_line("1", "e:1")]))
            .await
            .unwrap();
        let find = |cats: &[Category], n: &str| cats.iter().find(|c| c.name == n).map(|c| c.id);
        let cats = s.list_categories().await.unwrap();
        let (parent, target) = (find(&cats, "ce:1").unwrap(), find(&cats, "e:2").unwrap());
        s.delete_category(parent, Some(target)).await.unwrap();

        // Same data again, plus a new line in the deleted category and a new item under
        // the deleted parent: nothing is recreated and new lines follow the move.
        let again = batch(
            vec![imported("e:3", Some("ce:1"))],
            vec![
                imported_line("1", "e:1"),
                imported_line("2", "e:1"),
                imported_line("3", "e:3"),
            ],
        );
        let sum = s.import_paamonim(&again).await.unwrap();
        assert_eq!((sum.categories_added, sum.transactions_added), (1, 2));
        let names: Vec<_> = s
            .list_categories()
            .await
            .unwrap()
            .into_iter()
            .map(|c| c.name)
            .collect();
        assert_eq!(names, ["e:2"]);
        assert!(s
            .list_transactions()
            .await
            .unwrap()
            .iter()
            .all(|t| t.category_id == Some(target)));
        // And a third run adds nothing.
        let sum = s.import_paamonim(&again).await.unwrap();
        assert_eq!((sum.categories_added, sum.transactions_added), (0, 0));
    }

    #[tokio::test]
    async fn deleted_paamonim_category_without_replacement_leaves_new_lines_uncategorized() {
        let s = SqliteStorage::open(":memory:").unwrap();
        let batch = |lines| ImportBatch {
            categories: vec![imported("e:1", None)],
            transactions: lines,
            ..Default::default()
        };
        s.import_paamonim(&batch(vec![])).await.unwrap();
        let id = s.list_categories().await.unwrap()[0].id;
        s.delete_category(id, None).await.unwrap();
        s.import_paamonim(&batch(vec![imported_line("9", "e:1")]))
            .await
            .unwrap();
        assert!(s.list_categories().await.unwrap().is_empty());
        assert_eq!(s.list_transactions().await.unwrap()[0].category_id, None);
    }

    #[tokio::test]
    async fn repeating_rules_count_as_references_and_move_with_the_category() {
        use crate::recurring::NewRecurring;
        let s = SqliteStorage::open(":memory:").unwrap();
        let a = s.create_category(&new_cat("A", None, false)).await.unwrap();
        let b = s.create_category(&new_cat("B", None, false)).await.unwrap();
        let rule = |month: &str| NewRecurring {
            start_month: month.into(),
            end_month: None,
            day: 1,
            amount_agorot: -500,
            category_id: Some(a),
            payment_method_id: None,
            note: String::new(),
        };
        let first = s.insert_recurring(None, &rule("2026-01")).await.unwrap();
        s.insert_recurring(Some(first), &rule("2026-06"))
            .await
            .unwrap();
        // No transactions, but one series (two segments) still uses the category.
        let err = s.delete_category(a, None).await.unwrap_err();
        assert_eq!(
            err.downcast::<CategoryError>().unwrap(),
            CategoryError::NeedsReplacement {
                transactions: 0,
                rules: 1
            }
        );
        s.delete_category(a, Some(b)).await.unwrap();
        let rules = s.list_recurring().await.unwrap();
        assert!(rules.iter().all(|r| r.category_id == Some(b)));
        assert_eq!(rules.len(), 2);
    }
}
