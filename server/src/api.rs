//! `/api/v1`: the JSON API the web app uses. Same sessions, roles and Origin check as the
//! classic pages; mutations take JSON bodies, which a cross-site form can't send.
//!
//! Errors are `{"error": "..."}` with 400 (bad input), 401 (not signed in), 403 (role),
//! 404, 409 (conflict) or 500.

use crate::auth::{self, base_url};
use crate::db::{AuditRow, Device, Role, User};
use crate::jobs::{self, Counts, Job, Run, Task};
use crate::jobs_ui;
use crate::{now, random_hex, send_to_agent, Shared};
use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::sse::{KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use deskhand_proto::{Inventory, ScriptShell, ServerMsg};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::convert::Infallible;
use std::net::SocketAddr;

pub fn router() -> Router<Shared> {
    Router::new()
        .route("/auth", get(auth_info))
        .route("/login", post(login))
        .route("/logout", post(logout))
        .route("/setup", post(setup))
        .route("/me", get(me))
        .route("/events", get(events))
        .route("/devices", get(devices))
        .route("/devices.csv", get(crate::inventory::csv))
        .route("/devices/{id}", get(device).delete(delete_device))
        .route("/devices/{id}/rename", post(rename_device))
        .route("/devices/{id}/move", post(move_device))
        .route("/devices/{id}/refresh", post(refresh_device))
        .route("/groups", get(groups).post(add_group))
        .route("/groups/{id}", delete(delete_group))
        .route("/enroll-tokens", get(enroll_tokens).post(add_enroll_token))
        .route("/enroll-tokens/{id}", delete(revoke_enroll_token))
        .route("/users", get(users).post(add_user))
        .route("/users/{id}", delete(delete_user))
        .route("/users/{id}/role", post(set_role))
        .route("/users/{id}/email", post(set_email))
        .route("/users/{id}/identities/{provider}", delete(unlink_identity))
        .route("/audit", get(audit))
        .route("/scripts", get(scripts).post(add_script))
        .route("/scripts/{id}", get(script).put(update_script).delete(delete_script))
        .route("/jobs", get(jobs_list).post(add_job))
        .route("/jobs/{id}", get(job).delete(delete_job))
        .route("/jobs/{id}/events", get(jobs_ui::events))
        .route("/jobs/{id}/{op}", post(job_op))
        .fallback(|| async { ApiError(StatusCode::NOT_FOUND, "no such API endpoint".into()) })
}

// --- errors and auth ---

pub struct ApiError(StatusCode, String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "error": self.1 }))).into_response()
    }
}

impl From<rusqlite::Error> for ApiError {
    fn from(e: rusqlite::Error) -> Self {
        log::error!("db: {e}");
        ApiError(StatusCode::INTERNAL_SERVER_ERROR, "database error".into())
    }
}

impl From<(StatusCode, String)> for ApiError {
    fn from((code, msg): (StatusCode, String)) -> Self {
        ApiError(code, msg)
    }
}

type Api<T> = Result<Json<T>, ApiError>;

fn err(code: StatusCode, msg: impl Into<String>) -> ApiError {
    ApiError(code, msg.into())
}

fn bad(msg: impl Into<String>) -> ApiError {
    err(StatusCode::BAD_REQUEST, msg)
}

/// The signed-in user if their role is at least `min`.
fn need(app: &Shared, headers: &HeaderMap, min: Role) -> Result<User, ApiError> {
    match auth::user(app, headers) {
        Some(u) if u.role >= min => Ok(u),
        Some(_) => Err(err(StatusCode::FORBIDDEN, "your role doesn't allow this")),
        None => Err(err(StatusCode::UNAUTHORIZED, "sign in first")),
    }
}

fn ok() -> Json<serde_json::Value> {
    Json(json!({ "ok": true }))
}

// --- signing in ---

#[derive(Serialize)]
struct Me {
    id: i64,
    username: String,
    role: &'static str,
}

fn me_of(u: &User) -> Me {
    Me { id: u.id, username: u.username.clone(), role: u.role.as_str() }
}

#[derive(Serialize)]
struct AuthInfo {
    password_login: bool,
    providers: Vec<ProviderInfo>,
    setup_required: bool,
}

#[derive(Serialize)]
struct ProviderInfo {
    id: &'static str,
    label: &'static str,
}

async fn auth_info(State(app): State<Shared>) -> Json<AuthInfo> {
    Json(AuthInfo {
        password_login: auth::password_login(&app),
        providers: app
            .sso
            .providers
            .iter()
            .map(|p| ProviderInfo { id: p.kind.key(), label: p.kind.label() })
            .collect(),
        setup_required: app.setup_token.lock().unwrap().is_some(),
    })
}

async fn me(State(app): State<Shared>, headers: HeaderMap) -> Result<Json<Me>, ApiError> {
    need(&app, &headers, Role::Viewer).map(|u| Json(me_of(&u)))
}

#[derive(Deserialize)]
struct LoginBody {
    username: String,
    password: String,
}

