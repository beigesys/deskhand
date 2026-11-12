//! ithub agent: runs voxl on a loopback port and dials out to the ithub server, which
//! reaches that voxl through streams the agent opens on request.
//!
//! Settings: `--server` / `ITHUB_SERVER` (the server's base URL, ws:// or wss://, http(s)://
//! also accepted) and `--key` / `ITHUB_AGENT_KEY` (the server's agent key). Arguments after
//! `--` go to voxl as they are.

mod shell;

use futures_util::{SinkExt, StreamExt};
use ithub_proto::{AgentMsg, ServerMsg, StreamKind};
use std::path::PathBuf;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message;

fn random_hex(bytes: usize) -> String {
    let mut b = vec![0u8; bytes];
    getrandom::fill(&mut b).expect("random bytes");
    hex::encode(b)
}

/// The id this machine keeps across restarts, in the config dir.
fn agent_id() -> String {
    let path = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("ithub")
        .join("agent-id");
    if let Ok(id) = std::fs::read_to_string(&path) {
        let id = id.trim();
        if !id.is_empty() {
            return id.to_owned();
        }
    }
    let id = random_hex(16);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = std::fs::write(&path, &id) {
        log::warn!("couldn't save the agent id to {}: {e}", path.display());
    }
    id
}

fn hostname() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .or_else(|| std::env::var("COMPUTERNAME").ok())
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok())
        .map(|h| h.trim().to_owned())
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "unknown".into())
}

struct Config {
    server: String,
    key: String,
    id: String,
    voxl_port: u16,
    voxl_token: String,
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let mut server = std::env::var("ITHUB_SERVER").ok();
    let mut key = std::env::var("ITHUB_AGENT_KEY").ok();
    let mut voxl_args = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--" {
            voxl_args.extend(args.by_ref());
        } else if let Some(v) = a.strip_prefix("--server=") {
            server = Some(v.into());
        } else if a == "--server" {
            server = args.next();
        } else if let Some(v) = a.strip_prefix("--key=") {
            key = Some(v.into());
        } else if a == "--key" {
            key = args.next();
        } else {
            eprintln!("unknown argument {a:?}; voxl's flags go after --");
            std::process::exit(2);
        }
    }
    let (Some(server), Some(key)) = (server, key) else {
        eprintln!("usage: ithub-agent --server wss://host --key KEY [-- voxl flags]");
        eprintln!("(or set ITHUB_SERVER and ITHUB_AGENT_KEY)");
        std::process::exit(2);
    };
    let server = server
        .trim_end_matches('/')
        .replacen("https://", "wss://", 1)
        .replacen("http://", "ws://", 1);

    // A free loopback port for voxl: bind, read it back, release.
    let voxl_port = std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .expect("free port")
        .port();
    let cfg = Config {
        server,
        key,
        id: agent_id(),
        voxl_port,
        voxl_token: random_hex(24),
    };
    let mut voxl = vec![
        "--addr=127.0.0.1".to_owned(),
        format!("--port={voxl_port}"),
        format!("--basic-auth-password={}", cfg.voxl_token),
    ];
    voxl.extend(voxl_args);
    log::info!("agent {} -> {}", cfg.id, cfg.server);

    std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime")
            .block_on(tunnel(cfg));
    });
    // voxl takes over this thread (macOS needs it to be the main one).
    voxl::run(voxl);
}

fn request(cfg: &Config, path: &str) -> tokio_tungstenite::tungstenite::handshake::client::Request {
    let mut req = format!("{}{path}", cfg.server)
        .into_client_request()
        .expect("server URL");
    req.headers_mut().insert(
        "authorization",
        HeaderValue::from_str(&format!("Bearer {}", cfg.key)).expect("agent key"),
    );
    req
}

/// Keeps the control socket up, reconnecting with backoff.
async fn tunnel(cfg: Config) {
    let cfg = std::sync::Arc::new(cfg);
    let mut backoff = Duration::from_secs(1);
    loop {
        match control(&cfg).await {
            Ok(()) => {
                log::warn!("server closed the connection");
                backoff = Duration::from_secs(1);
            }
            Err(e) => log::warn!("server connection: {e}"),
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_secs(30));
    }
}

async fn control(cfg: &std::sync::Arc<Config>) -> Result<(), String> {
    let (ws, _) = tokio_tungstenite::connect_async(request(cfg, "/agent/connect"))
        .await
        .map_err(|e| e.to_string())?;
    let (mut tx, mut rx) = ws.split();
    let hello = AgentMsg::Hello {
        id: cfg.id.clone(),
        hostname: hostname(),
        os: std::env::consts::OS.into(),
        voxl_token: cfg.voxl_token.clone(),
    };
    tx.send(Message::Text(serde_json::to_string(&hello).unwrap().into()))
        .await
        .map_err(|e| e.to_string())?;
    log::info!("connected to {}", cfg.server);
    while let Some(m) = rx.next().await {
        match m.map_err(|e| e.to_string())? {
            Message::Text(t) => match serde_json::from_str(&t) {
                Ok(ServerMsg::Open { sid, kind }) => {
                    let cfg = cfg.clone();
                    tokio::spawn(async move {
                        let r = match kind {
                            StreamKind::Desktop => stream(&cfg, &sid).await,
                            StreamKind::Shell { cols, rows } => {
                                match open(&cfg, &sid).await {
                                    Ok(ws) => shell::run(ws, cols, rows).await,
                                    Err(e) => Err(e),
                                }
                            }
                        };
                        if let Err(e) = r {
                            log::warn!("stream {sid}: {e}");
                        }
                    });
                }
                Err(e) => log::warn!("unknown message from server: {e}"),
            },
            Message::Close(_) => break,
            _ => {}
        }
    }
    Ok(())
}

/// Dials the server's socket for this sid.
async fn open(
    cfg: &Config,
    sid: &str,
) -> Result<tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>, String> {
    let (ws, _) = tokio_tungstenite::connect_async(request(cfg, &format!("/agent/stream/{sid}")))
        .await
        .map_err(|e| e.to_string())?;
    Ok(ws)
}

/// One desktop stream: the server's socket for this sid, piped to a new connection to voxl.
async fn stream(cfg: &Config, sid: &str) -> Result<(), String> {
    let tcp = TcpStream::connect(("127.0.0.1", cfg.voxl_port))
        .await
        .map_err(|e| format!("voxl: {e}"))?;
    tcp.set_nodelay(true).ok();
    let ws = open(cfg, sid).await?;
    let (mut wtx, mut wrx) = ws.split();
    let (mut r, mut w) = tcp.into_split();
    let out = async {
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = match r.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            if wtx.send(Message::Binary(buf[..n].to_vec().into())).await.is_err() {
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
    Ok(())
}
