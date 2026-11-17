//! Agent self-update: an agent on another version than this server is sent
//! `ServerMsg::Update` for the build matching the server, which it downloads from this
//! server's own `/dl/` (never from anywhere else), checks and restarts into.
//!
//! - Automatic when an agent connects with a mismatched version, unless
//!   `DESKHAND_AGENT_AUTO_UPDATE=0`; operators can also ask for it per device or in bulk.
//! - Loop guard: at most [`MAX_TRIES_PER_HOUR`] tries per device. An agent that comes back
//!   after an update on a version that's still wrong (the build in the agents directory
//!   isn't this server's version) is marked failed and isn't tried again automatically.
//! - Each step goes to the audit log (`device.agent_update.*`), and the device's
//!   `agent_update` column holds `updating` / `failed` for the UI.

use crate::{now, send_to_agent, App, DeviceEvent, Shared};
use deskhand_proto::ServerMsg;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Updates sent to one device per rolling hour before the server stops trying.
pub const MAX_TRIES_PER_HOUR: usize = 3;
/// Automatic updates wait up to this long, so a server upgrade doesn't have every agent
/// download at the same moment.
const AUTO_JITTER: Duration = Duration::from_secs(20);

pub struct Updates {
    /// DESKHAND_AGENT_AUTO_UPDATE (default on).
    pub auto: bool,
    devices: Mutex<HashMap<String, Tries>>,
    hashes: Mutex<HashMap<&'static str, Hashed>>,
}

/// An agent build's SHA-256, with the size and mtime it was taken at.
type Hashed = (u64, Option<SystemTime>, String);

#[derive(Default)]
struct Tries {
    /// When updates were sent, within the last hour.
    sent: Vec<Instant>,
    /// The version the agent had when the last update was sent.
    from: Option<String>,
    /// An update came back on the wrong version; don't try again on our own.
    gave_up: bool,
}

impl Updates {
    pub fn from_env() -> Updates {
        Updates {
            auto: std::env::var("DESKHAND_AGENT_AUTO_UPDATE").map_or(true, |v| v != "0" && v != "false"),
            devices: Mutex::default(),
            hashes: Mutex::default(),
        }
    }
}

/// The download name for an agent's platform, if this server serves one.
pub fn build_for(platform: &str) -> Option<&'static str> {
    let name = format!("deskhand-agent-{platform}");
    crate::downloads::PLATFORMS.iter().map(|(_, f)| *f).find(|f| *f == name)
}

fn set_state(app: &App, id: &str, state: &str, detail: &str) {
    let _ = app.db.set_agent_update(id, state, detail);
    let _ = app.device_events.send(DeviceEvent::Inventory { id: id.to_owned() });
}

fn fail(app: &App, id: &str, user: Option<i64>, why: &str) {
    log::warn!("agent update for {id}: {why}");
    app.db.audit(now(), user, Some(id), "device.agent_update.failed", why);
    set_state(app, id, "failed", why);
}

/// An agent said hello: records how a pending update went and, if it's on another version
/// and automatic updates are on, starts one.
pub fn on_hello(app: &Shared, id: &str, version: &str, platform: &str) {
    let outcome = {
        let mut devices = app.updates.devices.lock().unwrap();
        let t = devices.entry(id.to_owned()).or_default();
        if version == VERSION {
            let from = t.from.take();
            devices.remove(id);
            Some(from)
        } else {
            // Came back from an update on a version other than the one it left with, but
            // still not ours: the build we hand out is the wrong one. Don't loop on it.
            if t.from.as_deref().is_some_and(|f| f != version) && !t.gave_up {
                t.gave_up = true;
                drop(devices);
                fail(
                    app,
                    id,
                    None,
                    &format!("updated, but came back as {version} instead of {VERSION}: the agent build on this server is a different version"),
                );
                return;
            }
            None
        }
    };
    match outcome {
        Some(Some(from)) => {
            log::info!("agent {id} updated {from} -> {VERSION}");
            app.db.audit(now(), None, Some(id), "device.agent_update.ok", &format!("{from} → {VERSION}"));
            set_state(app, id, "", "");
            return;
        }
        Some(None) => {
            // On our version; clear a state left from before a restart.
            let _ = app.db.set_agent_update(id, "", "");
            return;
        }
        None => {}
    }
    if !app.updates.auto || platform.is_empty() || version.is_empty() {
        return;
    }
    let _ = start(app, id, None, true);
}

/// `UpdateFailed` from the agent.
pub fn on_failed(app: &Shared, id: &str, version: &str, error: &str) {
    if let Some(t) = app.updates.devices.lock().unwrap().get_mut(id) {
        t.from = None;
    }
    let error: String = error.chars().take(500).collect();
    fail(app, id, None, &format!("update to {version}: {error}"));
}