fn signed_in(app: &Shared, headers: &HeaderMap, user: &User, action: &str, detail: &str) -> Result<Response, ApiError> {
    let cookie = auth::new_session(app, headers, user, action, detail)
        .ok_or_else(|| err(StatusCode::INTERNAL_SERVER_ERROR, "couldn't start a session"))?;
    Ok(([(header::SET_COOKIE, cookie)], Json(me_of(user))).into_response())
}

async fn login(
    State(app): State<Shared>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(b): Json<LoginBody>,
) -> Result<Response, ApiError> {
    if !auth::password_login(&app) {
        return Err(err(StatusCode::FORBIDDEN, "password sign-in is off; use single sign-on"));
    }
    let ip = auth::client_ip(&app, &headers, peer);
    if !app.login_limit.allow(ip) {
        return Err(err(StatusCode::TOO_MANY_REQUESTS, "too many attempts; wait a minute"));
    }
    let found = app.db.user_by_name(b.username.trim()).ok().flatten();
    let hash = found.as_ref().map(|(_, h)| h.as_str()).unwrap_or(auth::dummy_hash());
    let good = auth::verify_password(&b.password, hash);
    let Some((user, _)) = found.filter(|_| good) else {
        app.db.audit(now(), None, None, "login.failed", &format!("{} from {ip}", b.username.trim()));
        return Err(err(StatusCode::UNAUTHORIZED, "wrong user name or password"));
    };
    signed_in(&app, &headers, &user, "login", &format!("password from {ip}"))
}

async fn logout(State(app): State<Shared>, headers: HeaderMap) -> Response {
    if let Some(token) = auth::cookie_value(&headers) {
        if let Some(u) = auth::user(&app, &headers) {
            app.db.audit(now(), Some(u.id), None, "logout", "");
        }
        let _ = app.db.delete_session(&token);
    }
    ([(header::SET_COOKIE, format!("{}=; Path=/; Max-Age=0", auth::COOKIE))], ok()).into_response()
}

#[derive(Deserialize)]
struct SetupBody {
    token: String,
    username: String,
    password: String,
}

async fn setup(
    State(app): State<Shared>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(b): Json<SetupBody>,
) -> Result<Response, ApiError> {
    let ip = auth::client_ip(&app, &headers, peer);
    if !app.login_limit.allow(ip) {
        return Err(err(StatusCode::TOO_MANY_REQUESTS, "too many attempts; wait a minute"));
    }
    let username = b.username.trim().to_owned();
    {
        let mut setup = app.setup_token.lock().unwrap();
        let Some(expected) = setup.as_deref() else {
            return Err(err(StatusCode::CONFLICT, "setup is already done; sign in"));
        };
        if !crate::same(b.token.trim(), expected) {
            return Err(err(StatusCode::FORBIDDEN, "wrong setup token; use the link the server printed"));
        }
        if username.is_empty() || b.password.len() < 8 {
            return Err(bad("pick a user name and a password of 8+ characters"));
        }
        app.db.add_user(&username, &auth::hash_password(&b.password), Role::Admin, now())?;
        *setup = None;
    }
    let (user, _) = app
        .db
        .user_by_name(&username)?
        .ok_or_else(|| err(StatusCode::INTERNAL_SERVER_ERROR, "user vanished"))?;
    app.db.audit(now(), Some(user.id), None, "setup", "first admin created");
    signed_in(&app, &headers, &user, "login", &format!("setup from {ip}"))
}

// --- devices ---

#[derive(Serialize)]
pub struct DeviceOut {
    id: String,
    name: String,
    hostname: String,
    /// Inventory OS ("Ubuntu 26.04") when known, else the platform.
    os: String,
    /// What the agent said it runs on: `linux`, `windows`, `macos`.
    platform: String,
    group_id: Option<i64>,
    group: Option<String>,
    online: bool,
    online_since: Option<i64>,
    last_seen: i64,
    last_boot: Option<i64>,
    ip: Option<String>,
    public_ip: Option<String>,
    serial: Option<String>,
    model: Option<String>,
    logged_in_users: Vec<String>,
    /// Why the device needs a look: `reboot_pending`, `firewall_off`, `low_disk`,
    /// `av_off`, `offline_7d`.
    attention: Vec<&'static str>,
}

/// The first non-loopback IPv4 address on a real interface.
fn lan_ip(inv: &Inventory) -> Option<String> {
    inv.network
        .iter()
        .filter(|n| !["lo", "docker", "veth", "br-", "virbr", "vEthernet"].iter().any(|p| n.name.starts_with(p)))
        .flat_map(|n| n.ips.iter())
        .map(|ip| ip.split('/').next().unwrap_or(ip))
        .find(|ip| ip.contains('.') && !ip.starts_with("127.") && !ip.starts_with("169.254."))
        .map(str::to_owned)
}

fn attention(inv: Option<&Inventory>, online: bool, last_seen: i64, now: i64) -> Vec<&'static str> {
    let mut a = vec![];
    if let Some(inv) = inv {
        if inv.security.pending_reboot == Some(true) {
            a.push("reboot_pending");
        }
        if inv.security.firewall.iter().any(|f| f.enabled == Some(false)) {
            a.push("firewall_off");
        }
        if inv.disks.iter().any(|d| d.total > (1 << 30) && (d.free as f64) < d.total as f64 * 0.1) {
            a.push("low_disk");
        }
        if inv.security.antivirus.as_ref().is_some_and(|av| av.enabled == Some(false)) {
            a.push("av_off");
        }
    }
    if !online && now - last_seen > 7 * 86400 {
        a.push("offline_7d");
    }
    a
}

