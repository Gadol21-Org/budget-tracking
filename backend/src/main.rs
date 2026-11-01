mod api;
mod assets;
mod db;
mod fx;
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

use api::{mirroring_enabled, router, run_mirror, today, Db};
use storage::SqliteStorage;

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
        let n = db.requeue_skipped_sub_categories().await?;
        if n > 0 {
            tracing::info!("paamonim mirror: {n} skipped transactions queued again");
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
