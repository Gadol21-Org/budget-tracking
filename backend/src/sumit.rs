//! New receipts from Sumit (sumit.co.il) become income transactions.
//!
//! Sumit's API allows only about 250 calls a month, so the poller is frugal:
//! * it runs at fixed Israel-time slots (default 07:00, 13:00 and 19:00, never later than 22:30),
//!   never at start-up and never in a retry loop: a failed poll waits for the next slot;
//! * a poll is one `documents/list` call (plus one `getdetails` call per *new* receipt, to learn
//!   its products), filtered by date and by receipt types, and everything already seen is skipped;
//! * every call is counted per month and refused once the limit (`SUMIT_MONTHLY_CALL_LIMIT`,
//!   default 240) is reached.
//!
//! A new receipt is imported as income (one transaction per business its products map to, the
//! rest in one without a business), the user is notified (in the app, and by web push when
//! configured) and the receipt is queued for the Google Sheet (stubbed: see [`append_to_sheet`]).
//! Receipts are de-duplicated by Sumit's document id, both in the seen list and by the
//! `[sumit:<id>]` marker kept in the note of the imported transactions.
//!
//! Config (environment; the API key is a secret and is never logged or committed):
//! `SUMIT_API_KEY`, `SUMIT_COMPANY_ID`, optional `SUMIT_BASE_URL`, `SUMIT_POLL_HOURS` ("7,13,19"),
//! `SUMIT_MONTHLY_CALL_LIMIT`, `SUMIT_START_DATE` (first poll looks back to this day).

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Duration, NaiveDate, TimeZone, Utc};
use chrono_tz::Asia::Jerusalem;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::storage::{NewTransaction, Storage};

pub const STATE_KEY: &str = "sumit_state";
pub const CALLS_KEY: &str = "sumit_calls";
pub const NOTICES_KEY: &str = "sumit_notices";
pub const RECEIPTS_KEY: &str = "sumit_receipts";
pub const PRODUCTS_KEY: &str = "sumit_product_map";
pub const CATEGORY_KEY: &str = "sumit_income_category";

const DEFAULT_BASE: &str = "https://api.sumit.co.il";
const DEFAULT_LIMIT: u32 = 240;
/// Receipt-like document types: InvoiceAndReceipt (1), Receipt (2), DonationReceipt (4).
const RECEIPT_TYPES: [i64; 3] = [1, 2, 4];
const MAX_PAGES: usize = 3;
const MAX_SEEN: usize = 1500;
const MAX_NOTICES: usize = 30;
const MAX_RECEIPTS: usize = 80;

#[derive(Clone)]
pub struct Config {
    pub base_url: String,
    pub company_id: i64,
    api_key: String,
    pub monthly_limit: u32,
    pub hours: Vec<u32>,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("base_url", &self.base_url)
            .field("company_id", &self.company_id)
            .field("api_key", &"<hidden>")
            .finish()
    }
}

impl Config {
    /// `None` when `SUMIT_API_KEY` is not set (the feature is off); an error when it is set
    /// without a usable `SUMIT_COMPANY_ID`.
    pub fn from_env() -> Result<Option<Config>> {
        let Some(api_key) = env_nonempty("SUMIT_API_KEY") else {
            return Ok(None);
        };
        let company_id = env_nonempty("SUMIT_COMPANY_ID")
            .ok_or_else(|| anyhow!("SUMIT_API_KEY is set but SUMIT_COMPANY_ID is not"))?
            .parse()
            .context("SUMIT_COMPANY_ID must be a number")?;
        let hours = match env_nonempty("SUMIT_POLL_HOURS") {
            Some(h) => parse_hours(&h)?,
            None => vec![7, 13, 19],
        };
        Ok(Some(Config {
            base_url: env_nonempty("SUMIT_BASE_URL").unwrap_or_else(|| DEFAULT_BASE.into()),
            company_id,
            api_key,
            monthly_limit: env_nonempty("SUMIT_MONTHLY_CALL_LIMIT")
                .and_then(|v| v.parse().ok())
                .unwrap_or(DEFAULT_LIMIT),
            hours,
        }))
    }
}

fn env_nonempty(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|v| !v.trim().is_empty())
}

/// "7,13,19" -> hours (0-22; nothing is polled after 22:30).
fn parse_hours(s: &str) -> Result<Vec<u32>> {
    let mut hours: Vec<u32> = s
        .split(',')
        .map(|h| {
            h.trim()
                .parse::<u32>()
                .context("SUMIT_POLL_HOURS: bad hour")
        })
        .collect::<Result<_>>()?;
    if hours.iter().any(|h| *h > 22) || hours.is_empty() {
        bail!("SUMIT_POLL_HOURS must be hours between 0 and 22");
    }
    hours.sort_unstable();
    hours.dedup();
    Ok(hours)
}

// ---------------------------------------------------------------------------------------------
// Schedule

/// The next poll slot strictly after `now`, at one of `hours` o'clock in Israel time.
pub fn next_slot(now: DateTime<Utc>, hours: &[u32]) -> DateTime<Utc> {
    let local_today = now.with_timezone(&Jerusalem).date_naive();
    for day in 0..3 {
        let date = local_today + Duration::days(day);
        for h in hours {
            let Some(at) = date.and_hms_opt(*h, 0, 0) else {
                continue;
            };
            if let Some(t) = Jerusalem.from_local_datetime(&at).earliest() {
                let t = t.with_timezone(&Utc);
                if t > now {
                    return t;
                }
            }
        }
    }
    now + Duration::hours(6)
}

/// Identifies a slot (Israel local date and hour) so a restart never polls it twice.
pub fn slot_key(slot: DateTime<Utc>) -> String {
    slot.with_timezone(&Jerusalem)
        .format("%Y-%m-%dT%H")
        .to_string()
}

