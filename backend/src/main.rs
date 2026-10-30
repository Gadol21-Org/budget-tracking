mod db;
mod money;
mod paamonim;
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
use tower_http::services::{ServeDir, ServeFile};

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
    db.create_transaction(&t).await.map(Json).map_err(internal)
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

/// Initial import. Currently stores/returns the raw parsed lines so the HTML
/// parsing can be tuned against a real account.
async fn import_paamonim(
    Json(req): Json<ImportRequest>,
) -> Result<Json<Vec<String>>, (StatusCode, String)> {
    let base = std::env::var("PAAMONIM_BASE_URL").unwrap_or_else(|_| "https://app.paamonim.org.il".into());
    let user = std::env::var("PAAMONIM_USERNAME").map_err(|_| (StatusCode::PRECONDITION_FAILED, "PAAMONIM_USERNAME not set".into()))?;
    let pass = std::env::var("PAAMONIM_PASSWORD").map_err(|_| (StatusCode::PRECONDITION_FAILED, "PAAMONIM_PASSWORD not set".into()))?;
    let client = paamonim::Client::login(&base, &user, &pass).await.map_err(internal)?;
    let html = client.activity_history_html(&req.from, &req.to).await.map_err(internal)?;
    let lines = paamonim::Client::parse_history(&html);
    Ok(Json(lines.into_iter().map(|l| format!("{}: {}", l.line_id, l.text)).collect()))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt::init();

    // STORAGE=sqlite (default, local/tests) | nosql (Oracle NoSQL Cloud or the local Cloud Simulator)
    let db: Db = match std::env::var("STORAGE").as_deref().unwrap_or("sqlite") {
        "nosql" => Arc::new(nosql::NoSqlStorage::connect().await?),
        "sqlite" => {
            let db_path = std::env::var("DATABASE_URL").unwrap_or_else(|_| "budget.db".into());
            Arc::new(SqliteStorage::open(&db_path)?)
        }
        other => anyhow::bail!("unknown STORAGE '{other}' (expected sqlite or nosql)"),
    };

    let static_dir = std::env::var("STATIC_DIR").unwrap_or_else(|_| "frontend/dist".into());
    let spa = ServeDir::new(&static_dir).fallback(ServeFile::new(format!("{static_dir}/index.html")));

    let app = Router::new()
        .route("/api/transactions", get(list_transactions).post(create_transaction))
        .route("/api/transactions/:id", delete(delete_transaction))
        .route("/api/categories", get(list_categories))
        .route("/api/monthly-totals", get(monthly_totals))
        .route("/api/import/paamonim", axum::routing::post(import_paamonim))
        .with_state(db)
        .fallback_service(spa);

    let addr = std::env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:3000".into());
    tracing::info!("listening on {addr}");
    axum::serve(tokio::net::TcpListener::bind(&addr).await?, app).await?;
    Ok(())
}
