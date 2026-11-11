//! Email suggestions: the inbound endpoint (called by the Cloudflare Worker) and the review queue.

use super::error::{internal, ApiError};
use super::Db;
use crate::email::rules::{self, Rule};
use crate::email::{IngestError, Ingested, Service, Status};
use crate::storage::EmailSuggestion;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

type Svc = Extension<Arc<Service>>;

/// Mail larger than this is refused (the Worker also drops it).
pub(crate) const MAX_MAIL_BYTES: usize = 11 * 1024 * 1024;

pub(crate) async fn status(Extension(svc): Svc) -> Result<Json<Status>, ApiError> {
    svc.status().await.map(Json).map_err(internal)
}

/// The Worker posts the raw message here. Open to the internet (Cloudflare Access is bypassed for
/// this one path), so everything starts with the shared secret in `Authorization: Bearer`.
pub(crate) async fn inbound(
    Extension(svc): Svc,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Json<Ingested>, StatusCode> {
    // Off (no secret configured) looks like no such route.
    if svc.cfg.is_none() {
        return Err(StatusCode::NOT_FOUND);
    }
    let secret = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    if !svc.authorized(secret) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    match svc.ingest(&body).await {
        Ok(r) => Ok(Json(r)),
        // Accepted but not usable: answer 2xx-free errors only for what the sender can fix.
        Err(IngestError::Refused(why)) => {
            tracing::info!("inbound email refused: {why}");
            Err(StatusCode::UNPROCESSABLE_ENTITY)
        }
        Err(IngestError::Internal(e)) => {
            tracing::warn!("inbound email failed: {e:#}");
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

#[derive(Deserialize)]
pub(crate) struct StatusQuery {
    status: Option<String>,
}

pub(crate) async fn list(
    State(db): State<Db>,
    Query(q): Query<StatusQuery>,
) -> Result<Json<Vec<EmailSuggestion>>, ApiError> {
    db.list_email_suggestions(q.status.as_deref())
        .await
        .map(Json)
        .map_err(internal)
}

#[derive(Deserialize)]
pub(crate) struct Accepted {
    transaction_id: i64,
}

/// The app made the transaction (through the normal add dialog); record which one.
pub(crate) async fn accept(
    State(db): State<Db>,
    Path(id): Path<i64>,
    Json(b): Json<Accepted>,
) -> Result<StatusCode, ApiError> {
    if !db
        .list_transactions()
        .await
        .map_err(internal)?
        .iter()
        .any(|t| t.id == b.transaction_id)
    {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "unknown transaction".into(),
        ));
    }
    resolved(
        db.resolve_email_suggestion(id, "added", Some(b.transaction_id))
            .await,
    )
}

/// Which transactions came from an email (they get a badge in the list).
pub(crate) async fn transaction_ids(State(db): State<Db>) -> Result<Json<Vec<i64>>, ApiError> {
    db.email_transaction_ids().await.map(Json).map_err(internal)
}

pub(crate) async fn rules_list(State(db): State<Db>) -> Result<Json<Vec<Rule>>, ApiError> {
    rules::list(db.as_ref()).await.map(Json).map_err(internal)
}

/// A rule, and optionally the transaction it was made from: that one is moved to the rule's
/// category and business too.
#[derive(Deserialize)]
pub(crate) struct RulePut {
    #[serde(flatten)]
    rule: Rule,
    apply_to: Option<i64>,
}

/// Always file mail with this subject under a category (and business).
pub(crate) async fn rules_put(
    State(db): State<Db>,
    Json(RulePut { rule: r, apply_to }): Json<RulePut>,
) -> Result<StatusCode, ApiError> {
    let bad = |m: &str| ApiError::new(StatusCode::BAD_REQUEST, m.to_string());
    if rules::normalize(&r.subject).is_empty() {
        return Err(bad("the mail has no subject to match"));
    }
    let cats = db.list_categories().await.map_err(internal)?;
    if !cats.iter().any(|c| c.id == r.category_id && !c.is_income) {
        return Err(bad("unknown expense category"));
    }
    if let Some(b) = r.business_id {
        let known = db.list_businesses().await.map_err(internal)?;
        if !known.iter().any(|x| x.id == b) {
            return Err(bad("unknown business"));
        }
    }
    if let Some(id) = apply_to {
        let tx = db
            .list_transactions()
            .await
            .map_err(internal)?
            .into_iter()
            .find(|t| t.id == id)
            .ok_or_else(|| bad("unknown transaction"))?;
        db.update_transaction(
            id,
            &crate::storage::NewTransaction {
                date: tx.date,
                amount_agorot: tx.amount_agorot,
                category_id: Some(r.category_id),
                payment_method_id: tx.payment_method_id,
                note: tx.note,
            },
        )
        .await
        .map_err(internal)?;
        db.set_transactions_business(&[id], r.business_id)
            .await
            .map_err(internal)?;
        super::mirroring::trigger_mirror(&db);
    }
    rules::put(db.as_ref(), r).await.map_err(internal)?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{NewCategory, NewTransaction};

    #[tokio::test]
    async fn a_rule_made_from_a_transaction_moves_that_transaction_too() {
        let db: Db = std::sync::Arc::new(crate::testdb::fresh().await);
        let new = |name: &str, parent: Option<i64>| NewCategory {
            name: name.into(),
            is_income: false,
            parent_id: parent,
            linked_to: None,
        };
        let top = db.create_category(&new("Top", None)).await.unwrap();
        let a = db.create_category(&new("A", Some(top))).await.unwrap();
        let b = db.create_category(&new("B", Some(top))).await.unwrap();
        let biz = db.insert_business("Studio", false).await.unwrap();
        let tx = db
            .create_transaction(&NewTransaction {
                date: "2026-03-01".into(),
                amount_agorot: -1250,
                category_id: Some(a),
                payment_method_id: None,
                note: "shop".into(),
            })
            .await
            .unwrap();
        let body = RulePut {
            rule: Rule {
                subject: "Receipt".into(),
                category_id: b,
                business_id: Some(biz),
            },
            apply_to: Some(tx),
        };
        rules_put(State(db.clone()), Json(body)).await.unwrap();
        let t = &db.list_transactions().await.unwrap()[0];
        assert_eq!(
            (t.category_id, t.amount_agorot, t.note.as_str()),
            (Some(b), -1250, "shop")
        );
        assert_eq!(
            db.business_links().await.unwrap().transactions,
            vec![(tx, biz)]
        );
        assert_eq!(rules::list(db.as_ref()).await.unwrap().len(), 1);
    }
}

#[derive(Deserialize)]
pub(crate) struct RuleSubject {
    subject: String,
}

pub(crate) async fn rules_delete(
    State(db): State<Db>,
    Json(b): Json<RuleSubject>,
) -> Result<StatusCode, ApiError> {
    match rules::remove(db.as_ref(), &b.subject)
        .await
        .map_err(internal)?
    {
        true => Ok(StatusCode::NO_CONTENT),
        false => Err(ApiError::new(StatusCode::NOT_FOUND, "no such rule".into())),
    }
}

pub(crate) async fn reject(
    State(db): State<Db>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    resolved(db.resolve_email_suggestion(id, "rejected", None).await)
}

fn resolved(r: anyhow::Result<bool>) -> Result<StatusCode, ApiError> {
    match r.map_err(internal)? {
        true => Ok(StatusCode::NO_CONTENT),
        false => Err(ApiError::new(
            StatusCode::CONFLICT,
            "this suggestion was already handled".into(),
        )),
    }
}

/// Drop a message the AI could not read (queued or failed) without adding anything.
pub(crate) async fn discard(
    State(db): State<Db>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    match db.delete_email_suggestion(id).await.map_err(internal)? {
        true => Ok(StatusCode::NO_CONTENT),
        false => Err(ApiError::new(
            StatusCode::CONFLICT,
            "nothing to remove".into(),
        )),
    }
}

/// Read the waiting messages now (all of them, or just one).
pub(crate) async fn retry(
    Extension(svc): Svc,
    Query(q): Query<RetryQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let done = svc.retry_queued(q.id).await.map_err(internal)?;
    Ok(Json(serde_json::json!({ "read": done })))
}

#[derive(Deserialize)]
pub(crate) struct RetryQuery {
    id: Option<i64>,
}

/// The email as it looked, for the "view email" frame. It is untrusted HTML, so it is served
/// with a policy that allows no script, no remote content (no tracking pixels) and no
/// navigation, only inline styles and embedded images; the app also shows it in a sandboxed frame.
pub(crate) async fn view(State(db): State<Db>, Path(id): Path<i64>) -> Result<Response, ApiError> {
    let Some(html) = db.email_html(id).await.map_err(internal)? else {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "no email to show".into(),
        ));
    };
    Ok((
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (
                header::CONTENT_SECURITY_POLICY,
                "default-src 'none'; style-src 'unsafe-inline'; img-src data: https:; sandbox; base-uri 'none'; form-action 'none'",
            ),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (header::REFERRER_POLICY, "no-referrer"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        html,
    )
        .into_response())
}

#[derive(Deserialize)]
pub(crate) struct CostQuery {
    /// First day, `YYYY-MM-DD` (UTC).
    from: String,
    /// Last day, `YYYY-MM-DD` (UTC), included.
    to: String,
}

#[derive(Serialize)]
pub(crate) struct CostItem {
    /// When the AI was asked (UTC, ISO 8601).
    at: String,
    sender: String,
    subject: String,
    input_tokens: i64,
    output_tokens: i64,
    /// Millionths of a US dollar.
    cost_micro_usd: i64,
    /// `ignored`, `unreadable` or `N suggestions`.
    result: String,
}

#[derive(Serialize)]
pub(crate) struct Cost {
    items: Vec<CostItem>,
    total_micro_usd: i64,
}

fn valid_day(d: &str) -> Option<chrono::NaiveDate> {
    chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok()
}

/// What the AI calls that read emails cost in a period: one item per call, and the total.
pub(crate) async fn ai_cost(
    State(db): State<Db>,
    Query(q): Query<CostQuery>,
) -> Result<Json<Cost>, ApiError> {
    let bad = || {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "from and to must be YYYY-MM-DD".to_string(),
        )
    };
    let from = valid_day(&q.from).ok_or_else(bad)?;
    let to = valid_day(&q.to).ok_or_else(bad)?;
    let before = to.succ_opt().ok_or_else(bad)?;
    let stamp = |d: chrono::NaiveDate| format!("{d}T00:00:00Z");
    let entries = db
        .list_log_kind(crate::oplog::kind::AI_EMAIL, &stamp(from), &stamp(before))
        .await
        .map_err(internal)?;
    let items: Vec<CostItem> = entries
        .into_iter()
        .map(|e| {
            let n = e.new.unwrap_or_default();
            let text = |k: &str| n[k].as_str().unwrap_or_default().to_string();
            let num = |k: &str| n[k].as_i64().unwrap_or(0);
            CostItem {
                at: e.at,
                sender: text("sender"),
                subject: text("subject"),
                input_tokens: num("input_tokens"),
                output_tokens: num("output_tokens"),
                cost_micro_usd: num("cost_micro_usd"),
                result: text("result"),
            }
        })
        .collect();
    let total_micro_usd = items.iter().map(|i| i.cost_micro_usd).sum();
    Ok(Json(Cost {
        items,
        total_micro_usd,
    }))
}
