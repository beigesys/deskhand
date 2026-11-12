//! ithub server: agents enroll with a token and dial in over WebSocket, users sign in to
//! the dashboard, and `/d/{id}/` proxies the browser to that device's voxl through the
//! agent's tunnel.
//!
//! Settings come from the environment:
//! - `ITHUB_ADDR` (default `0.0.0.0:8080`)
//! - `ITHUB_DB` (default `./ithub.db`): the SQLite database
//! - `ITHUB_ADMIN_PASSWORD` (optional): with no users yet, creates `admin` with this
//!   password; without it the server prints a one-time setup link
//! - `ITHUB_TRUST_PROXY=1`: believe X-Forwarded-Proto/-For from a TLS proxy in front
//! - `ITHUB_PUBLIC_URL`, `ITHUB_ENTRA_*`, `ITHUB_GOOGLE_*`, `ITHUB_GITHUB_*`,
//!   `ITHUB_SSO_AUTO_PROVISION`, `ITHUB_PASSWORD_LOGIN`: single sign-on (docs/sso.md)

mod auth;
mod db;
mod inventory;
mod jobs;
mod jobs_ui;
mod proxy;
mod shell;
mod sso;
mod ui;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{any, get, post};
use axum::{Json, Router};
use db::{Db, Role};
use futures_util::{SinkExt, StreamExt};
use ithub_proto::{AgentMsg, EnrollRequest, EnrollResponse, ServerMsg, StreamKind};
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
    pub sso: sso::Sso,
    /// ITHUB_PASSWORD_LOGIN, which only takes effect with an SSO provider set up
    /// (see [`auth::password_login`]).
    pub password_login: bool,
    pub agents: Mutex<HashMap<String, Agent>>,
    /// Streams the server asked for and is waiting on, by sid, with the device they're for.
    pending: Mutex<HashMap<String, (String, oneshot::Sender<WebSocket>)>>,
    next_conn: AtomicU64,
    /// Job output and status changes, for open job pages.
    pub job_events: tokio::sync::broadcast::Sender<jobs::Event>,
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
    let addr = std::env::var("ITHUB_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".into());
    let db_path = std::env::var("ITHUB_DB").unwrap_or_else(|_| "ithub.db".into());
    let db = Db::open(&db_path).unwrap_or_else(|e| {
        eprintln!("can't open the database {db_path}: {e}");
        std::process::exit(2);
    });
    let mut setup_token = None;
    if db.user_count().unwrap_or(0) == 0 {
        match std::env::var("ITHUB_ADMIN_PASSWORD").ok().filter(|p| !p.is_empty()) {
            Some(pw) => {
                db.add_user("admin", &auth::hash_password(&pw), Role::Admin, now())
                    .expect("create admin");
                log::info!("created user admin from ITHUB_ADMIN_PASSWORD");
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
        trust_proxy: std::env::var("ITHUB_TRUST_PROXY").is_ok_and(|v| v == "1"),
        setup_token: Mutex::new(setup_token),
        login_limit: Default::default(),
        sso: sso::Sso::from_env(),
        password_login: std::env::var("ITHUB_PASSWORD_LOGIN").map_or(true, |v| v != "0"),
        agents: Mutex::default(),
        pending: Mutex::default(),
        next_conn: AtomicU64::new(1),
        job_events: tokio::sync::broadcast::channel(1024).0,
    });
    tokio::spawn(jobs::scheduler(app.clone()));
    let router = Router::new()
        .route("/", get(ui::devices))
        .route("/login", get(auth::login_page).post(auth::login))
        .route("/logout", post(auth::logout))
        .route("/setup", get(auth::setup_page).post(auth::setup))
        .route("/auth/{provider}/start", get(auth::sso_start))
        .route("/auth/{provider}/callback", get(auth::sso_callback))
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
        .route("/api/enroll", post(enroll))
        .route("/agent/connect", get(agent_connect))
        .route("/agent/stream/{sid}", get(agent_stream))
        .route("/d/{id}", get(|Path(id): Path<String>| async move { Redirect::to(&format!("/d/{id}/")) }))
        .route("/d/{id}/shell", get(shell::page))
        .route("/d/{id}/shell/ws", get(shell::socket))
        .route("/d/{id}/", any(proxy::device))
        .route("/d/{id}/{*rest}", any(proxy::device))
        .layer(axum::middleware::from_fn(auth::check_origin))
        .with_state(app);
    let listener = tokio::net::TcpListener::bind(&addr).await.expect("bind");
    log::info!("ithub server on http://{addr}");
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
    let Ok(AgentMsg::Hello { hostname, os, voxl_token, .. }) = serde_json::from_str(&t) else {
        log::warn!("agent {id} sent a bad hello");
        return;
    };
    let conn = app.next_conn.fetch_add(1, Ordering::Relaxed);
    let (tx, mut rx) = mpsc::unbounded_channel();
    log::info!("agent {id} ({hostname}, {os}) connected");
    let _ = app.db.device_seen(&id, Some(&hostname), Some(&os), now());
    app.agents
        .lock()
        .unwrap()
        .insert(id.clone(), Agent { voxl_token, since: now(), conn, tx });
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
