use super::businesses::check_business;
use super::error::{internal, ApiError};
use super::Db;
use crate::push;
use crate::sumit::{self, Notice, State as SumitState};
use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use chrono::Utc;
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
pub(crate) struct Product {
    name: String,
    business_id: Option<i64>,
    category_id: Option<i64>,
}

#[derive(Serialize)]
pub(crate) struct HeldReceipt {
    id: i64,
    customer: String,
    total: f64,
    products: Vec<String>,
}

#[derive(Serialize)]
pub(crate) struct Overview {
    /// `SUMIT_API_KEY` and `SUMIT_COMPANY_ID` are set on the server.
    configured: bool,
    last_poll_at: Option<String>,
    last_error: Option<String>,
    next_poll_at: Option<String>,
    calls_this_month: u32,
    monthly_limit: u32,
    products: Vec<Product>,
    notices: Vec<Notice>,
    /// Receipts waiting for a product to be given an income sub-category: (number, customer, total).
    held: Vec<HeldReceipt>,
    /// VAPID public key for subscribing to web push; absent when push is not set up.
    push_public_key: Option<String>,
}

fn config() -> Result<Option<sumit::Config>, ApiError> {
    sumit::Config::from_env()
        .map_err(|e| ApiError::new(StatusCode::SERVICE_UNAVAILABLE, format!("{e:#}")))
}

pub(crate) async fn overview(State(db): State<Db>) -> Result<Json<Overview>, ApiError> {
    let cfg = sumit::Config::from_env().ok().flatten();
    let state: SumitState = sumit::load(db.as_ref(), sumit::STATE_KEY)
        .await
        .map_err(internal)?;
    let map = sumit::product_map(db.as_ref()).await.map_err(internal)?;
    let mut products: Vec<Product> = state
        .products
        .iter()
        .map(|n| {
            let r = map.get(&sumit::product_key(n)).copied().unwrap_or_default();
            Product {
                name: n.clone(),
                business_id: r.business_id,
                category_id: r.category_id,
            }
        })
        .collect();
    for (k, b) in &map {
        if !state.products.iter().any(|n| sumit::product_key(n) == *k) {
            products.push(Product {
                name: k.clone(),
                business_id: b.business_id,
                category_id: b.category_id,
            });
        }
    }
    products.sort_by(|a, b| a.name.cmp(&b.name));
    let state_held: Vec<HeldReceipt> = state
        .held
        .values()
        .map(|h| HeldReceipt {
            id: h.doc.id,
            customer: h.doc.customer.clone(),
            total: h.doc.total,
            products: h.need.iter().filter(|n| !n.is_empty()).cloned().collect(),
        })
        .collect();
    let now = Utc::now();
    let hours = cfg
        .as_ref()
        .map_or(vec![sumit::DEFAULT_HOUR], |c| c.hours.clone());
    let mut notices: Vec<Notice> = sumit::load(db.as_ref(), sumit::NOTICES_KEY)
        .await
        .map_err(internal)?;
    notices.reverse();
    Ok(Json(Overview {
        configured: cfg.is_some(),
        last_poll_at: state.last_poll_at,
        last_error: state.last_error,
        next_poll_at: cfg
            .as_ref()
            .map(|_| sumit::next_slot(now, &hours).to_rfc3339()),
        calls_this_month: sumit::calls_this_month(db.as_ref(), now)
            .await
            .map_err(internal)?,
        monthly_limit: cfg.as_ref().map_or(0, |c| c.monthly_limit),
        products,
        held: state_held,
        notices,
        push_public_key: push::public_key(),
    }))
}

#[derive(Deserialize)]
pub(crate) struct ProductChoice {
    name: String,
    business_id: Option<i64>,
    category_id: Option<i64>,
}