fn device_out(d: Device, inv: Option<&Inventory>, since: Option<i64>, now: i64) -> DeviceOut {
    let os = match (&d.inv_os, &d.inv_os_version) {
        (Some(n), Some(v)) => format!("{n} {v}"),
        (Some(n), None) => n.clone(),
        _ => d.os.clone(),
    };
    DeviceOut {
        attention: attention(inv, since.is_some(), d.last_seen, now),
        ip: inv.and_then(lan_ip),
        public_ip: inv.and_then(|i| i.public_ip.clone()),
        serial: inv.and_then(|i| i.hardware.serial.clone()),
        model: inv.and_then(|i| match (&i.hardware.manufacturer, &i.hardware.model) {
            (Some(m), Some(n)) => Some(format!("{m} {n}")),
            (m, n) => m.clone().or(n.clone()),
        }),
        logged_in_users: inv.map(|i| i.logged_in_users.clone()).unwrap_or_default(),
        id: d.id,
        name: d.name,
        hostname: d.hostname,
        os,
        platform: d.os,
        group_id: d.group_id,
        group: d.group,
        online: since.is_some(),
        online_since: since,
        last_seen: d.last_seen,
        last_boot: d.inv_last_boot,
    }
}

fn online_map(app: &Shared) -> HashMap<String, i64> {
    app.agents.lock().unwrap().iter().map(|(id, a)| (id.clone(), a.since)).collect()
}

#[derive(Deserialize)]
struct DeviceQuery {
    group: Option<i64>,
    q: Option<String>,
    status: Option<String>,
}

async fn devices(State(app): State<Shared>, headers: HeaderMap, Query(f): Query<DeviceQuery>) -> Api<Vec<DeviceOut>> {
    need(&app, &headers, Role::Viewer)?;
    let invs = app.db.latest_inventories()?;
    let online = online_map(&app);
    let now = now();
    let q = f.q.as_deref().map(str::to_lowercase).filter(|q| !q.is_empty());
    let out = app
        .db
        .devices(f.group)?
        .into_iter()
        .map(|d| {
            let inv = invs.get(&d.id);
            let since = online.get(&d.id).copied();
            device_out(d, inv, since, now)
        })
        .filter(|d| match f.status.as_deref() {
            Some("online") => d.online,
            Some("offline") => !d.online,
            Some("attention") => !d.attention.is_empty(),
            _ => true,
        })
        .filter(|d| {
            q.as_deref().is_none_or(|q| {
                [Some(&d.name), Some(&d.hostname), Some(&d.os), d.ip.as_ref(), d.serial.as_ref(), d.group.as_ref()]
                    .into_iter()
                    .flatten()
                    .any(|s| s.to_lowercase().contains(q))
            })
        })
        .collect();
    Ok(Json(out))
}

#[derive(Serialize)]
struct DeviceDetail {
    device: DeviceOut,
    inventory: Option<Inventory>,
    inventory_at: Option<i64>,
    runs: Vec<DeviceRun>,
}

#[derive(Serialize)]
struct DeviceRun {
    job_id: i64,
    job_name: String,
    #[serde(flatten)]
    run: RunOut,
}

async fn device(State(app): State<Shared>, headers: HeaderMap, Path(id): Path<String>) -> Api<DeviceDetail> {
    need(&app, &headers, Role::Viewer)?;
    let d = app.db.device(&id)?.ok_or_else(|| err(StatusCode::NOT_FOUND, "no such device"))?;
    let inv = app.db.inventory(&id)?;
    let since = online_map(&app).get(&id).copied();
    let runs = app
        .db
        .device_runs(&id, 50)?
        .into_iter()
        .map(|(r, job_id, job_name)| DeviceRun { job_id, job_name, run: run_out(r) })
        .collect();
    Ok(Json(DeviceDetail {
        device: device_out(d, inv.as_ref().map(|(_, i)| i), since, now()),
        inventory_at: inv.as_ref().map(|(t, _)| *t),
        inventory: inv.map(|(_, i)| i),
        runs,
    }))
}

#[derive(Deserialize)]
struct NameBody {
    name: String,
}

async fn rename_device(
    State(app): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(b): Json<NameBody>,
) -> Api<serde_json::Value> {
    let user = need(&app, &headers, Role::Admin)?;
    let name: String = b.name.trim().chars().take(200).collect();
    if name.is_empty() {
        return Err(bad("a device needs a name"));
    }
    if app.db.rename_device(&id, &name)? == 0 {
        return Err(err(StatusCode::NOT_FOUND, "no such device"));
    }
    app.db.audit(now(), Some(user.id), Some(&id), "device.rename", &name);
    Ok(ok())
}

#[derive(Deserialize)]
struct MoveBody {
    group_id: Option<i64>,
}

