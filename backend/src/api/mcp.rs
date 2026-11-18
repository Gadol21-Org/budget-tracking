//! An MCP (Model Context Protocol) server, so an AI assistant can read and edit transactions.
//!
//! Transport: "Streamable HTTP", the stateless subset: every JSON-RPC request is a `POST /mcp`
//! answered with one JSON body (no sessions, no streaming, no server-initiated messages).
//!
//! Auth: none of its own. The endpoint is meant to be reached only from inside the VM's Docker
//! network (see `deploy/MCP.md`); on the public hostname it stays behind Cloudflare Access (no
//! bypass). As the
//! only guard against a web page POSTing to it from a browser, a request carrying an `Origin`
//! header is refused and the body must be sent as `application/json` (which makes a browser ask
//! first). Whatever the request claims about itself (including a forged Access header), changes
//! made here are recorded in the change log as made by `mcp`.
//!
//! The data model keeps no "where did this come from" column on a transaction, so the source in
//! each returned transaction is worked out from what is stored (see [`infer_source`]) and says
//! how it was worked out.

use super::error::ApiError;
use super::{settle, transactions, Db};
use crate::oplog;
use crate::storage::{
    Category, EmailSuggestion, LogFilter, MirrorLink, PaymentMethod, Transaction, TxFx,
};
use crate::sumit::ReceiptRecord;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{header, HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::NaiveDate;
use serde_json::{json, Map, Value};

/// Largest page either list will return; a larger `limit` is refused, not trimmed.
pub(crate) const MAX_LIMIT: usize = 100;
const LATEST_PROTOCOL: &str = "2025-06-18";
const PROTOCOLS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

/// `POST /mcp`: one JSON-RPC message (or a batch) in, one JSON answer out.
pub(crate) async fn handle(
    State(db): State<Db>,
    method: Method,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if method != Method::POST {
        return (StatusCode::METHOD_NOT_ALLOWED, [(header::ALLOW, "POST")]).into_response();
    }
    // Browsers always send Origin on a cross-site POST; MCP clients on the server side do not.
    if headers.contains_key(header::ORIGIN) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let json_body = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            v.trim_start()
                .to_ascii_lowercase()
                .starts_with("application/json")
        });
    if !json_body {
        return StatusCode::UNSUPPORTED_MEDIA_TYPE.into_response();
    }
    match process(&db, &body).await {
        Some(v) => axum::Json(v).into_response(),
        // Only notifications: nothing to answer.
        None => StatusCode::ACCEPTED.into_response(),
    }
}

/// The answer to a body of JSON-RPC, if any of it wanted one.
pub(crate) async fn process(db: &Db, body: &[u8]) -> Option<Value> {
    let parsed: Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(_) => return Some(rpc_error(Value::Null, -32700, "parse error")),
    };
    match parsed {
        Value::Array(items) if !items.is_empty() => {
            let mut out = Vec::new();
            for m in items {
                if let Some(a) = message(db, m).await {
                    out.push(a);
                }
            }
            (!out.is_empty()).then_some(Value::Array(out))
        }
        Value::Array(_) => Some(rpc_error(Value::Null, -32600, "empty batch")),
        m => message(db, m).await,
    }
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

async fn message(db: &Db, m: Value) -> Option<Value> {
    let Some(obj) = m.as_object() else {
        return Some(rpc_error(Value::Null, -32600, "invalid request"));
    };
    let id = obj.get("id").cloned();
    let Some(method) = obj.get("method").and_then(Value::as_str) else {
        // A response to something we never asked: ignore.
        return id.map(|id| rpc_error(id, -32600, "invalid request"));
    };
    let params = obj.get("params").cloned().unwrap_or(Value::Null);
    let result = match method {
        "initialize" => Ok(initialize(&params)),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tool_definitions() })),
        "tools/call" => Ok(call_tool(db, &params).await),
        m if m.starts_with("notifications/") => return None,
        _ => Err(rpc_error(
            id.clone().unwrap_or(Value::Null),
            -32601,
            "method not found",
        )),
    };
    // A notification (no id) gets no answer.
    let id = id?;
    Some(match result {
        Ok(r) => json!({"jsonrpc": "2.0", "id": id, "result": r}),
        Err(mut e) => {
            e["id"] = id;
            e
        }
    })
}

fn initialize(params: &Value) -> Value {
    let asked = params.get("protocolVersion").and_then(Value::as_str);
    let version = asked
        .filter(|v| PROTOCOLS.contains(v))
        .unwrap_or(LATEST_PROTOCOL);
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": { "name": "budget-tracker", "version": env!("CARGO_PKG_VERSION") },
        "instructions": "Amounts are shekels with 2 decimals, expenses negative. \
            Lists need `limit` (1-100); continue with `next_cursor`."
    })
}

