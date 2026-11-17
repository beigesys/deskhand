//! Agent downloads and install scripts, served by this server so a self-hosted Deskhand
//! never depends on anything outside it.
//!
//! - `/dl/{file}` hands out agent binaries from `DESKHAND_AGENT_DIR` (default `agents/`
//!   next to the database). Only the names in [`PLATFORMS`] are served, so a request
//!   can't reach any other file. No sign-in: a binary is useless without an enroll token.
//! - A build missing from that directory is fetched from the GitHub Release that matches
//!   this server's own version (`v{CARGO_PKG_VERSION}`), checked against the release's
//!   SHA256SUMS, and cached there, so agents always speak this server's protocol. Agents
//!   themselves only ever download from this server. `DESKHAND_AGENT_SOURCE=local` turns
//!   fetching off; `DESKHAND_AGENT_REPO` (default `beigesys/deskhand`) points at a fork.
//! - `/install.sh` and `/install.ps1` are generated with this server's own URL in them.

use crate::auth::client_ip;
use crate::Shared;
use axum::extract::{ConnectInfo, Path, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path as FsPath, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, UNIX_EPOCH};

/// The agent builds the server can hand out: (platform label, file name).
pub const PLATFORMS: &[(&str, &str)] = &[
    ("Linux x86_64", "deskhand-agent-linux-x86_64"),
    ("Linux ARM64", "deskhand-agent-linux-aarch64"),
    ("Windows x86_64", "deskhand-agent-windows-x86_64.exe"),
    ("macOS Apple silicon", "deskhand-agent-macos-aarch64"),
];

/// Downloads per IP per minute.
pub const DOWNLOADS_PER_MINUTE: usize = 30;

/// Where agent binaries live: DESKHAND_AGENT_DIR, else `agents/` beside the database.
pub fn agent_dir(db_path: &str) -> PathBuf {
    match std::env::var("DESKHAND_AGENT_DIR").ok().filter(|v| !v.is_empty()) {
        Some(d) => PathBuf::from(d),
        None => FsPath::new(db_path)
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(FsPath::new("."))
            .join("agents"),
    }
}

pub const DEFAULT_REPO: &str = "beigesys/deskhand";
/// The largest agent build we'll download.
const MAX_BINARY: usize = 256 << 20;
/// After a failed fetch, wait this long before asking GitHub again.
const RETRY_AFTER: Duration = Duration::from_secs(300);

/// Where missing agent builds come from.
pub enum Source {
    /// Release assets under this URL (`.../releases/download/vX.Y.Z`).
    Release(String),
    /// Only what's already in the agents directory.
    Local,
}

enum Fetch {
    Running,
    Done,
    Failed(String, Instant),
}

/// The agents directory and how its gaps get filled.
pub struct Agents {
    pub dir: PathBuf,
    pub source: Source,
    fetches: Mutex<HashMap<&'static str, Fetch>>,
    /// One fetch at a time, so concurrent downloads of a missing build fetch it once.
    lock: tokio::sync::Mutex<()>,
    client: reqwest::Client,
}

impl Agents {
    /// From DESKHAND_AGENT_DIR, DESKHAND_AGENT_SOURCE, DESKHAND_AGENT_REPO and
    /// DESKHAND_AGENT_RELEASE_URL (a full release URL, mainly for tests).
    pub fn from_env(db_path: &str) -> Agents {
        let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        let source = match env("DESKHAND_AGENT_SOURCE").as_deref() {
            Some("local") => Source::Local,
            Some("github") | None => Source::Release(env("DESKHAND_AGENT_RELEASE_URL").unwrap_or_else(|| {
                let repo = env("DESKHAND_AGENT_REPO").unwrap_or_else(|| DEFAULT_REPO.into());
                format!("https://github.com/{repo}/releases/download/v{}", env!("CARGO_PKG_VERSION"))
            })),
            Some(other) => {
                log::error!("DESKHAND_AGENT_SOURCE={other:?} isn't github or local; using local");
                Source::Local
            }
        };
        Agents::new(agent_dir(db_path), source)
    }

