//! Signing in: argon2 passwords, cookie sessions in the database (7 days, sliding), roles,
//! first-run setup, a login rate limit, and the Origin check on state-changing requests.

use crate::db::{Role, User};
use crate::{now, random_hex, ui, App, Shared};
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{header, HeaderMap, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::Form;
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const COOKIE: &str = "ithub_session";
const SESSION_SECS: i64 = 7 * 24 * 3600;

pub fn hash_password(password: &str) -> String {
    let mut salt = [0u8; 16];
    getrandom::fill(&mut salt).expect("random bytes");
    let salt = SaltString::encode_b64(&salt).expect("salt");
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .expect("argon2")
        .to_string()
}

fn verify_password(password: &str, hash: &str) -> bool {
    PasswordHash::new(hash)
        .is_ok_and(|h| Argon2::default().verify_password(password.as_bytes(), &h).is_ok())
}

/// A fixed hash to check against for unknown user names, so they take as long as known ones.
fn dummy_hash() -> &'static str {
    static H: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    H.get_or_init(|| hash_password("ithub-dummy-password"))
}

fn cookie_value(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|c| c.trim().split_once('='))
        .find(|(k, _)| *k == COOKIE)
        .map(|(_, v)| v.to_owned())
}

/// The signed-in user, if any.
pub fn user(app: &App, headers: &HeaderMap) -> Option<User> {
    let token = cookie_value(headers)?;
    let now = now();
    app.db.session_user(&token, now, now + SESSION_SECS).ok().flatten()
}

/// The signed-in user if their role is at least `min`: otherwise the page to show instead
/// (sign-in for nobody, 403 for too low a role).
pub fn require(app: &App, headers: &HeaderMap, min: Role) -> Result<User, Response> {
    match user(app, headers) {
        Some(u) if u.role >= min => Ok(u),
        Some(_) => Err((StatusCode::FORBIDDEN, "your role doesn't allow this").into_response()),
        None => Err(Redirect::to("/login").into_response()),
    }
}

/// Whether the browser reached us over HTTPS: only knowable behind a TLS proxy we trust.
pub fn is_https(app: &App, headers: &HeaderMap) -> bool {
    app.trust_proxy
        && headers
            .get("x-forwarded-proto")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.eq_ignore_ascii_case("https"))
}

/// The base URL the browser used, for links agents will follow.
pub fn base_url(app: &App, headers: &HeaderMap) -> String {
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("localhost");
    let scheme = if is_https(app, headers) { "https" } else { "http" };
    format!("{scheme}://{host}")
}

fn client_ip(app: &App, headers: &HeaderMap, peer: SocketAddr) -> IpAddr {
    if app.trust_proxy {
        if let Some(ip) = headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(',').next())
            .and_then(|v| v.trim().parse().ok())
        {
            return ip;
        }
    }
    peer.ip()
}

fn session_cookie(app: &App, headers: &HeaderMap, token: &str) -> String {
    let secure = if is_https(app, headers) { "; Secure" } else { "" };
    format!("{COOKIE}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={SESSION_SECS}{secure}")
}

/// Five sign-in attempts per IP per minute.
#[derive(Default)]
pub struct RateLimit(Mutex<HashMap<IpAddr, Vec<Instant>>>);

impl RateLimit {
    fn allow(&self, ip: IpAddr) -> bool {
        let mut m = self.0.lock().unwrap();
        let now = Instant::now();
        m.retain(|_, v| {
            v.retain(|t| now.duration_since(*t) < Duration::from_secs(60));
            !v.is_empty()
        });
        let v = m.entry(ip).or_default();
        if v.len() >= 5 {
            return false;
        }
        v.push(now);
        true
    }
}

/// Turns away state-changing requests and WebSocket upgrades that a browser sent from
/// another site's page. Clients without an Origin (agents, curl) pass; SameSite cookies
/// already keep a cross-site page from riding a session.
pub async fn check_origin(req: Request, next: Next) -> Response {
    let changes = !matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS)
        || req.headers().contains_key(header::UPGRADE);
    if changes {
        if let Some(origin) = req.headers().get(header::ORIGIN).and_then(|v| v.to_str().ok()) {
            let host = req.headers().get(header::HOST).and_then(|v| v.to_str().ok()).unwrap_or("");
            let origin_host = origin.split_once("://").map(|(_, h)| h).unwrap_or("");
            if !origin_host.eq_ignore_ascii_case(host) {
                return (StatusCode::FORBIDDEN, "cross-site request refused").into_response();
            }
        }
    }
    next.run(req).await
}