fn tool_definitions() -> Value {
    let limit = json!({"type": "integer", "minimum": 1, "maximum": MAX_LIMIT,
        "description": "Page size, required."});
    let cursor = json!({"type": "string", "description": "Previous `next_cursor`."});
    let date = |what: &str| json!({"type": "string", "format": "date", "description": what});
    let tx_fields = || {
        json!({
            "date": date("YYYY-MM-DD."),
            "amount": {"type": ["number", "string"], "description":
                "Signed shekels, at most 2 decimals, not zero (-12.50 = expense). Foreign \
                 currency: omit to use the day's rate, or give the shekels actually charged."},
            "currency": {"type": "string", "description":
                "Foreign currency code (USD, JPY...); default shekel."},
            "foreign_amount": {"type": ["number", "string"], "description":
                "With `currency`: signed amount in that currency."},
            "category_id": {"type": "integer", "description": "From list_categories."},
            "payment_method_id": {"type": "integer", "description": "From list_payment_methods."},
            "note": {"type": "string"},
        })
    };
    let detail = json!({"type": "string", "enum": ["compact", "full"], "description":
        "compact (default): short text lines; full: all fields as JSON."});
    let create_props = tx_fields();
    let mut update_props = tx_fields();
    update_props["id"] = json!({"type": "integer"});
    json!([
        {
            "name": "list_transactions",
            "description": "One page of transactions, newest first. Compact: one line each (id, date, shekels, category, note, extras); use get_transaction or detail=full for all metadata and source.",
            "inputSchema": {"type": "object", "required": ["limit"], "additionalProperties": false,
                "properties": {
                    "limit": limit.clone(),
                    "cursor": cursor.clone(),
                    "type": {"type": "string", "enum": ["income", "expense"], "description":
                        "By category kind (by sign if uncategorised)."},
                    "from": date("From this day."),
                    "to": date("Up to this day."),
                    "category_id": {"type": "integer", "description": "Only this category."},
                    "detail": detail.clone(),
                }},
            "annotations": {"readOnlyHint": true}
        },
        {
            "name": "summarize_transactions",
            "description": "Counts and totals (shekels) instead of rows, for questions like spending per category or month.",
            "inputSchema": {"type": "object", "additionalProperties": false,
                "properties": {
                    "from": date("From this day."),
                    "to": date("Up to this day."),
                    "type": {"type": "string", "enum": ["income", "expense"], "description":
                        "By category kind (by sign if uncategorised)."},
                    "category_id": {"type": "integer", "description": "Only this category."},
                    "group_by": {"type": "string", "enum": ["category", "month", "category_month"],
                        "description": "Default category."},
                }},
            "annotations": {"readOnlyHint": true}
        },
        {
            "name": "get_transaction",
            "description": "One transaction with all stored metadata.",
            "inputSchema": {"type": "object", "required": ["id"], "additionalProperties": false,
                "properties": {"id": {"type": "integer"}}},
            "annotations": {"readOnlyHint": true}
        },
        {
            "name": "create_transaction",
            "description": "Add a transaction.",
            "inputSchema": {"type": "object", "required": ["date"], "additionalProperties": false,
                "properties": create_props},
            "annotations": {"readOnlyHint": false, "destructiveHint": false}
        },
        {
            "name": "update_transaction",
            "description": "Change the given fields; null clears `category_id`/`payment_method_id`.",
            "inputSchema": {"type": "object", "required": ["id"], "additionalProperties": false,
                "properties": update_props},
            "annotations": {"readOnlyHint": false, "destructiveHint": true, "idempotentHint": true}
        },
        {
            "name": "delete_transaction",
            "description": "Permanently delete a transaction.",
            "inputSchema": {"type": "object", "required": ["id"], "additionalProperties": false,
                "properties": {"id": {"type": "integer"}}},
            "annotations": {"readOnlyHint": false, "destructiveHint": true}
        },
        {
            "name": "list_operations_log",
            "description": "One page of the change log (old/new values, author, time), newest first. Kept one year.",
            "inputSchema": {"type": "object", "required": ["limit"], "additionalProperties": false,
                "properties": {
                    "limit": limit,
                    "cursor": cursor,
                    "from": date("From this UTC day."),
                    "to": date("Up to this UTC day."),
                    "kind": {"type": "string", "description":
                        "Only this kind, e.g. transaction_create, transaction_update, \
                         transaction_delete, category_create, recurring_settle, import."},
                    "entity_id": {"type": "integer", "description": "Only this id."},
                }},
            "annotations": {"readOnlyHint": true}
        },
        {
            "name": "list_categories",
            "description": "All categories, one line each.",
            "inputSchema": {"type": "object", "additionalProperties": false,
                "properties": {"detail": detail}},
            "annotations": {"readOnlyHint": true}
        },
        {
            "name": "list_payment_methods",
            "description": "All payment methods.",
            "inputSchema": {"type": "object", "additionalProperties": false, "properties": {}},
            "annotations": {"readOnlyHint": true}
        }
    ])
}

/// A tool's answer: text for the model, and (in full mode) the same data as structured JSON.
struct Reply {
    text: String,
    structured: Option<Value>,
}

impl Reply {
    /// Full detail: the data as JSON in both `structuredContent` and as text.
    fn json(v: Value) -> Reply {
        Reply {
            text: serde_json::to_string(&v).unwrap_or_default(),
            structured: Some(v),
        }
    }

    /// Compact text only: no `structuredContent`, which would repeat it all.
    fn text(text: String) -> Reply {
        Reply {
            text,
            structured: None,
        }
    }
}

fn ok(r: Reply) -> Value {
    let mut v = json!({"content": [{"type": "text", "text": r.text}], "isError": false});
    if let Some(s) = r.structured {
        v["structuredContent"] = s;
    }
    v
}

fn tool_error(m: &str) -> Value {
    json!({"content": [{"type": "text", "text": m}], "isError": true})
}

fn api_err(e: ApiError) -> String {
    e.message
}

async fn call_tool(db: &Db, params: &Value) -> Value {
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return tool_error("missing tool name");
    };
    let args = match params.get("arguments") {
        None | Some(Value::Null) => Map::new(),
        Some(Value::Object(m)) => m.clone(),
        Some(_) => return tool_error("arguments must be an object"),
    };
    // Every change made here is recorded as made through MCP, whatever headers came with it.
    let result = oplog::as_mcp(run_tool(db, name, args)).await;
    match result {
        Ok(v) => ok(v),
        Err(m) => tool_error(&m),
    }
}

