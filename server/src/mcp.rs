//! MCP: `POST /mcp` speaks the Model Context Protocol (Streamable HTTP, no sessions) so
//! Claude, Claude Code, ChatGPT or an IDE can look after the fleet as the person who
//! connected it.
//!
//! Clients sign in through OAuth 2.1, served here, in the order they meet it:
//!
//! 1. Discovery: a request without a token gets 401 and a `WWW-Authenticate` naming
//!    `/.well-known/oauth-protected-resource/mcp`, which names this server as its own
//!    authorization server (`/.well-known/oauth-authorization-server`).
//! 2. Registration (RFC 7591): any client may register and gets an id. That grants
//!    nothing; it pins the redirect URIs a code may be sent to.
//! 3. `/oauth/authorize`: checks the request, then hands the browser to the web app's
//!    `/connect` page, where the person signs in the usual way (password or SSO), sees
//!    where the answer goes and what it may do, and allows or denies.
//! 4. `/oauth/token`: the client trades the code plus its PKCE verifier (S256, required)
//!    for an access token (a day) and a refresh token (30 days, rotated on use).
//!
//! People can also make API tokens by hand for clients that take a header. Either way
//! the token acts as its person with their role as it is *now*, and a token without the
//! `deskhand:act` scope is held to viewer. Tools are thin: each one calls `/api/v1`
//! in-process as that person, so roles, checks and the audit log are the API's own.

use crate::auth::{self, base_url};
use crate::db::{Role, User};
use crate::{now, random_hex, App, Shared};
use axum::body::Body;
use axum::extract::{ConnectInfo, Form, Path, Query, RawQuery, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, Request, StatusCode, Uri};
use axum::middleware::Next;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, Implementation, ProtocolVersion, ServerCapabilities, ServerConfig};
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{tool, tool_handler, tool_router, ErrorData as McpError, RoleServer, ServerHandler};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::net::SocketAddr;
use std::sync::OnceLock;
use std::time::Duration;
use tower::ServiceExt as _;

/// Read devices, inventory, jobs and scripts.
pub const SCOPE_READ: &str = "deskhand:read";
/// Act as the person's role allows: run commands and scripts, power actions, refreshes.
pub const SCOPE_ACT: &str = "deskhand:act";
const SCOPES: [&str; 2] = [SCOPE_READ, SCOPE_ACT];

/// The header an MCP tool calls `/api/v1` with: a key only this process knows, the
/// user id and the highest role the token allows.
const INTERNAL: &str = "x-deskhand-internal";

fn internal_key() -> &'static str {
    static K: OnceLock<String> = OnceLock::new();
    K.get_or_init(|| random_hex(32))
}

/// The user an in-process API call from an MCP tool acts as: the account as it is now,
/// its role capped by the token's scope.
pub fn internal_user(app: &App, headers: &HeaderMap) -> Option<User> {
    let v = headers.get(INTERNAL)?.to_str().ok()?;
    let (key, rest) = v.split_once(':')?;
    if !crate::same(key, internal_key()) {
        return None;
    }
    let (id, cap) = rest.split_once(':')?;
    let mut user = app.db.user_by_id(id.parse().ok()?).ok()??;
    user.role = user.role.min(Role::parse(cap)?);
    Some(user)
}

/// Who is behind an MCP request: put on it by [`require_token`].
#[derive(Clone)]
pub struct Caller {
    pub user: User,
    pub scope: String,
}

impl Caller {
    fn may_act(&self) -> bool {
        self.scope.split(' ').any(|s| s == SCOPE_ACT)
    }
    /// The highest role this token can use.
    fn cap(&self) -> Role {
        if self.may_act() {
            Role::Admin
        } else {
            Role::Viewer
        }
    }
}

/// Scopes as stored: the known ones asked for, in order, always with read.
fn normal_scope<'a>(asked: impl IntoIterator<Item = &'a str>) -> String {
    let asked: Vec<&str> = asked.into_iter().collect();
    SCOPES
        .into_iter()
        .filter(|s| *s == SCOPE_READ || asked.contains(s))
        .collect::<Vec<_>>()
        .join(" ")
}

// --- routes ---

/// `/mcp`, discovery and the OAuth endpoints. Outside the Origin check: nothing here
/// reads a cookie, so a cross-site page gains nothing, and browser-based clients need it.
pub fn routes(app: Shared) -> Router {
    let mut transport = StreamableHttpServerConfig::default()
        // No sessions: MCP 2026-07-28 has none, and a client on an older version that
        // still sends `initialize` is answered without being made to keep an id.
        .with_legacy_session_mode(false)
        .with_json_response(true);
    // rmcp checks Host against DNS rebinding, which defaults to loopback only. Every
    // request here needs a bearer token a rebinding page can't have, so the public
    // address is all it needs to know, and without one any host is let through.
    transport = match app.public_url.as_deref().and_then(|u| u.parse::<Uri>().ok()).and_then(|u| u.authority().cloned()) {
        Some(a) => transport.with_allowed_hosts([a.to_string(), a.host().to_owned(), "localhost".into(), "127.0.0.1".into(), "::1".into()]),
        None => transport.disable_allowed_hosts(),
    };
    let tools_app = app.clone();
    let service = StreamableHttpService::new(
        move || Ok(Deskhand::new(tools_app.clone())),
        LocalSessionManager::default().into(),
        transport,
    );
    let mcp = Router::new()
        .nest_service("/mcp", service)
        .layer(axum::middleware::from_fn_with_state(app.clone(), require_token));
    Router::new()
        .route("/.well-known/oauth-protected-resource", get(resource_metadata))
        .route("/.well-known/oauth-protected-resource/mcp", get(resource_metadata))
        .route("/.well-known/oauth-authorization-server", get(server_metadata))
        .route("/.well-known/oauth-authorization-server/mcp", get(server_metadata))
        .route("/oauth/register", post(register))
        .route("/oauth/authorize", get(authorize))
        .route("/oauth/token", post(token))
        .with_state(app)
        .merge(mcp)
        .layer(axum::middleware::from_fn(cors))
}

/// The web app's endpoints for the consent page, connections and API tokens: sessions,
/// under `/api/v1/mcp`.
pub fn api_routes() -> Router<Shared> {
    Router::new()
        .route("/mcp", get(info))
        .route("/mcp/authorize", get(consent_info).post(consent))
        .route("/mcp/connections", get(connections))
        .route("/mcp/connections/{family}", delete(revoke))
        .route("/mcp/tokens", post(new_api_token))
}

