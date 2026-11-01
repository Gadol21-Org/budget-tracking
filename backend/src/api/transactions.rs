use super::error::{internal, ApiError};
use super::mirroring::trigger_mirror;
use super::{settle, today, Db};
use crate::fx;
use crate::storage;
use crate::storage::{NewTransaction, Transaction};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

pub(crate) async fn list_transactions(
    State(db): State<Db>,
) -> Result<Json<Vec<Transaction>>, ApiError> {
    settle(&db).await?;
    db.list_transactions().await.map(Json).map_err(internal)
}

/// A transaction as sent by the app. Shekel entries carry `amount_agorot`. A foreign one
/// (`currency` USD or JPY) carries `orig_minor` (signed, in cents or whole yen): without
/// `amount_agorot` the shekel amount comes from the exchange rate for the transaction's day;
/// with it, the shekel amount is the one given (what the card really charged) and the rate is
/// worked out from it.
#[derive(Deserialize)]
pub(crate) struct TxBody {
    date: String,
    amount_agorot: Option<i64>,
    category_id: Option<i64>,
    payment_method_id: Option<i64>,
    #[serde(default)]
    note: String,
    currency: Option<String>,
    orig_minor: Option<i64>,
}

/// The stored transaction and, for a foreign entry, the details to keep with it.
pub(crate) async fn resolve_body(
    db: &Db,
    id: Option<i64>,
    b: TxBody,
) -> Result<(NewTransaction, Option<storage::TxFx>), ApiError> {
    let bad = |m: &str| ApiError::new(StatusCode::BAD_REQUEST, m.to_string());
    chrono::NaiveDate::parse_from_str(&b.date, "%Y-%m-%d")
        .map_err(|_| bad("date must be YYYY-MM-DD"))?;
    let cur = b.currency.filter(|c| c != "ILS");
    let (amount, fx) = match cur {
        None => (
            b.amount_agorot.ok_or_else(|| bad("amount is required"))?,
            None,
        ),
        Some(cur) => {
            fx::check_currency(&cur).map_err(|e| bad(&e.to_string()))?;
            let orig = b.orig_minor.ok_or_else(|| bad("amount is required"))?;
            if orig == 0 {
                return Err(bad("amount must not be zero"));
            }
            let today = today().format("%Y-%m-%d").to_string();
            let existing = match id {
                Some(id) => db
                    .list_tx_fx()
                    .await
                    .map_err(internal)?
                    .into_iter()
                    .find(|f| f.transaction_id == id),
                None => None,
            };
            match (b.amount_agorot, existing) {
                // Saved again unchanged (a note edit): keep the rate and its state.
                (Some(a), Some(f))
                    if f.currency == cur
                        && f.orig_minor == orig
                        && fx::to_agorot(&cur, orig, f.rate) == a =>
                {
                    (a, Some(f))
                }
                // The shekel amount was typed in: it is final, the rate follows from it.
                (Some(a), _) => {
                    if a == 0 || (a < 0) != (orig < 0) {
                        return Err(bad("shekel amount must match the sign of the amount"));
                    }
                    let r = fx::Rate {
                        rate: fx::implied_rate(&cur, orig, a),
                        date: today,
                        estimated: false,
                    };
                    (a, Some(fx::build_fx(id.unwrap_or(0), &cur, orig, &r)))
                }
                (None, _) => {
                    let r = fx::rate_for(db.as_ref(), &cur, &b.date, &today)
                        .await
                        .map_err(|e| {
                            ApiError::new(StatusCode::SERVICE_UNAVAILABLE, e.to_string())
                        })?;
                    let a = fx::to_agorot(&cur, orig, r.rate);
                    if a == 0 {
                        return Err(bad("amount is too small"));
                    }
                    (a, Some(fx::build_fx(id.unwrap_or(0), &cur, orig, &r)))
                }
            }
        }
    };
    if amount == 0 {
        return Err(bad("amount must not be zero"));
    }
    Ok((
        NewTransaction {
            date: b.date,
            amount_agorot: amount,
            category_id: b.category_id,
            payment_method_id: b.payment_method_id,
            note: b.note,
        },
        fx,
    ))
}