    pub fn new(dir: PathBuf, source: Source) -> Agents {
        let client = reqwest::Client::builder()
            .user_agent(concat!("deskhand-server/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(600))
            .build()
            .expect("http client");
        Agents { dir, source, fetches: Mutex::default(), lock: Default::default(), client }
    }

    fn has(&self, file: &str) -> bool {
        self.dir.join(file).is_file()
    }

    /// Makes sure `file` is in the agents directory, fetching it if it's missing and the
    /// source allows. `Err` says why it isn't available.
    pub async fn ensure(&self, file: &'static str) -> Result<(), String> {
        if self.has(file) {
            return Ok(());
        }
        let Source::Release(base) = &self.source else {
            return Err("this server only serves agent builds from its agents directory (DESKHAND_AGENT_SOURCE=local)".into());
        };
        if let Some(Fetch::Failed(why, at)) = self.fetches.lock().unwrap().get(file) {
            if at.elapsed() < RETRY_AFTER {
                return Err(why.clone());
            }
        }
        let _one = self.lock.lock().await;
        if self.has(file) {
            return Ok(());
        }
        self.fetches.lock().unwrap().insert(file, Fetch::Running);
        let r = self.fetch(base, file).await;
        let state = match &r {
            Ok(()) => Fetch::Done,
            Err(e) => Fetch::Failed(e.clone(), Instant::now()),
        };
        self.fetches.lock().unwrap().insert(file, state);
        match &r {
            Ok(()) => log::info!("fetched {file} from {base}"),
            Err(e) => log::warn!("couldn't fetch {file}: {e}"),
        }
        r
    }

    async fn fetch(&self, base: &str, file: &str) -> Result<(), String> {
        let sums_url = format!("{base}/SHA256SUMS");
        let sums = self.get(&sums_url).await?;
        let sums = String::from_utf8_lossy(&sums);
        let want = sums
            .lines()
            .filter_map(|l| l.split_once(char::is_whitespace))
            .find(|(_, name)| name.trim().trim_start_matches('*') == file)
            .map(|(hash, _)| hash.trim().to_ascii_lowercase())
            .ok_or_else(|| format!("{sums_url} doesn't list {file}"))?;
        let body = self.get(&format!("{base}/{file}")).await?;
        let got = hex::encode(Sha256::digest(&body));
        if got != want {
            return Err(format!("{file} from {base} doesn't match SHA256SUMS (got {got}, want {want})"));
        }
        tokio::fs::create_dir_all(&self.dir).await.map_err(|e| format!("can't create {}: {e}", self.dir.display()))?;
        let part = self.dir.join(format!(".{file}.part"));
        tokio::fs::write(&part, &body).await.map_err(|e| format!("can't write {}: {e}", part.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = tokio::fs::set_permissions(&part, std::fs::Permissions::from_mode(0o755)).await;
        }
        tokio::fs::rename(&part, self.dir.join(file)).await.map_err(|e| format!("can't save {file}: {e}"))
    }

    async fn get(&self, url: &str) -> Result<Vec<u8>, String> {
        let resp = self.client.get(url).send().await.map_err(|e| format!("{url}: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("{url}: HTTP {}", resp.status().as_u16()));
        }
        if resp.content_length().is_some_and(|n| n as usize > MAX_BINARY) {
            return Err(format!("{url}: too large"));
        }
        let body = resp.bytes().await.map_err(|e| format!("{url}: {e}"))?;
        if body.len() > MAX_BINARY {
            return Err(format!("{url}: too large"));
        }
        Ok(body.to_vec())
    }

    /// Fetches every missing build, one after another (at startup).
    pub async fn prefetch(&self) {
        for &(_, file) in PLATFORMS {
            let _ = self.ensure(file).await;
        }
    }

    /// Each platform's build and whether it's here, being fetched, or unavailable.
    pub fn binaries(&self) -> Vec<Binary> {
        let fetches = self.fetches.lock().unwrap();
        PLATFORMS
            .iter()
            .map(|&(platform, file)| {
                let size = std::fs::metadata(self.dir.join(file)).ok().filter(|m| m.is_file()).map(|m| m.len());
                let (status, detail) = match (size, fetches.get(file), &self.source) {
                    (Some(_), Some(Fetch::Done), _) => ("fetched", None),
                    (Some(_), _, _) => ("cached", None),
                    (None, Some(Fetch::Running), _) => ("fetching", None),
                    (None, Some(Fetch::Failed(why, _)), _) => ("unavailable", Some(why.clone())),
                    (None, _, Source::Local) => ("unavailable", Some("not in the agents directory".into())),
                    (None, _, Source::Release(_)) => ("unavailable", Some("not fetched yet".into())),
                };
                Binary { platform, file, present: size.is_some(), size, status, detail }
            })
            .collect()
    }
}

/// The URL agents and install scripts should use: DESKHAND_PUBLIC_URL when set, else the
/// scheme and host this request came in on (X-Forwarded-* only behind a trusted proxy).
/// `None` if the host isn't a plain host name, so nothing odd ends up inside a script.
pub fn install_base(public_url: Option<&str>, trust_proxy: bool, headers: &HeaderMap) -> Option<String> {
    if let Some(u) = public_url {
        let u = u.trim_end_matches('/');
        return safe_url(u).then(|| u.to_owned());
    }
    let header_str = |name: &str| headers.get(name).and_then(|v| v.to_str().ok()).map(str::trim);
    let host = trust_proxy
        .then(|| header_str("x-forwarded-host").and_then(|h| h.split(',').next()).map(str::trim))
        .flatten()
        .or_else(|| header_str(header::HOST.as_str()))?;
    let https = trust_proxy && header_str("x-forwarded-proto").is_some_and(|p| p.eq_ignore_ascii_case("https"));
    let url = format!("{}://{host}", if https { "https" } else { "http" });
    safe_url(&url).then_some(url)
}

/// `http(s)://host[:port][/path]` with only characters that are safe inside quotes in
/// both sh and PowerShell.
fn safe_url(u: &str) -> bool {
    let rest = u.strip_prefix("https://").or_else(|| u.strip_prefix("http://"));
    rest.is_some_and(|r| {
        !r.is_empty()
            && r.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-._:[]/".contains(&b))
            && !r.starts_with('/')
    })
}

fn base_or_400(app: &Shared, headers: &HeaderMap) -> Result<String, Response> {
    install_base(app.public_url.as_deref(), app.trust_proxy, headers).ok_or_else(|| {
        (StatusCode::BAD_REQUEST, "can't tell this server's address; set DESKHAND_PUBLIC_URL").into_response()
    })
}

/// The Linux/macOS one-liner for an enroll token.
pub fn linux_command(base: &str, token: &str) -> String {
    format!("curl -fsSL {base}/install.sh | sudo sh -s -- {token}")
}

/// The Windows one-liner for an enroll token.
pub fn windows_command(base: &str, token: &str) -> String {
    format!("irm {base}/install.ps1 | iex; Install-Deskhand -Token {token}")
}

#[derive(Serialize, Clone, Debug)]
pub struct Binary {
    pub platform: &'static str,
    pub file: &'static str,
    pub present: bool,
    pub size: Option<u64>,
    /// `cached` (in the agents directory), `fetched` (from the release, this run),
    /// `fetching`, or `unavailable` (see `detail`).
    pub status: &'static str,
    pub detail: Option<String>,
}

pub async fn download(
    State(app): State<Shared>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Response {
    if !app.download_limit.allow(client_ip(&app, &headers, peer)) {
        return (StatusCode::TOO_MANY_REQUESTS, "too many downloads; wait a minute").into_response();
    }
    if let Some(&(_, file)) = PLATFORMS.iter().find(|(_, f)| *f == name) {
        if let Err(why) = app.builds.ensure(file).await {
            return (
                StatusCode::NOT_FOUND,
                format!(
                    "{file} isn't available: {why}. Put the agent build at {} (scripts/package-agents.sh builds them all).",
                    app.builds.dir.join(file).display()
                ),
            )
                .into_response();
        }
    }
    serve_file(&app.builds.dir, &name, &headers).await
}

async fn serve_file(dir: &FsPath, name: &str, headers: &HeaderMap) -> Response {
    let Some(&(_, file)) = PLATFORMS.iter().find(|(_, f)| *f == name) else {
        let names: Vec<&str> = PLATFORMS.iter().map(|(_, f)| *f).collect();
        return (StatusCode::NOT_FOUND, format!("no such download; available names: {}", names.join(", ")))
            .into_response();
    };
    let path = dir.join(file);
    let meta = match tokio::fs::metadata(&path).await {
        Ok(m) if m.is_file() => m,
        _ => {
            return (
                StatusCode::NOT_FOUND,
                format!(
                    "{file} isn't on this server yet. Put the agent build at {} (scripts/package-agents.sh builds them all), or set DESKHAND_AGENT_DIR.",
                    path.display()
                ),
            )
                .into_response()
        }
    };
    let mtime = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
    let etag = format!("\"{:x}-{:x}\"", meta.len(), mtime);
    let cached = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(',').any(|t| t.trim() == etag));
    let etag_value = HeaderValue::from_str(&etag).expect("hex etag");
    if cached {
        return (StatusCode::NOT_MODIFIED, [(header::ETAG, etag_value)]).into_response();
    }
    let body = match tokio::fs::read(&path).await {
        Ok(b) => b,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("can't read {file}: {e}")).into_response(),
    };
    (
        [
            (header::CONTENT_TYPE, HeaderValue::from_static("application/octet-stream")),
            (
                header::CONTENT_DISPOSITION,
                HeaderValue::from_str(&format!("attachment; filename=\"{file}\"")).expect("plain name"),
            ),
            (header::ETAG, etag_value),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-cache")),
        ],
        body,
    )
        .into_response()
}

pub async fn install_sh(State(app): State<Shared>, headers: HeaderMap) -> Response {
    match base_or_400(&app, &headers) {
        Ok(base) => script(install_sh_for(&base), "text/x-shellscript; charset=utf-8"),
        Err(r) => r,
    }
}

pub async fn install_ps1(State(app): State<Shared>, headers: HeaderMap) -> Response {
    match base_or_400(&app, &headers) {
        Ok(base) => script(install_ps1_for(&base), "text/plain; charset=utf-8"),
        Err(r) => r,
    }
}

fn script(body: String, content_type: &'static str) -> Response {
    (
        [
            (header::CONTENT_TYPE, HeaderValue::from_static(content_type)),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
        ],
        body,
    )
        .into_response()
}

pub fn install_sh_for(base: &str) -> String {
    include_str!("install.sh").replace("@SERVER@", base)
}

pub fn install_ps1_for(base: &str) -> String {
    include_str!("install.ps1").replace("@SERVER@", base)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(), HeaderValue::from_str(v).unwrap());
        }
        h
    }

