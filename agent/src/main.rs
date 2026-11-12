//! deskhand agent: runs voxl on a loopback port and dials out to the deskhand server, which
//! reaches that voxl through streams the agent opens on request.
//!
//! - `deskhand-agent enroll --server URL --token T`: trades a dashboard enrollment token for
//!   this device's id and secret, saved to `agent.json` in the config dir.
//! - `deskhand-agent [run]`: connects with that identity. `--server` overrides the saved
//!   server URL; `--config` / `DESKHAND_CONFIG` picks another file (several agents on one
//!   machine while developing). Arguments after `--` go to voxl as they are.

mod inventory;
mod jobs;
mod shell;

use futures_util::{SinkExt, StreamExt};
use deskhand_proto::{AgentMsg, EnrollRequest, EnrollResponse, ServerMsg, StreamKind};
use std::path::PathBuf;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message;

pub fn random_hex(bytes: usize) -> String {
    let mut b = vec![0u8; bytes];
    getrandom::fill(&mut b).expect("random bytes");
    hex::encode(b)
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// `YYYY-MM-DD` (UTC) for Unix seconds.
pub fn iso_date(t: i64) -> String {
    // Howard Hinnant's days-to-civil.
    let z = t.div_euclid(86400) + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
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

/// What enrolling saves: where the server is and who this device is to it.
#[derive(serde::Serialize, serde::Deserialize)]
struct Identity {
    server: String,
    device_id: String,
    secret: String,
}

fn default_config() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("deskhand")
        .join("agent.json")
}

fn load(path: &PathBuf) -> Result<Identity, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// Writes the identity readable by its owner only: the secret is the device's password.
fn save(path: &PathBuf, id: &Identity) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let text = serde_json::to_string_pretty(id).unwrap();
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    let mut f = opts.open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = f.set_permissions(std::fs::Permissions::from_mode(0o600));
    }
    std::io::Write::write_all(&mut f, text.as_bytes()).map_err(|e| format!("{}: {e}", path.display()))
}

/// http(s) for the enroll call; the sockets use ws(s).
fn http_base(server: &str) -> String {
    server
        .trim_end_matches('/')
        .replacen("wss://", "https://", 1)
        .replacen("ws://", "http://", 1)
}

fn ws_base(server: &str) -> String {
    server
        .trim_end_matches('/')
        .replacen("https://", "wss://", 1)
        .replacen("http://", "ws://", 1)
}

fn enroll(server: &str, token: &str, path: &PathBuf) -> Result<(), String> {
    let server = http_base(server);
    let req = EnrollRequest {
        token: token.trim().to_owned(),
        hostname: hostname(),
        os: std::env::consts::OS.into(),
    };
    let mut resp = ureq::post(&format!("{server}/api/enroll"))
        .config()
        .http_status_as_error(false)
        .build()
        .send_json(&req)
        .map_err(|e| format!("{server}: {e}"))?;
    if !resp.status().is_success() {
        let body = resp.body_mut().read_to_string().unwrap_or_default();
        return Err(format!("server said {}: {body}", resp.status()));
    }
    let r: EnrollResponse = resp.body_mut().read_json().map_err(|e| e.to_string())?;
    save(path, &Identity { server, device_id: r.device_id.clone(), secret: r.secret })?;
    println!("enrolled as {}; identity saved to {}", r.device_id, path.display());
    Ok(())
}

struct Config {
    server: String,
    device_id: String,
    secret: String,
    voxl_port: u16,
    voxl_token: String,
}

const USAGE: &str = "usage:
  deskhand-agent enroll --server URL --token TOKEN [--config FILE]
  deskhand-agent [run] [--server URL] [--config FILE] [-- voxl flags]";

fn fail(msg: &str) -> ! {
    eprintln!("{msg}");
    std::process::exit(2);
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let mut server = std::env::var("DESKHAND_SERVER").ok();
    let mut config = std::env::var("DESKHAND_CONFIG").ok().map(PathBuf::from);
    let mut token = None;
    let mut voxl_args = Vec::new();
    let mut args = std::env::args().skip(1).peekable();
    let command = match args.peek().map(String::as_str) {
        Some("enroll") | Some("run") => args.next().unwrap(),
        _ => "run".into(),
    };
    while let Some(a) = args.next() {
        let (flag, inline) = match a.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f.to_owned(), Some(v.to_owned())),
            _ => (a.clone(), None),
        };
        let mut value = || inline.clone().or_else(|| args.next()).unwrap_or_else(|| fail(USAGE));
        match flag.as_str() {
            "--" => voxl_args.extend(args.by_ref()),
            "--server" => server = Some(value()),
            "--config" => config = Some(PathBuf::from(value())),
            "--token" => token = Some(value()),
            "-h" | "--help" => {
                println!("{USAGE}");
                return;
            }
            _ => fail(&format!("unknown argument {a:?}; voxl's flags go after --\n{USAGE}")),
        }
    }
    let config = config.unwrap_or_else(default_config);

    if command == "enroll" {
        let (Some(server), Some(token)) = (server, token) else {
            fail(USAGE);
        };
        if let Err(e) = enroll(&server, &token, &config) {
            fail(&format!("enroll failed: {e}"));
        }
        return;
    }

    let identity = load(&config).unwrap_or_else(|e| {
        fail(&format!("not enrolled ({e}); run `deskhand-agent enroll --server URL --token TOKEN` first"))
    });
    // A free loopback port for voxl: bind, read it back, release.
    let voxl_port = std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .expect("free port")
        .port();
    let cfg = Config {
        server: ws_base(server.as_deref().unwrap_or(&identity.server)),
        device_id: identity.device_id,
        secret: identity.secret,
        voxl_port,
        voxl_token: random_hex(24),
    };
    let mut voxl = vec![
        "--addr=127.0.0.1".to_owned(),
        format!("--port={voxl_port}"),
        format!("--basic-auth-password={}", cfg.voxl_token),
    ];
    voxl.extend(voxl_args);
    log::info!("agent {} -> {}", cfg.device_id, cfg.server);

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
        HeaderValue::from_str(&format!("Bearer {}:{}", cfg.device_id, cfg.secret)).expect("device secret"),
    );
    req
}