/// Any origin may read discovery and call `/mcp` and `/oauth/*`: they take bearer
/// tokens or nothing, never cookies.
async fn cors(req: Request<Body>, next: Next) -> Response {
    let preflight = req.method() == Method::OPTIONS;
    let mut res = if preflight { StatusCode::NO_CONTENT.into_response() } else { next.run(req).await };
    let h = res.headers_mut();
    h.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
    h.insert(
        header::ACCESS_CONTROL_EXPOSE_HEADERS,
        HeaderValue::from_static("www-authenticate, mcp-session-id, mcp-protocol-version"),
    );
    if preflight {
        h.insert(header::ACCESS_CONTROL_ALLOW_METHODS, HeaderValue::from_static("GET, POST, DELETE, OPTIONS"));
        h.insert(
            header::ACCESS_CONTROL_ALLOW_HEADERS,
            HeaderValue::from_static(
                "authorization, content-type, accept, x-api-key, mcp-protocol-version, mcp-session-id, mcp-method, mcp-name, last-event-id",
            ),
        );
        h.insert(header::ACCESS_CONTROL_MAX_AGE, HeaderValue::from_static("86400"));
    }
    res
}

// --- discovery ---

fn mcp_url(base: &str) -> String {
    format!("{base}/mcp")
}

async fn resource_metadata(State(app): State<Shared>, headers: HeaderMap) -> Json<Value> {
    let base = base_url(&app, &headers);
    Json(json!({
        "resource": mcp_url(&base),
        "resource_name": "Deskhand",
        "authorization_servers": [base],
        "scopes_supported": SCOPES,
        "bearer_methods_supported": ["header"],
    }))
}

async fn server_metadata(State(app): State<Shared>, headers: HeaderMap) -> Json<Value> {
    let base = base_url(&app, &headers);
    Json(json!({
        "issuer": base,
        "authorization_endpoint": format!("{base}/oauth/authorize"),
        "token_endpoint": format!("{base}/oauth/token"),
        "registration_endpoint": format!("{base}/oauth/register"),
        "scopes_supported": SCOPES,
        "response_types_supported": ["code"],
        "response_modes_supported": ["query"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "code_challenge_methods_supported": ["S256"],
        // Every client is public: PKCE binds the code to it, not a secret it would keep.
        "token_endpoint_auth_methods_supported": ["none"],
    }))
}

// --- redirect URIs ---

fn is_loopback(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "::1" | "[::1]")
}

/// Where a code may go: anywhere over HTTPS, or a program on the person's own machine
/// over plain HTTP. Plain HTTP elsewhere puts the code on the wire; a custom scheme is
/// a claim any app on the machine can make.
fn redirect_allowed(uri: &str) -> bool {
    let Ok(u) = uri.parse::<Uri>() else {
        return false;
    };
    match (u.scheme_str(), u.host()) {
        (Some("https"), Some(_)) => true,
        (Some("http"), Some(h)) => is_loopback(h),
        _ => false,
    }
}

/// Exactly a registered URI, except that a loopback one may change port: a program on
/// the person's machine binds whatever port is free (RFC 8252 §7.3).
fn redirect_matches(registered: &str, presented: &str) -> bool {
    if registered == presented {
        return true;
    }
    let (Ok(a), Ok(b)) = (registered.parse::<Uri>(), presented.parse::<Uri>()) else {
        return false;
    };
    a.scheme_str() == Some("http")
        && b.scheme_str() == Some("http")
        && a.host().is_some_and(is_loopback)
        && a.host() == b.host()
        && a.path() == b.path()
        && a.query() == b.query()
}

fn redirect_with(base: &str, pairs: &[(&str, &str)]) -> String {
    let mut url = base.to_owned();
    let mut sep = if base.contains('?') { '&' } else { '?' };
    for (k, v) in pairs {
        url.push(sep);
        url.push_str(k);
        url.push('=');
        url.extend(url::form_urlencoded::byte_serialize(v.as_bytes()));
        sep = '&';
    }
    url
}

// --- registration ---

#[derive(Deserialize)]
struct Registration {
    #[serde(default)]
    redirect_uris: Vec<String>,
    client_name: Option<String>,
}

fn oauth_error(status: StatusCode, error: &str, description: &str) -> Response {
    (
        status,
        [(header::CACHE_CONTROL, "no-store")],
        Json(json!({ "error": error, "error_description": description })),
    )
        .into_response()
}

fn register_limit() -> &'static auth::RateLimit {
    static L: OnceLock<auth::RateLimit> = OnceLock::new();
    L.get_or_init(|| auth::RateLimit::with_max(20))
}

/// `POST /oauth/register` (RFC 7591), open to anyone, a few a minute per address.
async fn register(
    State(app): State<Shared>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(r): Json<Registration>,
) -> Response {
    if !register_limit().allow(auth::client_ip(&app, &headers, peer)) {
        return oauth_error(StatusCode::TOO_MANY_REQUESTS, "slow_down", "too many registrations; wait a minute");
    }
    let uris = &r.redirect_uris;
    if uris.is_empty() || uris.len() > 16 || uris.iter().any(|u| u.len() > 1024 || !redirect_allowed(u)) {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_redirect_uri",
            "redirect URIs must be https, or http to localhost",
        );
    }
    let name: Option<String> =
        r.client_name.as_deref().map(str::trim).filter(|n| !n.is_empty()).map(|n| n.chars().take(120).collect());
    let id = format!("dhc_{}", random_hex(16));
    if let Err(e) = app.db.oauth_register(&id, name.as_deref(), uris, now()) {
        log::error!("oauth register: {e}");
        return oauth_error(StatusCode::INTERNAL_SERVER_ERROR, "server_error", "couldn't register");
    }
    (
        StatusCode::CREATED,
        Json(json!({
            "client_id": id,
            "client_name": name,
            "redirect_uris": uris,
            "token_endpoint_auth_method": "none",
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
        })),
    )
        .into_response()
}

// --- authorize ---

#[derive(Deserialize, Serialize, Clone)]
struct AuthorizeParams {
    #[serde(default)]
    response_type: String,
    #[serde(default)]
    client_id: String,
    #[serde(default)]
    redirect_uri: String,
    state: Option<String>,
    code_challenge: Option<String>,
    code_challenge_method: Option<String>,
    scope: Option<String>,
    /// RFC 8707: must be this server's `/mcp` if given.
    resource: Option<String>,
}