async fn run_tool(db: &Db, name: &str, args: Map<String, Value>) -> Result<Reply, String> {
    match name {
        "list_transactions" => list_transactions(db, args).await,
        "summarize_transactions" => summarize_transactions(db, args).await,
        "get_transaction" => get_transaction(db, args).await.map(Reply::json),
        "create_transaction" => create_transaction(db, args).await.map(Reply::json),
        "update_transaction" => update_transaction(db, args).await.map(Reply::json),
        "delete_transaction" => delete_transaction(db, args).await.map(Reply::json),
        "list_operations_log" => list_operations_log(db, args).await.map(Reply::json),
        "list_categories" => list_categories(db, args).await,
        "list_payment_methods" => {
            only_keys(&args, &[])?;
            let pms = db.list_payment_methods().await.map_err(|e| e.to_string())?;
            Ok(Reply::json(
                json!({ "payment_methods": pms.iter().map(|p| json!({"id": p.id, "name": p.name})).collect::<Vec<_>>() }),
            ))
        }
        _ => Err(format!("unknown tool {name:?}")),
    }
}

// ---- argument helpers -----------------------------------------------------------------------

/// Refuse arguments the tool does not know (a typo such as `page` must not be silently ignored).
fn only_keys(a: &Map<String, Value>, allowed: &[&str]) -> Result<(), String> {
    match a.keys().find(|k| !allowed.contains(&k.as_str())) {
        Some(k) => Err(format!(
            "unknown argument {k:?}; this tool takes: {}",
            if allowed.is_empty() {
                "none".to_string()
            } else {
                allowed.join(", ")
            }
        )),
        None => Ok(()),
    }
}

fn int(a: &Map<String, Value>, k: &str) -> Result<Option<i64>, String> {
    match a.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_i64()
            .map(Some)
            .ok_or_else(|| format!("{k} must be an integer")),
    }
}

fn req_int(a: &Map<String, Value>, k: &str) -> Result<i64, String> {
    int(a, k)?.ok_or_else(|| format!("{k} is required"))
}

fn string(a: &Map<String, Value>, k: &str) -> Result<Option<String>, String> {
    match a.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(format!("{k} must be a string")),
    }
}

fn date(a: &Map<String, Value>, k: &str) -> Result<Option<NaiveDate>, String> {
    match string(a, k)? {
        None => Ok(None),
        Some(s) => NaiveDate::parse_from_str(&s, "%Y-%m-%d")
            .map(Some)
            .map_err(|_| format!("{k} must be YYYY-MM-DD")),
    }
}

/// `limit` is required and at most [`MAX_LIMIT`]: there is no way to ask for everything.
fn page_limit(a: &Map<String, Value>) -> Result<usize, String> {
    let n = int(a, "limit")?
        .ok_or_else(|| format!("limit is required (1-{MAX_LIMIT}); results are paginated"))?;
    if !(1..=MAX_LIMIT as i64).contains(&n) {
        return Err(format!("limit must be between 1 and {MAX_LIMIT}"));
    }
    Ok(n as usize)
}

/// `detail`: `compact` (default, short text for the model) or `full` (all the JSON).
fn full_detail(a: &Map<String, Value>) -> Result<bool, String> {
    match string(a, "detail")?.as_deref() {
        None | Some("compact") => Ok(false),
        Some("full") => Ok(true),
        Some(_) => Err("detail must be \"compact\" or \"full\"".into()),
    }
}

/// Text that fits in one `|`-separated line.
fn cell(s: &str) -> String {
    s.replace('|', "/")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn encode_cursor(parts: &str) -> String {
    URL_SAFE_NO_PAD.encode(parts)
}

fn decode_cursor(c: &str) -> Result<String, String> {
    URL_SAFE_NO_PAD
        .decode(c)
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
        .ok_or_else(|| "cursor is not one returned by this server".to_string())
}

/// A signed amount in minor units as an exact decimal, e.g. (-1850, 2) is "-18.50".
fn minor_text(minor: i64, decimals: u32) -> String {
    let abs = minor.unsigned_abs();
    let sign = if minor < 0 { "-" } else { "" };
    if decimals == 0 {
        return format!("{sign}{abs}");
    }
    let unit = 10u64.pow(decimals);
    format!(
        "{sign}{}.{:0w$}",
        abs / unit,
        abs % unit,
        w = decimals as usize
    )
}

/// Shekels, two decimals: the form every amount takes in and out of this endpoint.
fn money(agorot: i64) -> String {
    minor_text(agorot, 2)
}

/// An exact decimal ("-12.5", "3") to minor units, with no floating point in between. Fewer
/// decimals than `decimals` is fine; more is refused rather than rounded.
fn decimal_to_minor(text: &str, decimals: u32, what: &str) -> Result<i64, String> {
    let bad =
        || format!("{what} must be a decimal number like -12.50 (at most {decimals} decimals)");
    let t = text.trim();
    let (neg, digits) = match t.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    let (whole, frac) = digits.split_once('.').unwrap_or((digits, ""));
    let all_digits = |x: &str| x.bytes().all(|b| b.is_ascii_digit());
    if (whole.is_empty() && frac.is_empty()) || !all_digits(whole) || !all_digits(frac) {
        return Err(bad());
    }
    if frac.len() > decimals as usize {
        return Err(bad());
    }
    let padded = format!("{frac:0<w$}", w = decimals as usize);
    let magnitude = format!("{}{padded}", if whole.is_empty() { "0" } else { whole })
        .parse::<i64>()
        .map_err(|_| format!("{what} is too large"))?;
    Ok(if neg { -magnitude } else { magnitude })
}

/// A JSON number or string holding a decimal amount, read exactly (a JSON number is taken from
/// its shortest text form, so 12.34 is 1234 agorot and not 1233.999...).
fn amount(a: &Map<String, Value>, k: &str, decimals: u32) -> Result<Option<i64>, String> {
    match a.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => decimal_to_minor(s, decimals, k).map(Some),
        Some(Value::Number(n)) => decimal_to_minor(&n.to_string(), decimals, k).map(Some),
        Some(_) => Err(format!("{k} must be a number")),
    }
}

