//! An MCP (Model Context Protocol) server, so an AI assistant can read and edit transactions.
//!
//! Transport: "Streamable HTTP", the stateless subset: every JSON-RPC request is a `POST /mcp`
//! answered with one JSON body (no sessions, no streaming, no server-initiated messages).
//!
//! Auth: Cloudflare Access cannot be passed by an MCP client, so (like `/api/email/inbound`) this
//! one path is bypassed in Access and checks its own secret: `Authorization: Bearer <MCP_TOKEN>`,
//! compared in constant time before anything is read. Off, and answering 404 as if the route did
//! not exist, unless `MCP_TOKEN` is set to at least 32 characters. Whatever the request claims
//! about itself (including a forged Access header), changes made here are recorded in the change
//! log as made by `mcp`.
//!
//! The data model keeps no "where did this come from" column on a transaction, so the source in
//! each returned transaction is worked out from what is stored (see [`infer_source`]) and says
//! how it was worked out.

use super::error::ApiError;
use super::{settle, transactions, Db};
use crate::email::same;
use crate::oplog;
use crate::storage::{
    Category, EmailSuggestion, LogFilter, MirrorLink, PaymentMethod, Transaction, TxFx,
};
use crate::sumit::ReceiptRecord;
use axum::body::Bytes;
use axum::extract::{Extension, State};
use axum::http::{header, HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::NaiveDate;
use serde_json::{json, Map, Value};
use std::sync::Arc;

/// Largest page either list will return; a larger `limit` is refused, not trimmed.
pub(crate) const MAX_LIMIT: usize = 100;
const LATEST_PROTOCOL: &str = "2025-06-18";
const PROTOCOLS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];
const MIN_TOKEN_LEN: usize = 32;

pub(crate) struct Config {
    token: Option<String>,
}

impl Config {
    pub(crate) fn from_env() -> Config {
        Config::with_token(std::env::var("MCP_TOKEN").ok())
    }

    pub(crate) fn with_token(token: Option<String>) -> Config {
        let token = token
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty());
        let token = match token {
            Some(t) if t.len() < MIN_TOKEN_LEN => {
                tracing::error!(
                    "MCP_TOKEN is shorter than {MIN_TOKEN_LEN} characters: the MCP endpoint is off"
                );
                None
            }
            t => t,
        };
        Config { token }
    }
}

/// `POST /mcp`: one JSON-RPC message (or a batch) in, one JSON answer out.
pub(crate) async fn handle(
    Extension(cfg): Extension<Arc<Config>>,
    State(db): State<Db>,
    method: Method,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    // Off looks like no such route.
    let Some(token) = &cfg.token else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let given = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    if !given.is_some_and(|g| same(token, g)) {
        return (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Bearer")],
        )
            .into_response();
    }
    if method != Method::POST {
        return (StatusCode::METHOD_NOT_ALLOWED, [(header::ALLOW, "POST")]).into_response();
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
        "instructions": "Household budget data. Amounts are exact integer agorot (1/100 shekel), \
            signed: expenses negative, income positive. Lists are paginated: `limit` is required \
            (1-100); pass the returned `next_cursor` to get the next page. Every transaction \
            carries its source (manual, email, sumit, recurring, paamonim_import, mcp) and how that \
            was determined. Writes are recorded in the change log as made by `mcp` and are mirrored \
            to Paamonim like any other change."
    })
}

