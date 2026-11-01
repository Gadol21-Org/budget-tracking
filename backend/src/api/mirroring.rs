use super::error::{internal, ApiError};
use super::Db;
use crate::import;
use crate::mirror;
use crate::paamonim;
use crate::storage;
use crate::storage::Storage;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

#[derive(Deserialize)]
pub(crate) struct ImportRequest {
    from: String,
    to: String,
}

/// Import from Paamonim (idempotent). `from`/`to` are YYYY-MM-DD.
pub(crate) async fn import_paamonim(
    State(db): State<Db>,
    Json(req): Json<ImportRequest>,
) -> Result<Json<storage::ImportSummary>, ApiError> {
    let client = paamonim_session().await.map_err(|e| match e {
        SessionError::NoCredentials(e) => e,
        SessionError::Login(e) => internal(e),
    })?;
    import::run(client, db.as_ref(), &req.from, &req.to)
        .await
        .map(Json)
        .map_err(|e| {
            tracing::warn!("paamonim import failed: {e:#}");
            internal(format!("{e:#}"))
        })
}

pub(crate) fn paamonim_credentials() -> Result<(String, String, String), ApiError> {
    let base =
        std::env::var("PAAMONIM_BASE_URL").unwrap_or_else(|_| "https://app.paamonim.org.il".into());
    let user = std::env::var("PAAMONIM_USERNAME").map_err(|_| {
        ApiError::new(
            StatusCode::PRECONDITION_FAILED,
            "PAAMONIM_USERNAME not set".into(),
        )
    })?;
    let pass = std::env::var("PAAMONIM_PASSWORD").map_err(|_| {
        ApiError::new(
            StatusCode::PRECONDITION_FAILED,
            "PAAMONIM_PASSWORD not set".into(),
        )
    })?;
    Ok((base, user, pass))
}

pub(crate) enum SessionError {
    NoCredentials(ApiError),
    Login(anyhow::Error),
}

/// One Paamonim session for the whole server: logged in on first use and kept, so each change
/// doesn't log in again. The client logs in again by itself when the session expires.
pub(crate) async fn paamonim_session() -> Result<&'static paamonim::Client, SessionError> {
    static SESSION: tokio::sync::OnceCell<paamonim::Client> = tokio::sync::OnceCell::const_new();
    let (base, user, pass) = paamonim_credentials().map_err(SessionError::NoCredentials)?;
    SESSION
        .get_or_try_init(|| paamonim::Client::login(&base, &user, &pass))
        .await
        .map_err(SessionError::Login)
}

/// Writing to Paamonim is on whenever Paamonim credentials are set; `PAAMONIM_MIRROR=0` turns
/// it off. Without credentials it is off, so nothing waits for a sync that can't happen.
pub(crate) fn mirroring_enabled() -> bool {
    let off = matches!(
        std::env::var("PAAMONIM_MIRROR").as_deref(),
        Ok("0") | Ok("false")
    );
    let has = |k: &str| std::env::var(k).is_ok_and(|v| !v.is_empty());
    !off && has("PAAMONIM_USERNAME") && has("PAAMONIM_PASSWORD")
}

/// Serialises mirror runs so overlapping triggers can't post the same line twice.
pub(crate) static MIRROR_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub(crate) async fn run_mirror(db: &dyn Storage) -> anyhow::Result<mirror::MirrorSummary> {
    let _guard = MIRROR_LOCK.lock().await;
    // Nothing to do: don't log in to Paamonim at all.
    if db.pending_mirrors().await?.is_empty() && db.pending_line_deletes().await?.is_empty() {
        return Ok(mirror::MirrorSummary::default());
    }
    let client = paamonim_session().await.map_err(|e| match e {
        SessionError::NoCredentials(e) => anyhow::anyhow!(e.message),
        SessionError::Login(e) => e,
    })?;
    mirror::run(client, db, &mirror::Options::from_env()).await
}

/// After a local change: mirror it, in the background. Failures stay visible via `sync_state`
/// and the log; nothing is retried blindly.
pub(crate) fn trigger_mirror(db: &Db) {
    if mirroring_enabled() {
        let db = db.clone();
        tokio::spawn(async move {
            match run_mirror(db.as_ref()).await {
                Ok(s) => {
                    if let Some(why) = &s.stopped {
                        tracing::warn!("paamonim mirror stopped: {why}");
                    }
                }
                Err(e) => tracing::warn!("paamonim mirror failed: {e}"),
            }
        });
    }
}

#[derive(serde::Serialize)]
pub(crate) struct MirrorStatus {
    /// Writing to Paamonim is on, so `pending`/`modified`/`verifying` lines are on their way.
    enabled: bool,
}

pub(crate) async fn mirror_status() -> Json<MirrorStatus> {
    Json(MirrorStatus {
        enabled: mirroring_enabled(),
    })
}

/// Put a transaction in `error` back in the queue (after checking Paamonim by hand).
pub(crate) async fn retry_mirror(
    State(db): State<Db>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    if db.retry_mirror(id).await.map_err(internal)? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::new(
            StatusCode::CONFLICT,
            "transaction is not in error".into(),
        ))
    }
}

pub(crate) async fn mirror_paamonim(
    State(db): State<Db>,
) -> Result<Json<mirror::MirrorSummary>, ApiError> {
    if !mirroring_enabled() {
        return Err(ApiError::new(
            StatusCode::PRECONDITION_FAILED,
            "PAAMONIM_MIRROR is not enabled".into(),
        ));
    }
    run_mirror(db.as_ref()).await.map(Json).map_err(internal)
}