/// Snapshots in the change log keep `amount_agorot`; show them as `amount` in shekels.
fn shekelize(v: &mut Value) {
    match v {
        Value::Object(m) => {
            if let Some(n) = m.remove("amount_agorot") {
                match n.as_i64() {
                    Some(n) => {
                        m.insert("amount".into(), Value::String(money(n)));
                    }
                    None => {
                        m.insert("amount_agorot".into(), n);
                    }
                }
            }
            m.values_mut().for_each(shekelize);
        }
        Value::Array(items) => items.iter_mut().for_each(shekelize),
        _ => {}
    }
}

// ---- categories -----------------------------------------------------------------------------

fn category_path(cats: &[Category], c: &Category, sep: &str) -> String {
    match c.parent_id.and_then(|p| cats.iter().find(|x| x.id == p)) {
        Some(p) => format!("{}{sep}{}", p.name, c.name),
        None => c.name.clone(),
    }
}

async fn list_categories(db: &Db, a: Map<String, Value>) -> Result<Reply, String> {
    only_keys(&a, &["detail"])?;
    let full = full_detail(&a)?;
    let cats = db.list_categories().await.map_err(|e| e.to_string())?;
    let links = db.business_links().await.map_err(|e| e.to_string())?;
    let businesses = db.list_businesses().await.map_err(|e| e.to_string())?;
    let biz_of = |c: &Category| {
        links
            .categories
            .iter()
            .find(|(cid, _)| *cid == c.id)
            .map(|(_, b)| *b)
    };
    let biz_name = |b: i64| {
        businesses
            .iter()
            .find(|x| x.id == b)
            .map(|x| x.name.clone())
    };
    if !full {
        let mut out = String::from(
            "Categories. Columns: id|path (parent > child)|income or expense|extras\n",
        );
        for c in &cats {
            out.push_str(&format!(
                "{}|{}|{}",
                c.id,
                cell(&category_path(&cats, c, " > ")),
                if c.is_income { "income" } else { "expense" }
            ));
            if let Some(name) = biz_of(c).and_then(biz_name) {
                out.push_str(&format!("|business {}", cell(&name)));
            }
            out.push('\n');
        }
        return Ok(Reply::text(out));
    }
    let items: Vec<Value> = cats
        .iter()
        .map(|c| {
            let biz = biz_of(c);
            json!({
                "id": c.id,
                "name": c.name,
                "path": category_path(&cats, c, " › "),
                "type": if c.is_income { "income" } else { "expense" },
                "parent_id": c.parent_id,
                "from_paamonim": c.from_paamonim,
                "linked_to_paamonim_category_id": c.linked_to,
                "business": biz.map(|b| json!({"id": b, "name": biz_name(b)})),
                "business_required": links.required.contains(&c.id),
            })
        })
        .collect();
    Ok(Reply::json(
        json!({ "count": items.len(), "categories": items }),
    ))
}

// ---- transactions ---------------------------------------------------------------------------

fn is_income(cats: &[Category], t: &Transaction) -> bool {
    match t.category_id.and_then(|c| cats.iter().find(|x| x.id == c)) {
        Some(c) => c.is_income,
        None => t.amount_agorot > 0,
    }
}

/// Which transactions a list or a summary covers.
struct TxFilter {
    from: Option<String>,
    to: Option<String>,
    category: Option<i64>,
    /// `income` or `expense`.
    kind: Option<String>,
}

impl TxFilter {
    fn parse(a: &Map<String, Value>) -> Result<TxFilter, String> {
        let kind = string(a, "type")?;
        if !matches!(kind.as_deref(), None | Some("income") | Some("expense")) {
            return Err("type must be \"income\" or \"expense\"".into());
        }
        let (from, to) = (date(a, "from")?, date(a, "to")?);
        if let (Some(f), Some(t)) = (from, to) {
            if f > t {
                return Err("from is after to".into());
            }
        }
        let day = |d: NaiveDate| d.format("%Y-%m-%d").to_string();
        Ok(TxFilter {
            from: from.map(day),
            to: to.map(day),
            category: int(a, "category_id")?,
            kind,
        })
    }

    fn matches(&self, cats: &[Category], t: &Transaction) -> bool {
        self.from.as_ref().is_none_or(|f| t.date >= *f)
            && self.to.as_ref().is_none_or(|x| t.date <= *x)
            && self.category.is_none_or(|c| t.category_id == Some(c))
            && match self.kind.as_deref() {
                Some("income") => is_income(cats, t),
                Some(_) => !is_income(cats, t),
                None => true,
            }
    }
}

/// The categories and the matching transactions, newest first (date, then id, descending).
async fn matching(db: &Db, f: &TxFilter) -> Result<(Vec<Category>, Vec<Transaction>), String> {
    // Make charges that came due real transactions first, like the app's own list does. Not
    // attributed to MCP: it is the server's housekeeping, not this caller's change.
    oplog::ACTOR
        .scope(None, settle(db))
        .await
        .map_err(api_err)?;
    let cats = db.list_categories().await.map_err(|e| e.to_string())?;
    let all = db.list_transactions().await.map_err(|e| e.to_string())?;
    let hits = all.into_iter().filter(|t| f.matches(&cats, t)).collect();
    Ok((cats, hits))
}