// --- pages ---

pub async fn login_page(State(app): State<Shared>) -> Response {
    if app.setup_token.lock().unwrap().is_some() {
        return Redirect::to("/setup").into_response();
    }
    Html(include_str!("login.html")).into_response()
}

#[derive(serde::Deserialize)]
pub struct LoginForm {
    username: String,
    password: String,
}

pub async fn login(
    State(app): State<Shared>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Form(f): Form<LoginForm>,
) -> Response {
    let ip = client_ip(&app, &headers, peer);
    if !app.login_limit.allow(ip) {
        return Redirect::to("/login?slow").into_response();
    }
    let found = app.db.user_by_name(f.username.trim()).ok().flatten();
    let hash = found.as_ref().map(|(_, h)| h.as_str()).unwrap_or(dummy_hash());
    let ok = verify_password(&f.password, hash);
    let Some((user, _)) = found.filter(|_| ok) else {
        app.db.audit(now(), None, None, "login.failed", &format!("{} from {ip}", f.username.trim()));
        return Redirect::to("/login?bad").into_response();
    };
    start_session(&app, &headers, &user, &ip.to_string())
}

fn start_session(app: &App, headers: &HeaderMap, user: &User, from: &str) -> Response {
    let token = random_hex(32);
    let now = now();
    if let Err(e) = app.db.add_session(&token, user.id, now + SESSION_SECS) {
        log::error!("session: {e}");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    let _ = app.db.purge_sessions(now);
    app.db.audit(now, Some(user.id), None, "login", &format!("from {from}"));
    (
        [(header::SET_COOKIE, session_cookie(app, headers, &token))],
        Redirect::to("/"),
    )
        .into_response()
}

pub async fn logout(State(app): State<Shared>, headers: HeaderMap) -> Response {
    if let Some(token) = cookie_value(&headers) {
        if let Some(u) = user(&app, &headers) {
            app.db.audit(now(), Some(u.id), None, "logout", "");
        }
        let _ = app.db.delete_session(&token);
    }
    (
        [(header::SET_COOKIE, format!("{COOKIE}=; Path=/; Max-Age=0"))],
        Redirect::to("/login"),
    )
        .into_response()
}

// --- first-run setup ---

#[derive(serde::Deserialize)]
pub struct SetupQuery {
    token: Option<String>,
}

pub async fn setup_page(
    State(app): State<Shared>,
    axum::extract::Query(q): axum::extract::Query<SetupQuery>,
) -> Response {
    if app.setup_token.lock().unwrap().is_none() {
        return Redirect::to("/login").into_response();
    }
    let token = ui::esc(q.token.as_deref().unwrap_or(""));
    Html(include_str!("setup.html").replace("{{token}}", &token)).into_response()
}

#[derive(serde::Deserialize)]
pub struct SetupForm {
    token: String,
    username: String,
    password: String,
}

pub async fn setup(
    State(app): State<Shared>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Form(f): Form<SetupForm>,
) -> Response {
    let ip = client_ip(&app, &headers, peer);
    if !app.login_limit.allow(ip) {
        return (StatusCode::TOO_MANY_REQUESTS, "too many attempts, wait a minute").into_response();
    }
    {
        let mut setup = app.setup_token.lock().unwrap();
        let Some(expected) = setup.as_deref() else {
            return Redirect::to("/login").into_response();
        };
        if !crate::same(f.token.trim(), expected) {
            return (StatusCode::FORBIDDEN, "wrong setup token; use the link the server printed").into_response();
        }
        let username = f.username.trim();
        if username.is_empty() || f.password.len() < 8 {
            return (StatusCode::BAD_REQUEST, "pick a user name and a password of 8+ characters").into_response();
        }
        if app.db.add_user(username, &hash_password(&f.password), Role::Admin, now()).is_err() {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
        *setup = None;
    }
    let Some((user, _)) = app.db.user_by_name(f.username.trim()).ok().flatten() else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    app.db.audit(now(), Some(user.id), None, "setup", "first admin created");
    start_session(&app, &headers, &user, &ip.to_string())
}
