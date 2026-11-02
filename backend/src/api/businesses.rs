use super::error::{internal, ApiError};
use super::Db;
use crate::storage::{Business, BusinessLinks};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

#[derive(serde::Serialize)]
pub(crate) struct BusinessData {
    businesses: Vec<Business>,
    links: BusinessLinks,
}

/// The businesses with their tags and category links.
pub(crate) async fn get_businesses(State(db): State<Db>) -> Result<Json<BusinessData>, ApiError> {
    Ok(Json(BusinessData {
        businesses: db.list_businesses().await.map_err(internal)?,
        links: db.business_links().await.map_err(internal)?,
    }))
}

#[derive(Deserialize)]
pub(crate) struct NewBusiness {
    name: String,
    #[serde(default)]
    is_general: bool,
}

pub(crate) async fn create_business(
    State(db): State<Db>,
    Json(b): Json<NewBusiness>,
) -> Result<Json<i64>, ApiError> {
    let name = b.name.trim();
    if name.is_empty() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "name must not be empty".into(),
        ));
    }
    let existing = db.list_businesses().await.map_err(internal)?;
    if existing.iter().any(|x| x.name == name) {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "a business with this name exists".into(),
        ));
    }
    if b.is_general && existing.iter().any(|x| x.is_general) {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "there is already a general business".into(),
        ));
    }
    db.insert_business(name, b.is_general)
        .await
        .map(Json)
        .map_err(internal)
}

#[derive(Deserialize)]
pub(crate) struct RenameBusiness {
    name: String,
}

pub(crate) async fn rename_business(
    State(db): State<Db>,
    Path(id): Path<i64>,
    Json(b): Json<RenameBusiness>,
) -> Result<StatusCode, ApiError> {
    let name = b.name.trim();
    if name.is_empty() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "name must not be empty".into(),
        ));
    }
    db.rename_business(id, name).await.map_err(internal)?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn delete_business(
    State(db): State<Db>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    db.remove_business(id).await.map_err(internal)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub(crate) struct BusinessChoice {
    business_id: Option<i64>,
}

/// Make a category's transactions count in a business (null removes the link).
pub(crate) async fn link_category_business(
    State(db): State<Db>,
    Path(id): Path<i64>,
    Json(b): Json<BusinessChoice>,
) -> Result<StatusCode, ApiError> {
    check_business(&db, b.business_id).await?;
    db.set_category_business(id, b.business_id)
        .await
        .map_err(internal)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub(crate) struct BusinessRequired {
    required: bool,
}

/// Require (or stop requiring) new transactions in a category to say which business they are in.
pub(crate) async fn require_category_business(
    State(db): State<Db>,
    Path(id): Path<i64>,
    Json(b): Json<BusinessRequired>,
) -> Result<StatusCode, ApiError> {
    db.set_category_business_required(id, b.required)
        .await
        .map_err(internal)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub(crate) struct TagTransactions {
    ids: Vec<i64>,
    business_id: Option<i64>,
}

/// Tag transactions as a business's (null removes their tags).
pub(crate) async fn tag_transactions(
    State(db): State<Db>,
    Json(b): Json<TagTransactions>,
) -> Result<StatusCode, ApiError> {
    check_business(&db, b.business_id).await?;
    db.set_transactions_business(&b.ids, b.business_id)
        .await
        .map_err(internal)?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn check_business(db: &Db, id: Option<i64>) -> Result<(), ApiError> {
    if let Some(id) = id {
        let all = db.list_businesses().await.map_err(internal)?;
        if !all.iter().any(|b| b.id == id) {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "unknown business".into(),
            ));
        }
    }
    Ok(())
}