async fn list_transactions(db: &Db, a: Map<String, Value>) -> Result<Reply, String> {
    only_keys(
        &a,
        &[
            "limit",
            "cursor",
            "type",
            "from",
            "to",
            "category_id",
            "detail",
        ],
    )?;
    let limit = page_limit(&a)?;
    let full = full_detail(&a)?;
    let filter = TxFilter::parse(&a)?;
    let after = match string(&a, "cursor")? {
        None => None,
        Some(c) => {
            let c = decode_cursor(&c)?;
            let (d, id) = c.split_once('|').ok_or("bad cursor")?;
            Some((d.to_string(), id.parse::<i64>().map_err(|_| "bad cursor")?))
        }
    };
    let (cats, hits) = matching(db, &filter).await?;
    let total = hits.len();
    // Pages are keyed on (date, id) rather than an offset, so a transaction added or deleted
    // between two calls cannot shift the pages.
    let rest: Vec<&Transaction> = hits
        .iter()
        .filter(|t| {
            after
                .as_ref()
                .is_none_or(|(d, id)| (t.date.as_str(), t.id) < (d.as_str(), *id))
        })
        .collect();
    let page: Vec<Transaction> = rest.iter().take(limit).map(|t| (*t).clone()).collect();
    let next = (rest.len() > limit)
        .then(|| page.last())
        .flatten()
        .map(|t| encode_cursor(&format!("{}|{}", t.date, t.id)));
    if !full {
        return Ok(Reply::text(
            compact_transactions(db, &cats, &page, total, next.as_deref()).await?,
        ));
    }
    let items = describe(db, &page).await?;
    Ok(Reply::json(json!({
        "transactions": items,
        "returned": page.len(),
        "total_matching": total,
        "limit": limit,
        "order": "date desc, id desc",
        "next_cursor": next,
    })))
}

/// One line per transaction with only what answers a question; `get_transaction` has the rest.
async fn compact_transactions(
    db: &Db,
    cats: &[Category],
    page: &[Transaction],
    total: usize,
    next: Option<&str>,
) -> Result<String, String> {
    let e = |e: anyhow::Error| e.to_string();
    let fx = db.list_tx_fx().await.map_err(e)?;
    let links = db.business_links().await.map_err(e)?;
    let businesses = db.list_businesses().await.map_err(e)?;
    let mut out = format!(
        "Transactions, newest first: returned {} of {total} matching. next_cursor: {}\n\
         Columns: id|date|amount in shekels (expenses negative)|category id and path|note|extras\n",
        page.len(),
        next.unwrap_or("none (no more)")
    );
    for t in page {
        let cat = t
            .category_id
            .and_then(|id| cats.iter().find(|x| x.id == id));
        let mut extras = Vec::new();
        if let Some(f) = fx.iter().find(|f| f.transaction_id == t.id) {
            let amount = minor_text(f.orig_minor.abs(), crate::fx::decimals(&f.currency));
            extras.push(format!(
                "foreign {amount} {}{}",
                f.currency,
                if f.estimated { " (estimated rate)" } else { "" }
            ));
        }
        let biz = links
            .transactions
            .iter()
            .find(|(tid, _)| *tid == t.id)
            .map(|(_, b)| *b)
            .or_else(|| business_via_category(&links, cats, cat));
        if let Some(name) = biz.and_then(|b| businesses.iter().find(|x| x.id == b)) {
            extras.push(format!("business {}", cell(&name.name)));
        }
        if t.recurring_series_id.is_some() {
            extras.push("recurring".to_string());
        }
        out.push_str(&format!(
            "{}|{}|{}|{}|{}",
            t.id,
            t.date,
            money(t.amount_agorot),
            cat.map(|c| format!("{} {}", c.id, cell(&category_path(cats, c, " > "))))
                .unwrap_or_default(),
            cell(&t.note),
        ));
        if !extras.is_empty() {
            out.push('|');
            out.push_str(&extras.join("; "));
        }
        out.push('\n');
    }
    Ok(out)
}

/// The business a category (or its parent) is linked to.
fn business_via_category<'a>(
    links: &crate::storage::BusinessLinks,
    cats: &'a [Category],
    mut cur: Option<&'a Category>,
) -> Option<i64> {
    while let Some(x) = cur {
        if let Some((_, b)) = links.categories.iter().find(|(cid, _)| *cid == x.id) {
            return Some(*b);
        }
        cur = x.parent_id.and_then(|p| cats.iter().find(|y| y.id == p));
    }
    None
}

