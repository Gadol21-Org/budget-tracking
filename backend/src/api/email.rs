//! Email suggestions: connecting Gmail, the review queue, and the Pub/Sub push endpoint.

use super::error::{internal, ApiError};
use super::Db;
use crate::email::{Service, Status, SyncSummary};
use crate::storage::EmailSuggestion;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Redirect;
use axum::{Extension, Json};
use base64::Engine;
use serde::Deserialize;
use std::sync::Arc;

type Svc = Extension<Arc<Service>>;

pub(crate) async fn status(Extension(svc): Svc) -> Result<Json<Status>, ApiError> {
    svc.status().await.map(Json).map_err(internal)
}

/// Sends the browser to Google's consent screen.
pub(crate) async fn connect(Extension(svc): Svc) -> Result<Redirect, ApiError> {
    svc.consent_url()
        .await
        .map(|u| Redirect::to(&u))
        .map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, format!("{e:#}")))
}

#[derive(Deserialize)]
pub(crate) struct Callback {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

/// Google sends the browser back here. The answer is a redirect into the app, which shows the result.
pub(crate) async fn callback(Extension(svc): Svc, Query(q): Query<Callback>) -> Redirect {
    let fail = |msg: &str| {
        let mut u = reqwest::Url::parse("http://x/").unwrap();
        u.query_pairs_mut()
            .append_pair("gmail_error", &crate::email::truncate(msg, 300));
        Redirect::to(&format!("/?{}", u.query().unwrap_or("")))
    };
    if let Some(e) = q.error {
        return fail(&format!("Google: {e}"));
    }
    let (Some(code), Some(state)) = (q.code, q.state) else {
        return fail("missing code");
    };
    match svc.finish_connect(&code, &state).await {
        Ok(()) => {
            // First look at the label right away.
            svc.nudge();
            Redirect::to("/?gmail=connected")
        }
        Err(e) => {
            tracing::warn!("connecting Gmail failed: {e:#}");
            fail(&format!("{e:#}"))
        }
    }
}

pub(crate) async fn disconnect(Extension(svc): Svc) -> Result<StatusCode, ApiError> {
    svc.disconnect().await.map_err(internal)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize, Default)]
pub(crate) struct ScanBody {
    days: Option<u32>,
}

/// Read the label now. With `days` it also looks back that far (the default sync only looks
/// at what changed since the last one).
pub(crate) async fn sync(
    Extension(svc): Svc,
    body: Option<Json<ScanBody>>,
) -> Result<Json<SyncSummary>, ApiError> {
    let days = body.and_then(|b| b.0.days);
    let r = match days {
        Some(d) => svc.scan(Some(d)).await,
        None => svc.sync().await,
    };
    r.map(Json)
        .map_err(|e| ApiError::new(StatusCode::BAD_GATEWAY, format!("{e:#}")))
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

#[derive(Deserialize)]
pub(crate) struct PushQuery {
    token: Option<String>,
}

/// Pub/Sub push endpoint. Open to the internet (it has to be reachable by Google), so it
/// checks the caller first and does nothing with the message but start a sync.
pub(crate) async fn push(
    Extension(svc): Svc,
    Query(q): Query<PushQuery>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> StatusCode {
    let bearer = headers.get("authorization").and_then(|v| v.to_str().ok());
    if let Err(code) = svc.authorize_push(q.token.as_deref(), bearer).await {
        return code;
    }
    // {"message":{"data":"<base64 of {emailAddress, historyId}>"}, ...}
    let email = serde_json::from_slice::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| v["message"]["data"].as_str().map(String::from))
        .and_then(|d| {
            base64::engine::general_purpose::STANDARD
                .decode(d.trim())
                .ok()
        })
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .and_then(|v| v["emailAddress"].as_str().map(String::from));
    // Answer 2xx either way so Pub/Sub does not keep retrying a message we cannot use.
    if let Some(email) = email {
        if svc.is_our_mailbox(&email).await {
            svc.nudge();
        }
    }
    StatusCode::NO_CONTENT
}
