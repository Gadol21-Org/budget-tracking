use super::error::{internal, ApiError};
use super::mirroring::trigger_mirror;
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
    // The month is now an ordinary `pending` transaction: send it to Paamonim, or it waits
    // (spinner and all) until some other change happens to trigger a mirror run.
    trigger_mirror(&db);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oplog::Logged;
    use crate::recurring::NewRecurring;
    use std::sync::Arc;

    /// Editing one month on its own leaves a `pending` transaction for the mirror to pick up
    /// (the handler then triggers the mirror, like every other save).
    #[tokio::test]
    async fn editing_one_month_leaves_a_pending_line_for_the_mirror() {
        let db: Db = Arc::new(Logged(Arc::new(crate::testdb::fresh().await)));
        let month = today().format("%Y-%m").to_string();
        let series = recurring::create(
            db.as_ref(),
            &NewRecurring {
                start_month: month.clone(),
                end_month: None,
                day: 28,
                amount_agorot: -50_000,
                category_id: None,
                payment_method_id: None,
                note: String::new(),
                currency: None,
                orig_minor: None,
            },
        )
        .await
        .unwrap();
        let t = crate::storage::NewTransaction {
            date: today().format("%Y-%m-%d").to_string(),
            amount_agorot: -40_000,
            category_id: None,
            payment_method_id: None,
            note: "once".into(),
        };
        recurring::settle_month(db.as_ref(), series, &month, &t, None)
            .await
            .unwrap();
        let pending = db.pending_mirrors().await.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].amount_agorot, -40_000);
    }
}
