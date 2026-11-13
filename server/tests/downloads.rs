//! `/dl` and the install scripts against a real server: agent builds come from the
//! server's own agents directory (filled from a release it checks against SHA256SUMS),
//! and the scripts point back at this server.

use std::process::{Child, Command};
use std::time::Duration;

struct Server {
    child: Child,
    base: String,
    dir: std::path::PathBuf,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn start(tag: &str, env: &[(&str, &str)]) -> Server {
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let dir = std::env::temp_dir().join(format!("deskhand-dl-test-{tag}-{port}-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("agents")).unwrap();
    std::fs::write(dir.join("agents/deskhand-agent-linux-x86_64"), b"\x7fELF fake agent").unwrap();
    std::fs::write(dir.join("secret.txt"), b"not for download").unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_deskhand-server"));
    cmd.env("DESKHAND_ADDR", format!("127.0.0.1:{port}"))
        .env("DESKHAND_DB", dir.join("t.db"))
        .env("DESKHAND_ADMIN_PASSWORD", "admin-password")
        .env_remove("DESKHAND_PUBLIC_URL")
        .env_remove("DESKHAND_TRUST_PROXY")
        .env_remove("DESKHAND_AGENT_DIR")
        .env("DESKHAND_AGENT_SOURCE", "local")
        .env("RUST_LOG", "error");
    for (k, v) in env {
        cmd.env(k, v);
    }
    let child = cmd.spawn().unwrap();
    for _ in 0..100 {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Server { child, base: format!("http://127.0.0.1:{port}"), dir }
}

fn get(url: &str, headers: &[(&str, &str)]) -> (u16, Vec<(String, String)>, Vec<u8>) {
    let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).build().into();
    let mut req = agent.get(url);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let mut resp = req.call().unwrap();
    let status = resp.status().as_u16();
    let hs = resp
        .headers()
        .iter()
        .map(|(k, v)| (k.as_str().to_owned(), v.to_str().unwrap_or("").to_owned()))
        .collect();
    let body = resp.body_mut().read_to_vec().unwrap();
    (status, hs, body)
}

fn header<'a>(hs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    hs.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
}

#[test]
fn downloads_and_install_scripts() {
    let s = start("plain", &[]);

    // Present: served with a file name and an ETag that 304s.
    let (st, hs, body) = get(&format!("{}/dl/deskhand-agent-linux-x86_64", s.base), &[]);
    assert_eq!(st, 200);
    assert_eq!(body, b"\x7fELF fake agent");
    assert!(header(&hs, "content-disposition").unwrap().contains("deskhand-agent-linux-x86_64"));
    let etag = header(&hs, "etag").unwrap().to_owned();
    let (st, _, _) = get(&format!("{}/dl/deskhand-agent-linux-x86_64", s.base), &[("If-None-Match", &etag)]);
    assert_eq!(st, 304);

    // Missing: a 404 that says where the file goes.
    let (st, _, body) = get(&format!("{}/dl/deskhand-agent-windows-x86_64.exe", s.base), &[]);
    assert_eq!(st, 404);
    let msg = String::from_utf8(body).unwrap();
    assert!(msg.contains("agents") && msg.contains("deskhand-agent-windows-x86_64.exe"), "{msg}");
    assert!(msg.contains("DESKHAND_AGENT_SOURCE=local"), "{msg}");

    // Anything else, including path tricks, is a 404 and never the file.
    for bad in ["secret.txt", "..%2fsecret.txt", "%2e%2e%2fsecret.txt", "..%5csecret.txt"] {
        let (st, _, body) = get(&format!("{}/dl/{bad}", s.base), &[]);
        assert_eq!(st, 404, "{bad}");
        assert!(!String::from_utf8_lossy(&body).contains("not for download"), "{bad}");
    }

    // Without PUBLIC_URL the scripts use the Host the request came in on, and ignore
    // X-Forwarded-* because no proxy is trusted.
    let (st, hs, body) = get(&format!("{}/install.sh", s.base), &[("X-Forwarded-Proto", "https"), ("X-Forwarded-Host", "evil.example")]);
    assert_eq!(st, 200);
    assert!(header(&hs, "content-type").unwrap().contains("shellscript"));
    let sh = String::from_utf8(body).unwrap();
    assert!(sh.contains(&format!("SERVER='{}'", s.base)), "{sh}");
    assert!(!sh.contains("evil.example") && !sh.contains("deskhand.sh"));
    let path = s.dir.join("install.sh");
    std::fs::write(&path, &sh).unwrap();
    if let Ok(st) = Command::new("sh").arg("-n").arg(&path).status() {
        assert!(st.success(), "sh -n failed");
    }
    let (st, _, body) = get(&format!("{}/install.ps1", s.base), &[]);
    assert_eq!(st, 200);
    let ps = String::from_utf8(body).unwrap();
    assert!(ps.contains(&format!("$server = '{}'", s.base)));
    assert!(ps.contains("function Install-Deskhand"));

    // A Host that isn't a plain host name is refused rather than put in a script.
    let (st, _, _) = get(&format!("{}/install.sh", s.base), &[("Host", "a'b")]);
    assert_eq!(st, 400);
}

#[test]
fn public_url_and_trusted_proxy() {
    let s = start("public", &[("DESKHAND_PUBLIC_URL", "https://dh.example.com:8443/")]);
    let (_, _, body) = get(&format!("{}/install.sh", s.base), &[("Host", "other.example")]);
    assert!(String::from_utf8(body).unwrap().contains("SERVER='https://dh.example.com:8443'"));
    drop(s);

    let s = start("proxy", &[("DESKHAND_TRUST_PROXY", "1")]);
    let (_, _, body) = get(
        &format!("{}/install.sh", s.base),
        &[("Host", "127.0.0.1"), ("X-Forwarded-Proto", "https"), ("X-Forwarded-Host", "dh.example.com")],
    );
    assert!(String::from_utf8(body).unwrap().contains("SERVER='https://dh.example.com'"));
}

/// A stand-in for a GitHub Release: a static file server over a directory.
struct Release {
    child: Child,
    url: String,
    dir: std::path::PathBuf,
}

impl Drop for Release {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn release(files: &[(&str, &[u8])], sums: &str) -> Option<Release> {
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let dir = std::env::temp_dir().join(format!("deskhand-release-{port}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for (name, body) in files {
        std::fs::write(dir.join(name), body).unwrap();
    }
    std::fs::write(dir.join("SHA256SUMS"), sums).unwrap();
    let child = Command::new("python3")
        .args(["-I", "-m", "http.server", &port.to_string(), "--bind", "127.0.0.1", "--directory"])
        .arg(&dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    for _ in 0..100 {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return Some(Release { child, url: format!("http://127.0.0.1:{port}"), dir });
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    None
}

fn sha256_hex(b: &[u8]) -> String {
    let out = Command::new("sha256sum").stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).spawn();
    let mut child = out.unwrap();
    use std::io::Write;
    child.stdin.take().unwrap().write_all(b).unwrap();
    let o = child.wait_with_output().unwrap();
    String::from_utf8(o.stdout).unwrap().split_whitespace().next().unwrap().to_owned()
}

#[test]
fn fetches_missing_builds_from_the_release() {
    let arm = b"\x7fELF arm agent";
    let win = b"MZ windows agent";
    let sums = format!(
        "{}  deskhand-agent-linux-aarch64\n{}  deskhand-agent-windows-x86_64.exe\n",
        sha256_hex(arm),
        "0".repeat(64) // wrong on purpose
    );
    let Some(rel) = release(&[("deskhand-agent-linux-aarch64", arm), ("deskhand-agent-windows-x86_64.exe", win)], &sums) else {
        eprintln!("python3 unavailable; skipping");
        return;
    };
    let s = start("fetch", &[("DESKHAND_AGENT_SOURCE", "github"), ("DESKHAND_AGENT_RELEASE_URL", &rel.url)]);

    // Missing locally, listed in SHA256SUMS: fetched, checked, cached, served.
    let (st, _, body) = get(&format!("{}/dl/deskhand-agent-linux-aarch64", s.base), &[]);
    assert_eq!(st, 200);
    assert_eq!(body, arm);
    assert_eq!(std::fs::read(s.dir.join("agents/deskhand-agent-linux-aarch64")).unwrap(), arm);

    // A checksum mismatch is never served or cached.
    let (st, _, body) = get(&format!("{}/dl/deskhand-agent-windows-x86_64.exe", s.base), &[]);
    assert_eq!(st, 404);
    assert!(String::from_utf8_lossy(&body).contains("SHA256SUMS"));
    assert!(!s.dir.join("agents/deskhand-agent-windows-x86_64.exe").exists());

    // Not in the release at all: a 404 that says so.
    let (st, _, body) = get(&format!("{}/dl/deskhand-agent-macos-aarch64", s.base), &[]);
    assert_eq!(st, 404);
    assert!(String::from_utf8_lossy(&body).contains("doesn't list"));

    // The local copy is still served.
    let (st, _, _) = get(&format!("{}/dl/deskhand-agent-linux-x86_64", s.base), &[]);
    assert_eq!(st, 200);
}
