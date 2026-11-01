mod businesses;
mod categories;
mod error;
mod mirroring;
mod repeating;
mod transactions;

pub(crate) use mirroring::{mirroring_enabled, run_mirror};

use self::error::{internal, ApiError};
use self::mirroring::trigger_mirror;
use crate::assets;
use crate::oplog;
use crate::recurring;
use crate::sheets;
use crate::storage::{MonthlyTotal, Storage};
use axum::extract::{Query, State};
use axum::routing::{delete, get};
use axum::{Json, Router};
use serde::Deserialize;
use std::sync::Arc;

pub(crate) type Db = Arc<dyn Storage>;

pub(crate) fn today() -> chrono::NaiveDate {
    chrono::Local::now().date_naive()
}

/// Make charges that have come due real transactions before anything reads them.
pub(crate) async fn settle(db: &Db) -> Result<(), ApiError> {
    let added = recurring::settle_due(db.as_ref(), today())
        .await
        .map_err(internal)?;
    // Newly settled charges are ordinary transactions waiting to be mirrored.
    if added > 0 {
        trigger_mirror(db);
    }
    Ok(())
}

#[derive(Deserialize)]
pub(crate) struct LogQuery {
    /// Entries per page, newest first (default 100, at most 500).
    limit: Option<usize>,
    /// Only entries older than this one: pass the last id of the previous page.
    before_id: Option<i64>,
}

pub(crate) async fn change_log(
    State(db): State<Db>,
    Query(q): Query<LogQuery>,
) -> Result<Json<Vec<oplog::LogEntry>>, ApiError> {
    db.list_log(q.limit.unwrap_or(100).clamp(1, 500), q.before_id)
        .await
        .map(Json)
        .map_err(internal)
}

/// Totals of real transactions plus the upcoming repeating ones up to the default horizon.
pub(crate) async fn monthly_totals(
    State(db): State<Db>,
) -> Result<Json<Vec<MonthlyTotal>>, ApiError> {
    settle(&db).await?;
    let mut totals = db.monthly_totals().await.map_err(internal)?;
    let through = recurring::default_through(today());
    let ahead = recurring::upcoming_stored(db.as_ref(), &through, today())
        .await
        .map_err(internal)?;
    recurring::add_to_totals(&mut totals, &ahead);
    Ok(Json(totals))
}

pub(crate) async fn export_sheets(
    State(db): State<Db>,
) -> Result<Json<sheets::ExportSummary>, ApiError> {
    settle(&db).await?;
    sheets::export(db.as_ref())
        .await
        .map(Json)
        .map_err(internal)
}

/// All HTTP routes. A separate function so a test can build it: axum panics on
/// overlapping routes when the router is constructed, which would otherwise only
/// show up when the server starts.
pub(crate) fn router(db: Db) -> Router {
    Router::new()
        .route(
            "/api/transactions",
            get(transactions::list_transactions).post(transactions::create_transaction),
        )
        .route("/api/fx/rate", get(transactions::fx_rate))
        .route("/api/foreign", get(transactions::list_foreign))
        .route(
            "/api/settings/currencies",
            get(transactions::get_currency_settings).put(transactions::put_currency_settings),
        )
        .route(
            "/api/transactions/:id/refresh-rate",
            axum::routing::post(transactions::refresh_rate),
        )
        .route(
            "/api/transactions/:id",
            delete(transactions::delete_transaction).put(transactions::update_transaction),
        )
        .route(
            "/api/categories",
            get(categories::list_categories).post(categories::create_category),
        )
        .route("/api/categories/:id", delete(categories::delete_category))
        .route(
            "/api/categories/:id/link",
            axum::routing::put(categories::link_category),
        )
        .route(
            "/api/categories/:id/move",
            axum::routing::post(categories::move_category),
        )
        .route(
            "/api/recurring",
            get(repeating::list_recurring).post(repeating::create_recurring),
        )
        .route(
            "/api/recurring/occurrences",
            get(repeating::recurring_occurrences),
        )
        .route(
            "/api/recurring/:series_id/months/:month",
            axum::routing::post(repeating::settle_recurring_month)
                .delete(repeating::skip_recurring_month),
        )
        .route(
            "/api/recurring/:series_id",
            axum::routing::put(repeating::change_recurring).delete(repeating::cancel_recurring),
        )
        .route(
            "/api/businesses",
            get(businesses::get_businesses).post(businesses::create_business),
        )
        .route(
            "/api/businesses/:id",
            delete(businesses::delete_business).put(businesses::rename_business),
        )
        .route(
            "/api/businesses/tag",
            axum::routing::put(businesses::tag_transactions),
        )
        .route(
            "/api/categories/:id/business",
            axum::routing::put(businesses::link_category_business),
        )
        .route(
            "/api/categories/:id/business-required",
            axum::routing::put(businesses::require_category_business),
        )
        .route("/api/monthly-totals", get(monthly_totals))
        .route("/api/log", get(change_log))
        .route(
            "/api/import/paamonim",
            axum::routing::post(mirroring::import_paamonim),
        )
        .route("/api/export/sheets", axum::routing::post(export_sheets))
        .route(
            "/api/mirror/paamonim",
            axum::routing::post(mirroring::mirror_paamonim),
        )
        .route("/api/mirror/status", get(mirroring::mirror_status))
        .route(
            "/api/mirror/paamonim/retry/:id",
            axum::routing::post(mirroring::retry_mirror),
        )
        .with_state(db)
        .fallback(assets::static_handler)
}