    #[test]
    fn base_prefers_public_url() {
        let h = headers(&[("host", "evil.example")]);
        assert_eq!(install_base(Some("https://dh.example.com/"), false, &h).as_deref(), Some("https://dh.example.com"));
    }

    #[test]
    fn base_from_host_without_proxy_ignores_forwarded() {
        let h = headers(&[("host", "10.0.0.5:8080"), ("x-forwarded-proto", "https"), ("x-forwarded-host", "x.example")]);
        assert_eq!(install_base(None, false, &h).as_deref(), Some("http://10.0.0.5:8080"));
    }

    #[test]
    fn base_honors_forwarded_behind_trusted_proxy() {
        let h = headers(&[("host", "127.0.0.1:8080"), ("x-forwarded-proto", "https"), ("x-forwarded-host", "dh.example.com")]);
        assert_eq!(install_base(None, true, &h).as_deref(), Some("https://dh.example.com"));
        let h = headers(&[("host", "dh.example.com"), ("x-forwarded-proto", "https")]);
        assert_eq!(install_base(None, true, &h).as_deref(), Some("https://dh.example.com"));
    }

    #[test]
    fn base_rejects_odd_hosts() {
        for bad in ["a'b", "a b", "a\"b", "a$(id)", "a;b", "a`b"] {
            let mut h = HeaderMap::new();
            if let Ok(v) = HeaderValue::from_str(bad) {
                h.insert(header::HOST, v);
            }
            assert_eq!(install_base(None, false, &h), None, "{bad}");
        }
        assert_eq!(install_base(Some("https://x'y"), false, &HeaderMap::new()), None);
        assert_eq!(install_base(Some("ftp://x"), false, &HeaderMap::new()), None);
    }

