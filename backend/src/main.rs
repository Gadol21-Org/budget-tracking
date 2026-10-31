mod assets;
mod db;
mod import;
mod mirror;
mod money;
mod mysql;
mod oplog;
mod paamonim;
mod recurring;
mod sheets;
mod storage;

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{delete, get},
    Json, Router,
};
use serde::Deserialize;
use storage::{
    Category, CategoryError, MonthlyTotal, NewCategory, NewTransaction, SqliteStorage, Storage,
    Transaction,
};

type Db = Arc<dyn Storage>;

fn internal<E: std::fmt::Display>(e: E) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

fn today() -> chrono::NaiveDate {
    chrono::Local::now().date_naive()
}

/// Make charges that have come due real transactions before anything reads them.
async fn settle(db: &Db) -> Result<(), (StatusCode, String)> {
    let added = recurring::settle_due(db.as_ref(), today())
        .await
        .map_err(internal)?;
    // Newly settled charges are ordinary transactions waiting to be mirrored.
    if added > 0 {
        trigger_mirror(db);
    }
    Ok(())
}

async fn list_transactions(
    State(db): State<Db>,
) -> Result<Json<Vec<Transaction>>, (StatusCode, String)> {
    settle(&db).await?;
    db.list_transactions().await.map(Json).map_err(internal)
}

async fn create_transaction(
    State(db): State<Db>,
    Json(t): Json<NewTransaction>,
) -> Result<Json<i64>, (StatusCode, String)> {
    chrono::NaiveDate::parse_from_str(&t.date, "%Y-%m-%d")
        .map_err(|_| (StatusCode::BAD_REQUEST, "date must be YYYY-MM-DD".into()))?;
    let id = db.create_transaction(&t).await.map_err(internal)?;
    trigger_mirror(&db);
    Ok(Json(id))
}

async fn update_transaction(
    State(db): State<Db>,
    Path(id): Path<i64>,
    Json(t): Json<NewTransaction>,
) -> Result<StatusCode, (StatusCode, String)> {
    chrono::NaiveDate::parse_from_str(&t.date, "%Y-%m-%d")
        .map_err(|_| (StatusCode::BAD_REQUEST, "date must be YYYY-MM-DD".into()))?;
    db.update_transaction(id, &t).await.map_err(internal)?;
    trigger_mirror(&db);
    Ok(StatusCode::NO_CONTENT)
}

