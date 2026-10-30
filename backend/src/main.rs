mod assets;
mod db;
mod import;
mod mirror;
mod money;
mod paamonim;
mod sheets;
mod nosql;
mod storage;

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get},
    Json, Router,
};
use serde::Deserialize;
use storage::{Category, MonthlyTotal, NewTransaction, SqliteStorage, Storage, Transaction};

type Db = Arc<dyn Storage>;

fn internal<E: std::fmt::Display>(e: E) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

async fn list_transactions(State(db): State<Db>) -> Result<Json<Vec<Transaction>>, (StatusCode, String)> {
    db.list_transactions().await.map(Json).map_err(internal)
}

async fn create_transaction(
    State(db): State<Db>,
    Json(t): Json<NewTransaction>,
) -> Result<Json<i64>, (StatusCode, String)> {
    chrono::NaiveDate::parse_from_str(&t.date, "%Y-%m-%d")
        .map_err(|_| (StatusCode::BAD_REQUEST, "date must be YYYY-MM-DD".into()))?;
    let id = db.create_transaction(&t).await.map_err(internal)?;
    if mirroring_enabled() {
        // Best effort in the background; failed lines stay visible via sync_state.
        let db = db.clone();
        tokio::spawn(async move {
            if let Err(e) = run_mirror(db.as_ref()).await {
                tracing::warn!("paamonim mirror failed: {e}");
            }
        });
    }
    Ok(Json(id))
}