pub(crate) async fn create_transaction(
    State(db): State<Db>,
    Json(b): Json<TxBody>,
) -> Result<Json<i64>, ApiError> {
    let (t, fx) = resolve_body(&db, None, b).await?;
    let id = db.create_transaction(&t).await.map_err(internal)?;
    if let Some(mut f) = fx {
        f.transaction_id = id;
        db.set_tx_fx(id, Some(&f)).await.map_err(internal)?;
    }
    trigger_mirror(&db);
    Ok(Json(id))
}

pub(crate) async fn update_transaction(
    State(db): State<Db>,
    Path(id): Path<i64>,
    Json(b): Json<TxBody>,
) -> Result<StatusCode, ApiError> {
    let (t, fx) = resolve_body(&db, Some(id), b).await?;
    db.update_transaction(id, &t).await.map_err(internal)?;
    let fx = fx.map(|mut f| {
        f.transaction_id = id;
        f
    });
    db.set_tx_fx(id, fx.as_ref()).await.map_err(internal)?;
    trigger_mirror(&db);
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub(crate) struct RateQuery {
    currency: String,
    date: String,
}

/// The rate that would be used for a currency on a day (the app previews it while typing).
pub(crate) async fn fx_rate(
    State(db): State<Db>,
    Query(q): Query<RateQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    chrono::NaiveDate::parse_from_str(&q.date, "%Y-%m-%d")
        .map_err(|_| ApiError::new(StatusCode::BAD_REQUEST, "date must be YYYY-MM-DD".into()))?;
    let today = today().format("%Y-%m-%d").to_string();
    let r = fx::rate_for(db.as_ref(), &q.currency, &q.date, &today)
        .await
        .map_err(|e| ApiError::new(StatusCode::SERVICE_UNAVAILABLE, e.to_string()))?;
    Ok(Json(
        serde_json::json!({ "rate": r.rate, "date": r.date, "estimated": r.estimated }),
    ))
}

/// The foreign-currency details of transactions. Rates that were stand-ins and whose day
/// has closed are replaced first, so a charge that settled later shows its real shekel value.
pub(crate) async fn list_foreign(
    State(db): State<Db>,
) -> Result<Json<Vec<storage::TxFx>>, ApiError> {
    let today = today().format("%Y-%m-%d").to_string();
    if fx::refresh_estimates(db.as_ref(), &today, 20).await > 0 {
        trigger_mirror(&db);
    }
    db.list_tx_fx().await.map(Json).map_err(internal)
}

/// Look the rate up again for the transaction's day (e.g. after the charge settled).
pub(crate) async fn refresh_rate(
    State(db): State<Db>,
    Path(id): Path<i64>,
) -> Result<Json<storage::TxFx>, ApiError> {
    let fx_row = db
        .list_tx_fx()
        .await
        .map_err(internal)?
        .into_iter()
        .find(|f| f.transaction_id == id)
        .ok_or(ApiError::new(
            StatusCode::NOT_FOUND,
            "not a foreign-currency transaction".into(),
        ))?;
    let t = db
        .list_transactions()
        .await
        .map_err(internal)?
        .into_iter()
        .find(|t| t.id == id)
        .ok_or(ApiError::new(
            StatusCode::NOT_FOUND,
            "no such transaction".into(),
        ))?;
    let today = today().format("%Y-%m-%d").to_string();
    let new = fx::reprice(db.as_ref(), &t, &fx_row, &today)
        .await
        .map_err(|e| ApiError::new(StatusCode::SERVICE_UNAVAILABLE, e.to_string()))?;
    trigger_mirror(&db);
    Ok(Json(new))
}

pub(crate) async fn delete_transaction(
    State(db): State<Db>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    db.delete_transaction(id).await.map_err(internal)?;
    trigger_mirror(&db);
    Ok(StatusCode::NO_CONTENT)
}
