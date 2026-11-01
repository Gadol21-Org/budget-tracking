use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

/// A failed request: the status to answer with and the message the app shows.
#[derive(Debug)]
pub(crate) struct ApiError {
    pub(crate) status: StatusCode,
    pub(crate) message: String,
}

impl ApiError {
    pub(crate) fn new(status: StatusCode, message: String) -> Self {
        Self { status, message }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, self.message).into_response()
    }
}

/// Anything unexpected is a server error.
pub(crate) fn internal<E: std::fmt::Display>(e: E) -> ApiError {
    ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}
