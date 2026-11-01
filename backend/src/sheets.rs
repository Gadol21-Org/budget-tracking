//! Snapshot export to a Google Sheet owned by the user. A service account with
//! Editor access to the sheet writes one tab per table the app stores (transactions,
//! categories, payment methods, repeating rules, change log, Paamonim sync state), so the sheet
//! is a complete backup. Every tab is cleared and rewritten on each export.
//!
//! Config: `GOOGLE_SERVICE_ACCOUNT_JSON_B64` (base64 of the key file; raw JSON also accepted)
//! and `GOOGLE_SHEET_ID`.
//!

use std::collections::HashMap;

use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::storage::Storage;

const SCOPE: &str = "https://www.googleapis.com/auth/spreadsheets";
const SHEETS: &str = "https://sheets.googleapis.com/v4/spreadsheets";

#[derive(Deserialize)]
struct ServiceAccount {
    client_email: String,
    private_key: String,
    #[serde(default = "default_token_uri")]
    token_uri: String,
}

fn default_token_uri() -> String {
    "https://oauth2.googleapis.com/token".into()
}

#[derive(Debug, Default, Serialize)]
pub struct ExportSummary {
    pub transactions: usize,
    pub tabs: usize,
}

fn parse_service_account(raw: &str) -> Result<ServiceAccount> {
    let raw = raw.trim();
    let json = if raw.starts_with('{') {
        raw.to_string()
    } else {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(raw)
            .context("service account is neither JSON nor base64")?;
        String::from_utf8(bytes)?
    };
    serde_json::from_str(&json).context("invalid service account JSON")
}

async fn access_token(http: &reqwest::Client, sa: &ServiceAccount) -> Result<String> {
    #[derive(Serialize)]
    struct Claims<'a> {
        iss: &'a str,
        scope: &'a str,
        aud: &'a str,
        iat: u64,
        exp: u64,
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    let claims = Claims {
        iss: &sa.client_email,
        scope: SCOPE,
        aud: &sa.token_uri,
        iat: now,
        exp: now + 3600,
    };
    let jwt = jsonwebtoken::encode(
        &Header::new(Algorithm::RS256),
        &claims,
        &EncodingKey::from_rsa_pem(sa.private_key.as_bytes())?,
    )?;
    let resp: Value = http
        .post(&sa.token_uri)
        .form(&[
            ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
            ("assertion", &jwt),
        ])
        .send()
        .await?
        .json()
        .await?;
    resp["access_token"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| anyhow!("token exchange failed: {}", resp["error"]))
}

fn money(agorot: i64) -> f64 {
    agorot as f64 / 100.0
}

type Rows = Vec<Vec<Value>>;

fn header(cols: &[&str]) -> Rows {
    vec![cols.iter().map(|c| Value::from(*c)).collect()]
}

fn opt<T: Into<Value>>(v: Option<T>) -> Value {
    v.map(Into::into).unwrap_or(Value::from(""))
}