// ---------------------------------------------------------------------------------------------
// Sumit API

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocSummary {
    pub id: i64,
    pub number: Option<i64>,
    pub date: NaiveDate,
    pub customer: String,
    /// Value in shekels (`CompanyValue`, else `DocumentValue`).
    pub total: f64,
    pub doc_type: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocItem {
    pub name: String,
    pub quantity: f64,
    pub total: f64,
}

pub struct ListPage {
    pub docs: Vec<DocSummary>,
    pub has_next: bool,
}

#[async_trait]
pub trait SumitApi: Send + Sync {
    /// One page (index `page`) of receipts dated `from` through `to`.
    async fn list(&self, from: NaiveDate, to: NaiveDate, page: usize) -> Result<ListPage>;
    async fn items(&self, doc_id: i64) -> Result<Vec<DocItem>>;
}

pub struct HttpApi {
    cfg: Config,
    http: reqwest::Client,
}

impl HttpApi {
    pub fn new(cfg: Config) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("http client");
        HttpApi { cfg, http }
    }

    /// One POST; no retry. Errors never include the request (which holds the key).
    async fn call(&self, path: &str, mut body: Value) -> Result<Value> {
        body["Credentials"] = json!({"CompanyID": self.cfg.company_id, "APIKey": self.cfg.api_key});
        let url = format!("{}{path}", self.cfg.base_url.trim_end_matches('/'));
        let resp = self
            .http
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| anyhow!("sumit request failed: {}", e.without_url()))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            bail!("sumit returned HTTP {status}");
        }
        let v: Value = serde_json::from_str(&text).context("sumit sent invalid JSON")?;
        check_status(&v)?;
        Ok(v)
    }
}

fn check_status(v: &Value) -> Result<()> {
    let ok = match &v["Status"] {
        Value::Number(n) => n.as_i64() == Some(0),
        Value::String(s) => s == "Success" || s == "0",
        _ => false,
    };
    if ok {
        return Ok(());
    }
    let msg = v["UserErrorMessage"]
        .as_str()
        .or_else(|| v["TechnicalErrorDetails"].as_str())
        .unwrap_or("unknown error");
    bail!("sumit error: {msg}")
}

fn parse_date(v: &Value) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(v.as_str()?.get(..10)?, "%Y-%m-%d").ok()
}

/// Reads a Sumit document type, sent either as a number or as its name.
fn doc_type_of(v: &Value) -> i64 {
    match v {
        Value::Number(n) => n.as_i64().unwrap_or(-1),
        Value::String(s) => match s.as_str() {
            "InvoiceAndReceipt" => 1,
            "Receipt" => 2,
            "DonationReceipt" => 4,
            other => other.parse().unwrap_or(-1),
        },
        _ => -1,
    }
}

pub fn parse_list(v: &Value) -> ListPage {
    let data = &v["Data"];
    let docs = data["Documents"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter_map(|d| {
            Some(DocSummary {
                id: d["DocumentID"].as_i64()?,
                number: d["DocumentNumber"].as_i64(),
                date: parse_date(&d["Date"])?,
                customer: d["CustomerName"].as_str().unwrap_or("").trim().to_string(),
                total: d["CompanyValue"]
                    .as_f64()
                    .or_else(|| d["DocumentValue"].as_f64())?,
                doc_type: doc_type_of(&d["Type"]),
            })
        })
        .collect();
    ListPage {
        docs,
        has_next: data["HasNextPage"].as_bool().unwrap_or(false),
    }
}

pub fn parse_items(v: &Value) -> Vec<DocItem> {
    v["Data"]["Items"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .map(|i| {
            let name = i["Item"]["Name"]
                .as_str()
                .or_else(|| i["Description"].as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            let quantity = i["Quantity"].as_f64().unwrap_or(1.0);
            let total = i["TotalPrice"]
                .as_f64()
                .or_else(|| i["UnitPrice"].as_f64().map(|p| p * quantity))
                .unwrap_or(0.0);
            DocItem {
                name,
                quantity,
                total,
            }
        })
        .collect()
}

#[async_trait]
impl SumitApi for HttpApi {
    async fn list(&self, from: NaiveDate, to: NaiveDate, page: usize) -> Result<ListPage> {
        let v = self
            .call(
                "/accounting/documents/list/",
                json!({
                    "DocumentTypes": RECEIPT_TYPES,
                    "DateFrom": format!("{from}T00:00:00"),
                    "DateTo": format!("{to}T23:59:59"),
                    "IncludeDrafts": false,
                    "Paging": {"StartIndex": page * 100, "PageSize": 100},
                }),
            )
            .await?;
        Ok(parse_list(&v))
    }

    async fn items(&self, doc_id: i64) -> Result<Vec<DocItem>> {
        let v = self
            .call(
                "/accounting/documents/getdetails/",
                json!({"DocumentID": doc_id}),
            )
            .await?;
        Ok(parse_items(&v))
    }
}

// ---------------------------------------------------------------------------------------------
// Persistent state (kept in app settings; each value is small JSON)

#[derive(Serialize, Deserialize, Default, Clone, Debug)]
pub struct State {
    /// Sumit document ids already imported.
    #[serde(default)]
    pub seen: Vec<i64>,
    /// Date of the newest receipt seen: the next list call starts a few days before it.
    pub last_date: Option<String>,
    /// Highest Sumit document id that was already looked at: only newer receipts are imported.
    /// Unset until the first successful poll, which only records it (a baseline).
    pub watermark: Option<i64>,
    pub last_slot: Option<String>,
    pub last_poll_at: Option<String>,
    pub last_error: Option<String>,
    /// Failed detail fetches per document: after two, it is imported without product details.
    #[serde(default)]
    pub failures: BTreeMap<i64, u32>,
    /// Product names seen on receipts (for the mapping screen).
    #[serde(default)]
    pub products: BTreeSet<String>,
    /// Receipts that could not be imported yet because a product has no income sub-category to
    /// go to. They are retried (without calling Sumit) whenever the mapping changes.
    #[serde(default)]
    pub held: BTreeMap<i64, Held>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Held {
    pub doc: DocSummary,
    pub items: Vec<DocItem>,
    /// Products that have no usable category.
    pub need: Vec<String>,
}

/// An imported transaction must be on a category with no sub-categories of its own: a
/// category that has sub-categories only groups them.
#[derive(Debug)]
pub struct NeedsCategory(pub Vec<String>);

impl std::fmt::Display for NeedsCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "no income sub-category set for: {}", self.0.join(", "))
    }
}

impl std::error::Error for NeedsCategory {}

/// Is `id` an income category that transactions may be put on (it has no sub-categories)?
pub fn is_usable_category(cats: &[crate::storage::Category], id: i64) -> bool {
    cats.iter().any(|c| c.id == id && c.is_income) && !cats.iter().any(|c| c.parent_id == Some(id))
}

#[derive(Serialize, Deserialize, Default, Clone, Debug)]
pub struct Calls {
    pub month: String,
    pub n: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Notice {
    pub id: i64,
    pub at: String,
    pub title: String,
    pub body: String,
    pub tx_ids: Vec<i64>,
    pub read: bool,
}

/// What the Google Sheet row will be built from. Kept for every imported receipt so rows can be
/// appended (or back-filled) once the sheet and its format are known.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ReceiptRecord {
    pub doc_id: i64,
    pub number: Option<i64>,
    pub date: String,
    pub customer: String,
    pub total_agorot: i64,
    pub items: Vec<RecordItem>,
    pub tx_ids: Vec<i64>,
    #[serde(default)]
    pub sheet_sent: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct RecordItem {
    pub name: String,
    pub quantity: f64,
    pub total: f64,
    pub business_id: Option<i64>,
}

pub async fn load<T: for<'de> Deserialize<'de> + Default>(
    db: &dyn Storage,
    key: &str,
) -> Result<T> {
    Ok(match db.get_setting(key).await? {
        Some(s) => serde_json::from_str(&s).with_context(|| format!("bad setting {key}"))?,
        None => T::default(),
    })
}

pub async fn save<T: Serialize>(db: &dyn Storage, key: &str, v: &T) -> Result<()> {
    db.set_setting(key, &serde_json::to_string(v)?).await
}

/// Where a Sumit product goes: its business and its income category (either may be unset; an
/// unset category means the default one).
#[derive(Serialize, Clone, Copy, Debug, Default, PartialEq)]
pub struct ProductRule {
    pub business_id: Option<i64>,
    pub category_id: Option<i64>,
}

impl<'de> Deserialize<'de> for ProductRule {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        // Earlier versions stored just the business id.
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Business(i64),
            Rule {
                business_id: Option<i64>,
                category_id: Option<i64>,
            },
        }
        Ok(match Raw::deserialize(d)? {
            Raw::Business(b) => ProductRule {
                business_id: Some(b),
                category_id: None,
            },
            Raw::Rule {
                business_id,
                category_id,
            } => ProductRule {
                business_id,
                category_id,
            },
        })
    }
}

