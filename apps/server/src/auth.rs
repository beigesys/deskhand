//! Signing in: argon2 passwords, cookie sessions in the database (7 days, sliding), roles,
//! first-run setup, a login rate limit, and the Origin check on state-changing requests.

use crate::db::{Role, User};
use crate::{now, random_hex, ui, App, Shared};
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use crate::sso::{self, Kind, Resolved, SsoError};
use axum::extract::{ConnectInfo, Path, Query, Request, State};
use axum::http::{header, HeaderMap, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{Html, IntoResponse, Redirect, Response};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const COOKIE: &str = "deskhand_session";
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

pub(crate) fn verify_password(password: &str, hash: &str) -> bool {
    PasswordHash::new(hash)
        .is_ok_and(|h| Argon2::default().verify_password(password.as_bytes(), &h).is_ok())
}

/// A fixed hash to check against for unknown user names, so they take as long as known ones.
pub(crate) fn dummy_hash() -> &'static str {
    static H: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    H.get_or_init(|| hash_password("deskhand-dummy-password"))
}

pub(crate) fn cookie_value(headers: &HeaderMap) -> Option<String> {
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
#[allow(clippy::result_large_err)] // a Response, built once per refused request
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

/// The base URL agents should use: DESKHAND_PUBLIC_URL when set, else the one the
/// browser used.
pub fn base_url(app: &App, headers: &HeaderMap) -> String {
    if let Some(u) = &app.public_url {
        return u.trim_end_matches('/').to_owned();
    }
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("localhost");
    let scheme = if is_https(app, headers) { "https" } else { "http" };
    format!("{scheme}://{host}")
}

pub fn client_ip(app: &App, headers: &HeaderMap, peer: SocketAddr) -> IpAddr {
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

/// At most `max` requests per IP per minute (five sign-in attempts by default).
pub struct RateLimit {
    hits: Mutex<HashMap<IpAddr, Vec<Instant>>>,
    max: usize,
}

impl Default for RateLimit {
    fn default() -> Self {
        RateLimit::with_max(5)
    }
}

impl RateLimit {
    pub fn with_max(max: usize) -> Self {
        RateLimit { hits: Mutex::default(), max }
    }

    pub(crate) fn allow(&self, ip: IpAddr) -> bool {
        let mut m = self.hits.lock().unwrap();
        let now = Instant::now();
        m.retain(|_, v| {
            v.retain(|t| now.duration_since(*t) < Duration::from_secs(60));
            !v.is_empty()
        });
        let v = m.entry(ip).or_default();
        if v.len() >= self.max {
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

// --- sign-in ---

/// Password sign-in is on unless DESKHAND_PASSWORD_LOGIN=0 and some SSO provider is set up,
/// so turning it off can never lock everyone out.
pub fn password_login(app: &App) -> bool {
    app.password_login || app.sso.providers.is_empty()
}

fn message(status: StatusCode, title: &str, body: &str) -> Response {
    (
        status,
        Html(
            include_str!("message.html")
                .replace("{{title}}", &ui::esc(title))
                .replace("{{body}}", body),
        ),
    )
        .into_response()
}

/// Starts a session for `user` and audits it: the Set-Cookie value that carries it.
pub(crate) fn new_session(app: &App, headers: &HeaderMap, user: &User, action: &str, detail: &str) -> Option<String> {
    let token = random_hex(32);
    let now = now();
    if let Err(e) = app.db.add_session(&token, user.id, now + SESSION_SECS) {
        log::error!("session: {e}");
        return None;
    }
    let _ = app.db.purge_sessions(now);
    app.db.audit(now, Some(user.id), None, action, detail);
    Some(session_cookie(app, headers, &token))
}

fn start_session(app: &App, headers: &HeaderMap, user: &User, action: &str, detail: &str) -> Response {
    match new_session(app, headers, user, action, detail) {
        Some(cookie) => ([(header::SET_COOKIE, cookie)], Redirect::to("/")).into_response(),
        None => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

// --- single sign-on ---

pub async fn sso_start(State(app): State<Shared>, Path(provider): Path<String>) -> Response {
    let Some(kind) = Kind::parse(&provider).filter(|k| app.sso.provider(*k).is_some()) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match app.sso.start(kind).await {
        Ok(url) => Redirect::to(&url).into_response(),
        Err(e) => {
            log::warn!("{} sign-in: {e}", kind.label());
            message(StatusCode::BAD_GATEWAY, "Sign-in unavailable", &format!("<p>{}</p>", ui::esc(&e.to_string())))
        }
    }
}

#[derive(serde::Deserialize)]
pub struct Callback {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

pub async fn sso_callback(
    State(app): State<Shared>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Path(provider): Path<String>,
    Query(q): Query<Callback>,
) -> Response {
    let Some(kind) = Kind::parse(&provider).filter(|k| app.sso.provider(*k).is_some()) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let ip = client_ip(&app, &headers, peer);
    let (Some(code), Some(state)) = (q.code, q.state) else {
        let why = q.error_description.or(q.error).unwrap_or_else(|| "no authorization code".into());
        return message(
            StatusCode::BAD_REQUEST,
            "Sign-in cancelled",
            &format!("<p>{} said: {}</p>", kind.label(), ui::esc(&why)),
        );
    };
    let id = match app.sso.finish(kind, &code, &state).await {
        Ok(id) => id,
        Err(e) => {
            log::warn!("{} sign-in from {ip}: {e}", kind.label());
            app.db.audit(now(), None, None, "sso.failed", &format!("{}: {e} (from {ip})", kind.key()));
            let status = if matches!(e, SsoError::Provider(_)) { StatusCode::BAD_GATEWAY } else { StatusCode::FORBIDDEN };
            return message(status, "Sign-in failed", &format!("<p>{}</p>", ui::esc(&e.to_string())));
        }
    };
    match sso::resolve(&app.db, &app.sso, &id, now()) {
        Ok(Some((user, how))) => {
            let what = format!("{} {}", kind.key(), id.display);
            match how {
                Resolved::Linked => app.db.audit(now(), Some(user.id), None, "sso.link", &what),
                Resolved::Provisioned => app.db.audit(now(), Some(user.id), None, "sso.provision", &format!("{what} as {}", user.role.as_str())),
                Resolved::Known => {}
            }
            start_session(&app, &headers, &user, "sso.login", &format!("{what} from {ip}"))
        }
        Ok(None) => {
            app.db.audit(now(), None, None, "sso.denied", &format!("{} {} has no user (from {ip})", kind.key(), id.display));
            let email = match &id.verified_email {
                Some(e) => format!("<p>Ask an admin to add a user with the email <b>{}</b>, then sign in again.</p>", ui::esc(e)),
                None => format!(
                    "<p>{} didn't share a verified email, so an admin can't link you by email. Ask an admin to add you.</p>",
                    kind.label()
                ),
            };
            message(
                StatusCode::FORBIDDEN,
                "No deskhand account yet",
                &format!("<p>You signed in to {} as <b>{}</b>, but no deskhand user matches.</p>{email}", kind.label(), ui::esc(&id.display)),
            )
        }
        Err(e) => {
            log::error!("sso resolve: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}