/// Build every tab as rows of cells: a complete, restorable copy of what the app stores.
/// Names are included next to ids for readability; the ids are what link the tabs together.
pub async fn build_tabs(storage: &dyn Storage) -> Result<Vec<(&'static str, Rows)>> {
    let raw = storage.raw_backup().await?;
    let cat_names: HashMap<i64, String> = raw
        .categories
        .iter()
        .map(|c| {
            let name = match c
                .parent_id
                .and_then(|p| raw.categories.iter().find(|x| x.id == p))
            {
                Some(p) => format!("{} › {}", p.name, c.name),
                None => c.name.clone(),
            };
            (c.id, name)
        })
        .collect();
    let pm_names: HashMap<i64, String> = raw
        .payment_methods
        .iter()
        .map(|p| (p.id, p.name.clone()))
        .collect();
    let name = |m: &HashMap<i64, String>, id: Option<i64>| {
        id.and_then(|i| m.get(&i).cloned()).unwrap_or_default()
    };

    let links: HashMap<i64, (Option<String>, Option<String>)> = raw
        .tx_links
        .into_iter()
        .map(|(id, line, date)| (id, (line, date)))
        .collect();
    let mut tx = header(&[
        "Id",
        "Date",
        "Amount (ILS)",
        "Amount (agorot)",
        "Category id",
        "Category",
        "Payment method id",
        "Payment method",
        "Note",
        "Sync state",
        "Recurring series id",
        "Paamonim line id",
        "Paamonim line date",
    ]);
    for t in storage.list_transactions().await? {
        let (line, mdate) = links.get(&t.id).cloned().unwrap_or_default();
        tx.push(vec![
            json!(t.id),
            t.date.into(),
            json!(money(t.amount_agorot)),
            json!(t.amount_agorot),
            opt(t.category_id),
            name(&cat_names, t.category_id).into(),
            opt(t.payment_method_id),
            name(&pm_names, t.payment_method_id).into(),
            t.note.into(),
            t.sync_state.as_str().into(),
            opt(t.recurring_series_id),
            opt(line),
            opt(mdate),
        ]);
    }

    let mut cats = header(&[
        "Id",
        "Name",
        "Parent id",
        "Parent",
        "Type",
        "Paamonim item id",
        "Deleted",
        "Merged into id",
        "Merged into",
        "Linked to Paamonim category id",
        "Linked to Paamonim category",
    ]);
    for c in &raw.categories {
        cats.push(vec![
            json!(c.id),
            c.name.clone().into(),
            opt(c.parent_id),
            name(&cat_names, c.parent_id).into(),
            (if c.is_income { "income" } else { "expense" }).into(),
            opt(c.paamonim_item_id.clone()),
            (if c.deleted { "yes" } else { "" }).into(),
            opt(c.merged_into),
            name(&cat_names, c.merged_into).into(),
            opt(c.linked_to),
            name(&cat_names, c.linked_to).into(),
        ]);
    }

    let mut pms = header(&["Id", "Name", "Paamonim id"]);
    for p in &raw.payment_methods {
        pms.push(vec![
            json!(p.id),
            p.name.clone().into(),
            opt(p.paamonim_id.clone()),
        ]);
    }

    let mut rec = header(&[
        "Id",
        "Series id",
        "Start month",
        "End month",
        "Day",
        "Amount (ILS)",
        "Amount (agorot)",
        "Category id",
        "Category",
        "Payment method id",
        "Payment method",
        "Note",
    ]);
    for r in storage.list_recurring().await? {
        rec.push(vec![
            json!(r.id),
            json!(r.series_id),
            r.start_month.into(),
            opt(r.end_month),
            json!(r.day),
            json!(money(r.amount_agorot)),
            json!(r.amount_agorot),
            opt(r.category_id),
            name(&cat_names, r.category_id).into(),
            opt(r.payment_method_id),
            name(&pm_names, r.payment_method_id).into(),
            r.note.into(),
        ]);
    }

    let mut settled = header(&["Series id", "Month"]);
    for (series, month) in storage.settled_recurring().await? {
        settled.push(vec![json!(series), month.into()]);
    }

    let json_cell = |v: Option<Value>| v.map(|v| v.to_string()).unwrap_or_default();
    let mut log = header(&["Id", "At (UTC)", "Kind", "Entity id", "Old", "New"]);
    for e in storage.list_log(1_000_000, None).await? {
        log.push(vec![
            json!(e.id),
            e.at.into(),
            e.kind.into(),
            opt(e.entity_id),
            json_cell(e.old).into(),
            json_cell(e.new).into(),
        ]);
    }

    let mut foreign = header(&[
        "Transaction id",
        "Currency",
        "Original amount",
        "Rate (ILS per unit)",
        "Rate date",
        "Estimated",
    ]);
    for f in storage.list_tx_fx().await? {
        foreign.push(vec![
            json!(f.transaction_id),
            f.currency.clone().into(),
            json!(f.orig_minor as f64 / 10f64.powi(crate::fx::decimals(&f.currency) as i32)),
            json!(f.rate),
            f.rate_date.into(),
            json!(f.estimated),
        ]);
    }

    let mut deletes = header(&["Paamonim line id", "Date"]);
    for d in storage.pending_line_deletes().await? {
        deletes.push(vec![d.line_id.into(), d.date.into()]);
    }

    let mut meta = header(&["Key", "Value"]);
    for (k, v) in raw.meta {
        meta.push(vec![k.into(), v.into()]);
    }

    Ok(vec![
        ("Transactions", tx),
        ("Categories", cats),
        ("Payment methods", pms),
        ("Recurring", rec),
        ("Recurring settled", settled),
        ("Change log", log),
        ("Foreign currency", foreign),
        ("Paamonim pending deletes", deletes),
        ("Paamonim meta", meta),
    ])
}

