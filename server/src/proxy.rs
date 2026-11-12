//! `/d/{id}/...`: forwards a browser request, WebSocket upgrades included, to the device's
//! voxl over a fresh agent stream. voxl's page uses relative URLs, so it works under the
//! `/d/{id}/` prefix unchanged.

use crate::auth::require;
use crate::db::Role;
use crate::{now, open_stream, Shared};
use ithub_proto::StreamKind;
use axum::body::{Body, Bytes};
use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use futures_util::{SinkExt, StreamExt};
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};

pub async fn device(State(app): State<Shared>, mut req: Request) -> Response {
    let user = match require(&app, req.headers(), Role::Operator) {
        Ok(u) => u,
        Err(r) => return r,
    };
    // /d/{id}/{rest}: the id and what follows it.
    let path = req.uri().path().to_owned();
    let Some((id, rest)) = path
        .strip_prefix("/d/")
        .and_then(|p| p.split_once('/'))
        .map(|(id, rest)| (id.to_owned(), rest.to_owned()))
    else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(token) = app.agents.lock().unwrap().get(&id).map(|a| a.voxl_token.clone()) else {
        return (StatusCode::NOT_FOUND, "device offline").into_response();
    };
    // The page's video socket marks a desktop session; its other requests are assets.
    if rest == "ws" {
        app.db.audit(now(), Some(user.id), Some(&id), "desktop.open", "");
    }
    let Some(ws) = open_stream(&app, &id, StreamKind::Desktop).await else {
        return (StatusCode::BAD_GATEWAY, "device didn't open a stream").into_response();
    };
    let (io, far) = tokio::io::duplex(256 * 1024);
    tokio::spawn(pump(ws, far));

    let upgrade = req.headers().contains_key(header::UPGRADE);
    let from_browser = upgrade.then(|| hyper::upgrade::on(&mut req));
    let (parts, body) = req.into_parts();
    let uri = match parts.uri.query() {
        Some(q) => format!("/{rest}?{q}"),
        None => format!("/{rest}"),
    };
    let mut up = hyper::Request::builder().method(parts.method).uri(uri);
    for (k, v) in parts.headers.iter() {
        // voxl checks Host and Origin against DNS rebinding; as a non-browser client
        // with a loopback Host, the proxy passes. Its own login replaces the cookie.
        if k == header::HOST || k == header::ORIGIN || k == header::COOKIE || k == header::AUTHORIZATION {
            continue;
        }
        up = up.header(k, v);
    }
    let up = up
        .header(header::HOST, HeaderValue::from_static("127.0.0.1"))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(if upgrade { Body::empty() } else { body });
    let Ok(up) = up else {
        return StatusCode::BAD_REQUEST.into_response();
    };

    let (mut sender, conn) = match hyper::client::conn::http1::handshake(TokioIo::new(io)).await {
        Ok(x) => x,
        Err(e) => return (StatusCode::BAD_GATEWAY, e.to_string()).into_response(),
    };
    tokio::spawn(conn.with_upgrades());
    let mut resp = match sender.send_request(up).await {
        Ok(r) => r,
        Err(e) => return (StatusCode::BAD_GATEWAY, e.to_string()).into_response(),
    };
    if resp.status() == StatusCode::SWITCHING_PROTOCOLS {
        let from_voxl = hyper::upgrade::on(&mut resp);
        if let Some(from_browser) = from_browser {
            tokio::spawn(async move {
                let (Ok(b), Ok(v)) = tokio::join!(from_browser, from_voxl) else {
                    return;
                };
                let _ = tokio::io::copy_bidirectional(&mut TokioIo::new(b), &mut TokioIo::new(v)).await;
            });
        }
    }
    let (parts, body) = resp.into_parts();
    Response::from_parts(parts, Body::new(body))
}

/// Pipes the agent's stream socket to one end of a duplex pipe, binary frames as bytes.
async fn pump(ws: WebSocket, io: DuplexStream) {
    let (mut wtx, mut wrx) = ws.split();
    let (mut r, mut w) = tokio::io::split(io);
    let out = async {
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = match r.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            if wtx.send(Message::Binary(Bytes::copy_from_slice(&buf[..n]))).await.is_err() {
                break;
            }
        }
        let _ = wtx.close().await;
    };
    let inn = async {
        while let Some(Ok(m)) = wrx.next().await {
            match m {
                Message::Binary(b) => {
                    if w.write_all(&b).await.is_err() {
                        break;
                    }
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
        let _ = w.shutdown().await;
    };
    tokio::join!(out, inn);
}
