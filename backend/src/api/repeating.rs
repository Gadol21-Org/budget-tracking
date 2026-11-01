use super::error::{internal, ApiError};
use super::transactions::{resolve_body, TxBody};
use super::{settle, today, Db};
use crate::recurring;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

pub(crate) fn recurring_error(e: recurring::RecurringError) -> ApiError {
    match e {
        recurring::RecurringError::Invalid(m) => ApiError::new(StatusCode::BAD_REQUEST, m),
        recurring::RecurringError::Storage(e) => internal(e),
    }
}

pub(crate) async fn list_recurring(
    State(db): State<Db>,
) -> Result<Json<Vec<recurring::RecurringSegment>>, ApiError> {
    db.list_recurring().await.map(Json).map_err(internal)
}

#[derive(Deserialize)]
pub(crate) struct OccurrencesQuery {
    /// Last month to generate, YYYY-MM; defaults to two years ahead. Only upcoming
    /// (unsettled) occurrences are returned; settled ones are transactions.
    through: Option<String>,
}

pub(crate) async fn recurring_occurrences(
    State(db): State<Db>,
    Query(q): Query<OccurrencesQuery>,
) -> Result<Json<Vec<recurring::Occurrence>>, ApiError> {
    let through = match q.through {
        Some(m) if recurring::is_month(&m) => m,
        Some(_) => {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "through must be YYYY-MM".into(),
            ))
        }
        None => recurring::default_through(today()),
    };
    settle(&db).await?;
    recurring::upcoming_stored(db.as_ref(), &through, today())
        .await
        .map(Json)
        .map_err(internal)
}

/// Start a series; returns its id.
pub(crate) async fn create_recurring(
    State(db): State<Db>,
    Json(mut r): Json<recurring::NewRecurring>,
) -> Result<Json<i64>, ApiError> {
    recurring::price_rule(db.as_ref(), &mut r, today())
        .await
        .map_err(recurring_error)?;
    recurring::create(db.as_ref(), &r)
        .await
        .map(Json)
        .map_err(recurring_error)
}

/// Change the series from `start_month` of the body onward; earlier months are untouched.
pub(crate) async fn change_recurring(
    State(db): State<Db>,
    Path(series_id): Path<i64>,
    Json(mut r): Json<recurring::NewRecurring>,
) -> Result<StatusCode, ApiError> {
    settle(&db).await?;
    recurring::price_rule(db.as_ref(), &mut r, today())
        .await
        .map_err(recurring_error)?;
    recurring::change_from(db.as_ref(), series_id, &r, today())
        .await
        .map_err(recurring_error)?;
    Ok(StatusCode::NO_CONTENT)
}

/// Edit one month of a series on its own (`month` is YYYY-MM): it becomes a transaction with
/// the body's values and the rest of the series is unchanged.
pub(crate) async fn settle_recurring_month(
    State(db): State<Db>,
    Path((series_id, month)): Path<(i64, String)>,
    Json(b): Json<TxBody>,
) -> Result<StatusCode, ApiError> {
    settle(&db).await?;
    // A foreign month is priced like any foreign transaction: at its own day's rate.
    let (t, fx) = resolve_body(&db, None, b).await?;
    recurring::settle_month(db.as_ref(), series_id, &month, &t, fx.as_ref())
        .await
        .map_err(recurring_error)?;
    Ok(StatusCode::NO_CONTENT)
}

/// Skip one month of a series (`month` is YYYY-MM); the rest is unchanged.
pub(crate) async fn skip_recurring_month(
    State(db): State<Db>,
    Path((series_id, month)): Path<(i64, String)>,
) -> Result<StatusCode, ApiError> {
    settle(&db).await?;
    recurring::skip_month(db.as_ref(), series_id, &month)
        .await
        .map_err(recurring_error)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub(crate) struct CancelQuery {
    /// First month without an occurrence, YYYY-MM.
    from: String,
}

pub(crate) async fn cancel_recurring(
    State(db): State<Db>,
    Path(series_id): Path<i64>,
    Query(q): Query<CancelQuery>,
) -> Result<StatusCode, ApiError> {
    settle(&db).await?;
    recurring::cancel_from(db.as_ref(), series_id, &q.from, today())
        .await
        .map_err(recurring_error)?;
    Ok(StatusCode::NO_CONTENT)
}
