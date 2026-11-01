use super::error::{internal, ApiError};
use super::Db;
use crate::storage::{Category, CategoryError, NewCategory};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

pub(crate) async fn list_categories(State(db): State<Db>) -> Result<Json<Vec<Category>>, ApiError> {
    db.list_categories().await.map(Json).map_err(internal)
}

/// Refusals of a category change are the caller's doing; anything else is a server error.
pub(crate) fn category_error(e: anyhow::Error) -> ApiError {
    match e.downcast_ref::<CategoryError>() {
        Some(CategoryError::NotFound) => ApiError::new(StatusCode::NOT_FOUND, e.to_string()),
        Some(CategoryError::Invalid(_)) => ApiError::new(StatusCode::BAD_REQUEST, e.to_string()),
        Some(CategoryError::NeedsReplacement { .. }) => {
            ApiError::new(StatusCode::CONFLICT, e.to_string())
        }
        None => internal(e),
    }
}

pub(crate) async fn create_category(
    State(db): State<Db>,
    Json(c): Json<NewCategory>,
) -> Result<Json<i64>, ApiError> {
    db.create_category(&c)
        .await
        .map(Json)
        .map_err(category_error)
}

#[derive(Deserialize)]
pub(crate) struct DeleteCategoryQuery {
    /// Category that receives the transactions of the one being deleted.
    move_to: Option<i64>,
}

#[derive(Deserialize)]
pub(crate) struct LinkCategory {
    /// The Paamonim top-level category this one stands in for.
    linked_to: Option<i64>,
}

/// Link a top-level category made here to a Paamonim top-level category.
pub(crate) async fn link_category(
    State(db): State<Db>,
    Path(id): Path<i64>,
    Json(b): Json<LinkCategory>,
) -> Result<Json<()>, ApiError> {
    db.link_category_to(id, b.linked_to)
        .await
        .map(Json)
        .map_err(category_error)
}

#[derive(Deserialize)]
pub(crate) struct MoveCategory {
    /// The top-level category to put the sub-category under.
    parent_id: i64,
}

/// Move a sub-category, with its transactions and repeating rules, under another top-level
/// category.
pub(crate) async fn move_category(
    State(db): State<Db>,
    Path(id): Path<i64>,
    Json(b): Json<MoveCategory>,
) -> Result<Json<()>, ApiError> {
    db.move_category(id, b.parent_id)
        .await
        .map(Json)
        .map_err(category_error)
}

/// Also deletes the sub-categories. Answers 409 when transactions need a `move_to`.
pub(crate) async fn delete_category(
    State(db): State<Db>,
    Path(id): Path<i64>,
    Query(q): Query<DeleteCategoryQuery>,
) -> Result<Json<usize>, ApiError> {
    db.delete_category(id, q.move_to)
        .await
        .map(Json)
        .map_err(category_error)
}