async fn move_device(
    State(app): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(b): Json<MoveBody>,
) -> Api<serde_json::Value> {
    let user = need(&app, &headers, Role::Admin)?;
    if let Some(g) = b.group_id {
        if !app.db.group_exists(g)? {
            return Err(bad("no such group"));
        }
    }
    if app.db.move_device(&id, b.group_id)? == 0 {
        return Err(err(StatusCode::NOT_FOUND, "no such device"));
    }
    let to = b.group_id.map_or("no group".into(), |g| format!("group {g}"));
    app.db.audit(now(), Some(user.id), Some(&id), "device.move", &to);
    Ok(ok())
}

async fn delete_device(State(app): State<Shared>, headers: HeaderMap, Path(id): Path<String>) -> Api<serde_json::Value> {
    let user = need(&app, &headers, Role::Admin)?;
    let name = app.db.device_name(&id)?.ok_or_else(|| err(StatusCode::NOT_FOUND, "no such device"))?;
    app.db.delete_device(&id)?;
    // Its agent's secret is gone with it; drop the live connection too.
    app.agents.lock().unwrap().remove(&id);
    app.db.audit(now(), Some(user.id), None, "device.delete", &format!("{name} ({id})"));
    let _ = app.device_events.send(crate::DeviceEvent::Removed { id });
    Ok(ok())
}

async fn refresh_device(State(app): State<Shared>, headers: HeaderMap, Path(id): Path<String>) -> Api<serde_json::Value> {
    let user = need(&app, &headers, Role::Operator)?;
    if !send_to_agent(&app, &id, ServerMsg::RefreshInventory) {
        return Err(err(StatusCode::CONFLICT, "device offline"));
    }
    app.db.audit(now(), Some(user.id), Some(&id), "device.inventory_refresh", "");
    Ok(ok())
}

/// `GET /api/v1/events`: device changes as server-sent events, for live lists.
async fn events(State(app): State<Shared>, headers: HeaderMap) -> Result<Response, ApiError> {
    need(&app, &headers, Role::Viewer)?;
    let rx = app.device_events.subscribe();
    let stream = futures_util::stream::unfold(rx, |mut rx| async move {
        loop {
            match rx.recv().await {
                Ok(e) => {
                    let ev = axum::response::sse::Event::default().data(serde_json::to_string(&e).unwrap_or_default());
                    return Some((Ok::<_, Infallible>(ev), rx));
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
            }
        }
    });
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()).into_response())
}

// --- groups ---

#[derive(Serialize)]
struct GroupOut {
    id: i64,
    name: String,
    devices: i64,
}

async fn groups(State(app): State<Shared>, headers: HeaderMap) -> Api<Vec<GroupOut>> {
    need(&app, &headers, Role::Viewer)?;
    Ok(Json(app.db.groups()?.into_iter().map(|g| GroupOut { id: g.id, name: g.name, devices: g.devices }).collect()))
}

async fn add_group(State(app): State<Shared>, headers: HeaderMap, Json(b): Json<NameBody>) -> Api<serde_json::Value> {
    let user = need(&app, &headers, Role::Admin)?;
    let name: String = b.name.trim().chars().take(100).collect();
    if name.is_empty() {
        return Err(bad("a group needs a name"));
    }
    let id = match app.db.add_group(&name) {
        Ok(id) => id,
        Err(rusqlite::Error::SqliteFailure(e, _)) if e.code == rusqlite::ErrorCode::ConstraintViolation => {
            return Err(err(StatusCode::CONFLICT, "there's already a group with that name"))
        }
        Err(e) => return Err(e.into()),
    };
    app.db.audit(now(), Some(user.id), None, "group.add", &name);
    Ok(Json(json!({ "id": id })))
}

async fn delete_group(State(app): State<Shared>, headers: HeaderMap, Path(id): Path<i64>) -> Api<serde_json::Value> {
    let user = need(&app, &headers, Role::Admin)?;
    if app.db.delete_group(id)? == 0 {
        return Err(err(StatusCode::NOT_FOUND, "no such group"));
    }
    app.db.audit(now(), Some(user.id), None, "group.delete", &format!("group {id}"));
    Ok(ok())
}

// --- enrollment ---

#[derive(Serialize)]
struct TokenOut {
    id: i64,
    group: Option<String>,
    expires: i64,
    max_uses: i64,
    uses: i64,
}

async fn enroll_tokens(State(app): State<Shared>, headers: HeaderMap) -> Api<Vec<TokenOut>> {
    need(&app, &headers, Role::Admin)?;
    Ok(Json(
        app.db
            .enroll_tokens(now())?
            .into_iter()
            .map(|t| TokenOut { id: t.id, group: t.group, expires: t.expires, max_uses: t.max_uses, uses: t.uses })
            .collect(),
    ))
}

#[derive(Deserialize)]
struct TokenBody {
    group_id: Option<i64>,
    hours: i64,
    max_uses: i64,
}

#[derive(Serialize)]
struct NewToken {
    token: String,
    server: String,
    linux: String,
    windows: String,
}

