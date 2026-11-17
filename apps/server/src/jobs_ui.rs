//! Turning job requests into jobs, the text that describes them, and the live event
//! stream a job page listens to.

use crate::auth::require;
use crate::db::{Device, Role, User};
use crate::jobs::{self, Event, NewJob, Target, Task, ACTION_TIMEOUT};
use crate::{now, Shared};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use futures_util::StreamExt;
use deskhand_proto::{Action, ScriptShell};
use std::convert::Infallible;

fn get<'a>(f: &'a [(String, String)], k: &str) -> &'a str {
    f.iter().find(|(key, _)| key == k).map_or("", |(_, v)| v.trim())
}

// --- scripts ---

/// The script fields from a form, checked.
pub(crate) fn script_fields(f: &[(String, String)]) -> Result<(String, ScriptShell, String, i64), String> {
    let name: String = get(f, "name").chars().take(200).collect();
    if name.is_empty() {
        return Err("a script needs a name".into());
    }
    let shell = ScriptShell::parse(get(f, "shell")).ok_or("pick a shell")?;
    let timeout: i64 = get(f, "timeout_s").parse().map_err(|_| "the timeout is a number of seconds")?;
    let body = f.iter().find(|(k, _)| k == "body").map(|(_, v)| v.clone()).unwrap_or_default();
    if body.trim().is_empty() {
        return Err("the script is empty".into());
    }
    if body.len() > 1024 * 1024 {
        return Err("scripts are limited to 1 MB".into());
    }
    Ok((name, shell, body, timeout.clamp(1, 86400)))
}

// --- making a job ---

/// The longest one-off command (`source=inline`), and its timeout unless one is given.
const INLINE_MAX: usize = 64 * 1024;
const INLINE_TIMEOUT: i64 = 300;

pub(crate) fn bad_request(msg: impl Into<String>) -> (StatusCode, String) {
    (StatusCode::BAD_REQUEST, msg.into())
}

pub(crate) fn db_fail(e: rusqlite::Error) -> (StatusCode, String) {
    log::error!("db: {e}");
    (StatusCode::INTERNAL_SERVER_ERROR, "database error".into())
}

/// Makes a job from the job form's fields (also what `/api/v1/jobs` turns its JSON into)
/// and fires it if it runs now.
pub(crate) fn create_job_from(app: &Shared, user: &User, f: &[(String, String)]) -> Result<i64, (StatusCode, String)> {
    let source = get(f, "source");
    let (task, timeout_s, default_name) = if let Some(id) = source.strip_prefix("script:") {
        let Some(s) = id.parse().ok().and_then(|id| app.db.script(id).ok().flatten()) else {
            return Err(bad_request("pick a script"));
        };
        let name = s.name.clone();
        (Task::Script { script_id: Some(s.id), shell: s.shell, body: s.body }, s.timeout_s, name)
    } else if source == "inline" {
        // A one-off command that isn't a saved script.
        let Some(shell) = ScriptShell::parse(get(f, "shell")) else {
            return Err(bad_request("pick a shell: powershell, cmd, bash or sh"));
        };
        let body = get(f, "body");
        if body.trim().is_empty() || body.len() > INLINE_MAX {
            return Err(bad_request(format!("a command needs 1 to {INLINE_MAX} bytes")));
        }
        let timeout_s = get(f, "timeout_s").parse().unwrap_or(INLINE_TIMEOUT).clamp(5, 86400);
        (Task::Script { script_id: None, shell, body: body.to_owned() }, timeout_s, format!("{} command", shell.as_str()))
    } else {
        let delay_s: u32 = get(f, "delay_s").parse().unwrap_or(0).min(86400);
        let message = Some(get(f, "message").chars().take(512).collect::<String>()).filter(|m| !m.is_empty());
        let action = match source.strip_prefix("action:") {
            Some("reboot") => Action::Reboot { delay_s, message },
            Some("shutdown") => Action::Shutdown { delay_s, message },
            Some("logoff") => Action::Logoff,
            Some("lock") => Action::Lock,
            // The MACs and where to send them are worked out per device when it fires.
            Some("wake") => Action::WakeOnLan { macs: vec![], broadcasts: vec![] },
            _ => return Err(bad_request("pick a script or an action")),
        };
        let name = action.name().to_owned();
        (Task::Action(action), ACTION_TIMEOUT, name)
    };
    let target = match get(f, "target") {
        "group" => match get(f, "group").parse::<i64>() {
            Ok(g) => Target::Group(Some(g)),
            Err(_) => return Err(bad_request("pick a group")),
        },
        "all" => Target::All,
        _ => Target::Devices(f.iter().filter(|(k, _)| k == "device").map(|(_, v)| v.clone()).collect()),
    };
    let devices = match app.db.target_devices(&target) {
        Ok(d) => d,
        Err(e) => return Err(db_fail(e)),
    };
    if devices.is_empty() {
        return Err(bad_request("that selects no devices"));
    }
    // Reboot and shutdown: typed confirmation, checked here as well as in the page.
    if let Task::Action(Action::Reboot { .. } | Action::Shutdown { .. }) = &task {
        let confirm = get(f, "confirm");
        let ok = if devices.len() == 1 {
            app.db.device(&devices[0]).ok().flatten().is_some_and(|d: Device| {
                confirm.eq_ignore_ascii_case(&d.hostname) || confirm.eq_ignore_ascii_case(&d.name)
            })
        } else {
            confirm == devices.len().to_string()
        };
        if !ok {
            let want = if devices.len() == 1 {
                "the device's hostname".to_owned()
            } else {
                format!("the number of devices, {}", devices.len())
            };
            return Err(bad_request(format!("to confirm, type {want}")));
        }
    }
    let schedule = match get(f, "when") {
        "cron" => {
            let expr = get(f, "cron").to_owned();
            if let Err(e) = jobs::check_cron(&expr) {
                return Err(bad_request(e));
            }
            Some(expr)
        }
        _ => None,
    };
    let hours: f64 = get(f, "expires_h").parse().unwrap_or(24.0);
    let expires_after = ((hours * 3600.0) as i64).clamp(60, 30 * 86400);
    let target_text = match (&target, devices.len()) {
        (Target::Devices(_), 1) => app.db.device_name(&devices[0]).ok().flatten().unwrap_or_default(),
        (Target::Devices(_), n) => format!("{n} devices"),
        (Target::Group(Some(g)), _) => app
            .db
            .groups()
            .ok()
            .and_then(|gs| gs.into_iter().find(|x| x.id == *g))
            .map_or("group".into(), |x| format!("group {}", x.name)),
        (Target::Group(None), _) => "group".into(),
        (Target::All, _) => "all devices".into(),
    };
    let name = Some(get(f, "name").chars().take(200).collect::<String>())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| format!("{default_name} on {target_text}"));
    let job = NewJob { name: name.clone(), task, timeout_s, target, schedule: schedule.clone(), expires_after, created_by: user.id };
    let id = match app.db.add_job(&job, now()) {
        Ok(id) => id,
        Err(e) => return Err(db_fail(e)),
    };
    let when = schedule.as_deref().map_or("now".to_owned(), |s| format!("cron {s}"));
    app.db.record(crate::audit::Entry::new("job.create").by(user.id).target("job", id).detail(format!("{name} ({when})")));
    if schedule.is_none() {
        if let Err(e) = jobs::fire(app, id) {
            log::error!("firing job {id}: {e}");
        }
    }
    Ok(id)
}