/// Counts and totals instead of rows, for "how much did we spend on ..." questions.
async fn summarize_transactions(db: &Db, a: Map<String, Value>) -> Result<Reply, String> {
    only_keys(&a, &["from", "to", "type", "category_id", "group_by"])?;
    let filter = TxFilter::parse(&a)?;
    let group = string(&a, "group_by")?.unwrap_or_else(|| "category".into());
    if !matches!(group.as_str(), "category" | "month" | "category_month") {
        return Err("group_by must be category, month or category_month".into());
    }
    let (cats, hits) = matching(db, &filter).await?;
    let label = |t: &Transaction| match t
        .category_id
        .and_then(|id| cats.iter().find(|c| c.id == id))
    {
        Some(c) => cell(&category_path(&cats, c, " > ")),
        None => "(no category)".to_string(),
    };
    // (month, category) -> (count, total); an empty part when it is not a grouping key.
    let mut groups: std::collections::BTreeMap<(String, String), (usize, i64)> = Default::default();
    let (mut income, mut expense) = (0i64, 0i64);
    for t in &hits {
        let month = if group == "category" {
            String::new()
        } else {
            t.date[..7].to_string()
        };
        let cat = if group == "month" {
            String::new()
        } else {
            label(t)
        };
        let g = groups.entry((month, cat)).or_default();
        g.0 += 1;
        g.1 += t.amount_agorot;
        if is_income(&cats, t) {
            income += t.amount_agorot;
        } else {
            expense += t.amount_agorot;
        }
    }
    // Months newest first, then the biggest totals first.
    let mut rows: Vec<_> = groups.into_iter().collect();
    rows.sort_by(|a, b| {
        b.0 .0
            .cmp(&a.0 .0)
            .then(b.1 .1.abs().cmp(&a.1 .1.abs()))
            .then(a.0 .1.cmp(&b.0 .1))
    });
    let range = match (&filter.from, &filter.to) {
        (None, None) => "all dates".to_string(),
        (f, t) => format!(
            "{} to {}",
            f.as_deref().unwrap_or("start"),
            t.as_deref().unwrap_or("now")
        ),
    };
    let columns = match group.as_str() {
        "category" => "category path|count|total in shekels",
        "month" => "month|count|total in shekels",
        _ => "month|category path|count|total in shekels",
    };
    let mut out = format!(
        "Summary, {range}: {} transactions, net total {} (income {}, expenses {}). Amounts in shekels, expenses negative.\nColumns: {columns}\n",
        hits.len(),
        money(income + expense),
        money(income),
        money(expense),
    );
    for ((month, cat), (count, total)) in rows {
        let key = match group.as_str() {
            "category" => cat,
            "month" => month,
            _ => format!("{month}|{cat}"),
        };
        out.push_str(&format!("{key}|{count}|{}\n", money(total)));
    }
    Ok(Reply::text(out))
}

async fn find_transaction(db: &Db, id: i64) -> Result<Transaction, String> {
    db.list_transactions()
        .await
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|t| t.id == id)
        .ok_or_else(|| format!("no transaction with id {id}"))
}

async fn get_transaction(db: &Db, a: Map<String, Value>) -> Result<Value, String> {
    only_keys(&a, &["id"])?;
    let t = find_transaction(db, req_int(&a, "id")?).await?;
    Ok(json!({ "transaction": describe(db, &[t]).await?.remove(0) }))
}

const WRITE_KEYS: [&str; 7] = [
    "date",
    "amount",
    "currency",
    "foreign_amount",
    "category_id",
    "payment_method_id",
    "note",
];

/// The category and payment method must exist (the database would only say "server error").
async fn check_links(db: &Db, category: Option<i64>, pm: Option<i64>) -> Result<(), String> {
    if let Some(c) = category {
        let cats = db.list_categories().await.map_err(|e| e.to_string())?;
        if !cats.iter().any(|x| x.id == c) {
            return Err(format!("no category with id {c}; see list_categories"));
        }
    }
    if let Some(p) = pm {
        let pms = db.list_payment_methods().await.map_err(|e| e.to_string())?;
        if !pms.iter().any(|x| x.id == p) {
            return Err(format!(
                "no payment method with id {p}; see list_payment_methods"
            ));
        }
    }
    Ok(())
}

/// `foreign_amount` in the currency's minor units (cents, whole yen).
fn foreign_minor(a: &Map<String, Value>, currency: Option<&str>) -> Result<Option<i64>, String> {
    if !a.contains_key("foreign_amount") {
        return Ok(None);
    }
    let currency = currency.ok_or("foreign_amount needs a currency")?;
    amount(a, "foreign_amount", crate::fx::decimals(currency))
}

async fn create_transaction(db: &Db, a: Map<String, Value>) -> Result<Value, String> {
    only_keys(&a, &WRITE_KEYS)?;
    let date = string(&a, "date")?.ok_or("date is required")?;
    let (category, pm) = (int(&a, "category_id")?, int(&a, "payment_method_id")?);
    check_links(db, category, pm).await?;
    let currency = string(&a, "currency")?;
    let orig = foreign_minor(&a, currency.as_deref())?;
    let body = transactions::TxBody::new(
        date,
        amount(&a, "amount", 2)?,
        category,
        pm,
        string(&a, "note")?.unwrap_or_default(),
        currency,
        orig,
    );
    let id = transactions::create(db, body).await.map_err(api_err)?;
    let t = find_transaction(db, id).await?;
    Ok(json!({ "transaction": describe(db, &[t]).await?.remove(0) }))
}

async fn update_transaction(db: &Db, a: Map<String, Value>) -> Result<Value, String> {
    let mut allowed = vec!["id"];
    allowed.extend(WRITE_KEYS);
    only_keys(&a, &allowed)?;
    let id = req_int(&a, "id")?;
    let old = find_transaction(db, id).await?;
    let fx: Option<TxFx> = db
        .list_tx_fx()
        .await
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|f| f.transaction_id == id);
    // A key that is present (even null) replaces the stored value; an absent one keeps it.
    let category = match a.get("category_id") {
        None => old.category_id,
        Some(_) => int(&a, "category_id")?,
    };
    let pm = match a.get("payment_method_id") {
        None => old.payment_method_id,
        Some(_) => int(&a, "payment_method_id")?,
    };
    check_links(db, category, pm).await?;
    let amount_touched = ["amount", "currency", "foreign_amount"]
        .iter()
        .any(|k| a.contains_key(*k));
    let (amount, currency, orig) = if !amount_touched {
        // Saved again as it is: a foreign line keeps its rate and its state.
        (
            Some(old.amount_agorot),
            fx.as_ref().map(|f| f.currency.clone()),
            fx.as_ref().map(|f| f.orig_minor),
        )
    } else {
        let currency = string(&a, "currency")?.or_else(|| fx.as_ref().map(|f| f.currency.clone()));
        let orig =
            foreign_minor(&a, currency.as_deref())?.or_else(|| fx.as_ref().map(|f| f.orig_minor));
        (amount(&a, "amount", 2)?, currency, orig)
    };
    let body = transactions::TxBody::new(
        string(&a, "date")?.unwrap_or(old.date.clone()),
        amount,
        category,
        pm,
        string(&a, "note")?.unwrap_or(old.note.clone()),
        currency,
        orig,
    );
    transactions::update(db, id, body).await.map_err(api_err)?;
    let t = find_transaction(db, id).await?;
    Ok(json!({ "transaction": describe(db, &[t]).await?.remove(0) }))
}