async fn add_enroll_token(State(app): State<Shared>, headers: HeaderMap, Json(b): Json<TokenBody>) -> Api<NewToken> {
    let user = need(&app, &headers, Role::Admin)?;
    if let Some(g) = b.group_id {
        if !app.db.group_exists(g)? {
            return Err(bad("no such group"));
        }
    }
    let hours = b.hours.clamp(1, 24 * 365);
    let max_uses = b.max_uses.clamp(1, 100_000);
    let token = random_hex(20);
    let now = now();
    app.db.add_enroll_token(&token, b.group_id, now + hours * 3600, max_uses, now)?;
    app.db.audit(
        now,
        Some(user.id),
        None,
        "enroll_token.create",
        &format!("{max_uses} uses, {hours}h, group {}", b.group_id.map_or("none".into(), |g| g.to_string())),
    );
    let server = base_url(&app, &headers);
    Ok(Json(NewToken {
        linux: format!(
            "curl -fsSL https://deskhand.sh/dl/deskhand-agent-linux -o /usr/local/bin/deskhand-agent && chmod +x /usr/local/bin/deskhand-agent && deskhand-agent enroll --server {server} --token {token} && deskhand-agent"
        ),
        windows: format!(
            r#"New-Item -ItemType Directory -Force "$env:ProgramFiles\Deskhand" | Out-Null; iwr https://deskhand.sh/dl/deskhand-agent-windows.exe -OutFile "$env:ProgramFiles\Deskhand\deskhand-agent.exe"; & "$env:ProgramFiles\Deskhand\deskhand-agent.exe" enroll --server {server} --token {token}; & "$env:ProgramFiles\Deskhand\deskhand-agent.exe""#
        ),
        server,
        token,
    }))
}

async fn revoke_enroll_token(State(app): State<Shared>, headers: HeaderMap, Path(id): Path<i64>) -> Api<serde_json::Value> {
    let user = need(&app, &headers, Role::Admin)?;
    if app.db.revoke_enroll_token(id)? == 0 {
        return Err(err(StatusCode::NOT_FOUND, "no such token"));
    }
    app.db.audit(now(), Some(user.id), None, "enroll_token.revoke", &format!("token {id}"));
    Ok(ok())
}

// --- users ---

#[derive(Serialize)]
struct UserOut {
    id: i64,
    username: String,
    role: &'static str,
    created: i64,
    email: Option<String>,
    identities: Vec<IdentityOut>,
}

#[derive(Serialize)]
struct IdentityOut {
    provider: String,
    label: String,
    email: Option<String>,
}

#[derive(Serialize)]
struct UsersOut {
    users: Vec<UserOut>,
    sso_enabled: bool,
}

async fn users(State(app): State<Shared>, headers: HeaderMap) -> Api<UsersOut> {
    need(&app, &headers, Role::Admin)?;
    let emails = app.db.user_emails()?;
    let mut ids: HashMap<i64, Vec<IdentityOut>> = HashMap::new();
    for (uid, provider, email) in app.db.identities()? {
        let label = crate::sso::Kind::parse(&provider).map_or(provider.clone(), |k| k.label().to_owned());
        ids.entry(uid).or_default().push(IdentityOut { provider, label, email });
    }
    let users = app
        .db
        .users()?
        .into_iter()
        .map(|(u, created)| UserOut {
            email: emails.get(&u.id).cloned(),
            identities: ids.remove(&u.id).unwrap_or_default(),
            id: u.id,
            username: u.username,
            role: u.role.as_str(),
            created,
        })
        .collect();
    Ok(Json(UsersOut { users, sso_enabled: !app.sso.providers.is_empty() }))
}

fn parse_email(s: Option<&str>) -> Result<Option<String>, ApiError> {
    let e = s.unwrap_or("").trim().to_ascii_lowercase();
    if e.is_empty() {
        return Ok(None);
    }
    if !e.contains('@') || e.len() > 254 {
        return Err(bad("that isn't an email address"));
    }
    Ok(Some(e))
}

#[derive(Deserialize)]
struct NewUserBody {
    username: String,
    email: Option<String>,
    password: Option<String>,
    role: String,
}

async fn add_user(State(app): State<Shared>, headers: HeaderMap, Json(b): Json<NewUserBody>) -> Api<serde_json::Value> {
    let user = need(&app, &headers, Role::Admin)?;
    let username = b.username.trim();
    let role = Role::parse(&b.role).ok_or_else(|| bad("unknown role"))?;
    let email = parse_email(b.email.as_deref())?;
    let password = b.password.unwrap_or_default();
    // Without a password the user can only come in through single sign-on, by email.
    let sso_only = password.is_empty() && email.is_some() && !app.sso.providers.is_empty();
    if username.is_empty() || (!sso_only && password.len() < 8) {
        return Err(bad("user name required, and a password of 8+ characters (or an email, with single sign-on set up)"));
    }
    if let Some(e) = &email {
        if app.db.user_by_email(e)?.is_some() {
            return Err(err(StatusCode::CONFLICT, "another user has that email"));
        }
    }
    let hash = if sso_only { "!".to_owned() } else { auth::hash_password(&password) };
    let id = match app.db.add_user(username, &hash, role, now()) {
        Ok(id) => id,
        Err(rusqlite::Error::SqliteFailure(e, _)) if e.code == rusqlite::ErrorCode::ConstraintViolation => {
            return Err(err(StatusCode::CONFLICT, "that user name is taken"))
        }
        Err(e) => return Err(e.into()),
    };
    if let Some(e) = &email {
        app.db.set_email(id, Some(e))?;
    }
    app.db.audit(now(), Some(user.id), None, "user.add", &format!("{username} as {}", role.as_str()));
    Ok(Json(json!({ "id": id })))
}