/// Why an authorization request can't go on: shown on a page (`Page`) while the redirect
/// URI isn't known to be the client's, else sent back to it (`Redirect`).
enum Refusal {
    Page(&'static str),
    Redirect(String),
}

fn check_request(app: &App, headers: &HeaderMap, p: &AuthorizeParams) -> Result<crate::db_oauth::Client, Refusal> {
    let client = match app.db.oauth_client(&p.client_id) {
        Ok(Some(c)) => c,
        _ => return Err(Refusal::Page("This client isn't registered here. Start connecting again from the client.")),
    };
    if !client.redirect_uris.iter().any(|r| redirect_matches(r, &p.redirect_uri)) {
        return Err(Refusal::Page("The client asked to send the answer somewhere it didn't register."));
    }
    let back = |error: &str| {
        let mut pairs = vec![("error", error)];
        if let Some(s) = p.state.as_deref() {
            pairs.push(("state", s));
        }
        Refusal::Redirect(redirect_with(&p.redirect_uri, &pairs))
    };
    if p.response_type != "code" {
        return Err(back("unsupported_response_type"));
    }
    if p.code_challenge.as_deref().is_none_or(str::is_empty) || p.code_challenge_method.as_deref() != Some("S256") {
        return Err(back("invalid_request"));
    }
    if let Some(r) = p.resource.as_deref() {
        let r = r.trim_end_matches('/');
        // The path is what matters: the host can differ between the address the client
        // was given and the one this request came in on (a proxy, a tailnet name).
        if !(r == mcp_url(&base_url(app, headers)) || r.parse::<Uri>().is_ok_and(|u| u.path() == "/mcp")) {
            return Err(back("invalid_target"));
        }
    }
    Ok(client)
}

fn refusal_page(msg: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Html(format!(
            "<!doctype html><meta charset=utf-8><meta name=viewport content='width=device-width'><title>Can't connect</title>\
             <body style='font:15px system-ui;max-width:32rem;margin:15vh auto;padding:0 16px'><h1 style='font-size:20px'>Can't connect</h1><p>{}</p>",
            crate::esc(msg)
        )),
    )
        .into_response()
}

/// `GET /oauth/authorize`: checks the request and passes it to the web app's consent page.
async fn authorize(
    State(app): State<Shared>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
    Query(p): Query<AuthorizeParams>,
) -> Response {
    match check_request(&app, &headers, &p) {
        Ok(_) => Redirect::to(&format!("/connect?{}", raw.unwrap_or_default())).into_response(),
        Err(Refusal::Page(m)) => refusal_page(m),
        Err(Refusal::Redirect(u)) => Redirect::to(&u).into_response(),
    }
}

#[derive(Serialize)]
struct ConsentInfo {
    client_name: Option<String>,
    /// Where the code goes: what the person should judge the request by.
    redirect_host: String,
    /// A program on this computer (loopback redirect).
    local: bool,
    /// The scopes that would be granted if allowed as asked.
    scopes: Vec<&'static str>,
    username: String,
    role: &'static str,
}

fn api_err(code: StatusCode, msg: &str) -> Response {
    (code, Json(json!({ "error": msg }))).into_response()
}

#[allow(clippy::result_large_err)]
fn need(app: &App, headers: &HeaderMap) -> Result<User, Response> {
    auth::user(app, headers).ok_or_else(|| api_err(StatusCode::UNAUTHORIZED, "sign in first"))
}

/// What a client asked for, as the scopes it would get.
fn asked_scopes(p: &AuthorizeParams) -> Vec<&'static str> {
    match p.scope.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        // A client that names none gets both; the person can still untick acting.
        None => SCOPES.to_vec(),
        Some(s) => SCOPES.into_iter().filter(|k| *k == SCOPE_READ || s.split(' ').any(|x| x == *k)).collect(),
    }
}

/// `GET /api/v1/mcp/authorize?...`: who is asking, for the consent page.
async fn consent_info(State(app): State<Shared>, headers: HeaderMap, Query(p): Query<AuthorizeParams>) -> Response {
    let user = match need(&app, &headers) {
        Ok(u) => u,
        Err(r) => return r,
    };
    let client = match check_request(&app, &headers, &p) {
        Ok(c) => c,
        Err(Refusal::Page(m)) => return api_err(StatusCode::BAD_REQUEST, m),
        Err(Refusal::Redirect(_)) => return api_err(StatusCode::BAD_REQUEST, "the client's request is malformed; start connecting again"),
    };
    let host = p.redirect_uri.parse::<Uri>().ok().and_then(|u| u.host().map(str::to_owned)).unwrap_or_default();
    Json(ConsentInfo {
        client_name: client.name,
        local: is_loopback(&host),
        redirect_host: host,
        scopes: asked_scopes(&p),
        username: user.username,
        role: user.role.as_str(),
    })
    .into_response()
}

#[derive(Deserialize)]
struct Decision {
    #[serde(flatten)]
    request: AuthorizeParams,
    allow: bool,
    /// What the person granted, at most what was asked.
    #[serde(default)]
    scopes: Vec<String>,
}

/// `POST /api/v1/mcp/authorize`: the person's answer, and where to send the browser.
async fn consent(State(app): State<Shared>, headers: HeaderMap, Json(d): Json<Decision>) -> Response {
    let user = match need(&app, &headers) {
        Ok(u) => u,
        Err(r) => return r,
    };
    let p = d.request;
    let client = match check_request(&app, &headers, &p) {
        Ok(c) => c,
        Err(Refusal::Page(m)) => return api_err(StatusCode::BAD_REQUEST, m),
        Err(Refusal::Redirect(u)) => return Json(json!({ "redirect": u })).into_response(),
    };
    let state: Vec<(&str, &str)> = p.state.as_deref().map(|s| ("state", s)).into_iter().collect();
    if !d.allow {
        app.db.audit(now(), Some(user.id), None, "mcp.deny", &client_label(&client));
        let pairs: Vec<(&str, &str)> = [("error", "access_denied")].into_iter().chain(state).collect();
        return Json(json!({ "redirect": redirect_with(&p.redirect_uri, &pairs) })).into_response();
    }
    let asked = asked_scopes(&p);
    let scope = normal_scope(d.scopes.iter().map(String::as_str).filter(|s| asked.contains(s)));
    let code = format!("dhk_{}", random_hex(32));
    let challenge = p.code_challenge.clone().unwrap_or_default();
    if let Err(e) = app.db.oauth_issue_code(&code, &client.id, user.id, &p.redirect_uri, &challenge, &scope, now()) {
        log::error!("oauth code: {e}");
        return api_err(StatusCode::INTERNAL_SERVER_ERROR, "couldn't issue a code");
    }
    app.db.audit(now(), Some(user.id), None, "mcp.connect", &format!("{} ({scope})", client_label(&client)));
    let pairs: Vec<(&str, &str)> = [("code", code.as_str())].into_iter().chain(state).collect();
    Json(json!({ "redirect": redirect_with(&p.redirect_uri, &pairs) })).into_response()
}

