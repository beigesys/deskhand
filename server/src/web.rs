//! The web app (web/dist, built with bun) embedded in the binary.

use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "../web/dist"]
struct Dist;

/// `GET` anything else: a file from the build, or `index.html` for the app's own routes.
pub async fn spa(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    if let Some(f) = Dist::get(path).filter(|_| !path.is_empty()) {
        // Vite puts content hashes in everything under assets/.
        let cache = if path.starts_with("assets/") { "public, max-age=31536000, immutable" } else { "no-cache" };
        return (
            [
                (header::CONTENT_TYPE, f.metadata.mimetype().to_owned()),
                (header::CACHE_CONTROL, cache.to_owned()),
            ],
            f.data,
        )
            .into_response();
    }
    // A missing file (it has an extension) is a 404, not the app.
    let last = path.rsplit('/').next().unwrap_or("");
    if last.contains('.') {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }
    match Dist::get("index.html") {
        Some(f) => (
            [
                (header::CONTENT_TYPE, "text/html; charset=utf-8"),
                (header::CACHE_CONTROL, "no-cache"),
            ],
            f.data,
        )
            .into_response(),
        None => (StatusCode::NOT_FOUND, "the web app wasn't built (cd web && bun run build)").into_response(),
    }
}