/// Whether removing admin from `id` (by demoting or deleting) would leave no admins.
fn last_admin(app: &Shared, id: i64) -> Result<bool, ApiError> {
    let is_admin = app.db.users()?.iter().any(|(u, _)| u.id == id && u.role == Role::Admin);
    Ok(is_admin && app.db.admin_count()? <= 1)
}

#[derive(Deserialize)]
struct RoleBody {
    role: String,
}

async fn set_role(
    State(app): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(b): Json<RoleBody>,
) -> Api<serde_json::Value> {
    let user = need(&app, &headers, Role::Admin)?;
    let role = Role::parse(&b.role).ok_or_else(|| bad("unknown role"))?;
    if role != Role::Admin && last_admin(&app, id)? {
        return Err(err(StatusCode::CONFLICT, "that's the last admin"));
    }
    if app.db.set_role(id, role)? != 1 {
        return Err(err(StatusCode::NOT_FOUND, "no such user"));
    }
    app.db.audit(now(), Some(user.id), None, "user.role", &format!("user {id} -> {}", role.as_str()));
    Ok(ok())
}

#[derive(Deserialize)]
struct EmailBody {
    email: Option<String>,
}

async fn set_email(
    State(app): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(b): Json<EmailBody>,
) -> Api<serde_json::Value> {
    let user = need(&app, &headers, Role::Admin)?;
    let email = parse_email(b.email.as_deref())?;
    match app.db.set_email(id, email.as_deref()) {
        Ok(1) => {}
        Ok(_) => return Err(err(StatusCode::NOT_FOUND, "no such user")),
        Err(rusqlite::Error::SqliteFailure(e, _)) if e.code == rusqlite::ErrorCode::ConstraintViolation => {
            return Err(err(StatusCode::CONFLICT, "another user has that email"))
        }
        Err(e) => return Err(e.into()),
    }
    app.db.audit(now(), Some(user.id), None, "user.email", &format!("user {id} -> {}", email.as_deref().unwrap_or("none")));
    Ok(ok())
}

async fn delete_user(State(app): State<Shared>, headers: HeaderMap, Path(id): Path<i64>) -> Api<serde_json::Value> {
    let user = need(&app, &headers, Role::Admin)?;
    if last_admin(&app, id)? {
        return Err(err(StatusCode::CONFLICT, "that's the last admin"));
    }
    let name = app.db.users()?.into_iter().find(|(u, _)| u.id == id).map(|(u, _)| u.username);
    let Some(name) = name else {
        return Err(err(StatusCode::NOT_FOUND, "no such user"));
    };
    app.db.delete_user(id)?;
    app.db.audit(now(), Some(user.id), None, "user.delete", &name);
    Ok(ok())
}

async fn unlink_identity(
    State(app): State<Shared>,
    headers: HeaderMap,
    Path((id, provider)): Path<(i64, String)>,
) -> Api<serde_json::Value> {
    let user = need(&app, &headers, Role::Admin)?;
    if app.db.unlink_identity(id, &provider)? == 0 {
        return Err(err(StatusCode::NOT_FOUND, "no such linked sign-in"));
    }
    app.db.audit(now(), Some(user.id), None, "user.unlink", &format!("user {id}: {provider}"));
    Ok(ok())
}

// --- audit ---

#[derive(Deserialize)]
struct AuditQuery {
    before: Option<i64>,
    limit: Option<i64>,
    action: Option<String>,
    q: Option<String>,
}

#[derive(Serialize)]
struct AuditOut {
    rows: Vec<AuditRowOut>,
    /// Pass as `before` for the next page; absent on the last.
    next: Option<i64>,
    actions: Vec<String>,
}

#[derive(Serialize)]
struct AuditRowOut {
    id: i64,
    ts: i64,
    user: Option<String>,
    device: Option<String>,
    action: String,
    detail: String,
}

async fn audit(State(app): State<Shared>, headers: HeaderMap, Query(q): Query<AuditQuery>) -> Api<AuditOut> {
    need(&app, &headers, Role::Admin)?;
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    let action = q.action.as_deref().filter(|s| !s.is_empty());
    let text = q.q.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let page = app.db.audit_page(q.before, limit, action, text)?;
    let next = (page.len() as i64 == limit).then(|| page.last().map(|(id, _)| *id)).flatten();
    let rows = page
        .into_iter()
        .map(|(id, AuditRow { ts, user, device, action, detail })| AuditRowOut { id, ts, user, device, action, detail })
        .collect();
    Ok(Json(AuditOut { rows, next, actions: app.db.audit_actions()? }))
}

