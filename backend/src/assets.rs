//! The built frontend, embedded in the executable (debug builds too: `debug-embed`).

use axum::{
    http::{header, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "../frontend/dist"]
struct Assets;

fn file_response(path: &str) -> Option<Response> {
    let file = Assets::get(path)?;
    // Hashed build assets never change; everything else must be revalidated so
    // a new deployment (and the service worker) is picked up.
    let cache = if path.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    Some(
        (
            [
                (
                    header::CONTENT_TYPE,
                    mime_guess::from_path(path)
                        .first_or_octet_stream()
                        .to_string(),
                ),
                (header::CACHE_CONTROL, cache.to_string()),
            ],
            file.data.into_owned(),
        )
            .into_response(),
    )
}

/// Serves embedded files; unknown non-API paths get `index.html` (single-page app).
pub async fn static_handler(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    if path.starts_with("api/") {
        return StatusCode::NOT_FOUND.into_response();
    }
    let path = if path.is_empty() { "index.html" } else { path };
    // A missing build asset (e.g. a stale hash) is a real 404, not the app page:
    // otherwise the browser gets HTML where it expected JS/CSS.
    if path.starts_with("assets/") {
        return file_response(path).unwrap_or_else(|| StatusCode::NOT_FOUND.into_response());
    }
    file_response(path)
        .or_else(|| file_response("index.html"))
        .unwrap_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                "frontend not built: install Node.js and rebuild (see README)",
            )
                .into_response()
        })
}
