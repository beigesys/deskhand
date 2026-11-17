//! The audit log's entries: who (user and IP), did what (an action like `user.add`), to
//! what (a target kind and id, and the device when there is one), with a short detail line
//! and structured details as JSON.
//!
//! Every request runs inside [`scope_ip`], so entries written while handling it get the
//! client's IP without each handler passing it along. Work that outlives the request (a
//! WebSocket session) passes the IP with [`Entry::ip`].

use crate::{now, Shared};
use axum::extract::{ConnectInfo, Request, State};
use axum::middleware::Next;
use axum::response::Response;
use std::net::{IpAddr, SocketAddr};

tokio::task_local! {
    static CLIENT_IP: IpAddr;
}

/// The IP of the client whose request is being handled, if any.
pub fn current_ip() -> Option<IpAddr> {
    CLIENT_IP.try_with(|ip| *ip).ok()
}

/// Middleware: runs the request with its client IP (see [`crate::auth::client_ip`]) in scope.
pub async fn scope_ip(State(app): State<Shared>, req: Request, next: Next) -> Response {
    let Some(peer) = req.extensions().get::<ConnectInfo<SocketAddr>>().map(|c| c.0) else {
        return next.run(req).await;
    };
    let ip = crate::auth::client_ip(&app, req.headers(), peer);
    CLIENT_IP.scope(ip, next.run(req)).await
}

/// `1h 2m`, `3m 4s`, `5s`: how long a session lasted.
pub fn duration(secs: i64) -> String {
    let (h, m, s) = (secs / 3600, secs % 3600 / 60, secs % 60);
    match (h, m) {
        (0, 0) => format!("{s}s"),
        (0, _) => format!("{m}m {s}s"),
        _ => format!("{h}h {m}m"),
    }
}

/// One audit entry. Build with [`Entry::new`] and the setters, write with `Db::record`.
pub struct Entry {
    pub ts: i64,
    pub action: String,
    pub user: Option<i64>,
    pub ip: Option<IpAddr>,
    pub device: Option<String>,
    pub target: Option<(&'static str, String)>,
    pub detail: String,
    pub data: Option<serde_json::Value>,
}

impl Entry {
    pub fn new(action: impl Into<String>) -> Entry {
        Entry {
            ts: now(),
            action: action.into(),
            user: None,
            ip: current_ip(),
            device: None,
            target: None,
            detail: String::new(),
            data: None,
        }
    }

    /// The user who did it.
    pub fn by(mut self, user: impl Into<Option<i64>>) -> Self {
        self.user = user.into();
        self
    }

    pub fn ip(mut self, ip: Option<IpAddr>) -> Self {
        if ip.is_some() {
            self.ip = ip;
        }
        self
    }

    pub fn at(mut self, ts: i64) -> Self {
        self.ts = ts;
        self
    }

    /// The device it happened on; also the target unless one is set.
    pub fn device(mut self, id: impl Into<String>) -> Self {
        self.device = Some(id.into());
        self
    }

    /// What it was done to: `user`, `group`, `enroll_token`, `job`, `script`, `device`.
    pub fn target(mut self, kind: &'static str, id: impl ToString) -> Self {
        self.target = Some((kind, id.to_string()));
        self
    }

    pub fn detail(mut self, d: impl Into<String>) -> Self {
        self.detail = d.into();
        self
    }

    pub fn data(mut self, v: serde_json::Value) -> Self {
        self.data = Some(v);
        self
    }
}