// --- scripts ---

#[derive(Serialize)]
struct ScriptOut {
    id: i64,
    name: String,
    shell: ScriptShell,
    timeout_s: i64,
    creator: Option<String>,
    updated: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    body: Option<String>,
}

fn script_out(s: jobs::Script, with_body: bool) -> ScriptOut {
    ScriptOut {
        id: s.id,
        name: s.name,
        shell: s.shell,
        timeout_s: s.timeout_s,
        creator: s.creator,
        updated: s.updated,
        body: with_body.then_some(s.body),
    }
}

async fn scripts(State(app): State<Shared>, headers: HeaderMap) -> Api<Vec<ScriptOut>> {
    need(&app, &headers, Role::Viewer)?;
    Ok(Json(app.db.scripts()?.into_iter().map(|s| script_out(s, false)).collect()))
}

async fn script(State(app): State<Shared>, headers: HeaderMap, Path(id): Path<i64>) -> Api<ScriptOut> {
    need(&app, &headers, Role::Viewer)?;
    let s = app.db.script(id)?.ok_or_else(|| err(StatusCode::NOT_FOUND, "no such script"))?;
    Ok(Json(script_out(s, true)))
}

#[derive(Deserialize)]
struct ScriptBody {
    name: String,
    shell: String,
    body: String,
    timeout_s: i64,
}

/// Checked the same way as the classic form.
fn script_fields(b: ScriptBody) -> Result<(String, ScriptShell, String, i64), ApiError> {
    let f = vec![
        ("name".to_owned(), b.name.trim().to_owned()),
        ("shell".to_owned(), b.shell),
        ("timeout_s".to_owned(), b.timeout_s.to_string()),
        ("body".to_owned(), b.body),
    ];
    jobs_ui::script_fields(&f).map_err(bad)
}

async fn add_script(State(app): State<Shared>, headers: HeaderMap, Json(b): Json<ScriptBody>) -> Api<serde_json::Value> {
    let user = need(&app, &headers, Role::Operator)?;
    let (name, shell, body, timeout) = script_fields(b)?;
    let id = app.db.add_script(&name, shell, &body, timeout, user.id, now())?;
    app.db.audit(now(), Some(user.id), None, "script.create", &format!("{id}: {name}"));
    Ok(Json(json!({ "id": id })))
}

async fn update_script(
    State(app): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(b): Json<ScriptBody>,
) -> Api<serde_json::Value> {
    let user = need(&app, &headers, Role::Operator)?;
    let (name, shell, body, timeout) = script_fields(b)?;
    if app.db.update_script(id, &name, shell, &body, timeout, now())? == 0 {
        return Err(err(StatusCode::NOT_FOUND, "no such script"));
    }
    app.db.audit(now(), Some(user.id), None, "script.update", &format!("{id}: {name}"));
    Ok(ok())
}

async fn delete_script(State(app): State<Shared>, headers: HeaderMap, Path(id): Path<i64>) -> Api<serde_json::Value> {
    let user = need(&app, &headers, Role::Operator)?;
    let name = app.db.script(id)?.map(|s| s.name).ok_or_else(|| err(StatusCode::NOT_FOUND, "no such script"))?;
    app.db.delete_script(id)?;
    app.db.audit(now(), Some(user.id), None, "script.delete", &format!("{id}: {name}"));
    Ok(ok())
}

// --- jobs ---

#[derive(Serialize)]
pub struct RunOut {
    id: i64,
    device_id: String,
    device_name: String,
    /// For wake-on-LAN: the device that sent the packet.
    via: Option<String>,
    status: String,
    exit_code: Option<i32>,
    stdout: String,
    stderr: String,
    queued: i64,
    started: Option<i64>,
    finished: Option<i64>,
}

fn run_out(r: Run) -> RunOut {
    RunOut {
        id: r.id,
        device_id: r.device_id,
        device_name: r.device_name,
        via: r.via,
        status: r.status,
        exit_code: r.exit_code,
        stdout: r.stdout,
        stderr: r.stderr,
        queued: r.queued,
        started: r.started,
        finished: r.finished,
    }
}

#[derive(Serialize, Default)]
struct CountsOut {
    queued: i64,
    running: i64,
    ok: i64,
    failed: i64,
    timeout: i64,
    offline: i64,
}

#[derive(Serialize)]
struct JobOut {
    id: i64,
    name: String,
    /// `script` or the action's name (`reboot`, `shutdown`, `logoff`, `lock`, `wake`).
    kind: String,
    /// e.g. "bash script", "reboot".
    task: String,
    script_id: Option<i64>,
    target: String,
    schedule: Option<String>,
    next_fire: Option<i64>,
    last_fired: Option<i64>,
    enabled: bool,
    creator: Option<String>,
    created: i64,
    timeout_s: i64,
    expires_after: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    counts: Option<CountsOut>,
}

