//! The data types the storage layer exchanges with the rest of the app.

use serde::{Deserialize, Serialize};

/// Where a transaction stands with its Paamonim copy (the `sync_state` column).
///
/// `Pending` was never sent; `Modified` was edited after it was sent (the old Paamonim line has
/// to be replaced); `Verifying` may have been sent but not confirmed, so it is checked before
/// anything is posted again; `Synced` matches Paamonim; `Error` failed and is retried by hand;
/// `Skipped` has no place in Paamonim (its category is not linked); `Local` is not mirrored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SyncState {
    Pending,
    Synced,
    Modified,
    Verifying,
    Error,
    Skipped,
    Local,
}

impl SyncState {
    pub fn as_str(self) -> &'static str {
        match self {
            SyncState::Pending => "pending",
            SyncState::Synced => "synced",
            SyncState::Modified => "modified",
            SyncState::Verifying => "verifying",
            SyncState::Error => "error",
            SyncState::Skipped => "skipped",
            SyncState::Local => "local",
        }
    }
}

impl std::str::FromStr for SyncState {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> anyhow::Result<Self> {
        Ok(match s {
            "pending" => SyncState::Pending,
            "synced" => SyncState::Synced,
            "modified" => SyncState::Modified,
            "verifying" => SyncState::Verifying,
            "error" => SyncState::Error,
            "skipped" => SyncState::Skipped,
            "local" => SyncState::Local,
            other => anyhow::bail!("unknown sync_state {other:?}"),
        })
    }
}

#[derive(Serialize, Clone)]
pub struct Transaction {
    pub id: i64,
    pub date: String,
    pub amount_agorot: i64,
    pub category_id: Option<i64>,
    pub payment_method_id: Option<i64>,
    pub note: String,
    pub sync_state: SyncState,
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
    pub state: SyncState,
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