async fn check(resp: reqwest::Response) -> Result<Value> {
    let status = resp.status();
    let body: Value = resp.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        bail!("Google Sheets API {status}: {}", body["error"]["message"]);
    }
    Ok(body)
}

pub async fn export(storage: &dyn Storage) -> Result<ExportSummary> {
    let sa = parse_service_account(
        &std::env::var("GOOGLE_SERVICE_ACCOUNT_JSON_B64")
            .or_else(|_| std::env::var("GOOGLE_SERVICE_ACCOUNT_JSON"))
            .context("GOOGLE_SERVICE_ACCOUNT_JSON_B64 not set")?,
    )?;
    let sheet_id = std::env::var("GOOGLE_SHEET_ID").context("GOOGLE_SHEET_ID not set")?;
    let tabs = build_tabs(storage).await?;

    let http = reqwest::Client::new();
    let token = access_token(&http, &sa).await?;
    let base = format!("{SHEETS}/{sheet_id}");

    // Create the tabs that are missing; drop the old `Monthly` tab, which is no longer written.
    let meta = check(
        http.get(format!("{base}?fields=sheets.properties.title"))
            .bearer_auth(&token)
            .send()
            .await?,
    )
    .await?;
    let sheets_meta: Vec<(i64, &str)> = meta["sheets"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| {
            Some((
                s["properties"]["sheetId"].as_i64()?,
                s["properties"]["title"].as_str()?,
            ))
        })
        .collect();
    let mut requests: Vec<Value> = tabs
        .iter()
        .filter(|(t, _)| !sheets_meta.iter().any(|(_, e)| e == t))
        .map(|(t, _)| json!({"addSheet": {"properties": {"title": t}}}))
        .collect();
    // Only if another tab remains, since a spreadsheet can't have zero.
    if let Some((id, _)) = sheets_meta.iter().find(|(_, t)| *t == "Monthly") {
        requests.push(json!({"deleteSheet": {"sheetId": id}}));
    }
    if !requests.is_empty() {
        check(
            http.post(format!("{base}:batchUpdate"))
                .bearer_auth(&token)
                .json(&json!({"requests": requests}))
                .send()
                .await?,
        )
        .await?;
    }

    for (tab, rows) in &tabs {
        let range = format!("'{tab}'");
        check(
            http.post(format!("{base}/values/{range}:clear"))
                .bearer_auth(&token)
                .json(&json!({}))
                .send()
                .await?,
        )
        .await?;
        check(
            http.put(format!("{base}/values/{range}!A1?valueInputOption=RAW"))
                .bearer_auth(&token)
                .json(&json!({"range": format!("{range}!A1"), "majorDimension": "ROWS", "values": rows}))
                .send()
                .await?,
        )
        .await?;
    }
    Ok(ExportSummary {
        transactions: tabs[0].1.len() - 1,
        tabs: tabs.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{NewTransaction, SqliteStorage};

    #[test]
    fn accepts_raw_json_and_base64() {
        let raw = r#"{"client_email":"a@b.iam.gserviceaccount.com","private_key":"k"}"#;
        assert_eq!(
            parse_service_account(raw).unwrap().client_email,
            "a@b.iam.gserviceaccount.com"
        );
        let b64 = base64::engine::general_purpose::STANDARD.encode(raw);
        assert_eq!(
            parse_service_account(&b64).unwrap().token_uri,
            "https://oauth2.googleapis.com/token"
        );
    }

    #[tokio::test]
    async fn tabs_keep_non_round_amounts_and_all_tables() {
        let s = SqliteStorage::open(":memory:").unwrap();
        s.create_transaction(&NewTransaction {
            date: "2026-10-03".into(),
            amount_agorot: -1050,
            category_id: None,
            payment_method_id: None,
            note: "x".into(),
        })
        .await
        .unwrap();
        let tabs = build_tabs(&s).await.unwrap();
        let titles: Vec<_> = tabs.iter().map(|(t, _)| *t).collect();
        assert!(titles.contains(&"Categories") && titles.contains(&"Recurring"));
        assert!(!titles.contains(&"Monthly"));
        let tx = &tabs[0].1;
        assert_eq!(tx[1][2], json!(-10.5));
        assert_eq!(tx[1][3], json!(-1050));
    }
}
