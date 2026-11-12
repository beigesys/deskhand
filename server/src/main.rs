//! ithub server: agents dial in over WebSocket, admins sign in to a dashboard, and
//! `/d/{id}/` proxies the browser to that device's voxl through the agent's tunnel.
//!
//! Settings come from the environment:
//! - `ITHUB_ADDR` (default `0.0.0.0:8080`)
//! - `ITHUB_ADMIN_PASSWORD` (required): the dashboard password
//! - `ITHUB_AGENT_KEY` (required): the shared secret agents present

mod proxy;
mod shell;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{any, get, post};
use axum::{Form, Router};
use futures_util::{SinkExt, StreamExt};
use ithub_proto::{AgentMsg, ServerMsg, StreamKind};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{mpsc, oneshot};

pub struct Agent {
    pub hostname: String,
    pub os: String,
    pub voxl_token: String,
    pub since: u64,
    /// Which connection this is, so a stale one going away doesn't remove its replacement.
    conn: u64,
    tx: mpsc::UnboundedSender<ServerMsg>,
}

pub struct App {
    admin_password: String,
    agent_key: String,
    pub agents: Mutex<HashMap<String, Agent>>,
    /// Streams the server asked for and is waiting on, by sid.
    pending: Mutex<HashMap<String, oneshot::Sender<WebSocket>>>,
    sessions: Mutex<HashSet<String>>,
    next_conn: AtomicU64,
}

type Shared = Arc<App>;

pub fn random_hex(bytes: usize) -> String {
    let mut b = vec![0u8; bytes];
    getrandom::fill(&mut b).expect("random bytes");
    hex::encode(b)
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn env_required(name: &str) -> String {
    match std::env::var(name) {
        Ok(v) if !v.is_empty() => v,
        _ => {
            eprintln!("{name} must be set");
            std::process::exit(2);
        }
    }
}

#[tokio::main]
async fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let addr = std::env::var("ITHUB_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".into());
    let app = Arc::new(App {
        admin_password: env_required("ITHUB_ADMIN_PASSWORD"),
        agent_key: env_required("ITHUB_AGENT_KEY"),
        agents: Mutex::default(),
        pending: Mutex::default(),
        sessions: Mutex::default(),
        next_conn: AtomicU64::new(1),
    });
    let router = Router::new()
        .route("/", get(dashboard))
        .route("/login", get(login_page).post(login))
        .route("/logout", post(logout))
        .route("/agent/connect", get(agent_connect))
        .route("/agent/stream/{sid}", get(agent_stream))
        .route("/d/{id}", get(|Path(id): Path<String>| async move { Redirect::to(&format!("/d/{id}/")) }))
        .route("/d/{id}/shell", get(shell::page))
        .route("/d/{id}/shell/ws", get(shell::socket))
        .route("/d/{id}/", any(proxy::device))
        .route("/d/{id}/{*rest}", any(proxy::device))
        .with_state(app);
    let listener = tokio::net::TcpListener::bind(&addr).await.expect("bind");
    log::info!("ithub server on http://{addr}");
    axum::serve(listener, router).await.expect("serve");
}

// --- admin sessions ---

const COOKIE: &str = "ithub_session";

pub fn signed_in(app: &App, headers: &HeaderMap) -> bool {
    let sessions = app.sessions.lock().unwrap();
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|c| c.trim().split_once('='))
        .any(|(k, v)| k == COOKIE && sessions.contains(v))
}

fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0, |d, (x, y)| d | (x ^ y)) == 0
}

async fn login_page() -> Html<&'static str> {
    Html(include_str!("login.html"))
}

#[derive(serde::Deserialize)]
struct LoginForm {
    password: String,
}

async fn login(State(app): State<Shared>, Form(f): Form<LoginForm>) -> Response {
    if !same(&f.password, &app.admin_password) {
        return Redirect::to("/login?bad").into_response();
    }
    let sid = random_hex(32);
    app.sessions.lock().unwrap().insert(sid.clone());
    (
        [(header::SET_COOKIE, format!("{COOKIE}={sid}; Path=/; HttpOnly; SameSite=Lax"))],
        Redirect::to("/"),
    )
        .into_response()
}

