//! Small helpers for the few server-rendered responses left (the shell page, sign-in
//! messages and the CSV export).

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

pub fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

pub(crate) fn db_error(e: rusqlite::Error) -> Response {
    log::error!("db: {e}");
    (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
}