    #[test]
    fn scripts_carry_the_base() {
        let sh = install_sh_for("https://dh.example.com:8443");
        assert!(sh.contains("SERVER='https://dh.example.com:8443'"));
        assert!(!sh.contains("@SERVER@") && !sh.contains("deskhand.sh"));
        let ps = install_ps1_for("https://dh.example.com:8443");
        assert!(ps.contains("$server = 'https://dh.example.com:8443'"));
        assert!(!ps.contains("@SERVER@"));
    }

    #[test]
    fn install_sh_parses() {
        let dir = std::env::temp_dir().join(format!("dh-sh-{}", std::process::id()));
        std::fs::write(&dir, install_sh_for("https://dh.example.com")).unwrap();
        let ok = std::process::Command::new("sh").arg("-n").arg(&dir).status().map(|s| s.success());
        let _ = std::fs::remove_file(&dir);
        if let Ok(ok) = ok {
            assert!(ok, "sh -n failed");
        }
    }

    #[tokio::test]
    async fn serves_only_known_names() {
        let dir = std::env::temp_dir().join(format!("dh-dl-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("deskhand-agent-linux-x86_64"), b"ELF...").unwrap();
        std::fs::write(dir.join("secret.txt"), b"nope").unwrap();
        let h = HeaderMap::new();
        let r = serve_file(&dir, "deskhand-agent-linux-x86_64", &h).await;
        assert_eq!(r.status(), StatusCode::OK);
        assert!(r.headers()[header::CONTENT_DISPOSITION].to_str().unwrap().contains("deskhand-agent-linux-x86_64"));
        let etag = r.headers()[header::ETAG].clone();
        let r = serve_file(&dir, "deskhand-agent-linux-x86_64", &headers(&[("if-none-match", etag.to_str().unwrap())])).await;
        assert_eq!(r.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(serve_file(&dir, "deskhand-agent-linux-aarch64", &h).await.status(), StatusCode::NOT_FOUND);
        for bad in ["secret.txt", "../secret.txt", "..%2fsecret.txt", "/etc/passwd", "deskhand-agent-linux-x86_64/..", ""] {
            assert_eq!(serve_file(&dir, bad, &h).await.status(), StatusCode::NOT_FOUND, "{bad}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