async fn delete_transaction(db: &Db, a: Map<String, Value>) -> Result<Value, String> {
    only_keys(&a, &["id"])?;
    let id = req_int(&a, "id")?;
    let old = find_transaction(db, id).await?;
    let shown = describe(db, &[old]).await?.remove(0);
    transactions::delete(db, id).await.map_err(api_err)?;
    Ok(json!({ "deleted": shown }))
}

// ---- describing a transaction, with everything stored about it -------------------------------

/// What the transaction pages are built from, fetched once per page.
struct Context {
    cats: Vec<Category>,
    pms: Vec<PaymentMethod>,
    fx: Vec<TxFx>,
    businesses: Vec<crate::storage::Business>,
    links: crate::storage::BusinessLinks,
    mirror: Vec<MirrorLink>,
    emails: Vec<EmailSuggestion>,
    receipts: Vec<ReceiptRecord>,
    log: Vec<oplog::LogEntry>,
}

async fn context(db: &Db, ids: &[i64]) -> Result<Context, String> {
    let e = |e: anyhow::Error| e.to_string();
    let receipts = match db
        .get_setting(crate::sumit::RECEIPTS_KEY)
        .await
        .map_err(e)?
    {
        Some(s) => serde_json::from_str(&s).unwrap_or_default(),
        None => Vec::new(),
    };
    // Every create/update entry about these ids, newest first (transaction ids only collide with
    // other entities' ids in kinds this filter leaves out).
    let log = if ids.is_empty() {
        Vec::new()
    } else {
        let f = LogFilter {
            kinds: vec![oplog::kind::TX_CREATE.into(), oplog::kind::TX_UPDATE.into()],
            entity_ids: ids.to_vec(),
            ..Default::default()
        };
        db.list_log_filtered(&f, 20 * ids.len().max(1), None)
            .await
            .map_err(e)?
    };
    Ok(Context {
        cats: db.list_categories().await.map_err(e)?,
        pms: db.list_payment_methods().await.map_err(e)?,
        fx: db.list_tx_fx().await.map_err(e)?,
        businesses: db.list_businesses().await.map_err(e)?,
        links: db.business_links().await.map_err(e)?,
        mirror: db.mirror_links(ids).await.map_err(e)?,
        emails: db.emails_for_transactions(ids).await.map_err(e)?,
        receipts,
        log,
    })
}

/// Who a change-log author is: a person (their Access email), a background job, or MCP.
fn actor_type(user: Option<&str>) -> &'static str {
    match user {
        None => "unknown",
        Some(u) if u == oplog::SYNC_ACTOR => "sumit_sync",
        Some(u) if u == oplog::MCP_ACTOR => "mcp",
        Some(_) => "person",
    }
}

/// Where a transaction came from. Nothing on the transaction row says so, so this reads the
/// other records that point at it, strongest evidence first; `basis` names the one used.
fn infer_source(t: &Transaction, c: &Context, created_by: Option<&str>, has_create: bool) -> Value {
    if let Some(m) = c.emails.iter().find(|m| m.transaction_id == Some(t.id)) {
        return json!({
            "type": "email",
            "basis": "linked email row",
            "details": {
                "email_item_id": m.id, "message_id": m.message_id, "sender": m.sender,
                "subject": m.subject, "received": m.received,
                // `auto`: a rule added it on arrival; `accepted`: a person approved it (or an
                // `auto` one whose mail has since been closed).
                "status": m.status, "confidence": m.confidence,
            }
        });
    }
    if let Some(r) = c.receipts.iter().find(|r| r.tx_ids.contains(&t.id)) {
        return json!({
            "type": "sumit",
            "basis": "Sumit receipt record",
            "details": {"document_id": r.doc_id, "number": r.number, "customer": r.customer}
        });
    }
    if created_by == Some(oplog::SYNC_ACTOR) {
        return json!({"type": "sumit", "basis": "change log: created by the Sumit sync", "details": null});
    }
    if t.recurring_series_id.is_some() {
        return json!({
            "type": "recurring", "basis": "recurring_series_id",
            "details": {"series_id": t.recurring_series_id}
        });
    }
    match (created_by, has_create) {
        (Some(u), _) if u == oplog::MCP_ACTOR => {
            json!({"type": "mcp", "basis": "change log: created through MCP", "details": null})
        }
        (Some(u), true) if actor_type(Some(u)) == "person" => json!({
            "type": "manual", "basis": "change log: created by a person", "details": {"created_by": u}
        }),
        (None, true) => json!({
            "type": "manual",
            "basis": "change log: created with no signed-in user (local run or staging)",
            "details": null
        }),
        _ if c.mirror.iter().any(|m| m.transaction_id == t.id) && !has_create => json!({
            "type": "paamonim_import",
            "basis": "has a Paamonim line but no creation in the change log",
            "details": null
        }),
        _ => json!({
            "type": "unknown",
            "basis": "no creation entry in the change log (entries are kept one year) and no other record",
            "details": null
        }),
    }
}