// --- jobs ---

pub(crate) fn task_text(t: &Task) -> String {
    match t {
        Task::Script { shell, .. } => format!("{} script", shell.as_str()),
        Task::Action(a) => a.name().to_owned(),
    }
}

pub(crate) fn target_text(app: &Shared, t: &Target) -> String {
    match t {
        Target::Devices(ids) if ids.len() == 1 => {
            app.db.device_name(&ids[0]).ok().flatten().unwrap_or_else(|| "1 device".into())
        }
        Target::Devices(ids) => format!("{} devices", ids.len()),
        Target::Group(Some(g)) => app
            .db
            .groups()
            .ok()
            .and_then(|gs| gs.into_iter().find(|x| x.id == *g))
            .map_or("a deleted group".into(), |x| format!("group {}", x.name)),
        Target::Group(None) => "a deleted group".into(),
        Target::All => "all devices".into(),
    }
}

/// `GET /jobs/{id}/events`: server-sent events for the job's runs. It starts with the
/// current state of runs still in progress, so nothing between page load and connect is
/// lost.
pub async fn events(State(app): State<Shared>, headers: HeaderMap, Path(id): Path<i64>) -> Response {
    if let Err(r) = require(&app, &headers, Role::Viewer) {
        return r;
    }
    let rx = app.job_events.subscribe();
    let snapshot: Vec<Event> = app
        .db
        .runs(id)
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.status == "queued" || r.status == "running")
        .map(|r| Event::Status {
            job_id: id,
            run_id: r.id,
            status: r.status,
            exit_code: r.exit_code,
            started: r.started,
            finished: r.finished,
            stdout: r.stdout,
            stderr: r.stderr,
        })
        .collect();
    let to_sse = |e: &Event| {
        Ok::<_, Infallible>(axum::response::sse::Event::default().data(serde_json::to_string(e).unwrap_or_default()))
    };
    let first = futures_util::stream::iter(snapshot.iter().map(to_sse).collect::<Vec<_>>());
    let live = futures_util::stream::unfold(rx, move |mut rx| async move {
        loop {
            match rx.recv().await {
                Ok(e) if e.job_id() == id => return Some((to_sse(&e), rx)),
                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
            }
        }
    });
    Sse::new(first.chain(live)).keep_alive(KeepAlive::default()).into_response()
}