async fn delete_transaction(
    State(db): State<Db>,
    Path(id): Path<i64>,
) -> Result<StatusCode, (StatusCode, String)> {
    db.delete_transaction(id).await.map_err(internal)?;
    trigger_mirror(&db);
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct LogQuery {
    /// Entries per page, newest first (default 100, at most 500).
    limit: Option<usize>,
    /// Only entries older than this one: pass the last id of the previous page.
    before_id: Option<i64>,
}

async fn change_log(
    State(db): State<Db>,
    Query(q): Query<LogQuery>,
) -> Result<Json<Vec<oplog::LogEntry>>, (StatusCode, String)> {
    db.list_log(q.limit.unwrap_or(100).clamp(1, 500), q.before_id)
        .await
        .map(Json)
        .map_err(internal)
}

async fn list_categories(
    State(db): State<Db>,
) -> Result<Json<Vec<Category>>, (StatusCode, String)> {
    db.list_categories().await.map(Json).map_err(internal)
}

/// Refusals of a category change are the caller's doing; anything else is a server error.
fn category_error(e: anyhow::Error) -> (StatusCode, String) {
    match e.downcast_ref::<CategoryError>() {
        Some(CategoryError::NotFound) => (StatusCode::NOT_FOUND, e.to_string()),
        Some(CategoryError::Invalid(_)) => (StatusCode::BAD_REQUEST, e.to_string()),
        Some(CategoryError::NeedsReplacement { .. }) => (StatusCode::CONFLICT, e.to_string()),
        None => internal(e),
    }
}

async fn create_category(
    State(db): State<Db>,
    Json(c): Json<NewCategory>,
) -> Result<Json<i64>, (StatusCode, String)> {
    db.create_category(&c)
        .await
        .map(Json)
        .map_err(category_error)
}

#[derive(Deserialize)]
struct DeleteCategoryQuery {
    /// Category that receives the transactions of the one being deleted.
    move_to: Option<i64>,
}

/// Also deletes the sub-categories. Answers 409 when transactions need a `move_to`.
async fn delete_category(
    State(db): State<Db>,
    Path(id): Path<i64>,
    Query(q): Query<DeleteCategoryQuery>,
) -> Result<Json<usize>, (StatusCode, String)> {
    db.delete_category(id, q.move_to)
        .await
        .map(Json)
        .map_err(category_error)
}

/// Totals of real transactions plus the upcoming repeating ones up to the default horizon.
async fn monthly_totals(
    State(db): State<Db>,
) -> Result<Json<Vec<MonthlyTotal>>, (StatusCode, String)> {
    settle(&db).await?;
    let mut totals = db.monthly_totals().await.map_err(internal)?;
    let through = recurring::default_through(today());
    let ahead = recurring::upcoming_stored(db.as_ref(), &through, today())
        .await
        .map_err(internal)?;
    recurring::add_to_totals(&mut totals, &ahead);
    Ok(Json(totals))
}

fn recurring_error(e: recurring::RecurringError) -> (StatusCode, String) {
    match e {
        recurring::RecurringError::Invalid(m) => (StatusCode::BAD_REQUEST, m),
        recurring::RecurringError::Storage(e) => internal(e),
    }
}

async fn list_recurring(
    State(db): State<Db>,
) -> Result<Json<Vec<recurring::RecurringSegment>>, (StatusCode, String)> {
    db.list_recurring().await.map(Json).map_err(internal)
}

#[derive(Deserialize)]
struct OccurrencesQuery {
    /// Last month to generate, YYYY-MM; defaults to two years ahead. Only upcoming
    /// (unsettled) occurrences are returned; settled ones are transactions.
    through: Option<String>,
}

async fn recurring_occurrences(
    State(db): State<Db>,
    Query(q): Query<OccurrencesQuery>,
) -> Result<Json<Vec<recurring::Occurrence>>, (StatusCode, String)> {
    let through = match q.through {
        Some(m) if recurring::is_month(&m) => m,
        Some(_) => return Err((StatusCode::BAD_REQUEST, "through must be YYYY-MM".into())),
        None => recurring::default_through(today()),
    };
    settle(&db).await?;
    recurring::upcoming_stored(db.as_ref(), &through, today())
        .await
        .map(Json)
        .map_err(internal)
}

/// Start a series; returns its id.
async fn create_recurring(
    State(db): State<Db>,
    Json(r): Json<recurring::NewRecurring>,
) -> Result<Json<i64>, (StatusCode, String)> {
    recurring::create(db.as_ref(), &r)
        .await
        .map(Json)
        .map_err(recurring_error)
}

/// Change the series from `start_month` of the body onward; earlier months are untouched.
async fn change_recurring(
    State(db): State<Db>,
    Path(series_id): Path<i64>,
    Json(r): Json<recurring::NewRecurring>,
) -> Result<StatusCode, (StatusCode, String)> {
    settle(&db).await?;
    recurring::change_from(db.as_ref(), series_id, &r, today())
        .await
        .map_err(recurring_error)?;
    Ok(StatusCode::NO_CONTENT)
}

/// Edit one month of a series on its own (`month` is YYYY-MM): it becomes a transaction with
/// the body's values and the rest of the series is unchanged.
async fn settle_recurring_month(
    State(db): State<Db>,
    Path((series_id, month)): Path<(i64, String)>,
    Json(t): Json<NewTransaction>,
) -> Result<StatusCode, (StatusCode, String)> {
    settle(&db).await?;
    recurring::settle_month(db.as_ref(), series_id, &month, &t)
        .await
        .map_err(recurring_error)?;
    Ok(StatusCode::NO_CONTENT)
}

/// Skip one month of a series (`month` is YYYY-MM); the rest is unchanged.
async fn skip_recurring_month(
    State(db): State<Db>,
    Path((series_id, month)): Path<(i64, String)>,
) -> Result<StatusCode, (StatusCode, String)> {
    settle(&db).await?;
    recurring::skip_month(db.as_ref(), series_id, &month)
        .await
        .map_err(recurring_error)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct CancelQuery {
    /// First month without an occurrence, YYYY-MM.
    from: String,
}

async fn cancel_recurring(
    State(db): State<Db>,
    Path(series_id): Path<i64>,
    Query(q): Query<CancelQuery>,
) -> Result<StatusCode, (StatusCode, String)> {
    settle(&db).await?;
    recurring::cancel_from(db.as_ref(), series_id, &q.from, today())
        .await
        .map_err(recurring_error)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct ImportRequest {
    from: String,
    to: String,
}

/// Import from Paamonim (idempotent). `from`/`to` are YYYY-MM-DD.
async fn import_paamonim(
    State(db): State<Db>,
    Json(req): Json<ImportRequest>,
) -> Result<Json<storage::ImportSummary>, (StatusCode, String)> {
    let base =
        std::env::var("PAAMONIM_BASE_URL").unwrap_or_else(|_| "https://app.paamonim.org.il".into());
    let user = std::env::var("PAAMONIM_USERNAME").map_err(|_| {
        (
            StatusCode::PRECONDITION_FAILED,
            "PAAMONIM_USERNAME not set".into(),
        )
    })?;
    let pass = std::env::var("PAAMONIM_PASSWORD").map_err(|_| {
        (
            StatusCode::PRECONDITION_FAILED,
            "PAAMONIM_PASSWORD not set".into(),
        )
    })?;
    let client = paamonim::Client::login(&base, &user, &pass)
        .await
        .map_err(internal)?;
    import::run(&client, db.as_ref(), &req.from, &req.to)
        .await
        .map(Json)
        .map_err(|e| {
            tracing::warn!("paamonim import failed: {e:#}");
            internal(format!("{e:#}"))
        })
}

fn paamonim_credentials() -> Result<(String, String, String), (StatusCode, String)> {
    let base =
        std::env::var("PAAMONIM_BASE_URL").unwrap_or_else(|_| "https://app.paamonim.org.il".into());
    let user = std::env::var("PAAMONIM_USERNAME").map_err(|_| {
        (
            StatusCode::PRECONDITION_FAILED,
            "PAAMONIM_USERNAME not set".into(),
        )
    })?;
    let pass = std::env::var("PAAMONIM_PASSWORD").map_err(|_| {
        (
            StatusCode::PRECONDITION_FAILED,
            "PAAMONIM_PASSWORD not set".into(),
        )
    })?;
    Ok((base, user, pass))
}

/// Writing to Paamonim is on whenever Paamonim credentials are set; `PAAMONIM_MIRROR=0` turns
/// it off. Without credentials it is off, so nothing waits for a sync that can't happen.
fn mirroring_enabled() -> bool {
    let off = matches!(
        std::env::var("PAAMONIM_MIRROR").as_deref(),
        Ok("0") | Ok("false")
    );
    let has = |k: &str| std::env::var(k).is_ok_and(|v| !v.is_empty());
    !off && has("PAAMONIM_USERNAME") && has("PAAMONIM_PASSWORD")
}

/// Serialises mirror runs so overlapping triggers can't post the same line twice.
static MIRROR_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn run_mirror(db: &dyn Storage) -> anyhow::Result<mirror::MirrorSummary> {
    let _guard = MIRROR_LOCK.lock().await;
    // Nothing to do: don't log in to Paamonim at all.
    if db.pending_mirrors().await?.is_empty() && db.pending_line_deletes().await?.is_empty() {
        return Ok(mirror::MirrorSummary::default());
    }
    let (base, user, pass) = paamonim_credentials().map_err(|e| anyhow::anyhow!(e.1))?;
    let client = paamonim::Client::login(&base, &user, &pass).await?;
    mirror::run(&client, db, &mirror::Options::from_env()).await
}

/// After a local change: mirror it, in the background. Failures stay visible via `sync_state`
/// and the log; nothing is retried blindly.
fn trigger_mirror(db: &Db) {
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
struct MirrorStatus {
    /// Writing to Paamonim is on, so `pending`/`modified`/`verifying` lines are on their way.
    enabled: bool,
}

async fn mirror_status() -> Json<MirrorStatus> {
    Json(MirrorStatus {
        enabled: mirroring_enabled(),
    })
}

/// Put a transaction in `error` back in the queue (after checking Paamonim by hand).
async fn retry_mirror(
    State(db): State<Db>,
    Path(id): Path<i64>,
) -> Result<StatusCode, (StatusCode, String)> {
    if db.retry_mirror(id).await.map_err(internal)? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err((StatusCode::CONFLICT, "transaction is not in error".into()))
    }
}

async fn mirror_paamonim(
    State(db): State<Db>,
) -> Result<Json<mirror::MirrorSummary>, (StatusCode, String)> {
    if !mirroring_enabled() {
        return Err((
            StatusCode::PRECONDITION_FAILED,
            "PAAMONIM_MIRROR is not enabled".into(),
        ));
    }
    run_mirror(db.as_ref()).await.map(Json).map_err(internal)
}

async fn export_sheets(
    State(db): State<Db>,
) -> Result<Json<sheets::ExportSummary>, (StatusCode, String)> {
    settle(&db).await?;
    sheets::export(db.as_ref())
        .await
        .map(Json)
        .map_err(internal)
}

/// `budget-backend probe <out_dir>`: log in and save raw Paamonim responses so the
/// importer's parsing can be written against real data. Output contains personal
/// financial data: keep it out of git (`probe-out/` is ignored).
async fn probe(out_dir: &str) -> anyhow::Result<()> {
    let base =
        std::env::var("PAAMONIM_BASE_URL").unwrap_or_else(|_| "https://app.paamonim.org.il".into());
    let user = std::env::var("PAAMONIM_USERNAME")?;
    let pass = std::env::var("PAAMONIM_PASSWORD")?;
    let client = paamonim::Client::login(&base, &user, &pass).await?;
    std::fs::create_dir_all(out_dir)?;
    let month = chrono::Local::now().format("%Y-%m-01").to_string();
    type Target<'a> = (&'a str, &'a str, Vec<(&'a str, &'a str)>);
    let targets: Vec<Target> = vec![
        ("landing", "/", vec![]),
        (
            "data_collection",
            "/rishumit/data_collection",
            vec![("month", &month)],
        ),
        (
            "month_expense",
            "/rishumit/month_details_by_type",
            vec![("data_type", "expense"), ("month", &month)],
        ),
        (
            "month_income",
            "/rishumit/month_details_by_type",
            vec![("data_type", "income"), ("month", &month)],
        ),
        ("income_items", "/rishumit/get_income_items", vec![]),
        ("expense_items", "/rishumit/get_expense_items", vec![]),
        ("payment_methods", "/rishumit/get_payment_methods", vec![]),
        (
            "activity_history",
            "/rishumit/activity_history",
            vec![("start_date", "2020-01-01"), ("end_date", "2030-01-01")],
        ),
    ];
    for (name, path, query) in targets {
        let (status, body) = client.get_raw(path, &query).await?;
        std::fs::write(
            format!("{out_dir}/{name}.txt"),
            format!("HTTP {status}\n{body}"),
        )?;
        println!("{name}: HTTP {status}, {} bytes", body.len());
    }
    Ok(())
}

/// All HTTP routes. A separate function so a test can build it: axum panics on
/// overlapping routes when the router is constructed, which would otherwise only
/// show up when the server starts.
fn router(db: Db) -> Router {
    Router::new()
        .route(
            "/api/transactions",
            get(list_transactions).post(create_transaction),
        )
        .route(
            "/api/transactions/:id",
            delete(delete_transaction).put(update_transaction),
        )
        .route(
            "/api/categories",
            get(list_categories).post(create_category),
        )
        .route("/api/categories/:id", delete(delete_category))
        .route("/api/recurring", get(list_recurring).post(create_recurring))
        .route("/api/recurring/occurrences", get(recurring_occurrences))
        .route(
            "/api/recurring/:series_id/months/:month",
            axum::routing::post(settle_recurring_month).delete(skip_recurring_month),
        )
        .route(
            "/api/recurring/:series_id",
            axum::routing::put(change_recurring).delete(cancel_recurring),
        )
        .route("/api/monthly-totals", get(monthly_totals))
        .route("/api/log", get(change_log))
        .route("/api/import/paamonim", axum::routing::post(import_paamonim))
        .route("/api/export/sheets", axum::routing::post(export_sheets))
        .route("/api/mirror/paamonim", axum::routing::post(mirror_paamonim))
        .route("/api/mirror/status", get(mirror_status))
        .route(
            "/api/mirror/paamonim/retry/:id",
            axum::routing::post(retry_mirror),
        )
        .with_state(db)
        .fallback(assets::static_handler)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt::init();

    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("probe") {
        return probe(args.get(2).map(String::as_str).unwrap_or("probe-out")).await;
    }

    // STORAGE=sqlite (default, local/tests) | mysql (MySQL HeatWave or any MySQL 8)
    let db: Db = match std::env::var("STORAGE").as_deref().unwrap_or("sqlite") {
        "mysql" => Arc::new(mysql::MySqlStorage::connect().await?),
        "sqlite" => {
            let db_path = std::env::var("DATABASE_URL").unwrap_or_else(|_| "budget.db".into());
            Arc::new(SqliteStorage::open(&db_path)?)
        }
        other => anyhow::bail!("unknown STORAGE '{other}' (expected sqlite or mysql)"),
    };

    // Every change made through the app is recorded in the change log.
    let db: Db = Arc::new(oplog::Logged(db));

    // Writing to Paamonim starts from now: whatever was unsent or edited while it was off is
    // never replayed. Turning it off forgets the baseline so the next start is fresh again.
    if mirroring_enabled() {
        let n = db.baseline_mirror().await?;
        if n > 0 {
            tracing::info!("paamonim mirror: {n} earlier transactions set aside, not mirrored");
        }
        // Also sweeps up what no request triggered: settled repeating charges, category
        // merges, and the rest of a run that hit its operation limit.
        let mins: u64 = std::env::var("PAAMONIM_MIRROR_INTERVAL_MINUTES")
            .ok()
            .and_then(|m| m.parse().ok())
            .unwrap_or(10);
        let db = db.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(mins.max(1) * 60));
            loop {
                tick.tick().await;
                if let Err(e) = recurring::settle_due(db.as_ref(), today()).await {
                    tracing::warn!("settling repeating charges failed: {e}");
                }
                match run_mirror(db.as_ref()).await {
                    Ok(s) => {
                        if let Some(why) = &s.stopped {
                            tracing::warn!("paamonim mirror stopped: {why}");
                        }
                    }
                    Err(e) => tracing::warn!("paamonim mirror failed: {e}"),
                }
            }
        });
    } else {
        db.clear_mirror_baseline().await?;
    }

    // Nightly (configurable) snapshot to Google Sheets, only when configured.
    if std::env::var("GOOGLE_SHEET_ID").is_ok() {
        let hours: u64 = std::env::var("SHEETS_EXPORT_INTERVAL_HOURS")
            .ok()
            .and_then(|h| h.parse().ok())
            .unwrap_or(24);
        let db = db.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(hours * 3600));
            loop {
                tick.tick().await;
                if let Err(e) = recurring::settle_due(db.as_ref(), today()).await {
                    tracing::warn!("settling repeating charges failed: {e}");
                }
                if let Err(e) = sheets::export(db.as_ref()).await {
                    tracing::warn!("sheets export failed: {e}");
                }
            }
        });
    }

    // Keep the last year of the change log: prune now and then daily.
    {
        let db = db.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(24 * 3600));
            loop {
                tick.tick().await;
                match oplog::prune(db.as_ref()).await {
                    Ok(n) if n > 0 => tracing::info!("pruned {n} old change log entries"),
                    Ok(_) => {}
                    Err(e) => tracing::warn!("pruning the change log failed: {e}"),
                }
            }
        });
    }

    let app = router(db);

    let addr = std::env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:3000".into());
    tracing::info!("listening on http://{addr}");
    axum::serve(tokio::net::TcpListener::bind(&addr).await?, app).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn router_builds_without_overlapping_routes() {
        let db: Db = Arc::new(SqliteStorage::open(":memory:").unwrap());
        let _ = router(db);
    }
}
