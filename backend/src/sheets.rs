//! Snapshot export to a Google Sheet owned by the user. A service account with
//! Editor access to the sheet writes two tabs: `Transactions` and `Monthly`.
//! The tabs are rewritten on every export, so the sheet is a readable copy, not a source.
//!
//! Config: `GOOGLE_SERVICE_ACCOUNT_JSON_B64` (base64 of the key file; raw JSON also accepted)
//! and `GOOGLE_SHEET_ID`.
//!
//! NOTE: not yet run against Google.

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
    pub monthly_rows: usize,
}

fn parse_service_account(raw: &str) -> Result<ServiceAccount> {
    let raw = raw.trim();
    let json = if raw.starts_with('{') {
        raw.to_string()
    } else {
        let bytes = base64::engine::general_purpose::STANDARD.decode(raw).context("service account is neither JSON nor base64")?;
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
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs();
    let claims = Claims { iss: &sa.client_email, scope: SCOPE, aud: &sa.token_uri, iat: now, exp: now + 3600 };
    let jwt = jsonwebtoken::encode(&Header::new(Algorithm::RS256), &claims, &EncodingKey::from_rsa_pem(sa.private_key.as_bytes())?)?;
    let resp: Value = http
        .post(&sa.token_uri)
        .form(&[("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"), ("assertion", &jwt)])
        .send()
        .await?
        .json()
        .await?;
    resp["access_token"].as_str().map(str::to_string).ok_or_else(|| anyhow!("token exchange failed: {}", resp["error"]))
}

fn money(agorot: i64) -> f64 {
    agorot as f64 / 100.0
}

/// Build the two tabs as rows of cells.
pub async fn build_tabs(storage: &dyn Storage) -> Result<(Vec<Vec<Value>>, Vec<Vec<Value>>)> {
    let cats: HashMap<i64, String> = storage.list_categories().await?.into_iter().map(|c| (c.id, c.name)).collect();
    let pms: HashMap<i64, String> = storage.list_payment_methods().await?.into_iter().map(|p| (p.id, p.name)).collect();
    let name = |m: &HashMap<i64, String>, id: Option<i64>| id.and_then(|i| m.get(&i).cloned()).unwrap_or_default();

    let mut tx = vec![["Date", "Category", "Payment method", "Amount (ILS)", "Note", "Sync", "Id"].map(Value::from).to_vec()];
    for t in storage.list_transactions().await? {
        tx.push(vec![
            t.date.into(),
            name(&cats, t.category_id).into(),
            name(&pms, t.payment_method_id).into(),
            json!(money(t.amount_agorot)),
            t.note.into(),
            t.sync_state.into(),
            t.id.to_string().into(),
        ]);
    }
    let mut monthly = vec![["Month", "Category", "Total (ILS)", "Count"].map(Value::from).to_vec()];
    for m in storage.monthly_totals().await? {
        monthly.push(vec![m.month.into(), name(&cats, m.category_id).into(), json!(money(m.total_agorot)), json!(m.count)]);
    }
    Ok((tx, monthly))
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
    let sa = parse_service_account(&std::env::var("GOOGLE_SERVICE_ACCOUNT_JSON_B64").or_else(|_| std::env::var("GOOGLE_SERVICE_ACCOUNT_JSON")).context("GOOGLE_SERVICE_ACCOUNT_JSON_B64 not set")?)?;
    let sheet_id = std::env::var("GOOGLE_SHEET_ID").context("GOOGLE_SHEET_ID not set")?;
    let (tx, monthly) = build_tabs(storage).await?;

    let http = reqwest::Client::new();
    let token = access_token(&http, &sa).await?;
    let base = format!("{SHEETS}/{sheet_id}");

    // Create the two tabs if they're missing.
    let meta = check(http.get(format!("{base}?fields=sheets.properties.title")).bearer_auth(&token).send().await?).await?;
    let existing: Vec<&str> = meta["sheets"].as_array().into_iter().flatten().filter_map(|s| s["properties"]["title"].as_str()).collect();
    let missing: Vec<Value> = ["Transactions", "Monthly"]
        .iter()
        .filter(|t| !existing.contains(t))
        .map(|t| json!({"addSheet": {"properties": {"title": t}}}))
        .collect();
    if !missing.is_empty() {
        check(http.post(format!("{base}:batchUpdate")).bearer_auth(&token).json(&json!({"requests": missing})).send().await?).await?;
    }

    for (tab, rows) in [("Transactions", &tx), ("Monthly", &monthly)] {
        check(http.post(format!("{base}/values/{tab}:clear")).bearer_auth(&token).json(&json!({})).send().await?).await?;
        check(
            http.put(format!("{base}/values/{tab}!A1?valueInputOption=RAW"))
                .bearer_auth(&token)
                .json(&json!({"range": format!("{tab}!A1"), "majorDimension": "ROWS", "values": rows}))
                .send()
                .await?,
        )
        .await?;
    }
    Ok(ExportSummary { transactions: tx.len() - 1, monthly_rows: monthly.len() - 1 })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{NewTransaction, SqliteStorage};

    #[test]
    fn accepts_raw_json_and_base64() {
        let raw = r#"{"client_email":"a@b.iam.gserviceaccount.com","private_key":"k"}"#;
        assert_eq!(parse_service_account(raw).unwrap().client_email, "a@b.iam.gserviceaccount.com");
        let b64 = base64::engine::general_purpose::STANDARD.encode(raw);
        assert_eq!(parse_service_account(&b64).unwrap().token_uri, "https://oauth2.googleapis.com/token");
    }

    #[tokio::test]
    async fn tabs_keep_non_round_amounts() {
        let s = SqliteStorage::open(":memory:").unwrap();
        s.create_transaction(&NewTransaction { date: "2026-10-03".into(), amount_agorot: -1050, category_id: None, payment_method_id: None, note: "x".into() }).await.unwrap();
        let (tx, monthly) = build_tabs(&s).await.unwrap();
        assert_eq!(tx[1][3], json!(-10.5));
        assert_eq!(monthly[1][2], json!(-10.5));
    }
}
