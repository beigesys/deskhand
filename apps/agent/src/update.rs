//! Self-update: the server sends `Update` with the build matching its own version; the
//! agent downloads it from that server, checks its SHA-256, swaps it in for its own
//! executable and restarts into it with the same arguments.
//!
//! - Unix: the new file is written next to the executable and renamed over it (a running
//!   binary can be replaced that way), then the process `exec`s itself, keeping its PID,
//!   so systemd and friends see nothing.
//! - Windows: a running .exe can't be overwritten but can be renamed, so it moves to
//!   `{exe}.old`, the new one takes its name, and the agent starts it and exits. The next
//!   start deletes the `.old` file ([`cleanup`]).

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// The largest build the agent will download.
const MAX_BINARY: u64 = 256 << 20;

/// Downloads `path` from `server` (an http(s) base URL), checks it against `sha256`, swaps
/// it in and restarts. Only returns on failure, with why.
pub fn apply(server: &str, authorization: &str, path: &str, sha256: &str) -> String {
    match download(server, authorization, path, sha256).and_then(|b| swap(&b)) {
        Ok(exe) => restart(&exe),
        Err(e) => e,
    }
}

fn download(server: &str, authorization: &str, path: &str, sha256: &str) -> Result<Vec<u8>, String> {
    // Only ever from the server this agent is connected to.
    if !path.starts_with('/') || path.starts_with("//") {
        return Err(format!("refusing to download {path:?}: not a path on this server"));
    }
    let want = sha256.trim().to_ascii_lowercase();
    if want.len() != 64 || !want.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("bad sha256 {sha256:?}"));
    }
    let url = format!("{server}{path}");
    let mut resp = ureq::get(&url)
        .header("authorization", authorization)
        .config()
        .http_status_as_error(false)
        .build()
        .call()
        .map_err(|e| format!("{url}: {e}"))?;
    if !resp.status().is_success() {
        let body = resp.body_mut().read_to_string().unwrap_or_default();
        return Err(format!("{url}: HTTP {}: {}", resp.status().as_u16(), body.trim()));
    }
    let body = resp
        .body_mut()
        .with_config()
        .limit(MAX_BINARY)
        .read_to_vec()
        .map_err(|e| format!("{url}: {e}"))?;
    let got = hex::encode(Sha256::digest(&body));
    if got != want {
        return Err(format!("{url}: sha256 is {got}, expected {want}"));
    }
    Ok(body)
}

/// This executable's path. On Linux a binary replaced while running reads back as
/// `... (deleted)`; the name it was started as is what we want.
fn current_exe() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("can't find my own executable: {e}"))?;
    let s = exe.to_string_lossy();
    Ok(match s.strip_suffix(" (deleted)") {
        Some(p) => PathBuf::from(p),
        None => exe,
    })
}

fn sibling(exe: &Path, suffix: &str) -> Result<PathBuf, String> {
    let name = exe
        .file_name()
        .ok_or_else(|| format!("{} has no file name", exe.display()))?
        .to_string_lossy();
    Ok(exe.with_file_name(format!("{name}{suffix}")))
}

/// Puts `new` where this executable is; returns that path.
fn swap(new: &[u8]) -> Result<PathBuf, String> {
    let exe = current_exe()?;
    let tmp = sibling(&exe, ".new")?;
    let write = || -> std::io::Result<()> {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o755);
        let mut f = opts.open(&tmp)?;
        std::io::Write::write_all(&mut f, new)?;
        f.sync_all()
    };
    if let Err(e) = write() {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("can't write {}: {e}", tmp.display()));
    }
    #[cfg(windows)]
    {
        let old = sibling(&exe, ".old")?;
        let _ = std::fs::remove_file(&old);
        if let Err(e) = std::fs::rename(&exe, &old) {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!("can't move {} aside: {e}", exe.display()));
        }
        if let Err(e) = std::fs::rename(&tmp, &exe) {
            let _ = std::fs::rename(&old, &exe);
            let _ = std::fs::remove_file(&tmp);
            return Err(format!("can't put the new build at {}: {e}", exe.display()));
        }
    }
    #[cfg(not(windows))]
    if let Err(e) = std::fs::rename(&tmp, &exe) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("can't replace {}: {e}", exe.display()));
    }
    Ok(exe)
}

/// Runs `exe` with this process's arguments in place of this process. Only returns on
/// failure.
fn restart(exe: &Path) -> String {
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    log::info!("update installed; restarting {}", exe.display());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let e = std::process::Command::new(exe).args(&args).exec();
        format!("installed, but restarting failed: {e}")
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        match std::process::Command::new(exe)
            .args(&args)
            .creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP)
            .spawn()
        {
            Ok(_) => std::process::exit(0),
            Err(e) => format!("installed, but starting it failed: {e}"),
        }
    }
}

/// Deletes what an earlier update left behind (`.old` on Windows, a stray `.new`).
pub fn cleanup() {
    let Ok(exe) = current_exe() else { return };
    for suffix in [".old", ".new"] {
        if let Ok(p) = sibling(&exe, suffix) {
            if p.exists() {
                // The old process may take a moment to exit and let go of it.
                for _ in 0..10 {
                    if std::fs::remove_file(&p).is_ok() {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(500));
                }
            }
        }
    }
}