/// Product name (lower-cased) -> its rule.
pub async fn product_map(db: &dyn Storage) -> Result<BTreeMap<String, ProductRule>> {
    load(db, PRODUCTS_KEY).await
}

pub fn product_key(name: &str) -> String {
    name.trim().to_lowercase()
}

fn month_of(now: DateTime<Utc>) -> String {
    now.with_timezone(&Jerusalem).format("%Y-%m").to_string()
}

/// Count one API call against the month's budget; refuses once the limit is reached.
async fn spend_call(db: &dyn Storage, limit: u32, now: DateTime<Utc>) -> Result<()> {
    let month = month_of(now);
    let mut c: Calls = load(db, CALLS_KEY).await?;
    if c.month != month {
        c = Calls { month, n: 0 };
    }
    if c.n >= limit {
        bail!("Sumit monthly call limit reached ({limit}); not calling until next month");
    }
    c.n += 1;
    save(db, CALLS_KEY, &c).await
}

pub async fn calls_this_month(db: &dyn Storage, now: DateTime<Utc>) -> Result<u32> {
    let c: Calls = load(db, CALLS_KEY).await?;
    Ok(if c.month == month_of(now) { c.n } else { 0 })
}

// ---------------------------------------------------------------------------------------------
// Import

/// Split a receipt's total over the groups its products fall in (business and category). Each
/// group's share of the total is proportional to its products' prices (which may exclude VAT);
/// the last group takes the remainder so the parts add up exactly. The `plain` group (no
/// business, the default category) comes last.
pub fn split_total<K: Ord + Copy>(
    total_agorot: i64,
    items: &[(K, f64)],
    plain: K,
) -> Vec<(K, i64)> {
    let mut groups: BTreeMap<K, f64> = BTreeMap::new();
    for (k, v) in items {
        *groups.entry(*k).or_default() += v.max(0.0);
    }
    if groups.is_empty() {
        return vec![(plain, total_agorot)];
    }
    let sum: f64 = groups.values().sum();
    let mut order: Vec<K> = groups.keys().filter(|k| **k != plain).copied().collect();
    if groups.contains_key(&plain) {
        order.push(plain);
    }
    let mut out = Vec::new();
    let mut left = total_agorot;
    for (i, k) in order.iter().enumerate() {
        let part = if i + 1 == order.len() {
            left
        } else if sum > 0.0 {
            (total_agorot as f64 * groups[k] / sum).round() as i64
        } else {
            total_agorot / order.len() as i64
        };
        left -= part;
        out.push((*k, part));
    }
    out.retain(|(_, a)| *a != 0);
    if out.is_empty() {
        out.push((plain, total_agorot));
    }
    out
}

/// (business, category) a product's money goes to.
pub type Group = (Option<i64>, Option<i64>);

pub fn marker(doc_id: i64) -> String {
    format!("[sumit:{doc_id}]")
}

fn to_agorot(shekels: f64) -> i64 {
    (shekels * 100.0).round() as i64
}

#[derive(Debug, Default)]
pub struct Imported {
    pub tx_ids: Vec<i64>,
    pub record: Option<ReceiptRecord>,
}