/// Messages for the server that wait out a reconnect: job output and results.
struct Outbox {
    rx: tokio::sync::mpsc::UnboundedReceiver<AgentMsg>,
    /// Taken from `rx` but not sent because the socket went down.
    retry: Option<AgentMsg>,
}

/// Keeps the control socket up, reconnecting with backoff.
async fn tunnel(cfg: Config) {
    let cfg = std::sync::Arc::new(cfg);
    let (out_tx, out_rx) = tokio::sync::mpsc::unbounded_channel();
    let runner = jobs::Runner::new(out_tx);
    let mut outbox = Outbox { rx: out_rx, retry: None };
    let mut backoff = Duration::from_secs(1);
    loop {
        match control(&cfg, &runner, &mut outbox).await {
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

async fn control(cfg: &std::sync::Arc<Config>, runner: &jobs::Runner, outbox: &mut Outbox) -> Result<(), String> {
    let (ws, _) = tokio_tungstenite::connect_async(request(cfg, "/agent/connect"))
        .await
        .map_err(|e| e.to_string())?;
    let (mut tx, mut rx) = ws.split();
    let hello = AgentMsg::Hello {
        id: cfg.device_id.clone(),
        hostname: hostname(),
        os: std::env::consts::OS.into(),
        voxl_token: cfg.voxl_token.clone(),
    };
    tx.send(Message::Text(serde_json::to_string(&hello).unwrap().into()))
        .await
        .map_err(|e| e.to_string())?;
    log::info!("connected to {}", cfg.server);
    // Inventory: once now, every few hours, and when the server asks.
    let (inv_tx, mut inv_rx) = tokio::sync::mpsc::channel::<deskhand_proto::Inventory>(1);
    let mut inv_timer = tokio::time::interval(INVENTORY_EVERY);
    let mut collecting = false;
    if let Some(m) = outbox.retry.take() {
        send(&mut tx, outbox, m).await?;
    }
    loop {
        let m = tokio::select! {
            Some(out) = outbox.rx.recv() => {
                send(&mut tx, outbox, out).await?;
                continue;
            }
            m = rx.next() => match m {
                Some(m) => m,
                None => break,
            },
            _ = inv_timer.tick() => {
                start_inventory(&inv_tx, &mut collecting);
                continue;
            }
            Some(inv) = inv_rx.recv() => {
                collecting = false;
                let msg = AgentMsg::Inventory { inventory: Box::new(inv) };
                tx.send(Message::Text(serde_json::to_string(&msg).unwrap().into()))
                    .await
                    .map_err(|e| e.to_string())?;
                log::info!("sent inventory");
                continue;
            }
        };
        match m.map_err(|e| e.to_string())? {
            Message::Text(t) => match serde_json::from_str(&t) {
                Ok(ServerMsg::RefreshInventory) => start_inventory(&inv_tx, &mut collecting),
                Ok(ServerMsg::RunJob { run_id, shell, body, timeout_s }) => {
                    log::info!("job run {run_id}: {} script, timeout {timeout_s}s", shell.as_str());
                    runner.run_script(run_id, shell, body, timeout_s);
                }
                Ok(ServerMsg::RunAction { run_id, action }) => {
                    log::info!("job run {run_id}: {}", action.name());
                    runner.run_action(run_id, action);
                }
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

/// Sends a job message; one that can't go out is kept for the next connection.
async fn send<S>(tx: &mut S, outbox: &mut Outbox, m: AgentMsg) -> Result<(), String>
where
    S: futures_util::Sink<Message> + Unpin,
    S::Error: std::fmt::Display,
{
    let text = serde_json::to_string(&m).unwrap();
    if let Err(e) = tx.send(Message::Text(text.into())).await {
        outbox.retry = Some(m);
        return Err(e.to_string());
    }
    Ok(())
}

const INVENTORY_EVERY: Duration = Duration::from_secs(4 * 3600);

/// Starts a collection unless one is already running; the result arrives on `tx`.
fn start_inventory(tx: &tokio::sync::mpsc::Sender<deskhand_proto::Inventory>, collecting: &mut bool) {
    if std::mem::replace(collecting, true) {
        return;
    }
    let tx = tx.clone();
    tokio::spawn(async move {
        let _ = tx.send(inventory::collect().await).await;
    });
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