fn client_label(c: &crate::db_oauth::Client) -> String {
    let host = c.redirect_uris.first().and_then(|u| u.parse::<Uri>().ok()).and_then(|u| u.host().map(str::to_owned));
    match (&c.name, host) {
        (Some(n), Some(h)) => format!("{n} via {h}"),
        (Some(n), None) => n.clone(),
        (None, Some(h)) => h,
        (None, None) => c.id.clone(),
    }
}

// --- token ---

#[derive(Deserialize)]
struct TokenRequest {
    grant_type: String,
    client_id: Option<String>,
    code: Option<String>,
    redirect_uri: Option<String>,
    code_verifier: Option<String>,
    refresh_token: Option<String>,
}

fn pkce_matches(verifier: &str, challenge: &str) -> bool {
    crate::same(&URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())), challenge)
}

fn issue_pair(app: &App, family: &str, client_id: &str, user_id: i64, scope: &str) -> Response {
    let access = format!("dha_{}", random_hex(32));
    let refresh = format!("dhr_{}", random_hex(32));
    if let Err(e) = app.db.oauth_store_pair(&access, &refresh, family, client_id, user_id, scope, now()) {
        log::error!("oauth tokens: {e}");
        return oauth_error(StatusCode::INTERNAL_SERVER_ERROR, "server_error", "couldn't store tokens");
    }
    (
        [(header::CACHE_CONTROL, "no-store")],
        Json(json!({
            "access_token": access,
            "token_type": "Bearer",
            "expires_in": crate::db_oauth::ACCESS_SECS,
            "refresh_token": refresh,
            "scope": scope,
        })),
    )
        .into_response()
}

/// `POST /oauth/token`: a code and its PKCE verifier, or a refresh token, for tokens.
async fn token(State(app): State<Shared>, Form(b): Form<TokenRequest>) -> Response {
    let Some(client_id) = b.client_id.filter(|c| !c.is_empty()) else {
        return oauth_error(StatusCode::UNAUTHORIZED, "invalid_client", "client_id is required");
    };
    let invalid = |why: &str| oauth_error(StatusCode::BAD_REQUEST, "invalid_grant", why);
    match b.grant_type.as_str() {
        "authorization_code" => {
            let (Some(code), Some(verifier)) = (b.code, b.code_verifier) else {
                return oauth_error(StatusCode::BAD_REQUEST, "invalid_request", "code and code_verifier are required");
            };
            let Ok(Some(r)) = app.db.oauth_take_code(&code, now()) else {
                return invalid("unknown, expired or spent code");
            };
            if r.client_id != client_id || b.redirect_uri.as_deref() != Some(r.redirect_uri.as_str()) || !pkce_matches(&verifier, &r.challenge) {
                log::warn!("oauth: code for {} refused (client, redirect or PKCE mismatch)", r.client_id);
                return invalid("the code was issued to a different request");
            }
            if !matches!(app.db.user_by_id(r.user_id), Ok(Some(_))) {
                return invalid("the account is gone");
            }
            issue_pair(&app, &random_hex(16), &client_id, r.user_id, &r.scope)
        }
        "refresh_token" => {
            let Some(refresh) = b.refresh_token else {
                return oauth_error(StatusCode::BAD_REQUEST, "invalid_request", "refresh_token is required");
            };
            match app.db.oauth_take_refresh(&refresh, &client_id, now()) {
                Ok(Some((family, user_id, scope))) => issue_pair(&app, &family, &client_id, user_id, &scope),
                _ => invalid("unknown, expired or spent refresh token"),
            }
        }
        _ => oauth_error(StatusCode::BAD_REQUEST, "unsupported_grant_type", "authorization_code or refresh_token"),
    }
}

// --- bearer ---

fn presented_token(headers: &HeaderMap) -> Option<&str> {
    if let Some(t) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split_once(' '))
        .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("bearer"))
        .map(|(_, t)| t.trim())
    {
        return Some(t);
    }
    headers.get("x-api-key").and_then(|v| v.to_str().ok()).map(str::trim)
}