/// Say which business and income category a Sumit product belongs to (both null removes the mapping). Applies to
/// receipts imported from now on.
pub(crate) async fn set_product(
    State(db): State<Db>,
    Json(b): Json<ProductChoice>,
) -> Result<StatusCode, ApiError> {
    let name = b.name.trim();
    if name.is_empty() || name.len() > 200 {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "product name is required".into(),
        ));
    }
    check_business(&db, b.business_id).await?;
    let mut map = sumit::product_map(db.as_ref()).await.map_err(internal)?;
    if let Some(id) = b.category_id {
        let cats = db.list_categories().await.map_err(internal)?;
        if !sumit::is_usable_category(&cats, id) {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "choose an income category that has no sub-categories".into(),
            ));
        }
    }
    if b.business_id.is_none() && b.category_id.is_none() {
        map.remove(&sumit::product_key(name));
    } else {
        map.insert(
            sumit::product_key(name),
            sumit::ProductRule {
                business_id: b.business_id,
                category_id: b.category_id,
            },
        );
    }
    sumit::save(db.as_ref(), sumit::PRODUCTS_KEY, &map)
        .await
        .map_err(internal)?;
    let mut state: SumitState = sumit::load(db.as_ref(), sumit::STATE_KEY)
        .await
        .map_err(internal)?;
    if state.products.insert(name.to_string()) {
        sumit::save(db.as_ref(), sumit::STATE_KEY, &state)
            .await
            .map_err(internal)?;
    }
    let retried = sumit::retry_held(db.as_ref(), Utc::now()).await;
    // Held receipts that became importable are new transactions: send them to Paamonim now.
    super::trigger_mirror(&db);
    retried.map_err(internal)?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn mark_read(State(db): State<Db>) -> Result<StatusCode, ApiError> {
    let mut notices: Vec<Notice> = sumit::load(db.as_ref(), sumit::NOTICES_KEY)
        .await
        .map_err(internal)?;
    for n in &mut notices {
        n.read = true;
    }
    sumit::save(db.as_ref(), sumit::NOTICES_KEY, &notices)
        .await
        .map_err(internal)?;
    Ok(StatusCode::NO_CONTENT)
}

/// Poll now instead of waiting for the next slot. Spends API calls (counted against the monthly
/// limit, which refuses further calls once reached).
pub(crate) async fn poll_now(State(db): State<Db>) -> Result<Json<sumit::PollSummary>, ApiError> {
    let cfg = config()?
        .ok_or_else(|| ApiError::new(StatusCode::BAD_REQUEST, "Sumit is not configured".into()))?;
    let api = sumit::HttpApi::new(cfg.clone());
    let polled = sumit::poll_and_notify(db.as_ref(), &api, &cfg).await;
    // What was imported (even before an error stopped the poll) goes to Paamonim now.
    super::trigger_mirror(&db);
    polled
        .map(Json)
        .map_err(|e| ApiError::new(StatusCode::BAD_GATEWAY, format!("{e:#}")))
}

#[derive(Deserialize)]
pub(crate) struct Subscription {
    endpoint: String,
}

pub(crate) async fn subscribe(
    State(db): State<Db>,
    Json(b): Json<Subscription>,
) -> Result<StatusCode, ApiError> {
    push::subscribe(db.as_ref(), &b.endpoint)
        .await
        .map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, format!("{e:#}")))?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
pub(crate) struct Registered {
    registered: bool,
}

/// Does the server already hold this device's push endpoint? Asked before offering to enable notifications.
pub(crate) async fn push_check(
    State(db): State<Db>,
    Json(b): Json<Subscription>,
) -> Result<Json<Registered>, ApiError> {
    let registered = push::is_registered(db.as_ref(), &b.endpoint)
        .await
        .map_err(internal)?;
    Ok(Json(Registered { registered }))
}

/// The numbered list of recent pushes, for the service worker: it shows the ones it has not shown yet.
pub(crate) async fn push_events(State(db): State<Db>) -> Result<Json<Vec<push::Event>>, ApiError> {
    Ok(Json(push::events(db.as_ref()).await.map_err(internal)?))
}
