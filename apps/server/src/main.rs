//! deskhand server: agents enroll with a token and dial in over WebSocket, users sign in to
//! the dashboard, and `/d/{id}/` proxies the browser to that device's voxl through the
//! agent's tunnel.
//!
//! Settings come from the environment:
//! - `DESKHAND_ADDR` (default `0.0.0.0:8080`)
//! - `DESKHAND_DB` (default `./deskhand.db`): the SQLite database
//! - `DESKHAND_ADMIN_PASSWORD` (optional): with no users yet, creates `admin` with this
//!   password; without it the server prints a one-time setup link
//! - `DESKHAND_TRUST_PROXY=1`: believe X-Forwarded-Proto/-For from a TLS proxy in front
//! - `DESKHAND_LOGIN_RATE` (default 5): sign-in attempts allowed per IP per minute
//! - `DESKHAND_AGENT_DIR` (default `agents/` beside the database): agent builds served
//!   at `/dl/...` for the install scripts (`/install.sh`, `/install.ps1`)
//! - `DESKHAND_PUBLIC_URL`, `DESKHAND_ENTRA_*`, `DESKHAND_GOOGLE_*`, `DESKHAND_GITHUB_*`,
//!   `DESKHAND_SSO_AUTO_PROVISION`, `DESKHAND_PASSWORD_LOGIN`: single sign-on (docs/sso.md)

mod api;
mod auth;
mod db;
mod db_pages;
mod downloads;
mod inventory;
mod jobs;
mod jobs_ui;
mod proxy;
mod shell;
mod sso;
mod ui;
mod web;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{any, get, post};
use axum::{Json, Router};
use db::{Db, Role};
use futures_util::{SinkExt, StreamExt};
use deskhand_proto::{AgentMsg, EnrollRequest, EnrollResponse, ServerMsg, StreamKind};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{mpsc, oneshot};

pub use ui::esc;

/// A connected agent.
pub struct Agent {
    pub voxl_token: String,
    pub since: i64,
    /// Which connection this is, so a stale one going away doesn't remove its replacement.
    conn: u64,
    tx: mpsc::UnboundedSender<ServerMsg>,
}

pub struct App {
    pub db: Db,
    pub trust_proxy: bool,
    /// Set while there are no users: the token the first admin signs up with.
    pub setup_token: Mutex<Option<String>>,
    pub login_limit: auth::RateLimit,
    /// Agent binary downloads per IP (`/dl`).
    pub download_limit: auth::RateLimit,
    /// DESKHAND_PUBLIC_URL: the address agents and install scripts use.
    pub public_url: Option<String>,
    /// Agent builds for `/dl`: the agents directory, filled from the matching release.
    pub builds: downloads::Agents,
    pub sso: sso::Sso,
    /// DESKHAND_PASSWORD_LOGIN, which only takes effect with an SSO provider set up
    /// (see [`auth::password_login`]).
    pub password_login: bool,
    pub agents: Mutex<HashMap<String, Agent>>,
    /// Streams the server asked for and is waiting on, by sid, with the device they're for.
    pending: Mutex<HashMap<String, (String, oneshot::Sender<WebSocket>)>>,
    next_conn: AtomicU64,
    /// Job output and status changes, for open job pages.
    pub job_events: tokio::sync::broadcast::Sender<jobs::Event>,
    /// Devices coming and going, for live device lists (`/api/v1/events`).
    pub device_events: tokio::sync::broadcast::Sender<DeviceEvent>,
}

/// A change to a device that open device lists should show.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DeviceEvent {
    Online { id: String, since: i64 },
    Offline { id: String, last_seen: i64 },
    /// A new inventory: refetch the device.
    Inventory { id: String },
    Removed { id: String },
}

pub type Shared = Arc<App>;

pub fn random_hex(bytes: usize) -> String {
    let mut b = vec![0u8; bytes];
    getrandom::fill(&mut b).expect("random bytes");
    hex::encode(b)
}

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// `YYYY-MM-DDTHH:MM:SSZ` for Unix seconds.
pub fn iso_datetime(t: i64) -> String {
    // Howard Hinnant's days-to-civil.
    let (days, secs) = (t.div_euclid(86400), t.rem_euclid(86400));
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", secs / 3600, secs % 3600 / 60, secs % 60)
}

