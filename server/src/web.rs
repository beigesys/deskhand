//! The web app (web/dist, built with bun) embedded in the binary, and the path rewriting
//! that keeps the classic pages working under `/classic/`.

use axum::body::Body;
use axum::extract::Request;
use axum::http::{header, HeaderValue, StatusCode, Uri};
use axum::middleware::Next;
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

/// Paths that stay at the root when a classic page links to them.
fn stays(path: &str) -> bool {
    ["d/", "auth/", "classic", "api/", "/"].iter().any(|p| path.starts_with(p))
}

/// `/x` as a classic page's link: `/classic/x`, unless it stays at the root.
fn classic_link(url: &str) -> Option<String> {
    let rest = url.strip_prefix('/')?;
    if rest.is_empty() || rest.starts_with(['?', '#']) {
        return Some(format!("/classic{rest}"));
    }
    (!stays(rest)).then(|| format!("/classic/{rest}"))
}

/// Rewrites root-relative links in a classic page's HTML to point under `/classic/`.
fn rewrite_html(html: &str) -> String {
    const ATTRS: [&str; 4] = ["href=\"", "action=\"", "location.replace('", "href='"];
    let mut out = String::with_capacity(html.len() + 256);
    let mut i = 0;
    while i < html.len() {
        let next = ATTRS
            .iter()
            .filter_map(|a| html[i..].find(a).map(|p| (i + p + a.len(), a)))
            .min_by_key(|(p, _)| *p);
        let Some((at, _)) = next else { break };
        out.push_str(&html[i..at]);
        i = at;
        if let Some(rest) = html[at..].strip_prefix('/') {
            if rest.starts_with(['"', '\'', '?', '#']) {
                // The root itself: the classic devices page, `/classic` (no slash).
                out.push_str("/classic");
                i = at + 1;
            } else if !stays(rest) {
                out.push_str("/classic");
            }
        }
    }
    out.push_str(&html[i..]);
    out
}

/// Middleware for the classic pages: redirects and links that point at the root go under
/// `/classic/` instead.
pub async fn classic_paths(req: Request, next: Next) -> Response {
    let resp = next.run(req).await;
    let (mut parts, body) = resp.into_parts();
    if let Some(loc) = parts.headers.get(header::LOCATION).and_then(|v| v.to_str().ok()).and_then(classic_link) {
        if let Ok(v) = HeaderValue::from_str(&loc) {
            parts.headers.insert(header::LOCATION, v);
        }
    }
    let html = parts
        .headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("text/html"));
    if !html {
        return Response::from_parts(parts, body);
    }
    let Ok(bytes) = axum::body::to_bytes(body, 32 << 20).await else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let text = rewrite_html(&String::from_utf8_lossy(&bytes));
    parts.headers.remove(header::CONTENT_LENGTH);
    Response::from_parts(parts, Body::from(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classic_links() {
        assert_eq!(
            rewrite_html(r#"<a href="/jobs">x</a><form action="/users/1/role"><a href="/d/abc/">d</a><a href="/">h</a>"#),
            r#"<a href="/classic/jobs">x</a><form action="/classic/users/1/role"><a href="/d/abc/">d</a><a href="/classic">h</a>"#
        );
        assert_eq!(rewrite_html(r#"<a href="/auth/google/start">g</a>"#), r#"<a href="/auth/google/start">g</a>"#);
        assert_eq!(rewrite_html(r#"<a href="https://x.y/z">e</a>"#), r#"<a href="https://x.y/z">e</a>"#);
        assert_eq!(rewrite_html("location.replace('/devices/1?since=2')"), "location.replace('/classic/devices/1?since=2')");
        assert_eq!(classic_link("/login").as_deref(), Some("/classic/login"));
        assert_eq!(classic_link("/d/x/"), None);
        assert_eq!(classic_link("/").as_deref(), Some("/classic"));
        assert_eq!(classic_link("//evil"), None);
    }
}