/// Create the income transaction(s) of one receipt. Does nothing if a transaction carrying the
/// receipt's marker already exists (a crash between import and bookkeeping).
pub async fn import_receipt(
    db: &dyn Storage,
    doc: &DocSummary,
    items: &[DocItem],
    category: Option<i64>,
    map: &BTreeMap<String, ProductRule>,
) -> Result<Imported> {
    let mark = marker(doc.id);
    let existing: Vec<i64> = db
        .list_transactions()
        .await?
        .into_iter()
        .filter(|t| t.note.contains(&mark))
        .map(|t| t.id)
        .collect();
    if !existing.is_empty() {
        return Ok(Imported {
            tx_ids: existing,
            record: None,
        });
    }
    let total = to_agorot(doc.total);
    if total <= 0 {
        bail!("receipt {} has no positive amount", doc.id);
    }
    let known = db.list_businesses().await?;
    let cats = db.list_categories().await?;
    let usable = |c: i64| is_usable_category(&cats, c);
    let default = category.filter(|c| usable(*c));
    let rule_of = |name: &str| {
        let r = map.get(&product_key(name)).copied().unwrap_or_default();
        (
            r.business_id.filter(|b| known.iter().any(|k| k.id == *b)),
            r.category_id.filter(|c| usable(*c)).or(default),
        )
    };
    let missing: Vec<String> = if items.is_empty() {
        if default.is_none() {
            vec![String::new()]
        } else {
            Vec::new()
        }
    } else {
        items
            .iter()
            .filter(|i| rule_of(&i.name).1.is_none())
            .map(|i| i.name.clone())
            .collect()
    };
    if !missing.is_empty() {
        return Err(NeedsCategory(missing).into());
    }
    let biz_of = |name: &str| rule_of(name).0;
    let priced: Vec<(Group, f64)> = items.iter().map(|i| (rule_of(&i.name), i.total)).collect();
    let parts = split_total(total, &priced, (None, default));
    let date = doc.date.format("%Y-%m-%d").to_string();
    let mut tx_ids = Vec::new();
    for ((b, cat), amount) in &parts {
        let mut note = format!("קבלה {}", doc.number.unwrap_or(doc.id));
        if !doc.customer.is_empty() {
            note.push_str(&format!(" – {}", doc.customer));
        }
        let what: Vec<&str> = items
            .iter()
            .filter(|i| rule_of(&i.name) == (*b, *cat) && !i.name.is_empty())
            .map(|i| i.name.as_str())
            .collect();
        if !what.is_empty() {
            note.push_str(&format!(": {}", what.join(", ")));
        }
        note.push(' ');
        note.push_str(&mark);
        let id = db
            .create_transaction(&NewTransaction {
                date: date.clone(),
                amount_agorot: *amount,
                category_id: *cat,
                payment_method_id: None,
                note,
            })
            .await?;
        if let Some(b) = b {
            db.set_transactions_business(&[id], Some(*b)).await?;
        }
        tx_ids.push(id);
    }
    let record = ReceiptRecord {
        doc_id: doc.id,
        number: doc.number,
        date,
        customer: doc.customer.clone(),
        total_agorot: total,
        items: items
            .iter()
            .map(|i| RecordItem {
                name: i.name.clone(),
                quantity: i.quantity,
                total: i.total,
                business_id: biz_of(&i.name),
            })
            .collect(),
        tx_ids: tx_ids.clone(),
        sheet_sent: false,
    };
    Ok(Imported {
        tx_ids,
        record: Some(record),
    })
}

// ---------------------------------------------------------------------------------------------
// Google Sheet (STUB)

/// STUB. Omer will provide the sheet link and the row format; until then nothing is written.
/// When implemented, this appends one row for `rec` and returns `Ok(true)`. Every imported
/// receipt is kept in `sumit_receipts` with `sheet_sent == false` until then, so receipts that
/// arrive before the sheet exists are back-filled by [`flush_sheet`] later.
///
/// Configure with `SUMIT_SHEET_ID` (plus the existing Google service account settings).
pub async fn append_to_sheet(rec: &ReceiptRecord) -> Result<bool> {
    if env_nonempty("SUMIT_SHEET_ID").is_some() {
        tracing::info!(
            "sumit: sheet row for receipt {} is not implemented yet (waiting for the sheet format)",
            rec.doc_id
        );
    }
    Ok(false)
}

