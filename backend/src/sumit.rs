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
    pub start_date: Option<NaiveDate>,
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
            start_date: env_nonempty("SUMIT_START_DATE")
                .and_then(|d| NaiveDate::parse_from_str(&d, "%Y-%m-%d").ok()),
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

#[derive(Debug, Clone, PartialEq)]
pub struct DocSummary {
    pub id: i64,
    pub number: Option<i64>,
    pub date: NaiveDate,
    pub customer: String,
    /// Value in shekels (`CompanyValue`, else `DocumentValue`).
    pub total: f64,
    pub doc_type: i64,
}

#[derive(Debug, Clone, PartialEq)]
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
    /// First poll looks back to this day.
    pub since: Option<String>,
    pub last_slot: Option<String>,
    pub last_poll_at: Option<String>,
    pub last_error: Option<String>,
    /// Failed detail fetches per document: after two, it is imported without product details.
    #[serde(default)]
    pub failures: BTreeMap<i64, u32>,
    /// Product names seen on receipts (for the mapping screen).
    #[serde(default)]
    pub products: BTreeSet<String>,
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

/// Product name (lower-cased) -> business id.
pub async fn product_map(db: &dyn Storage) -> Result<BTreeMap<String, i64>> {
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

/// Split a receipt's total over the businesses its products belong to. Each group's share of
/// the total is proportional to its products' prices (which may exclude VAT); the last group
/// takes the remainder so the parts add up exactly. Returns (business, agorot), the group
/// without a business last.
pub fn split_total(total_agorot: i64, items: &[(Option<i64>, f64)]) -> Vec<(Option<i64>, i64)> {
    let mut groups: BTreeMap<Option<i64>, f64> = BTreeMap::new();
    for (b, v) in items {
        *groups.entry(*b).or_default() += v.max(0.0);
    }
    if groups.is_empty() {
        return vec![(None, total_agorot)];
    }
    let sum: f64 = groups.values().sum();
    let mut order: Vec<Option<i64>> = groups.keys().filter(|k| k.is_some()).copied().collect();
    if groups.contains_key(&None) {
        order.push(None);
    }
    let mut out = Vec::new();
    let mut left = total_agorot;
    for (i, b) in order.iter().enumerate() {
        let part = if i + 1 == order.len() {
            left
        } else if sum > 0.0 {
            (total_agorot as f64 * groups[b] / sum).round() as i64
        } else {
            total_agorot / order.len() as i64
        };
        left -= part;
        out.push((*b, part));
    }
    out.retain(|(_, a)| *a != 0);
    if out.is_empty() {
        out.push((None, total_agorot));
    }
    out
}

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
    map: &BTreeMap<String, i64>,
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
    let biz_of = |name: &str| {
        map.get(&product_key(name))
            .copied()
            .filter(|b| known.iter().any(|k| k.id == *b))
    };
    let priced: Vec<(Option<i64>, f64)> =
        items.iter().map(|i| (biz_of(&i.name), i.total)).collect();
    let parts = split_total(total, &priced);
    let date = doc.date.format("%Y-%m-%d").to_string();
    let mut tx_ids = Vec::new();
    for (b, amount) in &parts {
        let mut note = format!("קבלה {}", doc.number.unwrap_or(doc.id));
        if !doc.customer.is_empty() {
            note.push_str(&format!(" – {}", doc.customer));
        }
        let what: Vec<&str> = items
            .iter()
            .filter(|i| biz_of(&i.name) == *b && !i.name.is_empty())
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
                category_id: category,
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
    let mut state: State = load(db, STATE_KEY).await?;
    let today = now.with_timezone(&Jerusalem).date_naive();
    if state.since.is_none() {
        let since = cfg.start_date.unwrap_or(today - Duration::days(1));
        state.since = Some(since.to_string());
    }
    let since = state
        .since
        .as_deref()
        .and_then(|d| d.parse::<NaiveDate>().ok())
        .unwrap_or(today);
    // A few days before the newest receipt, in case one was dated back; seen ones are skipped.
    let from = match state
        .last_date
        .as_deref()
        .and_then(|d| d.parse::<NaiveDate>().ok())
    {
        Some(d) => (d - Duration::days(3)).max(since),
        None => since,
    };
    let result = run_poll(
        db,
        api,
        cfg,
        now,
        &mut state,
        from,
        today + Duration::days(1),
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

async fn run_poll(
    db: &dyn Storage,
    api: &dyn SumitApi,
    cfg: &Config,
    now: DateTime<Utc>,
    state: &mut State,
    from: NaiveDate,
    to: NaiveDate,
) -> Result<PollSummary> {
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
    let seen: BTreeSet<i64> = state.seen.iter().copied().collect();
    let mut fresh: Vec<DocSummary> = docs
        .into_iter()
        .filter(|d| RECEIPT_TYPES.contains(&d.doc_type) && !seen.contains(&d.id))
        .collect();
    fresh.sort_by_key(|d| (d.date, d.id));
    fresh.dedup_by_key(|d| d.id);
    if fresh.is_empty() {
        return Ok(summary);
    }
    let category: Option<i64> = load::<Option<i64>>(db, CATEGORY_KEY).await?;
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
        let map = product_map(db).await?;
        let imported = import_receipt(db, &doc, &items, category, &map).await?;
        for i in &items {
            if !i.name.is_empty() {
                state.products.insert(i.name.clone());
            }
        }
        state.seen.push(doc.id);
        state.failures.remove(&doc.id);
        let d = doc.date.to_string();
        if state.last_date.as_deref().is_none_or(|l| l < d.as_str()) {
            state.last_date = Some(d);
        }
        summary.new_receipts += 1;
        summary.transactions += imported.tx_ids.len();
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
    }
    flush_sheet(&mut recs).await;
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
    save(db, RECEIPTS_KEY, &recs).await?;
    save(db, NOTICES_KEY, &notices).await?;
    match failed {
        Some(e) => Err(e),
        None => Ok(summary),
    }
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
            if s.new_receipts > 0 {
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
        let s = split_total(11_800, &[(Some(1), 50.0), (Some(2), 30.0), (None, 20.0)]);
        assert_eq!(s, vec![(Some(1), 5_900), (Some(2), 3_540), (None, 2_360)]);
        assert_eq!(s.iter().map(|x| x.1).sum::<i64>(), 11_800);
        assert_eq!(split_total(1_000, &[(None, 5.0)]), vec![(None, 1_000)]);
        assert_eq!(split_total(1_000, &[]), vec![(None, 1_000)]);
        assert_eq!(split_total(100, &[(Some(3), 0.0)]), vec![(Some(3), 100)]);
        let odd = split_total(1_000, &[(Some(1), 1.0), (Some(2), 1.0), (None, 1.0)]);
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

    fn cfg() -> Config {
        Config {
            base_url: String::new(),
            company_id: 1,
            api_key: "SECRET".into(),
            monthly_limit: 5,
            hours: vec![7, 13, 19],
            start_date: None,
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
        save(
            &db,
            PRODUCTS_KEY,
            &BTreeMap::from([(product_key("שמלה"), biz)]),
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
        let a = import_receipt(&db, &doc, &[], None, &BTreeMap::new())
            .await
            .unwrap();
        let b = import_receipt(&db, &doc, &[], None, &BTreeMap::new())
            .await
            .unwrap();
        assert_eq!(a.tx_ids, b.tx_ids);
        assert!(b.record.is_none());
        assert_eq!(db.list_transactions().await.unwrap().len(), 1);
    }
}