fn tool_definitions() -> Value {
    let limit = json!({"type": "integer", "minimum": 1, "maximum": MAX_LIMIT,
        "description": format!("Page size, required, 1-{MAX_LIMIT}.")});
    let cursor = json!({"type": "string",
        "description": "`next_cursor` from the previous page; omit for the first page."});
    let date = |what: &str| json!({"type": "string", "format": "date", "description": what});
    let tx_fields = |req_note: bool| {
        let _ = req_note;
        json!({
            "date": date("Transaction day, YYYY-MM-DD."),
            "amount_agorot": {"type": "integer", "description":
                "Signed exact amount in agorot (-1250 = an expense of 12.50 shekels). Not zero. \
                 For a foreign-currency entry, leave it out to price it at the day's rate, or give \
                 the shekel amount the card really charged."},
            "currency": {"type": "string", "description":
                "Optional foreign currency (USD, EUR, JPY...); default is the shekel."},
            "orig_minor": {"type": "integer", "description":
                "With `currency`: signed amount in cents (whole yen)."},
            "category_id": {"type": "integer", "description": "From list_categories."},
            "payment_method_id": {"type": "integer", "description": "From list_payment_methods."},
            "note": {"type": "string"},
        })
    };
    let mut create_props = tx_fields(true);
    let mut update_props = tx_fields(false);
    update_props["id"] = json!({"type": "integer"});
    create_props["date"]["description"] = json!("Transaction day, YYYY-MM-DD (required).");
    json!([
        {
            "name": "list_transactions",
            "title": "List transactions",
            "description": "One page of transactions, newest first (by date, then id), with all \
                stored metadata including their source. Pagination is required.",
            "inputSchema": {"type": "object", "required": ["limit"], "additionalProperties": false,
                "properties": {
                    "limit": limit.clone(),
                    "cursor": cursor.clone(),
                    "type": {"type": "string", "enum": ["income", "expense"], "description":
                        "Income or expense, by the kind of the category (by sign when uncategorised)."},
                    "from": date("Only this day (inclusive) or later."),
                    "to": date("Only this day (inclusive) or earlier."),
                    "category_id": {"type": "integer", "description": "Only this category."},
                }},
            "annotations": {"readOnlyHint": true}
        },
        {
            "name": "get_transaction",
            "title": "Get a transaction",
            "description": "One transaction with all stored metadata.",
            "inputSchema": {"type": "object", "required": ["id"], "additionalProperties": false,
                "properties": {"id": {"type": "integer"}}},
            "annotations": {"readOnlyHint": true}
        },
        {
            "name": "create_transaction",
            "title": "Create a transaction",
            "description": "Add a transaction. Recorded in the change log and mirrored to Paamonim \
                like one added in the app.",
            "inputSchema": {"type": "object", "required": ["date"], "additionalProperties": false,
                "properties": create_props},
            "annotations": {"readOnlyHint": false, "destructiveHint": false}
        },
        {
            "name": "update_transaction",
            "title": "Update a transaction",
            "description": "Change the given fields of a transaction (the others stay). Pass \
                `category_id`/`payment_method_id` as null to clear them.",
            "inputSchema": {"type": "object", "required": ["id"], "additionalProperties": false,
                "properties": update_props},
            "annotations": {"readOnlyHint": false, "destructiveHint": true, "idempotentHint": true}
        },
        {
            "name": "delete_transaction",
            "title": "Delete a transaction",
            "description": "Permanently delete a transaction (its change-log entry keeps the old values).",
            "inputSchema": {"type": "object", "required": ["id"], "additionalProperties": false,
                "properties": {"id": {"type": "integer"}}},
            "annotations": {"readOnlyHint": false, "destructiveHint": true}
        },
        {
            "name": "list_operations_log",
            "title": "List the operations log",
            "description": "One page of the change log (what changed, old and new values, who, \
                when), newest first. Entries are kept for a year. Pagination is required.",
            "inputSchema": {"type": "object", "required": ["limit"], "additionalProperties": false,
                "properties": {
                    "limit": limit,
                    "cursor": cursor,
                    "from": date("Entries stamped on this UTC day (inclusive) or later."),
                    "to": date("Entries stamped on this UTC day (inclusive) or earlier."),
                    "kind": {"type": "string", "description":
                        "Only this kind, e.g. transaction_create, transaction_update, \
                         transaction_delete, category_create, recurring_settle, import."},
                    "entity_id": {"type": "integer", "description": "Only entries about this id."},
                }},
            "annotations": {"readOnlyHint": true}
        },
        {
            "name": "list_categories",
            "title": "List categories",
            "description": "All categories (income and expense, top-level and sub-categories).",
            "inputSchema": {"type": "object", "additionalProperties": false, "properties": {}},
            "annotations": {"readOnlyHint": true}
        },
        {
            "name": "list_payment_methods",
            "title": "List payment methods",
            "description": "All payment methods, for `payment_method_id`.",
            "inputSchema": {"type": "object", "additionalProperties": false, "properties": {}},
            "annotations": {"readOnlyHint": true}
        }
    ])
}