/// Offer every receipt not yet in the sheet to [`append_to_sheet`]; failures stay queued.
async fn flush_sheet(recs: &mut [ReceiptRecord]) {
    for r in recs.iter_mut().filter(|r| !r.sheet_sent) {
        match append_to_sheet(r).await {
            Ok(sent) => r.sheet_sent = sent,
            Err(e) => {
                tracing::warn!(
                    "sumit: appending receipt {} to the sheet failed: {e:#}",
                    r.doc_id
                );
                break;
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Polling

#[derive(Debug, Default, Serialize)]
pub struct PollSummary {
    /// The first poll: existing receipts were only noted, nothing was imported.
    pub baseline: bool,
    /// Receipts newly put on hold for want of a usable category.
    pub held: usize,
    pub new_receipts: usize,
    pub transactions: usize,
    pub calls: u32,
}

/// Look for receipts that were not seen yet and import them. `now` decides the date window and
/// the month's call budget. Never retries: an error ends the poll.
pub async fn poll_once(
    db: &dyn Storage,
    api: &dyn SumitApi,
    cfg: &Config,
    now: DateTime<Utc>,
) -> Result<PollSummary> {
    retry_held(db, now).await?;
    let mut state: State = load(db, STATE_KEY).await?;
    let today = now.with_timezone(&Jerusalem).date_naive();
    let baseline = state.watermark.is_none() && state.seen.is_empty();
    if state.watermark.is_none() {
        // Receipts imported before the watermark existed.
        state.watermark = state.seen.iter().max().copied();
    }
    // A few days before the newest receipt, in case one was dated back.
    let from = match state
        .last_date
        .as_deref()
        .and_then(|d| d.parse::<NaiveDate>().ok())
    {
        _ if baseline => today - Duration::days(60),
        Some(d) => d - Duration::days(3),
        None => today - Duration::days(14),
    };
    let result = run_poll(
        db,
        api,
        cfg,
        now,
        &mut state,
        Window {
            baseline,
            from,
            to: today + Duration::days(1),
        },
    )
    .await;
    state.last_poll_at = Some(now.to_rfc3339());
    state.last_error = result.as_ref().err().map(|e| format!("{e:#}"));
    state.seen.sort_unstable();
    if state.seen.len() > MAX_SEEN {
        let cut = state.seen.len() - MAX_SEEN;
        state.seen.drain(..cut);
    }
    save(db, STATE_KEY, &state).await?;
    result
}

/// Which receipts a poll asks Sumit for.
struct Window {
    /// First poll: only record where things stand.
    baseline: bool,
    from: NaiveDate,
    to: NaiveDate,
}

async fn run_poll(
    db: &dyn Storage,
    api: &dyn SumitApi,
    cfg: &Config,
    now: DateTime<Utc>,
    state: &mut State,
    window: Window,
) -> Result<PollSummary> {
    let Window { baseline, from, to } = window;
    let mut summary = PollSummary::default();
    let mut docs: Vec<DocSummary> = Vec::new();
    for page in 0..MAX_PAGES {
        spend_call(db, cfg.monthly_limit, now).await?;
        summary.calls += 1;
        let p = api.list(from, to, page).await?;
        docs.extend(p.docs);
        if !p.has_next {
            break;
        }
    }
    let receipts: Vec<DocSummary> = docs
        .into_iter()
        .filter(|d| RECEIPT_TYPES.contains(&d.doc_type))
        .collect();
    if baseline {
        // First poll: remember where things stand, import nothing.
        state.watermark = Some(receipts.iter().map(|d| d.id).max().unwrap_or(0));
        state.last_date = receipts.iter().map(|d| d.date.to_string()).max();
        summary.baseline = true;
        return Ok(summary);
    }
    let seen: BTreeSet<i64> = state.seen.iter().copied().collect();
    let watermark = state.watermark.unwrap_or(0);
    let mut fresh: Vec<DocSummary> = receipts
        .into_iter()
        .filter(|d| d.id > watermark && !seen.contains(&d.id) && !state.held.contains_key(&d.id))
        .collect();
    fresh.sort_by_key(|d| d.id);
    fresh.dedup_by_key(|d| d.id);
    let mut recs: Vec<ReceiptRecord> = load(db, RECEIPTS_KEY).await?;
    let mut notices: Vec<Notice> = load(db, NOTICES_KEY).await?;
    let mut failed: Option<anyhow::Error> = None;
    for doc in fresh {
        let items = if state.failures.get(&doc.id).copied().unwrap_or(0) >= 2 {
            Vec::new()
        } else {
            spend_call(db, cfg.monthly_limit, now).await?;
            summary.calls += 1;
            match api.items(doc.id).await {
                Ok(i) => i,
                Err(e) => {
                    *state.failures.entry(doc.id).or_default() += 1;
                    failed = Some(e.context(format!("details of receipt {}", doc.id)));
                    break;
                }
            }
        };
        state.failures.remove(&doc.id);
        let id = doc.id;
        let done = settle_doc(db, state, doc, items, now, &mut recs, &mut notices).await?;
        // Looked at (imported or held): the next poll starts after it.
        state.watermark = state.watermark.max(Some(id));
        if done.is_none() {
            summary.held += 1;
        }
        if let Some(n) = done {
            summary.new_receipts += 1;
            summary.transactions += n;
        }
    }
    persist_lists(db, &mut recs, &mut notices).await?;
    match failed {
        Some(e) => Err(e),
        None => Ok(summary),
    }
}

/// Import one receipt whose details are known, or hold it if it has nowhere valid to go.
/// Returns the number of transactions made (`None` when held).
async fn settle_doc(
    db: &dyn Storage,
    state: &mut State,
    doc: DocSummary,
    items: Vec<DocItem>,
    now: DateTime<Utc>,
    recs: &mut Vec<ReceiptRecord>,
    notices: &mut Vec<Notice>,
) -> Result<Option<usize>> {
    let category: Option<i64> = load(db, CATEGORY_KEY).await?;
    let map = product_map(db).await?;
    for i in &items {
        if !i.name.is_empty() {
            state.products.insert(i.name.clone());
        }
    }
    match import_receipt(db, &doc, &items, category, &map).await {
        Ok(imported) => {
            state.held.remove(&doc.id);
            state.seen.push(doc.id);
            let d = doc.date.to_string();
            if state.last_date.as_deref().is_none_or(|l| l < d.as_str()) {
                state.last_date = Some(d);
            }
            let n = imported.tx_ids.len();
            if let Some(r) = imported.record {
                recs.push(r);
            }
            let unmapped = items
                .iter()
                .any(|i| !map.contains_key(&product_key(&i.name)));
            notices.push(Notice {
                id: doc.id,
                at: now.to_rfc3339(),
                title: format!("התקבלה קבלה חדשה: ₪{:.2}", doc.total),
                body: notice_body(&doc, &items, unmapped),
                tx_ids: imported.tx_ids,
                read: false,
            });
            Ok(Some(n))
        }
        Err(e) => match e.downcast::<NeedsCategory>() {
            Ok(NeedsCategory(need)) => {
                if !state.held.contains_key(&doc.id) {
                    let what: Vec<&str> = need
                        .iter()
                        .map(String::as_str)
                        .filter(|n| !n.is_empty())
                        .collect();
                    notices.push(Notice {
                        id: doc.id,
                        at: now.to_rfc3339(),
                        title: format!("קבלה ממתינה לסיווג: ₪{:.2}", doc.total),
                        body: format!(
                            "{}הגדירו תת-קטגוריית הכנסה {} ב\"קבלות מ-Sumit\" והקבלה תיובא",
                            if doc.customer.is_empty() {
                                String::new()
                            } else {
                                format!("{} · ", doc.customer)
                            },
                            if what.is_empty() {
                                "ברירת מחדל".to_string()
                            } else {
                                format!("למוצרים: {}", what.join(", "))
                            },
                        ),
                        tx_ids: Vec::new(),
                        read: false,
                    });
                }
                state.held.insert(doc.id, Held { doc, items, need });
                Ok(None)
            }
            Err(e) => Err(e),
        },
    }
}

async fn persist_lists(
    db: &dyn Storage,
    recs: &mut Vec<ReceiptRecord>,
    notices: &mut Vec<Notice>,
) -> Result<()> {
    flush_sheet(recs).await;
    if recs.len() > MAX_RECEIPTS {
        // Keep the unsent ones; drop the oldest sent ones.
        let mut excess = recs.len() - MAX_RECEIPTS;
        recs.retain(|r| {
            if excess > 0 && r.sheet_sent {
                excess -= 1;
                false
            } else {
                true
            }
        });
    }
    if notices.len() > MAX_NOTICES {
        let cut = notices.len() - MAX_NOTICES;
        notices.drain(..cut);
    }
    save(db, RECEIPTS_KEY, recs).await?;
    save(db, NOTICES_KEY, notices).await
}

/// Try the held receipts again with the current mapping. Makes no Sumit calls. Returns how
/// many were imported.
pub async fn retry_held(db: &dyn Storage, now: DateTime<Utc>) -> Result<usize> {
    let mut state: State = load(db, STATE_KEY).await?;
    if state.held.is_empty() {
        return Ok(0);
    }
    let mut recs: Vec<ReceiptRecord> = load(db, RECEIPTS_KEY).await?;
    let mut notices: Vec<Notice> = load(db, NOTICES_KEY).await?;
    let held: Vec<Held> = state.held.values().cloned().collect();
    let mut done = 0;
    for h in held {
        if settle_doc(db, &mut state, h.doc, h.items, now, &mut recs, &mut notices)
            .await?
            .is_some()
        {
            done += 1;
        }
    }
    persist_lists(db, &mut recs, &mut notices).await?;
    save(db, STATE_KEY, &state).await?;
    if done > 0 {
        crate::push::notify_all(db).await;
    }
    Ok(done)
}

fn notice_body(doc: &DocSummary, items: &[DocItem], unmapped: bool) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !doc.customer.is_empty() {
        parts.push(doc.customer.clone());
    }
    let names: Vec<&str> = items
        .iter()
        .map(|i| i.name.as_str())
        .filter(|n| !n.is_empty())
        .collect();
    if !names.is_empty() {
        parts.push(names.join(", "));
    }
    let mut s = parts.join(" · ");
    if unmapped {
        s.push_str(" (יש מוצרים ללא עסק)");
    }
    s
}

/// Runs forever: wakes at each slot and polls, then tells subscribed devices.
pub async fn run_scheduler(db: std::sync::Arc<dyn Storage>, cfg: Config) {
    let api = HttpApi::new(cfg.clone());
    tracing::info!("sumit: polling at {:?} Israel time", cfg.hours);
    loop {
        let slot = next_slot(Utc::now(), &cfg.hours);
        let wait = (slot - Utc::now()).to_std().unwrap_or_default();
        tokio::time::sleep(wait).await;
        let key = slot_key(slot);
        match load::<State>(db.as_ref(), STATE_KEY).await {
            Ok(s) if s.last_slot.as_deref() == Some(key.as_str()) => continue,
            Ok(mut s) => {
                s.last_slot = Some(key);
                if let Err(e) = save(db.as_ref(), STATE_KEY, &s).await {
                    tracing::warn!("sumit: saving state failed: {e:#}");
                    continue;
                }
            }
            Err(e) => {
                tracing::warn!("sumit: reading state failed: {e:#}");
                continue;
            }
        }
        poll_and_notify(db.as_ref(), &api, &cfg).await.ok();
    }
}

/// One poll plus the push to devices when something new arrived. The error (if any) is also
/// kept in the state for the app to show.
pub async fn poll_and_notify(
    db: &dyn Storage,
    api: &dyn SumitApi,
    cfg: &Config,
) -> Result<PollSummary> {
    match poll_once(db, api, cfg, Utc::now()).await {
        Ok(s) => {
            tracing::info!(
                "sumit: {} new receipts, {} transactions, {} calls",
                s.new_receipts,
                s.transactions,
                s.calls
            );
            if s.new_receipts > 0 || s.held > 0 {
                crate::push::notify_all(db).await;
            }
            Ok(s)
        }
        Err(e) => {
            tracing::warn!("sumit poll failed: {e:#}");
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn slots_follow_israel_time() {
        // 2026-11-04 is winter time (UTC+2): 07:00 local = 05:00 UTC.
        let h = [7, 13, 19];
        let at = |s: &str| s.parse::<DateTime<Utc>>().unwrap();
        assert_eq!(
            next_slot(at("2026-11-04T03:00:00Z"), &h),
            at("2026-11-04T05:00:00Z")
        );
        assert_eq!(
            next_slot(at("2026-11-04T05:00:00Z"), &h),
            at("2026-11-04T11:00:00Z")
        );
        assert_eq!(
            next_slot(at("2026-11-04T17:30:00Z"), &h),
            at("2026-11-05T05:00:00Z")
        );
        // Summer time (UTC+3): 07:00 local = 04:00 UTC.
        assert_eq!(
            next_slot(at("2026-07-01T01:00:00Z"), &h),
            at("2026-07-01T04:00:00Z")
        );
        assert_eq!(slot_key(at("2026-11-04T11:00:00Z")), "2026-11-04T13");
    }

    #[test]
    fn hours_are_validated() {
        assert_eq!(parse_hours("19, 7,13").unwrap(), vec![7, 13, 19]);
        assert!(parse_hours("7,23").is_err());
        assert!(parse_hours("x").is_err());
    }

    #[test]
    fn totals_split_over_businesses() {
        let n = None;
        let s = split_total(11_800, &[(Some(1), 50.0), (Some(2), 30.0), (n, 20.0)], n);
        assert_eq!(s, vec![(Some(1), 5_900), (Some(2), 3_540), (n, 2_360)]);
        assert_eq!(s.iter().map(|x| x.1).sum::<i64>(), 11_800);
        assert_eq!(split_total(1_000, &[(n, 5.0)], n), vec![(n, 1_000)]);
        assert_eq!(split_total(1_000, &[], n), vec![(n, 1_000)]);
        assert_eq!(split_total(100, &[(Some(3), 0.0)], n), vec![(Some(3), 100)]);
        let odd = split_total(1_000, &[(Some(1), 1.0), (Some(2), 1.0), (n, 1.0)], n);
        assert_eq!(odd.iter().map(|x| x.1).sum::<i64>(), 1_000);
    }

    #[test]
    fn parses_list_and_details() {
        let v = json!({"Status": 0, "Data": {"HasNextPage": true, "Documents": [
            {"DocumentID": 7, "DocumentNumber": 1001, "Date": "2026-11-03T00:00:00+02:00",
             "CustomerName": " דנה ", "DocumentValue": 118.0, "CompanyValue": 118.0, "Type": 2},
            {"DocumentID": 8, "Date": "bad", "DocumentValue": 1.0, "Type": 2}]}});
        check_status(&v).unwrap();
        let p = parse_list(&v);
        assert!(p.has_next);
        assert_eq!(p.docs.len(), 1);
        assert_eq!(p.docs[0].customer, "דנה");
        assert_eq!(
            p.docs[0].date,
            NaiveDate::from_ymd_opt(2026, 11, 3).unwrap()
        );
        let d = json!({"Data": {"Items": [
            {"Quantity": 2, "UnitPrice": 40.0, "TotalPrice": 80.0, "Item": {"Name": "שמלה"}},
            {"Quantity": 1, "UnitPrice": 10.0, "Description": "משלוח"}]}});
        let items = parse_items(&d);
        assert_eq!(
            items[0],
            DocItem {
                name: "שמלה".into(),
                quantity: 2.0,
                total: 80.0
            }
        );
        assert_eq!(items[1].name, "משלוח");
        assert_eq!(items[1].total, 10.0);
        assert!(check_status(&json!({"Status": 1, "UserErrorMessage": "bad key"})).is_err());
    }

    /// A state past its baseline, so polls import.
    async fn armed(db: &dyn Storage) {
        let st = State {
            watermark: Some(0),
            ..State::default()
        };
        save(db, STATE_KEY, &st).await.unwrap();
    }

    fn cfg() -> Config {
        Config {
            base_url: String::new(),
            company_id: 1,
            api_key: "SECRET".into(),
            monthly_limit: 5,
            hours: vec![7, 13, 19],
        }
    }

    #[test]
    fn config_hides_the_key() {
        assert!(!format!("{:?}", cfg()).contains("SECRET"));
    }

    struct Fake {
        docs: Vec<DocSummary>,
        lists: Mutex<u32>,
        details: Mutex<u32>,
    }

    #[async_trait]
    impl SumitApi for Fake {
        async fn list(&self, _: NaiveDate, _: NaiveDate, _: usize) -> Result<ListPage> {
            *self.lists.lock().unwrap() += 1;
            Ok(ListPage {
                docs: self.docs.clone(),
                has_next: false,
            })
        }
        async fn items(&self, _: i64) -> Result<Vec<DocItem>> {
            *self.details.lock().unwrap() += 1;
            Ok(vec![
                DocItem {
                    name: "שמלה".into(),
                    quantity: 1.0,
                    total: 80.0,
                },
                DocItem {
                    name: "משלוח".into(),
                    quantity: 1.0,
                    total: 20.0,
                },
            ])
        }
    }

    #[tokio::test]
    async fn imports_once_splits_by_business_and_counts_calls() {
        let db = crate::testdb::fresh().await;
        let biz = db.insert_business("שמלות", false).await.unwrap();
        armed(&db).await;
        let cat = db.insert_category("הכנסות", true, None).await.unwrap();
        save(&db, CATEGORY_KEY, &Some(cat)).await.unwrap();
        save(
            &db,
            PRODUCTS_KEY,
            &BTreeMap::from([(
                product_key("שמלה"),
                ProductRule {
                    business_id: Some(biz),
                    category_id: None,
                },
            )]),
        )
        .await
        .unwrap();
        let now: DateTime<Utc> = "2026-11-04T11:00:00Z".parse().unwrap();
        let doc = DocSummary {
            id: 77,
            number: Some(1001),
            date: NaiveDate::from_ymd_opt(2026, 11, 4).unwrap(),
            customer: "דנה".into(),
            total: 118.0,
            doc_type: 2,
        };
        let api = Fake {
            docs: vec![doc],
            lists: Mutex::new(0),
            details: Mutex::new(0),
        };
        let s = poll_once(&db, &api, &cfg(), now).await.unwrap();
        assert_eq!((s.new_receipts, s.transactions, s.calls), (1, 2, 2));
        let txs = db.list_transactions().await.unwrap();
        assert_eq!(txs.len(), 2);
        assert_eq!(txs.iter().map(|t| t.amount_agorot).sum::<i64>(), 11_800);
        assert!(txs.iter().all(|t| t.note.contains("[sumit:77]")));
        assert_eq!(db.business_links().await.unwrap().transactions.len(), 1);
        let notices: Vec<Notice> = load(&db, NOTICES_KEY).await.unwrap();
        assert_eq!(notices.len(), 1);
        // The second poll sees the same receipt: one list call, no details, no new transaction.
        let s = poll_once(&db, &api, &cfg(), now).await.unwrap();
        assert_eq!((s.new_receipts, s.calls), (0, 1));
        assert_eq!(db.list_transactions().await.unwrap().len(), 2);
        assert_eq!(*api.details.lock().unwrap(), 1);
        assert_eq!(calls_this_month(&db, now).await.unwrap(), 3);
        // The budget is enforced: 5 calls in the month, 3 spent.
        poll_once(&db, &api, &cfg(), now).await.unwrap();
        poll_once(&db, &api, &cfg(), now).await.unwrap();
        let e = poll_once(&db, &api, &cfg(), now).await.unwrap_err();
        assert!(e.to_string().contains("limit"));
        assert_eq!(*api.lists.lock().unwrap(), 4);
    }

    #[tokio::test]
    async fn marker_prevents_duplicates_after_a_crash() {
        let db = crate::testdb::fresh().await;
        let doc = DocSummary {
            id: 5,
            number: None,
            date: NaiveDate::from_ymd_opt(2026, 11, 4).unwrap(),
            customer: String::new(),
            total: 10.0,
            doc_type: 2,
        };
        let cat = db.insert_category("הכנסות", true, None).await.unwrap();
        let a = import_receipt(&db, &doc, &[], Some(cat), &BTreeMap::new())
            .await
            .unwrap();
        let b = import_receipt(&db, &doc, &[], Some(cat), &BTreeMap::new())
            .await
            .unwrap();
        assert_eq!(a.tx_ids, b.tx_ids);
        assert!(b.record.is_none());
        assert_eq!(db.list_transactions().await.unwrap().len(), 1);
    }

    #[test]
    fn old_product_maps_still_load() {
        let m: BTreeMap<String, ProductRule> =
            serde_json::from_str(r#"{"a": 3, "b": {"business_id": null, "category_id": 5}}"#)
                .unwrap();
        assert_eq!(
            m["a"],
            ProductRule {
                business_id: Some(3),
                category_id: None
            }
        );
        assert_eq!(
            m["b"],
            ProductRule {
                business_id: None,
                category_id: Some(5)
            }
        );
    }

    #[tokio::test]
    async fn each_product_can_go_to_its_own_category() {
        let db = crate::testdb::fresh().await;
        let top = db.insert_category("הכנסות", true, None).await.unwrap();
        let sub = db
            .insert_category("תיקונים", true, Some(top))
            .await
            .unwrap();
        let plain = db.insert_category("אחר", true, None).await.unwrap();
        let doc = DocSummary {
            id: 6,
            number: Some(7),
            date: NaiveDate::from_ymd_opt(2026, 11, 4).unwrap(),
            customer: String::new(),
            total: 100.0,
            doc_type: 2,
        };
        let items = vec![
            DocItem {
                name: "שמלה".into(),
                quantity: 1.0,
                total: 75.0,
            },
            DocItem {
                name: "תיקון".into(),
                quantity: 1.0,
                total: 25.0,
            },
        ];
        let map = BTreeMap::from([(
            product_key("תיקון"),
            ProductRule {
                business_id: None,
                category_id: Some(sub),
            },
        )]);
        let r = import_receipt(&db, &doc, &items, Some(plain), &map)
            .await
            .unwrap();
        assert_eq!(r.tx_ids.len(), 2);
        let mut txs = db.list_transactions().await.unwrap();
        txs.sort_by_key(|t| t.amount_agorot);
        assert_eq!(
            (txs[0].amount_agorot, txs[0].category_id),
            (2_500, Some(sub))
        );
        assert_eq!(
            (txs[1].amount_agorot, txs[1].category_id),
            (7_500, Some(plain))
        );
    }

    #[tokio::test]
    async fn a_receipt_without_a_usable_category_is_held_then_imported() {
        let db = crate::testdb::fresh().await;
        let top = db.insert_category("הכנסות", true, None).await.unwrap();
        let sub = db.insert_category("שמלות", true, Some(top)).await.unwrap();
        armed(&db).await;
        // The default is a category that has sub-categories: not allowed.
        save(&db, CATEGORY_KEY, &Some(top)).await.unwrap();
        let now: DateTime<Utc> = "2026-11-04T11:00:00Z".parse().unwrap();
        let doc = DocSummary {
            id: 90,
            number: Some(5),
            date: NaiveDate::from_ymd_opt(2026, 11, 4).unwrap(),
            customer: "דנה".into(),
            total: 100.0,
            doc_type: 2,
        };
        let api = Fake {
            docs: vec![doc],
            lists: Mutex::new(0),
            details: Mutex::new(0),
        };
        let s = poll_once(&db, &api, &cfg(), now).await.unwrap();
        assert_eq!((s.new_receipts, s.transactions), (0, 0));
        assert!(db.list_transactions().await.unwrap().is_empty());
        let st: State = load(&db, STATE_KEY).await.unwrap();
        assert!(st.held.contains_key(&90) && st.seen.is_empty());
        let notices: Vec<Notice> = load(&db, NOTICES_KEY).await.unwrap();
        assert_eq!(notices.len(), 1);
        // The next poll does not fetch the held receipt's details again, nor notify again.
        poll_once(&db, &api, &cfg(), now).await.unwrap();
        assert_eq!(*api.details.lock().unwrap(), 1);
        assert_eq!(
            load::<Vec<Notice>>(&db, NOTICES_KEY).await.unwrap().len(),
            1
        );
        // A valid category releases it, without calling Sumit.
        save(&db, CATEGORY_KEY, &Some(sub)).await.unwrap();
        let lists = *api.lists.lock().unwrap();
        assert_eq!(retry_held(&db, now).await.unwrap(), 1);
        assert_eq!(*api.lists.lock().unwrap(), lists);
        let txs = db.list_transactions().await.unwrap();
        assert_eq!(txs.len(), 1);
        assert_eq!(txs[0].category_id, Some(sub));
        let st: State = load(&db, STATE_KEY).await.unwrap();
        assert!(st.held.is_empty() && st.seen == vec![90]);
        assert_eq!(retry_held(&db, now).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn the_first_poll_only_records_a_baseline() {
        let db = crate::testdb::fresh().await;
        let cat = db.insert_category("הכנסות", true, None).await.unwrap();
        save(&db, CATEGORY_KEY, &Some(cat)).await.unwrap();
        let now: DateTime<Utc> = "2026-11-04T11:00:00Z".parse().unwrap();
        let old = DocSummary {
            id: 10,
            number: Some(1),
            date: NaiveDate::from_ymd_opt(2026, 11, 1).unwrap(),
            customer: String::new(),
            total: 50.0,
            doc_type: 2,
        };
        let mut api = Fake {
            docs: vec![old.clone()],
            lists: Mutex::new(0),
            details: Mutex::new(0),
        };
        let s = poll_once(&db, &api, &cfg(), now).await.unwrap();
        assert!(s.baseline);
        assert_eq!((s.new_receipts, s.calls), (0, 1));
        assert!(db.list_transactions().await.unwrap().is_empty());
        assert_eq!(*api.details.lock().unwrap(), 0);
        // Next poll: the old receipt stays ignored, a newer one is imported.
        let newer = DocSummary {
            id: 11,
            number: Some(2),
            total: 20.0,
            ..old.clone()
        };
        api.docs = vec![old, newer];
        let s = poll_once(&db, &api, &cfg(), now).await.unwrap();
        assert!(!s.baseline);
        assert_eq!((s.new_receipts, s.transactions), (1, 1));
        let st: State = load(&db, STATE_KEY).await.unwrap();
        assert_eq!(st.watermark, Some(11));
    }
}
