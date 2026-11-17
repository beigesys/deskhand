//! `/d/{id}/shell`: a terminal on the device in the browser (xterm.js). The page's socket
//! is relayed frame for frame to a shell stream the agent opens.

use crate::auth::require;
use crate::db::Role;
use crate::audit::Entry;
use crate::{esc, now, open_stream, Shared};
use serde_json::json;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use futures_util::{SinkExt, StreamExt};
use deskhand_proto::StreamKind;

pub async fn page(State(app): State<Shared>, Path(id): Path<String>, headers: HeaderMap) -> Response {
    if let Err(r) = require(&app, &headers, Role::Operator) {
        return r;
    }
    if !app.agents.lock().unwrap().contains_key(&id) {
        return (StatusCode::NOT_FOUND, "device offline").into_response();
    }
    let host = app.db.device_name(&id).ok().flatten().unwrap_or_else(|| id.clone());
    Html(include_str!("shell.html").replace("{{host}}", &esc(&host))).into_response()
}

#[derive(serde::Deserialize)]
pub struct Size {
    cols: Option<u16>,
    rows: Option<u16>,
}

pub async fn socket(
    State(app): State<Shared>,
    Path(id): Path<String>,
    Query(size): Query<Size>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    let user = match require(&app, &headers, Role::Operator) {
        Ok(u) => u,
        // A socket can't follow a redirect: say 401 rather than sending the sign-in page.
        Err(r) if r.status() == StatusCode::FORBIDDEN => return r,
        Err(_) => return StatusCode::UNAUTHORIZED.into_response(),
    };
    if !app.agents.lock().unwrap().contains_key(&id) {
        return (StatusCode::NOT_FOUND, "device offline").into_response();
    }
    let kind = StreamKind::Shell {
        cols: size.cols.unwrap_or(80),
        rows: size.rows.unwrap_or(24),
    };
    let session = crate::random_hex(6);
    let ip = crate::audit::current_ip();
    app.db.record(Entry::new("shell.open").by(user.id).device(&id).data(json!({ "session": session })));
    ws.on_upgrade(move |browser| async move {
        match open_stream(&app, &id, kind).await {
            Some(agent) => {
                let started = now();
                relay(browser, agent).await;
                let secs = now() - started;
                app.db.record(
                    Entry::new("shell.close")
                        .by(user.id)
                        .ip(ip)
                        .device(&id)
                        .detail(crate::audit::duration(secs))
                        .data(json!({ "session": session, "duration_s": secs })),
                );
            }
            None => {
                let mut browser = browser;
                let _ = browser
                    .send(Message::Close(Some(CloseFrame {
                        code: 1011,
                        reason: "device didn't open a shell".into(),
                    })))
                    .await;
            }
        }
    })
}

/// Forwards binary and text frames both ways until either side closes.
async fn relay(browser: WebSocket, agent: WebSocket) {
    let (mut btx, mut brx) = browser.split();
    let (mut atx, mut arx) = agent.split();
    let up = async {
        while let Some(Ok(m)) = brx.next().await {
            match m {
                Message::Binary(_) | Message::Text(_) => {
                    if atx.send(m).await.is_err() {
                        break;
                    }
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
        let _ = atx.close().await;
    };
    let down = async {
        while let Some(Ok(m)) = arx.next().await {
            match m {
                Message::Binary(_) | Message::Text(_) => {
                    if btx.send(m).await.is_err() {
                        break;
                    }
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
        let _ = btx.close().await;
    };
    // Whichever side ends first ends the session.
    tokio::select! {
        _ = up => {}
        _ = down => {}
    }
}
