//! Email suggestions: the inbound endpoint (called by the Cloudflare Worker) and the review queue.

use super::error::{internal, ApiError};
use super::Db;
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
pub(crate) const MAX_MAIL_BYTES: usize = 2 * 1024 * 1024;

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
        db.resolve_email_suggestion(id, "accepted", Some(b.transaction_id))
            .await,
    )
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
pub(crate) struct Subscription {
    endpoint: String,
}

/// Remember this device's push endpoint, so new suggestions can notify it.
pub(crate) async fn push_subscribe(
    State(db): State<Db>,
    Json(b): Json<Subscription>,
) -> Result<StatusCode, ApiError> {
    crate::push::subscribe(db.as_ref(), &b.endpoint)
        .await
        .map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, format!("{e:#}")))?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
pub(crate) struct Registered {
    registered: bool,
}

/// Does the server already hold this device's push endpoint? Asked before offering to enable notifications.
pub(crate) async fn push_check(
    State(db): State<Db>,
    Json(b): Json<Subscription>,
) -> Result<Json<Registered>, ApiError> {
    let registered = crate::push::is_registered(db.as_ref(), &b.endpoint)
        .await
        .map_err(internal)?;
    Ok(Json(Registered { registered }))
}