fn job_out(app: &Shared, j: Job, counts: Option<Counts>) -> JobOut {
    let (kind, script_id) = match &j.task {
        Task::Script { script_id, .. } => ("script".to_owned(), *script_id),
        Task::Action(a) => (a.name().to_owned(), None),
    };
    JobOut {
        task: jobs_ui::task_text(&j.task),
        target: jobs_ui::target_text(app, &j.target),
        kind,
        script_id,
        id: j.id,
        name: j.name,
        schedule: j.schedule,
        next_fire: j.next_fire,
        last_fired: j.last_fired,
        enabled: j.enabled,
        creator: j.creator,
        created: j.created,
        timeout_s: j.timeout_s,
        expires_after: j.expires_after,
        counts: counts.map(|c| CountsOut {
            queued: c.queued,
            running: c.running,
            ok: c.ok,
            failed: c.failed,
            timeout: c.timeout,
            offline: c.offline,
        }),
    }
}

async fn jobs_list(State(app): State<Shared>, headers: HeaderMap) -> Api<Vec<JobOut>> {
    need(&app, &headers, Role::Viewer)?;
    let jobs = app.db.jobs()?;
    Ok(Json(jobs.into_iter().map(|(j, c)| job_out(&app, j, Some(c))).collect()))
}

#[derive(Serialize)]
struct JobDetail {
    job: JobOut,
    runs: Vec<RunOut>,
}

async fn job(State(app): State<Shared>, headers: HeaderMap, Path(id): Path<i64>) -> Api<JobDetail> {
    need(&app, &headers, Role::Viewer)?;
    let j = app.db.job(id)?.ok_or_else(|| err(StatusCode::NOT_FOUND, "no such job"))?;
    let runs = app.db.runs(id)?.into_iter().map(run_out).collect();
    Ok(Json(JobDetail { job: job_out(&app, j, None), runs }))
}

/// `POST /api/v1/jobs`. Exactly one of `script_id` / `action`, and one of `devices` /
/// `group_id` / `all`. Reboot and shutdown need `confirm`: the hostname for one device,
/// else the number of devices.
#[derive(Deserialize)]
struct JobBody {
    name: Option<String>,
    script_id: Option<i64>,
    action: Option<String>,
    delay_s: Option<u32>,
    message: Option<String>,
    devices: Option<Vec<String>>,
    group_id: Option<i64>,
    #[serde(default)]
    all: bool,
    cron: Option<String>,
    expires_h: Option<f64>,
    confirm: Option<String>,
}

async fn add_job(State(app): State<Shared>, headers: HeaderMap, Json(b): Json<JobBody>) -> Api<serde_json::Value> {
    let user = need(&app, &headers, Role::Operator)?;
    // The same fields the classic job form posts, checked by the same code.
    let mut f: Vec<(String, String)> = vec![];
    let mut put = |k: &str, v: String| f.push((k.to_owned(), v));
    match (b.script_id, b.action) {
        (Some(id), None) => put("source", format!("script:{id}")),
        (None, Some(a)) => put("source", format!("action:{a}")),
        _ => return Err(bad("give a script_id or an action")),
    }
    if b.all {
        put("target", "all".into());
    } else if let Some(g) = b.group_id {
        put("target", "group".into());
        put("group", g.to_string());
    } else {
        put("target", "devices".into());
        for d in b.devices.unwrap_or_default() {
            put("device", d);
        }
    }
    if let Some(c) = b.cron.filter(|c| !c.trim().is_empty()) {
        put("when", "cron".into());
        put("cron", c);
    }
    for (k, v) in [
        ("name", b.name),
        ("delay_s", b.delay_s.map(|d| d.to_string())),
        ("message", b.message),
        ("expires_h", b.expires_h.map(|h| h.to_string())),
        ("confirm", b.confirm),
    ] {
        if let Some(v) = v {
            put(k, v);
        }
    }
    let id = jobs_ui::create_job_from(&app, &user, &f)?;
    Ok(Json(json!({ "id": id })))
}

async fn job_op(
    State(app): State<Shared>,
    headers: HeaderMap,
    Path((id, op)): Path<(i64, String)>,
) -> Api<serde_json::Value> {
    let user = need(&app, &headers, Role::Operator)?;
    let name = app.db.job(id)?.map(|j| j.name).ok_or_else(|| err(StatusCode::NOT_FOUND, "no such job"))?;
    match op.as_str() {
        "run" => {
            jobs::fire(&app, id).map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
        }
        "enable" => {
            app.db.set_job_enabled(id, true, now())?;
        }
        "disable" => {
            app.db.set_job_enabled(id, false, now())?;
        }
        _ => return Err(err(StatusCode::NOT_FOUND, "unknown job operation")),
    }
    app.db.audit(now(), Some(user.id), None, &format!("job.{op}"), &format!("{id}: {name}"));
    Ok(ok())
}

async fn delete_job(State(app): State<Shared>, headers: HeaderMap, Path(id): Path<i64>) -> Api<serde_json::Value> {
    let user = need(&app, &headers, Role::Operator)?;
    let name = app.db.job(id)?.map(|j| j.name).ok_or_else(|| err(StatusCode::NOT_FOUND, "no such job"))?;
    app.db.delete_job(id)?;
    app.db.audit(now(), Some(user.id), None, "job.delete", &format!("{id}: {name}"));
    Ok(ok())
}