pub fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0, |d, (x, y)| d | (x ^ y)) == 0
}

#[tokio::main]
async fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let addr = std::env::var("DESKHAND_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".into());
    let db_path = std::env::var("DESKHAND_DB").unwrap_or_else(|_| "deskhand.db".into());
    let db = Db::open(&db_path).unwrap_or_else(|e| {
        eprintln!("can't open the database {db_path}: {e}");
        std::process::exit(2);
    });
    let mut setup_token = None;
    if db.user_count().unwrap_or(0) == 0 {
        match std::env::var("DESKHAND_ADMIN_PASSWORD").ok().filter(|p| !p.is_empty()) {
            Some(pw) => {
                db.add_user("admin", &auth::hash_password(&pw), Role::Admin, now())
                    .expect("create admin");
                log::info!("created user admin from DESKHAND_ADMIN_PASSWORD");
            }
            None => {
                let t = random_hex(16);
                let shown = addr.replace("0.0.0.0", "localhost");
                log::warn!("no users yet: create the first admin at http://{shown}/setup?token={t}");
                setup_token = Some(t);
            }
        }
    }
    let app = Arc::new(App {
        db,
        trust_proxy: std::env::var("DESKHAND_TRUST_PROXY").is_ok_and(|v| v == "1"),
        setup_token: Mutex::new(setup_token),
        login_limit: auth::RateLimit::with_max(
            std::env::var("DESKHAND_LOGIN_RATE").ok().and_then(|v| v.parse().ok()).filter(|n| *n > 0).unwrap_or(5),
        ),
        download_limit: auth::RateLimit::with_max(downloads::DOWNLOADS_PER_MINUTE),
        public_url: std::env::var("DESKHAND_PUBLIC_URL").ok().filter(|v| !v.is_empty()),
        builds: downloads::Agents::from_env(&db_path),
        sso: sso::Sso::from_env(),
        password_login: std::env::var("DESKHAND_PASSWORD_LOGIN").map_or(true, |v| v != "0"),
        agents: Mutex::default(),
        pending: Mutex::default(),
        next_conn: AtomicU64::new(1),
        job_events: tokio::sync::broadcast::channel(1024).0,
        device_events: tokio::sync::broadcast::channel(256).0,
    });
    tokio::spawn(jobs::scheduler(app.clone()));
    // The server-rendered pages, kept at /classic/ until the web app covers everything.
    let classic = Router::new()
        .route("/", get(ui::devices))
        .route("/login", get(auth::login_page).post(auth::login))
        .route("/logout", post(auth::logout))
        .route("/setup", get(auth::setup_page).post(auth::setup))
        .route("/devices.csv", get(inventory::csv))
        .route("/devices/{id}", get(inventory::detail))
        .route("/devices/{id}/refresh", post(inventory::refresh))
        .route("/devices/{id}/rename", post(ui::rename_device))
        .route("/devices/{id}/move", post(ui::move_device))
        .route("/devices/{id}/delete", post(ui::delete_device))
        .route("/groups", get(ui::groups).post(ui::add_group))
        .route("/groups/{id}/delete", post(ui::delete_group))
        .route("/enroll", get(ui::enroll).post(ui::create_token))
        .route("/enroll/{id}/revoke", post(ui::revoke_token))
        .route("/users", get(ui::users).post(ui::add_user))
        .route("/users/{id}/role", post(ui::set_role))
        .route("/users/{id}/email", post(ui::set_email))
        .route("/users/{id}/delete", post(ui::delete_user))
        .route("/audit", get(ui::audit))
        .route("/scripts", get(jobs_ui::scripts).post(jobs_ui::create_script))
        .route("/scripts/new", get(jobs_ui::new_script))
        .route("/scripts/{id}", get(jobs_ui::script).post(jobs_ui::update_script))
        .route("/scripts/{id}/delete", post(jobs_ui::delete_script))
        .route("/jobs", get(jobs_ui::jobs_list).post(jobs_ui::create_job))
        .route("/jobs/new", get(jobs_ui::new_job))
        .route("/jobs/{id}", get(jobs_ui::job))
        .route("/jobs/{id}/events", get(jobs_ui::events))
        .route("/jobs/{id}/{op}", post(jobs_ui::job_action))
        .layer(axum::middleware::from_fn(web::classic_paths));
    let router = Router::new()
        .nest("/api/v1", api::router())
        .nest("/classic", classic)
        .route("/classic/", get(|| async { Redirect::permanent("/classic") }))
        .route("/auth/{provider}/start", get(auth::sso_start))
        .route("/auth/{provider}/callback", get(auth::sso_callback))
        .route("/api/enroll", post(enroll))
        .route("/dl/{name}", get(downloads::download))
        .route("/install.sh", get(downloads::install_sh))
        .route("/install.ps1", get(downloads::install_ps1))
        .route("/agent/connect", get(agent_connect))
        .route("/agent/stream/{sid}", get(agent_stream))
        .route("/d/{id}", get(|Path(id): Path<String>| async move { Redirect::to(&format!("/d/{id}/")) }))
        .route("/d/{id}/shell", get(shell::page))
        .route("/d/{id}/shell/ws", get(shell::socket))
        .route("/d/{id}/", any(proxy::device))
        .route("/d/{id}/{*rest}", any(proxy::device))
        // Everything else is the web app: its files, else its index.html for its routes.
        .fallback(get(web::spa))
        .layer(axum::middleware::from_fn(auth::check_origin))
        .with_state(app.clone());
    let listener = tokio::net::TcpListener::bind(&addr).await.expect("bind");
    log::info!("deskhand server on http://{addr}");
    {
        let app = app.clone();
        tokio::spawn(async move {
            app.builds.prefetch().await;
            let missing: Vec<&str> = app.builds.binaries().iter().filter(|b| !b.present).map(|b| b.file).collect();
            if !missing.is_empty() {
                log::warn!(
                    "agent builds not available in {}: {} (scripts/package-agents.sh)",
                    app.builds.dir.display(),
                    missing.join(", ")
                );
            }
        });
    }
    axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>())
        .await
        .expect("serve");
}