async fn delete_transaction(State(db): State<Db>, Path(id): Path<i64>) -> Result<StatusCode, (StatusCode, String)> {
    db.delete_transaction(id).await.map_err(internal)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_categories(State(db): State<Db>) -> Result<Json<Vec<Category>>, (StatusCode, String)> {
    db.list_categories().await.map(Json).map_err(internal)
}

async fn monthly_totals(State(db): State<Db>) -> Result<Json<Vec<MonthlyTotal>>, (StatusCode, String)> {
    db.monthly_totals().await.map(Json).map_err(internal)
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
    let base = std::env::var("PAAMONIM_BASE_URL").unwrap_or_else(|_| "https://app.paamonim.org.il".into());
    let user = std::env::var("PAAMONIM_USERNAME").map_err(|_| (StatusCode::PRECONDITION_FAILED, "PAAMONIM_USERNAME not set".into()))?;
    let pass = std::env::var("PAAMONIM_PASSWORD").map_err(|_| (StatusCode::PRECONDITION_FAILED, "PAAMONIM_PASSWORD not set".into()))?;
    let client = paamonim::Client::login(&base, &user, &pass).await.map_err(internal)?;
    import::run(&client, db.as_ref(), &req.from, &req.to).await.map(Json).map_err(|e| {
        tracing::warn!("paamonim import failed: {e:#}");
        internal(format!("{e:#}"))
    })
}

fn paamonim_credentials() -> Result<(String, String, String), (StatusCode, String)> {
    let base = std::env::var("PAAMONIM_BASE_URL").unwrap_or_else(|_| "https://app.paamonim.org.il".into());
    let user = std::env::var("PAAMONIM_USERNAME").map_err(|_| (StatusCode::PRECONDITION_FAILED, "PAAMONIM_USERNAME not set".into()))?;
    let pass = std::env::var("PAAMONIM_PASSWORD").map_err(|_| (StatusCode::PRECONDITION_FAILED, "PAAMONIM_PASSWORD not set".into()))?;
    Ok((base, user, pass))
}

/// Writing to Paamonim is opt-in: set PAAMONIM_MIRROR=1.
fn mirroring_enabled() -> bool {
    matches!(std::env::var("PAAMONIM_MIRROR").as_deref(), Ok("1") | Ok("true"))
}

/// Serialises mirror runs so overlapping triggers can't post the same line twice.
static MIRROR_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn run_mirror(db: &dyn Storage) -> anyhow::Result<mirror::MirrorSummary> {
    let _guard = MIRROR_LOCK.lock().await;
    let (base, user, pass) = paamonim_credentials().map_err(|e| anyhow::anyhow!(e.1))?;
    let client = paamonim::Client::login(&base, &user, &pass).await?;
    mirror::run(&client, db).await
}

async fn mirror_paamonim(State(db): State<Db>) -> Result<Json<mirror::MirrorSummary>, (StatusCode, String)> {
    if !mirroring_enabled() {
        return Err((StatusCode::PRECONDITION_FAILED, "PAAMONIM_MIRROR is not enabled".into()));
    }
    run_mirror(db.as_ref()).await.map(Json).map_err(internal)
}

async fn export_sheets(State(db): State<Db>) -> Result<Json<sheets::ExportSummary>, (StatusCode, String)> {
    sheets::export(db.as_ref()).await.map(Json).map_err(internal)
}

/// `budget-backend probe <out_dir>`: log in and save raw Paamonim responses so the
/// importer's parsing can be written against real data. Output contains personal
/// financial data: keep it out of git (`probe-out/` is ignored).
async fn probe(out_dir: &str) -> anyhow::Result<()> {
    let base = std::env::var("PAAMONIM_BASE_URL").unwrap_or_else(|_| "https://app.paamonim.org.il".into());
    let user = std::env::var("PAAMONIM_USERNAME")?;
    let pass = std::env::var("PAAMONIM_PASSWORD")?;
    let client = paamonim::Client::login(&base, &user, &pass).await?;
    std::fs::create_dir_all(out_dir)?;
    let month = chrono::Local::now().format("%Y-%m-01").to_string();
    let targets: Vec<(&str, &str, Vec<(&str, &str)>)> = vec![
        ("landing", "/", vec![]),
        ("data_collection", "/rishumit/data_collection", vec![("month", &month)]),
        ("month_expense", "/rishumit/month_details_by_type", vec![("data_type", "expense"), ("month", &month)]),
        ("month_income", "/rishumit/month_details_by_type", vec![("data_type", "income"), ("month", &month)]),
        ("income_items", "/rishumit/get_income_items", vec![]),
        ("expense_items", "/rishumit/get_expense_items", vec![]),
        ("payment_methods", "/rishumit/get_payment_methods", vec![]),
        ("activity_history", "/rishumit/activity_history", vec![("start_date", "2020-01-01"), ("end_date", "2030-01-01")]),
    ];
    for (name, path, query) in targets {
        let (status, body) = client.get_raw(path, &query).await?;
        std::fs::write(format!("{out_dir}/{name}.txt"), format!("HTTP {status}\n{body}"))?;
        println!("{name}: HTTP {status}, {} bytes", body.len());
    }
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt::init();

    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("probe") {
        return probe(args.get(2).map(String::as_str).unwrap_or("probe-out")).await;
    }

    // STORAGE=sqlite (default, local/tests) | nosql (Oracle NoSQL Cloud or the local Cloud Simulator)
    let db: Db = match std::env::var("STORAGE").as_deref().unwrap_or("sqlite") {
        "nosql" => Arc::new(nosql::NoSqlStorage::connect().await?),
        "sqlite" => {
            let db_path = std::env::var("DATABASE_URL").unwrap_or_else(|_| "budget.db".into());
            Arc::new(SqliteStorage::open(&db_path)?)
        }
        other => anyhow::bail!("unknown STORAGE '{other}' (expected sqlite or nosql)"),
    };

    // Nightly (configurable) snapshot to Google Sheets, only when configured.
    if std::env::var("GOOGLE_SHEET_ID").is_ok() {
        let hours: u64 = std::env::var("SHEETS_EXPORT_INTERVAL_HOURS").ok().and_then(|h| h.parse().ok()).unwrap_or(24);
        let db = db.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(hours * 3600));
            loop {
                tick.tick().await;
                if let Err(e) = sheets::export(db.as_ref()).await {
                    tracing::warn!("sheets export failed: {e}");
                }
            }
        });
    }

    let app = Router::new()
        .route("/api/transactions", get(list_transactions).post(create_transaction))
        .route("/api/transactions/:id", delete(delete_transaction))
        .route("/api/categories", get(list_categories))
        .route("/api/monthly-totals", get(monthly_totals))
        .route("/api/import/paamonim", axum::routing::post(import_paamonim))
        .route("/api/export/sheets", axum::routing::post(export_sheets))
        .route("/api/mirror/paamonim", axum::routing::post(mirror_paamonim))
        .with_state(db)
        .fallback(assets::static_handler);

    let addr = std::env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:3000".into());
    tracing::info!("listening on http://{addr}");
    axum::serve(tokio::net::TcpListener::bind(&addr).await?, app).await?;
    Ok(())
}