/// A tool's answer: the data as JSON in both `structuredContent` and as text.
fn ok(v: Value) -> Value {
    json!({
        "content": [{"type": "text", "text": serde_json::to_string_pretty(&v).unwrap_or_default()}],
        "structuredContent": v,
        "isError": false
    })
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

async fn run_tool(db: &Db, name: &str, args: Map<String, Value>) -> Result<Value, String> {
    match name {
        "list_transactions" => list_transactions(db, args).await,
        "get_transaction" => get_transaction(db, args).await,
        "create_transaction" => create_transaction(db, args).await,
        "update_transaction" => update_transaction(db, args).await,
        "delete_transaction" => delete_transaction(db, args).await,
        "list_operations_log" => list_operations_log(db, args).await,
        "list_categories" => {
            only_keys(&args, &[])?;
            list_categories(db).await
        }
        "list_payment_methods" => {
            only_keys(&args, &[])?;
            let pms = db.list_payment_methods().await.map_err(|e| e.to_string())?;
            Ok(
                json!({ "payment_methods": pms.iter().map(|p| json!({"id": p.id, "name": p.name})).collect::<Vec<_>>() }),
            )
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

fn money(agorot: i64) -> String {
    let abs = agorot.unsigned_abs();
    format!(
        "{}{}.{:02}",
        if agorot < 0 { "-" } else { "" },
        abs / 100,
        abs % 100
    )
}

// ---- categories -----------------------------------------------------------------------------

fn category_path(cats: &[Category], c: &Category) -> String {
    match c.parent_id.and_then(|p| cats.iter().find(|x| x.id == p)) {
        Some(p) => format!("{} › {}", p.name, c.name),
        None => c.name.clone(),
    }
}

async fn list_categories(db: &Db) -> Result<Value, String> {
    let cats = db.list_categories().await.map_err(|e| e.to_string())?;
    let links = db.business_links().await.map_err(|e| e.to_string())?;
    let businesses = db.list_businesses().await.map_err(|e| e.to_string())?;
    let items: Vec<Value> = cats
        .iter()
        .map(|c| {
            let biz = links
                .categories
                .iter()
                .find(|(cid, _)| *cid == c.id)
                .map(|(_, b)| *b);
            json!({
                "id": c.id,
                "name": c.name,
                "path": category_path(&cats, c),
                "type": if c.is_income { "income" } else { "expense" },
                "parent_id": c.parent_id,
                "from_paamonim": c.from_paamonim,
                "linked_to_paamonim_category_id": c.linked_to,
                "business": biz.map(|b| json!({
                    "id": b,
                    "name": businesses.iter().find(|x| x.id == b).map(|x| x.name.clone()),
                })),
                "business_required": links.required.contains(&c.id),
            })
        })
        .collect();
    Ok(json!({ "count": items.len(), "categories": items }))
}

// ---- transactions ---------------------------------------------------------------------------

fn is_income(cats: &[Category], t: &Transaction) -> bool {
    match t.category_id.and_then(|c| cats.iter().find(|x| x.id == c)) {
        Some(c) => c.is_income,
        None => t.amount_agorot > 0,
    }
}

async fn list_transactions(db: &Db, a: Map<String, Value>) -> Result<Value, String> {
    only_keys(
        &a,
        &["limit", "cursor", "type", "from", "to", "category_id"],
    )?;
    let limit = page_limit(&a)?;
    let kind = string(&a, "type")?;
    if !matches!(kind.as_deref(), None | Some("income") | Some("expense")) {
        return Err("type must be \"income\" or \"expense\"".into());
    }
    let (from, to) = (date(&a, "from")?, date(&a, "to")?);
    if let (Some(f), Some(t)) = (from, to) {
        if f > t {
            return Err("from is after to".into());
        }
    }
    let category = int(&a, "category_id")?;
    let after = match string(&a, "cursor")? {
        None => None,
        Some(c) => {
            let c = decode_cursor(&c)?;
            let (d, id) = c.split_once('|').ok_or("bad cursor")?;
            Some((d.to_string(), id.parse::<i64>().map_err(|_| "bad cursor")?))
        }
    };
    // Make charges that came due real transactions first, like the app's own list does. Not
    // attributed to MCP: it is the server's housekeeping, not this caller's change.
    oplog::ACTOR
        .scope(None, settle(db))
        .await
        .map_err(api_err)?;
    let cats = db.list_categories().await.map_err(|e| e.to_string())?;
    let (fs, ts) = (
        from.map(|d| d.format("%Y-%m-%d").to_string()),
        to.map(|d| d.format("%Y-%m-%d").to_string()),
    );
    // Already newest first: date, then id, descending. Pages are keyed on (date, id) rather than
    // an offset, so a transaction added or deleted between two calls cannot shift the pages.
    let all = db.list_transactions().await.map_err(|e| e.to_string())?;
    let matching: Vec<&Transaction> = all
        .iter()
        .filter(|t| fs.as_ref().is_none_or(|f| t.date >= *f))
        .filter(|t| ts.as_ref().is_none_or(|x| t.date <= *x))
        .filter(|t| category.is_none_or(|c| t.category_id == Some(c)))
        .filter(|t| match kind.as_deref() {
            Some("income") => is_income(&cats, t),
            Some(_) => !is_income(&cats, t),
            None => true,
        })
        .collect();
    let total = matching.len();
    let rest: Vec<&Transaction> = matching
        .into_iter()
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
    let items = describe(db, &page).await?;
    Ok(json!({
        "transactions": items,
        "returned": page.len(),
        "total_matching": total,
        "limit": limit,
        "order": "date desc, id desc",
        "next_cursor": next,
    }))
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
    "amount_agorot",
    "currency",
    "orig_minor",
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

async fn create_transaction(db: &Db, a: Map<String, Value>) -> Result<Value, String> {
    only_keys(&a, &WRITE_KEYS)?;
    let date = string(&a, "date")?.ok_or("date is required")?;
    let (category, pm) = (int(&a, "category_id")?, int(&a, "payment_method_id")?);
    check_links(db, category, pm).await?;
    let body = transactions::TxBody::new(
        date,
        int(&a, "amount_agorot")?,
        category,
        pm,
        string(&a, "note")?.unwrap_or_default(),
        string(&a, "currency")?,
        int(&a, "orig_minor")?,
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
    let amount_touched = ["amount_agorot", "currency", "orig_minor"]
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
        let orig = int(&a, "orig_minor")?.or_else(|| fx.as_ref().map(|f| f.orig_minor));
        (int(&a, "amount_agorot")?, currency, orig)
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
        "amount_agorot": t.amount_agorot,
        "amount": money(t.amount_agorot),
        "currency": "ILS",
        "type": if is_income(&c.cats, t) { "income" } else { "expense" },
        "category": cat.map(|x| json!({
            "id": x.id, "name": x.name, "path": category_path(&c.cats, x),
            "type": if x.is_income { "income" } else { "expense" },
        })),
        "payment_method": pm.map(|p| json!({"id": p.id, "name": p.name})),
        "note": t.note,
        "foreign_currency": fx.map(|f| json!({
            "currency": f.currency, "original_minor": f.orig_minor, "rate": f.rate,
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
            json!({
                "id": e.id,
                "at": e.at,
                "kind": e.kind,
                "entity_id": e.entity_id,
                "old": e.old,
                "new": e.new,
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