// --- enrollment ---

/// `POST /api/enroll`: trades an enrollment token for a device id and secret.
async fn enroll(State(app): State<Shared>, Json(req): Json<EnrollRequest>) -> Response {
    let device_id = random_hex(16);
    let secret = random_hex(32);
    let hostname: String = req.hostname.chars().take(255).collect();
    let os: String = req.os.chars().take(64).collect();
    match app.db.enroll(req.token.trim(), &device_id, &db::token_hash(&secret), &hostname, &os, now()) {
        Ok(Some(())) => {
            log::info!("enrolled {hostname} as {device_id}");
            app.db.audit(now(), None, Some(&device_id), "device.enroll", &format!("{hostname} ({os})"));
            Json(EnrollResponse { device_id, secret }).into_response()
        }
        Ok(None) => (StatusCode::FORBIDDEN, "enrollment token unknown, expired or used up").into_response(),
        Err(e) => {
            log::error!("enroll: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

// --- agents ---

/// The device an agent's `Authorization: Bearer {device_id}:{secret}` proves it is.
fn agent_device(app: &App, headers: &HeaderMap) -> Option<String> {
    let (id, secret) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))?
        .trim()
        .split_once(':')?;
    let expected = app.db.device_secret_hash(id).ok().flatten()?;
    same(&db::token_hash(secret), &expected).then(|| id.to_owned())
}

async fn agent_connect(
    State(app): State<Shared>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    let Some(id) = agent_device(&app, &headers) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let ip = auth::client_ip(&app, &headers, peer).to_string();
    ws.on_upgrade(move |ws| control(app, id, ip, ws))
}

/// One agent's control socket: a Hello, then Open messages out until it closes.
async fn control(app: Shared, id: String, ip: String, ws: WebSocket) {
    let (mut wtx, mut wrx) = ws.split();
    let hello = tokio::time::timeout(Duration::from_secs(10), wrx.next()).await;
    let Ok(Some(Ok(Message::Text(t)))) = hello else {
        return;
    };
    let Ok(AgentMsg::Hello { hostname, os, voxl_token, version, .. }) = serde_json::from_str(&t) else {
        log::warn!("agent {id} sent a bad hello");
        return;
    };
    let conn = app.next_conn.fetch_add(1, Ordering::Relaxed);
    let (tx, mut rx) = mpsc::unbounded_channel();
    log::info!("agent {id} ({hostname}, {os}, v{version}) connected");
    let _ = app.db.device_seen(&id, Some(&hostname), Some(&os), now());
    let _ = app.db.set_agent_version(&id, &version);
    app.agents
        .lock()
        .unwrap()
        .insert(id.clone(), Agent { voxl_token, since: now(), conn, tx });
    let _ = app.device_events.send(DeviceEvent::Online { id: id.clone(), since: now() });
    // Runs that waited for this device to come online.
    jobs::dispatch(&app, &id);
    let mut ping = tokio::time::interval(Duration::from_secs(20));
    let mut ticks = 0u32;
    loop {
        tokio::select! {
            m = rx.recv() => {
                let Some(m) = m else { break };
                let text = serde_json::to_string(&m).unwrap();
                if wtx.send(Message::Text(text.into())).await.is_err() { break }
            }
            m = wrx.next() => match m {
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                Some(Ok(Message::Text(t))) => match serde_json::from_str::<AgentMsg>(&t) {
                    Ok(AgentMsg::Inventory { mut inventory }) => {
                        inventory.public_ip = Some(ip.clone());
                        if let Err(e) = app.db.save_inventory(&id, &inventory, now()) {
                            log::error!("saving inventory for {id}: {e}");
                        }
                        let _ = app.device_events.send(DeviceEvent::Inventory { id: id.clone() });
                    }
                    Ok(AgentMsg::JobOutput { run_id, stream, data }) => jobs::on_output(&app, &id, run_id, stream, data),
                    Ok(AgentMsg::JobResult { run_id, status, exit_code, stdout, stderr }) => {
                        jobs::on_result(&app, &id, run_id, status, exit_code, stdout, stderr)
                    }
                    Ok(AgentMsg::Hello { .. }) => {}
                    Err(e) => log::warn!("agent {id} sent something unreadable: {e}"),
                },
                _ => {}
            },
            _ = ping.tick() => {
                if wtx.send(Message::Ping(Default::default())).await.is_err() { break }
                // Last seen, every few minutes while it stays up.
                ticks += 1;
                if ticks % 15 == 0 {
                    let _ = app.db.device_seen(&id, None, None, now());
                }
            }
        }
    }
    let _ = app.db.device_seen(&id, None, None, now());
    let mut agents = app.agents.lock().unwrap();
    if agents.get(&id).is_some_and(|a| a.conn == conn) {
        agents.remove(&id);
        log::info!("agent {id} disconnected");
        let _ = app.device_events.send(DeviceEvent::Offline { id, last_seen: now() });
    }
}

async fn agent_stream(
    State(app): State<Shared>,
    Path(sid): Path<String>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    let Some(device) = agent_device(&app, &headers) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let waiter = {
        let mut pending = app.pending.lock().unwrap();
        // Only the device the stream was asked of can answer it.
        match pending.get(&sid) {
            Some((d, _)) if *d == device => pending.remove(&sid).map(|(_, w)| w),
            _ => None,
        }
    };
    let Some(waiter) = waiter else {
        return StatusCode::NOT_FOUND.into_response();
    };
    ws.on_upgrade(move |ws| async move {
        let _ = waiter.send(ws);
    })
}

/// Sends a control message to a connected agent; false when it's offline.
pub fn send_to_agent(app: &App, id: &str, msg: ServerMsg) -> bool {
    app.agents.lock().unwrap().get(id).is_some_and(|a| a.tx.send(msg).is_ok())
}

/// Asks the agent for a new stream and waits for it to connect back.
pub async fn open_stream(app: &App, id: &str, kind: StreamKind) -> Option<WebSocket> {
    let sid = random_hex(16);
    let (tx, rx) = oneshot::channel();
    app.pending.lock().unwrap().insert(sid.clone(), (id.to_owned(), tx));
    let sent = app
        .agents
        .lock()
        .unwrap()
        .get(id)
        .is_some_and(|a| a.tx.send(ServerMsg::Open { sid: sid.clone(), kind }).is_ok());
    let ws = if sent {
        tokio::time::timeout(Duration::from_secs(10), rx).await.ok().and_then(|r| r.ok())
    } else {
        None
    };
    app.pending.lock().unwrap().remove(&sid);
    ws
}