/// Starts an update of a connected agent to this server's version: `user` asked (and the
/// try limit resets), or `auto`. `Err` says why it can't.
pub fn start(app: &Shared, id: &str, user: Option<i64>, auto: bool) -> Result<(), String> {
    let (version, platform) = match app.agents.lock().unwrap().get(id) {
        Some(a) => (a.version.clone(), a.platform.clone()),
        None => return Err("device offline".into()),
    };
    if platform.is_empty() {
        return Err(format!("agent {version} is too old to update itself; reinstall it"));
    }
    if version == VERSION {
        return Err(format!("agent is already on {VERSION}"));
    }
    let file = build_for(&platform).ok_or_else(|| format!("this server has no agent build for {platform}"))?;
    {
        let mut devices = app.updates.devices.lock().unwrap();
        let t = devices.entry(id.to_owned()).or_default();
        if user.is_some() {
            *t = Tries::default();
        }
        if auto && t.gave_up {
            return Err("gave up on automatic updates for this device".into());
        }
        t.sent.retain(|at| at.elapsed() < Duration::from_secs(3600));
        if t.sent.len() >= MAX_TRIES_PER_HOUR {
            let first = !t.gave_up;
            t.gave_up = true;
            drop(devices);
            let why = format!("{MAX_TRIES_PER_HOUR} tries in the last hour didn't take; stopped trying");
            if first {
                fail(app, id, user, &why);
            }
            return Err(why);
        }
        t.sent.push(Instant::now());
        t.from = Some(version.clone());
    }
    app.db.audit(
        now(),
        user,
        Some(id),
        "device.agent_update.start",
        &format!("{version} → {VERSION}{}", if auto { " (automatic)" } else { "" }),
    );
    set_state(app, id, "updating", VERSION);
    let (app, id) = (app.clone(), id.to_owned());
    tokio::spawn(async move {
        if auto {
            let mut b = [0u8; 2];
            let _ = getrandom::fill(&mut b);
            let ms = u64::from(u16::from_le_bytes(b)) % AUTO_JITTER.as_millis() as u64;
            tokio::time::sleep(Duration::from_millis(ms)).await;
        }
        let sha256 = match app.builds.ensure(file).await {
            Ok(()) => hash(&app, file).await,
            Err(e) => Err(e),
        };
        let sha256 = match sha256 {
            Ok(h) => h,
            Err(e) => {
                if let Some(t) = app.updates.devices.lock().unwrap().get_mut(&id) {
                    t.from = None;
                }
                return fail(&app, &id, user, &format!("{file} isn't available: {e}"));
            }
        };
        let msg = ServerMsg::Update { version: VERSION.into(), url: format!("/dl/{file}"), sha256 };
        if !send_to_agent(&app, &id, msg) {
            if let Some(t) = app.updates.devices.lock().unwrap().get_mut(&id) {
                t.from = None;
            }
            set_state(&app, &id, "", "");
        }
    });
    Ok(())
}

/// The SHA-256 of an agent build in the agents directory, cached by size and mtime.
async fn hash(app: &App, file: &'static str) -> Result<String, String> {
    let path = app.builds.dir.join(file);
    let meta = tokio::fs::metadata(&path).await.map_err(|e| format!("{}: {e}", path.display()))?;
    let key = (meta.len(), meta.modified().ok());
    if let Some((len, mtime, h)) = app.updates.hashes.lock().unwrap().get(file) {
        if (*len, *mtime) == key {
            return Ok(h.clone());
        }
    }
    let body = tokio::fs::read(&path).await.map_err(|e| format!("{}: {e}", path.display()))?;
    let h = tokio::task::spawn_blocking(move || hex::encode(Sha256::digest(&body)))
        .await
        .map_err(|e| e.to_string())?;
    app.updates.hashes.lock().unwrap().insert(file, (key.0, key.1, h.clone()));
    Ok(h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_by_platform() {
        assert_eq!(build_for("linux-x86_64"), Some("deskhand-agent-linux-x86_64"));
        assert_eq!(build_for("windows-x86_64.exe"), Some("deskhand-agent-windows-x86_64.exe"));
        assert_eq!(build_for(&deskhand_proto::platform()).is_some(), cfg!(any(
            all(target_os = "linux", any(target_arch = "x86_64", target_arch = "aarch64")),
            all(windows, target_arch = "x86_64"),
            all(target_os = "macos", target_arch = "aarch64"),
        )));
        assert_eq!(build_for("../../etc/passwd"), None);
        assert_eq!(build_for(""), None);
    }
}