/// `/mcp` needs a live token; its holder goes on the request for the tools.
async fn require_token(State(app): State<Shared>, mut req: Request<Body>, next: Next) -> Response {
    let headers = req.headers();
    let presented = presented_token(headers);
    if let Some(h) = presented.and_then(|t| app.db.oauth_holder(t, now()).ok().flatten()) {
        req.extensions_mut().insert(Caller { user: h.user, scope: h.scope });
        return next.run(req).await;
    }
    log::info!("mcp: 401 for {} {} ({})", req.method(), req.uri().path(), if presented.is_some() { "bad token" } else { "no token" });
    let base = base_url(&app, headers);
    let mut challenge = format!(r#"Bearer resource_metadata="{base}/.well-known/oauth-protected-resource/mcp", scope="{}""#, SCOPES.join(" "));
    if presented.is_some() {
        challenge.push_str(r#", error="invalid_token""#);
    }
    let mut res = (StatusCode::UNAUTHORIZED, Json(json!({ "error": "sign in: this endpoint needs a Deskhand token" }))).into_response();
    if let Ok(v) = HeaderValue::from_str(&challenge) {
        res.headers_mut().insert(header::WWW_AUTHENTICATE, v);
    }
    res
}

// --- the web app's side ---

/// `GET /api/v1/mcp`: the address to give clients.
async fn info(State(app): State<Shared>, headers: HeaderMap) -> Response {
    if let Err(r) = need(&app, &headers) {
        return r;
    }
    Json(json!({ "url": mcp_url(&base_url(&app, &headers)), "scopes": SCOPES })).into_response()
}

#[derive(Deserialize)]
struct ConnectionsQuery {
    /// Admins: everyone's, not just your own.
    #[serde(default)]
    all: Option<String>,
}

/// `GET /api/v1/mcp/connections`: connected clients and API tokens, newest first.
async fn connections(State(app): State<Shared>, headers: HeaderMap, Query(q): Query<ConnectionsQuery>) -> Response {
    let user = match need(&app, &headers) {
        Ok(u) => u,
        Err(r) => return r,
    };
    let all = q.all.as_deref() == Some("1") && user.role == Role::Admin;
    match app.db.oauth_connections((!all).then_some(user.id), now()) {
        Ok(rows) => {
            let items: Vec<Value> = rows
                .into_iter()
                .map(|c| {
                    json!({
                        "id": c.family, "kind": c.kind, "name": c.name, "scopes": c.scope.split(' ').collect::<Vec<_>>(),
                        "created": c.created, "last_used": c.last_used, "expires": c.expires,
                        "user_id": c.user_id, "username": c.username,
                    })
                })
                .collect();
            Json(json!({ "items": items, "next_cursor": null })).into_response()
        }
        Err(e) => {
            log::error!("db: {e}");
            api_err(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        }
    }
}

/// `DELETE /api/v1/mcp/connections/{id}`: yours, or anyone's for an admin.
async fn revoke(State(app): State<Shared>, headers: HeaderMap, Path(family): Path<String>) -> Response {
    let user = match need(&app, &headers) {
        Ok(u) => u,
        Err(r) => return r,
    };
    let only = (user.role != Role::Admin).then_some(user.id);
    match app.db.oauth_revoke(&family, only) {
        Ok(0) => api_err(StatusCode::NOT_FOUND, "no such connection"),
        Ok(_) => {
            app.db.audit(now(), Some(user.id), None, "mcp.revoke", &family);
            Json(json!({ "ok": true })).into_response()
        }
        Err(e) => {
            log::error!("db: {e}");
            api_err(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        }
    }
}

#[derive(Deserialize)]
struct NewToken {
    name: String,
    #[serde(default)]
    scopes: Vec<String>,
    /// Days until it stops working; none for never.
    days: Option<i64>,
}

/// `POST /api/v1/mcp/tokens`: an API token, shown once.
async fn new_api_token(State(app): State<Shared>, headers: HeaderMap, Json(b): Json<NewToken>) -> Response {
    let user = match need(&app, &headers) {
        Ok(u) => u,
        Err(r) => return r,
    };
    let name: String = b.name.trim().chars().take(100).collect();
    if name.is_empty() {
        return api_err(StatusCode::BAD_REQUEST, "name the token after where it's used");
    }
    if b.days.is_some_and(|d| !(1..=3650).contains(&d)) {
        return api_err(StatusCode::BAD_REQUEST, "expiry is 1 to 3650 days, or never");
    }
    let scope = normal_scope(b.scopes.iter().map(String::as_str));
    let token = format!("dhp_{}", random_hex(32));
    let family = random_hex(16);
    let t = now();
    if let Err(e) = app.db.oauth_add_api_token(&token, &family, user.id, &name, &scope, b.days.map(|d| t + d * 86400), t) {
        log::error!("db: {e}");
        return api_err(StatusCode::INTERNAL_SERVER_ERROR, "database error");
    }
    app.db.audit(t, Some(user.id), None, "mcp.token", &format!("{name} ({scope})"));
    Json(json!({ "id": family, "token": token, "scopes": scope.split(' ').collect::<Vec<_>>() })).into_response()
}

// --- tools ---

#[derive(Clone)]
pub struct Deskhand {
    app: Shared,
    #[allow(dead_code)]
    tool_router: ToolRouter<Deskhand>,
}

#[derive(Deserialize, JsonSchema, Default)]
#[schemars(transform = optional_not_null)]
pub struct Page {
    /// Search text.
    query: Option<String>,
    /// From the previous page's `next_cursor`.
    cursor: Option<String>,
    /// Items per page, 1 to 100 (default 25).
    limit: Option<u32>,
}

#[derive(Deserialize, JsonSchema, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum DeviceStatus {
    Online,
    Offline,
    /// Needs a look: reboot pending, firewall or antivirus off, low disk, offline 7+ days.
    Attention,
}

#[derive(Deserialize, JsonSchema, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    Linux,
    Windows,
    Macos,
}

#[derive(Deserialize, JsonSchema)]
#[schemars(transform = optional_not_null)]
pub struct ListDevices {
    #[serde(flatten)]
    page: Page,
    status: Option<DeviceStatus>,
    os: Option<Platform>,
    /// A group id from list_groups, or "none" for devices in no group.
    group: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct DeviceId {
    /// The device's id, from list_devices.
    device_id: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct ScriptId {
    /// From list_scripts.
    script_id: i64,
}

#[derive(Deserialize, JsonSchema)]
#[schemars(transform = optional_not_null)]
pub struct GetJob {
    /// From list_jobs or a run tool.
    job_id: i64,
    /// Only runs in this status: queued, running, ok, failed, timeout, offline.
    status: Option<String>,
    #[serde(flatten)]
    page: Page,
}

/// Where a job runs: exactly one of these.
#[derive(Deserialize, JsonSchema, Default)]
#[schemars(transform = optional_not_null)]
pub struct Targets {
    /// Device ids from list_devices.
    device_ids: Option<Vec<String>>,
    /// A group id from list_groups.
    group_id: Option<i64>,
    /// Every device.
    #[serde(default)]
    all_devices: bool,
}

#[derive(Deserialize, JsonSchema, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum Shell {
    Powershell,
    Cmd,
    Bash,
    Sh,
}

#[derive(Deserialize, JsonSchema)]
#[schemars(transform = optional_not_null)]
pub struct RunCommand {
    shell: Shell,
    /// The command or script text.
    command: String,
    #[serde(flatten)]
    targets: Targets,
    /// Seconds the command may run, 5 to 86400 (default 300).
    timeout_s: Option<i64>,
    /// Seconds to wait for results before returning, 0 to 120 (default 30). The job
    /// keeps going either way; get_job reads it later.
    wait_s: Option<u64>,
    /// Hours an offline device's run waits for it to come online (default 24).
    expires_h: Option<f64>,
}

#[derive(Deserialize, JsonSchema)]
#[schemars(transform = optional_not_null)]
pub struct RunScript {
    script_id: i64,
    #[serde(flatten)]
    targets: Targets,
    /// Seconds to wait for results, 0 to 120 (default 30).
    wait_s: Option<u64>,
    /// Hours an offline device's run waits for it to come online (default 24).
    expires_h: Option<f64>,
}

#[derive(Deserialize, JsonSchema, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum Power {
    Reboot,
    Shutdown,
    Logoff,
    Lock,
    /// Wake-on-LAN, sent by an online device on the same network.
    Wake,
}

#[derive(Deserialize, JsonSchema)]
#[schemars(transform = optional_not_null)]
pub struct PowerAction {
    action: Power,
    #[serde(flatten)]
    targets: Targets,
    /// Reboot and shutdown only: the device's hostname for one device, else the number
    /// of devices. Ask the user for it; don't fill it in unasked.
    confirm: Option<String>,
    /// Reboot and shutdown: seconds before it happens.
    delay_s: Option<u32>,
    /// Reboot and shutdown: shown to the signed-in user.
    message: Option<String>,
}

/// Optional arguments as plain optional properties, not `["integer", "null"]` or an
/// `anyOf` with null: some clients map tool schemas onto dialects without a null type
/// (Gemini's OpenAPI subset). Absent and null mean the same here.
fn optional_not_null(s: &mut schemars::Schema) {
    let Some(props) = s.get_mut("properties").and_then(Value::as_object_mut) else {
        return;
    };
    for p in props.values_mut() {
        let Some(o) = p.as_object_mut() else {
            continue;
        };
        if let Some(Value::Array(t)) = o.get("type") {
            let rest: Vec<Value> = t.iter().filter(|x| *x != "null").cloned().collect();
            if let [one] = rest.as_slice() {
                o.insert("type".into(), one.clone());
            }
        }
        for k in ["anyOf", "oneOf"] {
            if let Some(Value::Array(v)) = o.get(k) {
                let rest: Vec<Value> = v.iter().filter(|x| x.get("type") != Some(&json!("null"))).cloned().collect();
                if let [Value::Object(one)] = rest.as_slice() {
                    let one = one.clone();
                    o.remove(k);
                    o.extend(one);
                }
            }
        }
        if o.get("default") == Some(&Value::Null) {
            o.remove("default");
        }
    }
}

/// One tool's answer: the API's JSON, or its error as a tool error the model can read.
type ToolOut = Result<CallToolResult, McpError>;

fn done(r: Result<Value, String>) -> ToolOut {
    Ok(match r {
        Ok(v) => CallToolResult::structured(v),
        Err(e) => CallToolResult::error(vec![ContentBlock::text(e)]),
    })
}

fn query_string(pairs: &[(&str, Option<String>)]) -> String {
    let mut s = url::form_urlencoded::Serializer::new(String::new());
    for (k, v) in pairs {
        if let Some(v) = v.as_deref().filter(|v| !v.is_empty()) {
            s.append_pair(k, v);
        }
    }
    let s = s.finish();
    if s.is_empty() {
        s
    } else {
        format!("?{s}")
    }
}

fn page_pairs(p: &Page) -> Vec<(&'static str, Option<String>)> {
    vec![
        ("q", p.query.clone()),
        ("cursor", p.cursor.clone()),
        ("limit", Some(p.limit.unwrap_or(25).clamp(1, 100).to_string())),
    ]
}

fn seg(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

fn targets_json(t: Targets) -> Value {
    json!({ "devices": t.device_ids, "group_id": t.group_id, "all": t.all_devices })
}

/// Longest stdout or stderr a tool returns per run; the rest is in the web app.
const OUTPUT_MAX: usize = 16 * 1024;

fn clip_outputs(v: &mut Value) {
    if let Some(items) = v.get_mut("items").and_then(Value::as_array_mut) {
        for run in items {
            for k in ["stdout", "stderr"] {
                if let Some(s) = run.get(k).and_then(Value::as_str).filter(|s| s.len() > OUTPUT_MAX) {
                    let mut cut = s.len() - OUTPUT_MAX;
                    while !s.is_char_boundary(cut) {
                        cut += 1;
                    }
                    let clipped = format!("[{cut} bytes cut]\n{}", &s[cut..]);
                    run[k] = Value::String(clipped);
                }
            }
        }
    }
}

fn caller(ctx: &RequestContext<RoleServer>) -> Option<Caller> {
    ctx.extensions.get::<axum::http::request::Parts>().and_then(|p| p.extensions.get::<Caller>().cloned())
}

#[tool_router]
impl Deskhand {
    fn new(app: Shared) -> Self {
        Deskhand { app, tool_router: Self::tool_router() }
    }

    /// Calls `/api/v1{path}` as the caller.
    async fn api(&self, ctx: &RequestContext<RoleServer>, method: Method, path: &str, body: Option<Value>) -> Result<Value, String> {
        let c = caller(ctx).ok_or("not signed in")?;
        let mut req = Request::builder()
            .method(method)
            .uri(path)
            .header(INTERNAL, format!("{}:{}:{}", internal_key(), c.user.id, c.cap().as_str()))
            .header(header::CONTENT_TYPE, "application/json")
            .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
            .map_err(|e| e.to_string())?;
        req.extensions_mut().insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 0))));
        let res = crate::api::router().with_state(self.app.clone()).oneshot(req).await.map_err(|e| e.to_string())?;
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), 64 << 20).await.map_err(|e| e.to_string())?;
        let v: Value = serde_json::from_slice(&bytes).unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
        if status.is_success() {
            return Ok(v);
        }
        let msg = v.get("error").and_then(Value::as_str).map(str::to_owned).unwrap_or_else(|| v.to_string());
        Err(match status {
            StatusCode::FORBIDDEN if !c.may_act() => {
                format!("{msg}: this connection has read-only access ({SCOPE_READ}); reconnect and allow acting to do this")
            }
            StatusCode::FORBIDDEN => format!("{msg}: {} is a {}", c.user.username, c.user.role.as_str()),
            _ => format!("{msg} (HTTP {})", status.as_u16()),
        })
    }

    async fn get(&self, ctx: &RequestContext<RoleServer>, path: &str) -> Result<Value, String> {
        self.api(ctx, Method::GET, path, None).await
    }

    /// Waits up to `wait_s` for a job's runs to finish, then returns them.
    async fn job_result(&self, ctx: &RequestContext<RoleServer>, id: i64, wait_s: u64) -> Result<Value, String> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(wait_s.min(120));
        let detail = loop {
            let d = self.get(ctx, &format!("/jobs/{id}")).await?;
            let n = |k: &str| d["counts"][k].as_i64().unwrap_or(0);
            if n("queued") + n("running") == 0 || tokio::time::Instant::now() >= deadline {
                break d;
            }
            tokio::time::sleep(Duration::from_millis(750)).await;
        };
        let mut runs = self.get(ctx, &format!("/jobs/{id}/runs?limit=50")).await?;
        clip_outputs(&mut runs);
        let n = |k: &str| detail["counts"][k].as_i64().unwrap_or(0);
        let finished = n("queued") + n("running") == 0;
        Ok(json!({
            "job_id": id,
            "finished": finished,
            "note": if finished { "" } else { "Still going: queued runs wait for offline devices to come online. Call get_job later." },
            "counts": detail["counts"],
            "runs": runs["items"],
            "runs_next_cursor": runs["next_cursor"],
        }))
    }

    async fn start_job(&self, ctx: &RequestContext<RoleServer>, body: Value, wait_s: u64) -> Result<Value, String> {
        let created = self.api(ctx, Method::POST, "/jobs", Some(body)).await?;
        let id = created["id"].as_i64().ok_or("the job wasn't created")?;
        self.job_result(ctx, id, wait_s).await
    }

    #[tool(
        description = "Who this connection acts as: user name, role (viewer, operator or admin) and the scopes granted. Viewers and read-only connections can look but not act.",
        annotations(title = "Who am I", read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    async fn whoami(&self, ctx: RequestContext<RoleServer>) -> ToolOut {
        let scope = caller(&ctx).map(|c| c.scope).unwrap_or_default();
        done(self.get(&ctx, "/me").await.map(|mut me| {
            me["scopes"] = json!(scope.split(' ').collect::<Vec<_>>());
            me
        }))
    }

    #[tool(
        description = "List managed devices in name order, with online status, OS, group, IPs, logged-in users, agent version and `attention` flags. Search matches name, hostname, IP, serial, model and logged-in users. Paginate with next_cursor.",
        annotations(title = "List devices", read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    async fn list_devices(&self, ctx: RequestContext<RoleServer>, Parameters(p): Parameters<ListDevices>) -> ToolOut {
        let mut q = page_pairs(&p.page);
        q.push(("status", p.status.map(|s| match s {
            DeviceStatus::Online => "online".into(),
            DeviceStatus::Offline => "offline".into(),
            DeviceStatus::Attention => "attention".into(),
        })));
        q.push(("os", p.os.map(|o| match o {
            Platform::Linux => "linux".into(),
            Platform::Windows => "windows".into(),
            Platform::Macos => "macos".into(),
        })));
        q.push(("group", p.group));
        done(self.get(&ctx, &format!("/devices{}", query_string(&q))).await)
    }

    #[tool(
        description = "One device in full: status, the latest hardware and software inventory (CPU, memory, disks, network, installed software, updates, security) and its recent job runs.",
        annotations(title = "Get device", read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    async fn get_device(&self, ctx: RequestContext<RoleServer>, Parameters(p): Parameters<DeviceId>) -> ToolOut {
        done(self.get(&ctx, &format!("/devices/{}", seg(&p.device_id))).await)
    }

    #[tool(
        description = "Ask an online device for a fresh inventory. It arrives within seconds; read it with get_device.",
        annotations(title = "Refresh inventory", read_only_hint = false, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    async fn refresh_inventory(&self, ctx: RequestContext<RoleServer>, Parameters(p): Parameters<DeviceId>) -> ToolOut {
        done(self.api(&ctx, Method::POST, &format!("/devices/{}/refresh", seg(&p.device_id)), None).await)
    }

    #[tool(
        description = "List device groups with their device counts. Paginate with next_cursor.",
        annotations(title = "List groups", read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    async fn list_groups(&self, ctx: RequestContext<RoleServer>, Parameters(p): Parameters<Page>) -> ToolOut {
        done(self.get(&ctx, &format!("/groups{}", query_string(&page_pairs(&p)))).await)
    }

    #[tool(
        description = "List saved scripts (name, shell, timeout). Paginate with next_cursor. get_script reads one's body.",
        annotations(title = "List scripts", read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    async fn list_scripts(&self, ctx: RequestContext<RoleServer>, Parameters(p): Parameters<Page>) -> ToolOut {
        done(self.get(&ctx, &format!("/scripts{}", query_string(&page_pairs(&p)))).await)
    }

    #[tool(
        description = "One saved script, with its body.",
        annotations(title = "Get script", read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    async fn get_script(&self, ctx: RequestContext<RoleServer>, Parameters(p): Parameters<ScriptId>) -> ToolOut {
        done(self.get(&ctx, &format!("/scripts/{}", p.script_id)).await)
    }

    #[tool(
        description = "List jobs newest first: one-off and scheduled script runs and power actions, with run counts by status. Paginate with next_cursor.",
        annotations(title = "List jobs", read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    async fn list_jobs(&self, ctx: RequestContext<RoleServer>, Parameters(p): Parameters<Page>) -> ToolOut {
        done(self.get(&ctx, &format!("/jobs{}", query_string(&page_pairs(&p)))).await)
    }

    #[tool(
        description = "One job and its runs, newest first: per device status, exit code, stdout and stderr (each cut to its last 16 KB). Filter runs by status or device name; paginate runs with next_cursor.",
        annotations(title = "Get job", read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    async fn get_job(&self, ctx: RequestContext<RoleServer>, Parameters(p): Parameters<GetJob>) -> ToolOut {
        let r = async {
            let job = self.get(&ctx, &format!("/jobs/{}", p.job_id)).await?;
            let mut q = page_pairs(&p.page);
            q.push(("status", p.status.clone()));
            let mut runs = self.get(&ctx, &format!("/jobs/{}/runs{}", p.job_id, query_string(&q))).await?;
            clip_outputs(&mut runs);
            Ok(json!({ "job": job["job"], "counts": job["counts"], "runs": runs["items"], "runs_next_cursor": runs["next_cursor"] }))
        };
        done(r.await)
    }

    #[tool(
        description = "Run a one-off command on devices through the agent (as root or SYSTEM when it runs as a service), and wait briefly for the output. Give exactly one of device_ids, group_id or all_devices. Offline devices run it when they come back, until it expires. This changes machines: say what you will run and where before you run it.",
        annotations(title = "Run command", read_only_hint = false, destructive_hint = true, idempotent_hint = false, open_world_hint = false)
    )]
    async fn run_command(&self, ctx: RequestContext<RoleServer>, Parameters(p): Parameters<RunCommand>) -> ToolOut {
        let mut body = targets_json(p.targets);
        body["command"] = json!(p.command);
        body["shell"] = json!(match p.shell {
            Shell::Powershell => "powershell",
            Shell::Cmd => "cmd",
            Shell::Bash => "bash",
            Shell::Sh => "sh",
        });
        body["timeout_s"] = json!(p.timeout_s);
        body["expires_h"] = json!(p.expires_h);
        done(self.start_job(&ctx, body, p.wait_s.unwrap_or(30)).await)
    }

    #[tool(
        description = "Run a saved script (see list_scripts) on devices, and wait briefly for the output. Give exactly one of device_ids, group_id or all_devices.",
        annotations(title = "Run script", read_only_hint = false, destructive_hint = true, idempotent_hint = false, open_world_hint = false)
    )]
    async fn run_script(&self, ctx: RequestContext<RoleServer>, Parameters(p): Parameters<RunScript>) -> ToolOut {
        let mut body = targets_json(p.targets);
        body["script_id"] = json!(p.script_id);
        body["expires_h"] = json!(p.expires_h);
        done(self.start_job(&ctx, body, p.wait_s.unwrap_or(30)).await)
    }

    #[tool(
        description = "Reboot, shut down, log off, lock or wake devices. Give exactly one of device_ids, group_id or all_devices. Reboot and shutdown need `confirm` (the hostname for one device, else the device count), which you should get from the user.",
        annotations(title = "Power action", read_only_hint = false, destructive_hint = true, idempotent_hint = false, open_world_hint = false)
    )]
    async fn power_action(&self, ctx: RequestContext<RoleServer>, Parameters(p): Parameters<PowerAction>) -> ToolOut {
        let mut body = targets_json(p.targets);
        body["action"] = json!(match p.action {
            Power::Reboot => "reboot",
            Power::Shutdown => "shutdown",
            Power::Logoff => "logoff",
            Power::Lock => "lock",
            Power::Wake => "wake",
        });
        body["confirm"] = json!(p.confirm);
        body["delay_s"] = json!(p.delay_s);
        body["message"] = json!(p.message);
        done(self.start_job(&ctx, body, 5).await)
    }
}

#[tool_handler]
impl ServerHandler for Deskhand {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_protocol_version(ProtocolVersion::LATEST_WITH_INITIALIZE)
            .with_server_info(Implementation::new("deskhand", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "Deskhand manages a fleet of Windows and Linux machines through an agent on each.\n\
                 \n\
                 Find devices with list_devices (search, status, OS, group), then get_device for one \
                 machine's full inventory. Act with run_command (one-off), run_script (saved scripts) and \
                 power_action; each starts a job, waits briefly, and returns per-device output. Read a \
                 job again later with get_job. Every action is in the audit log under the connected \
                 person's name.\n\
                 \n\
                 Commands run as the agent: root or SYSTEM when it runs as a service. Before running anything that changes a machine, or \
                 anything on more than one device, say what and where. Prefer reading first.",
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redirects_go_over_https_or_to_this_machine() {
        assert!(redirect_allowed("https://claude.ai/api/mcp/auth_callback"));
        assert!(redirect_allowed("http://localhost:33418/callback"));
        assert!(redirect_allowed("http://127.0.0.1/cb"));
        assert!(!redirect_allowed("http://evil.example/cb"));
        assert!(!redirect_allowed("myapp://cb"));
        assert!(!redirect_allowed("not a url"));
    }

    #[test]
    fn a_loopback_redirect_may_change_port_only() {
        assert!(redirect_matches("http://localhost:1/cb", "http://localhost:2/cb"));
        assert!(!redirect_matches("http://localhost:1/cb", "http://localhost:2/other"));
        assert!(!redirect_matches("http://localhost:1/cb", "http://127.0.0.1:1/cb"));
        assert!(!redirect_matches("https://a.example/cb", "https://a.example:8443/cb"));
        assert!(redirect_matches("https://a.example/cb", "https://a.example/cb"));
    }

    #[test]
    fn pkce_is_the_rfc_7636_test_vector() {
        assert!(pkce_matches("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk", "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"));
        assert!(!pkce_matches("wrong", "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"));
    }

    #[test]
    fn scopes_always_include_read_and_drop_unknowns() {
        assert_eq!(normal_scope(std::iter::empty()), SCOPE_READ);
        assert_eq!(normal_scope(["deskhand:act", "admin:everything"]), "deskhand:read deskhand:act");
        let p = AuthorizeParams {
            response_type: "code".into(),
            client_id: String::new(),
            redirect_uri: String::new(),
            state: None,
            code_challenge: None,
            code_challenge_method: None,
            scope: Some("deskhand:read".into()),
            resource: None,
        };
        assert_eq!(asked_scopes(&p), vec![SCOPE_READ]);
        assert_eq!(asked_scopes(&AuthorizeParams { scope: None, ..p }), SCOPES.to_vec());
    }

    #[test]
    fn redirect_with_encodes_and_appends() {
        assert_eq!(redirect_with("https://a/cb", &[("code", "x y"), ("state", "s&t")]), "https://a/cb?code=x+y&state=s%26t");
        assert_eq!(redirect_with("https://a/cb?k=1", &[("code", "c")]), "https://a/cb?k=1&code=c");
    }

    #[test]
    fn long_outputs_keep_their_end() {
        let mut v = json!({ "items": [{ "stdout": "a".repeat(OUTPUT_MAX + 10), "stderr": "short" }] });
        clip_outputs(&mut v);
        let out = v["items"][0]["stdout"].as_str().unwrap();
        assert!(out.starts_with("[10 bytes cut]\n"));
        assert_eq!(v["items"][0]["stderr"], "short");
    }
}