async fn logout(State(app): State<Shared>, headers: HeaderMap) -> Response {
    if let Some(v) = headers.get(header::COOKIE).and_then(|v| v.to_str().ok()) {
        for (k, v) in v.split(';').filter_map(|c| c.trim().split_once('=')) {
            if k == COOKIE {
                app.sessions.lock().unwrap().remove(v);
            }
        }
    }
    (
        [(header::SET_COOKIE, format!("{COOKIE}=; Path=/; Max-Age=0"))],
        Redirect::to("/login"),
    )
        .into_response()
}

// --- dashboard ---

pub fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

async fn dashboard(State(app): State<Shared>, headers: HeaderMap) -> Response {
    if !signed_in(&app, &headers) {
        return Redirect::to("/login").into_response();
    }
    let mut rows: Vec<(String, String, String, u64)> = app
        .agents
        .lock()
        .unwrap()
        .iter()
        .map(|(id, a)| (id.clone(), a.hostname.clone(), a.os.clone(), a.since))
        .collect();
    rows.sort_by(|a, b| a.1.cmp(&b.1));
    let now = now();
    let body: String = if rows.is_empty() {
        r#"<p class="empty">No devices online. Start an agent pointed at this server.</p>"#.into()
    } else {
        rows.iter()
            .map(|(id, host, os, since)| {
                format!(
                    r#"<div class="dev"><span class="dot"></span><b>{host}</b><span>{os}</span><span>online {up}</span><span class="acts"><a href="/d/{id}/">Desktop</a><a href="/d/{id}/shell">Shell</a></span></div>"#,
                    id = esc(id),
                    host = esc(host),
                    os = esc(os),
                    up = ago(now.saturating_sub(*since)),
                )
            })
            .collect()
    };
    Html(include_str!("dashboard.html").replace("{{devices}}", &body)).into_response()
}

fn ago(s: u64) -> String {
    match s {
        0..=59 => format!("{s}s"),
        60..=3599 => format!("{}m", s / 60),
        _ => format!("{}h", s / 3600),
    }
}

// --- agents ---

fn agent_authorized(app: &App, headers: &HeaderMap) -> bool {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|k| same(k.trim(), &app.agent_key))
}

async fn agent_connect(
    State(app): State<Shared>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    if !agent_authorized(&app, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    ws.on_upgrade(move |ws| control(app, ws))
}

/// One agent's control socket: a Hello, then Open messages out until it closes.
async fn control(app: Shared, ws: WebSocket) {
    let (mut wtx, mut wrx) = ws.split();
    let hello = tokio::time::timeout(Duration::from_secs(10), wrx.next()).await;
    let Ok(Some(Ok(Message::Text(t)))) = hello else {
        return;
    };
    let Ok(AgentMsg::Hello { id, hostname, os, voxl_token }) = serde_json::from_str(&t) else {
        log::warn!("agent sent a bad hello");
        return;
    };
    let conn = app.next_conn.fetch_add(1, Ordering::Relaxed);
    let (tx, mut rx) = mpsc::unbounded_channel();
    log::info!("agent {id} ({hostname}, {os}) connected");
    app.agents.lock().unwrap().insert(
        id.clone(),
        Agent { hostname, os, voxl_token, since: now(), conn, tx },
    );
    let mut ping = tokio::time::interval(Duration::from_secs(20));
    loop {
        tokio::select! {
            m = rx.recv() => {
                let Some(m) = m else { break };
                let text = serde_json::to_string(&m).unwrap();
                if wtx.send(Message::Text(text.into())).await.is_err() { break }
            }
            m = wrx.next() => match m {
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                _ => {}
            },
            _ = ping.tick() => {
                if wtx.send(Message::Ping(Default::default())).await.is_err() { break }
            }
        }
    }
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
    if !agent_authorized(&app, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Some(waiter) = app.pending.lock().unwrap().remove(&sid) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    ws.on_upgrade(move |ws| async move {
        let _ = waiter.send(ws);
    })
}

/// Asks the agent for a new stream and waits for it to connect back.
pub async fn open_stream(app: &App, id: &str, kind: StreamKind) -> Option<WebSocket> {
    let sid = random_hex(16);
    let (tx, rx) = oneshot::channel();
    app.pending.lock().unwrap().insert(sid.clone(), tx);
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