async fn describe(db: &Db, txs: &[Transaction]) -> Result<Vec<Value>, String> {
    let ids: Vec<i64> = txs.iter().map(|t| t.id).collect();
    let c = context(db, &ids).await?;
    Ok(txs.iter().map(|t| describe_one(t, &c)).collect())
}

fn describe_one(t: &Transaction, c: &Context) -> Value {
    let cat = t
        .category_id
        .and_then(|id| c.cats.iter().find(|x| x.id == id));
    let pm = t
        .payment_method_id
        .and_then(|id| c.pms.iter().find(|x| x.id == id));
    let fx = c.fx.iter().find(|f| f.transaction_id == t.id);
    let mirror = c.mirror.iter().find(|m| m.transaction_id == t.id);
    // The log is newest first: the creation is the oldest create entry, the last edit the newest
    // update entry.
    let mine = |kind: &'static str| {
        c.log
            .iter()
            .filter(move |e| e.entity_id == Some(t.id) && e.kind == kind)
    };
    let created = mine(oplog::kind::TX_CREATE).next_back();
    let updated = mine(oplog::kind::TX_UPDATE).next();
    // The business it counts in: its own tag, else the one its category (or the parent) is
    // linked to.
    let tag = c
        .links
        .transactions
        .iter()
        .find(|(tid, _)| *tid == t.id)
        .map(|(_, b)| (*b, "tag"));
    let via_category = || {
        let mut cur = cat;
        while let Some(x) = cur {
            if let Some((_, b)) = c.links.categories.iter().find(|(cid, _)| *cid == x.id) {
                return Some((*b, "category"));
            }
            cur = x.parent_id.and_then(|p| c.cats.iter().find(|y| y.id == p));
        }
        None
    };
    let business = tag.or_else(via_category).map(|(b, via)| {
        json!({
            "id": b,
            "name": c.businesses.iter().find(|x| x.id == b).map(|x| x.name.clone()),
            "via": via,
        })
    });
    json!({
        "id": t.id,
        "date": t.date,
        "amount": money(t.amount_agorot),
        "currency": "ILS",
        "type": if is_income(&c.cats, t) { "income" } else { "expense" },
        "category": cat.map(|x| json!({
            "id": x.id, "name": x.name, "path": category_path(&c.cats, x, " › "),
            "type": if x.is_income { "income" } else { "expense" },
        })),
        "payment_method": pm.map(|p| json!({"id": p.id, "name": p.name})),
        "note": t.note,
        "foreign_currency": fx.map(|f| json!({
            "currency": f.currency,
            "original_amount": minor_text(f.orig_minor, crate::fx::decimals(&f.currency)),
            "rate": f.rate,
            "rate_date": f.rate_date, "rate_is_estimate": f.estimated,
        })),
        "business": business,
        "source": infer_source(t, c, created.and_then(|e| e.user_email.as_deref()), created.is_some()),
        "created_at": created.map(|e| e.at.clone()),
        "created_by": created.and_then(|e| e.user_email.clone()),
        "last_updated_at": updated.map(|e| e.at.clone()),
        "last_updated_by": updated.and_then(|e| e.user_email.clone()),
        "recurring_series_id": t.recurring_series_id,
        "paamonim": {
            "sync_state": t.sync_state.as_str(),
            "line_id": mirror.and_then(|m| m.line_id.clone()),
            "line_date": mirror.and_then(|m| m.mirror_date.clone()),
        },
    })
}

// ---- operations log -------------------------------------------------------------------------

async fn list_operations_log(db: &Db, a: Map<String, Value>) -> Result<Value, String> {
    only_keys(&a, &["limit", "cursor", "from", "to", "kind", "entity_id"])?;
    let limit = page_limit(&a)?;
    let (from, to) = (date(&a, "from")?, date(&a, "to")?);
    if let (Some(f), Some(t)) = (from, to) {
        if f > t {
            return Err("from is after to".into());
        }
    }
    let before_id = match string(&a, "cursor")? {
        None => None,
        Some(c) => Some(
            decode_cursor(&c)?
                .parse::<i64>()
                .map_err(|_| "cursor is not one returned by this server")?,
        ),
    };
    // Entries are stamped in UTC: the days are UTC days, `to` through the end of that day.
    let filter = LogFilter {
        from: from.map(|d| format!("{d}T00:00:00Z")),
        before: to.map(|d| format!("{}T00:00:00Z", d + chrono::Duration::days(1))),
        kinds: string(&a, "kind")?.into_iter().collect(),
        entity_ids: int(&a, "entity_id")?.into_iter().collect(),
    };
    // One more than asked for tells whether there is another page. Pages are keyed on the entry
    // id, so entries written meanwhile (always newer) never shift them.
    let mut rows = db
        .list_log_filtered(&filter, limit + 1, before_id)
        .await
        .map_err(|e| e.to_string())?;
    let more = rows.len() > limit;
    rows.truncate(limit);
    let next = more
        .then(|| rows.last())
        .flatten()
        .map(|e| encode_cursor(&e.id.to_string()));
    let entries: Vec<Value> = rows
        .iter()
        .map(|e| {
            let (mut old, mut new) = (e.old.clone(), e.new.clone());
            old.iter_mut().chain(new.iter_mut()).for_each(shekelize);
            json!({
                "id": e.id,
                "at": e.at,
                "kind": e.kind,
                "entity_id": e.entity_id,
                "old": old,
                "new": new,
                "user_email": e.user_email,
                "actor_type": actor_type(e.user_email.as_deref()),
            })
        })
        .collect();
    Ok(json!({
        "entries": entries,
        "returned": entries.len(),
        "limit": limit,
        "order": "id desc (newest first)",
        "timestamps": "UTC",
        "next_cursor": next,
    }))
}

#[cfg(test)]
mod tests;
