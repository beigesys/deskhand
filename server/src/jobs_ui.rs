//! Pages for scripts and jobs, the device page's jobs card, and the live event stream a
//! job page listens to.

use crate::auth::require;
use crate::db::{Device, Role, User};
use crate::jobs::{self, Event, NewJob, Run, Target, Task, ACTION_TIMEOUT};
use crate::ui::{ago, db_error, page};
use crate::{esc, iso_datetime, now, Shared};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{KeepAlive, Sse};
use axum::response::{IntoResponse, Redirect, Response};
use futures_util::StreamExt;
use ithub_proto::{Action, ScriptShell};
use std::convert::Infallible;
use std::fmt::Write;

/// Repeated keys (`device=a&device=b`) survive, unlike with `Form`.
fn form(body: &str) -> Vec<(String, String)> {
    url::form_urlencoded::parse(body.as_bytes()).into_owned().collect()
}

fn get<'a>(f: &'a [(String, String)], k: &str) -> &'a str {
    f.iter().find(|(key, _)| key == k).map_or("", |(_, v)| v.trim())
}

fn bad(user: &User, msg: &str) -> Response {
    let body = format!(
        r#"<div class="card"><p>{}</p><p><a class="muted" href="javascript:history.back()">← Back</a></p></div>"#,
        esc(msg)
    );
    (StatusCode::BAD_REQUEST, page(user, "jobs", "Can't do that", &body)).into_response()
}

fn badge(status: &str) -> String {
    let class = match status {
        "ok" => "ok",
        "failed" | "timeout" => "bad",
        "running" | "queued" => "warn",
        _ => "",
    };
    format!(r#"<span class="badge {class}" data-status>{}</span>"#, esc(status))
}

fn duration(r: &Run, now: i64) -> String {
    match (r.started, r.finished) {
        (Some(s), Some(f)) => format!("{}s", (f - s).max(0)),
        (Some(s), None) => format!("{}s…", (now - s).max(0)),
        _ => String::new(),
    }
}

fn shell_options(selected: ScriptShell) -> String {
    ScriptShell::ALL
        .iter()
        .map(|s| {
            let sel = if *s == selected { " selected" } else { "" };
            format!(r#"<option{sel}>{}</option>"#, s.as_str())
        })
        .collect()
}

const STYLE: &str = r#"<style>
textarea.code { width: 100%; min-height: 320px; font: 13px ui-monospace, monospace; color: var(--fg); background: var(--bg); border: 1px solid var(--line); border-radius: 6px; padding: 8px; tab-size: 4; }
pre.err { color: var(--danger); }
tr.out td { padding-top: 0; border-bottom: 1px solid var(--line); }
tr.run td { border-bottom: none; }
.power { display: flex; flex-wrap: wrap; gap: 6px; align-items: center; }
.power form { margin: 0; }
fieldset { border: 1px solid var(--line); border-radius: 6px; display: grid; gap: 6px; }
.picks { max-height: 220px; overflow-y: auto; display: grid; gap: 2px; }
.picks label { display: flex; gap: 6px; align-items: center; font-size: 13px; }
</style>"#;

/// Tab inserts a tab instead of leaving the editor.
const TAB_JS: &str = r#"<script>
document.querySelectorAll('textarea.code').forEach(t => t.addEventListener('keydown', e => {
  if (e.key !== 'Tab' || e.ctrlKey || e.altKey || e.metaKey) return;
  e.preventDefault();
  const s = t.selectionStart, end = t.selectionEnd;
  t.setRangeText('\t', s, end, 'end');
}));
</script>"#;

// --- scripts ---

pub async fn scripts(State(app): State<Shared>, headers: HeaderMap) -> Response {
    let user = match require(&app, &headers, Role::Viewer) {
        Ok(u) => u,
        Err(r) => return r,
    };
    let list = match app.db.scripts() {
        Ok(s) => s,
        Err(e) => return db_error(e),
    };
    let now = now();
    let mut b = String::from(STYLE);
    if user.role >= Role::Operator {
        b.push_str(r#"<div class="bar"><a class="muted" href="/scripts/new">+ New script</a></div>"#);
    }
    if list.is_empty() {
        b.push_str(r#"<p class="empty">No scripts yet.</p>"#);
    } else {
        b.push_str(r#"<div class="card tablewrap"><table><thead><tr><th>Name</th><th>Shell</th><th>Timeout</th><th>Updated</th><th></th></tr></thead><tbody>"#);
        for s in &list {
            let run = if user.role >= Role::Operator {
                format!(r#"<a class="muted" href="/jobs/new?source=script:{}">Run…</a>"#, s.id)
            } else {
                String::new()
            };
            let _ = write!(
                b,
                r#"<tr><td><a class="name" href="/scripts/{id}">{name}</a></td><td>{shell}</td><td>{t}s</td><td>{upd}{by}</td><td>{run}</td></tr>"#,
                id = s.id,
                name = esc(&s.name),
                shell = s.shell.as_str(),
                t = s.timeout_s,
                upd = ago(now - s.updated),
                by = s.creator.as_deref().map(|c| format!(" · {}", esc(c))).unwrap_or_default(),
            );
        }
        b.push_str("</tbody></table></div>");
    }
    page(&user, "scripts", "Scripts", &b)
}

fn editor(user: &User, s: Option<&jobs::Script>) -> Response {
    let can_edit = user.role >= Role::Operator;
    let (action, name, shell, body, timeout) = match s {
        Some(s) => (format!("/scripts/{}", s.id), s.name.as_str(), s.shell, s.body.as_str(), s.timeout_s),
        None => ("/scripts".into(), "", ScriptShell::Powershell, "", 300),
    };
    let ro = if can_edit { "" } else { " readonly disabled" };
    let mut b = String::from(STYLE);
    let _ = write!(
        b,
        r#"<a class="muted" href="/scripts">← Scripts</a>
<form class="card" method="post" action="{action}" style="display:grid;gap:8px">
<div class="grid"><label>Name<input name="name" value="{name}" required maxlength="200"{ro}></label>
<label>Shell<select name="shell"{ro}>{shells}</select></label>
<label>Timeout (seconds)<input name="timeout_s" type="number" min="1" max="86400" value="{timeout}"{ro}></label></div>
<textarea class="code" name="body" spellcheck="false"{ro}>{body}</textarea>"#,
        name = esc(name),
        shells = shell_options(shell),
        body = esc(body),
    );
    if can_edit {
        b.push_str(r#"<div class="bar"><button>Save</button>"#);
        if let Some(s) = s {
            let _ = write!(b, r#"<a class="muted" href="/jobs/new?source=script:{}">Run…</a>"#, s.id);
        }
        b.push_str("</div>");
    }
    b.push_str("</form>");
    if let (true, Some(s)) = (can_edit, s) {
        let _ = write!(
            b,
            r#"<form method="post" action="/scripts/{}/delete" onsubmit="return confirm('Delete this script? Jobs already made from it keep their copy.')"><button class="danger">Delete script</button></form>"#,
            s.id
        );
    }
    b.push_str(TAB_JS);
    page(user, "scripts", s.map_or("New script", |s| s.name.as_str()), &b)
}

pub async fn new_script(State(app): State<Shared>, headers: HeaderMap) -> Response {
    match require(&app, &headers, Role::Operator) {
        Ok(u) => editor(&u, None),
        Err(r) => r,
    }
}

pub async fn script(State(app): State<Shared>, headers: HeaderMap, Path(id): Path<i64>) -> Response {
    let user = match require(&app, &headers, Role::Viewer) {
        Ok(u) => u,
        Err(r) => return r,
    };
    match app.db.script(id) {
        Ok(Some(s)) => editor(&user, Some(&s)),
        Ok(None) => (StatusCode::NOT_FOUND, "no such script").into_response(),
        Err(e) => db_error(e),
    }
}

/// The script fields from a form, checked.
fn script_fields(f: &[(String, String)]) -> Result<(String, ScriptShell, String, i64), String> {
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

pub async fn create_script(State(app): State<Shared>, headers: HeaderMap, body: String) -> Response {
    let user = match require(&app, &headers, Role::Operator) {
        Ok(u) => u,
        Err(r) => return r,
    };
    let (name, shell, script, timeout) = match script_fields(&form(&body)) {
        Ok(x) => x,
        Err(e) => return bad(&user, &e),
    };
    match app.db.add_script(&name, shell, &script, timeout, user.id, now()) {
        Ok(id) => {
            app.db.audit(now(), Some(user.id), None, "script.create", &format!("{id}: {name}"));
            Redirect::to(&format!("/scripts/{id}")).into_response()
        }
        Err(e) => db_error(e),
    }
}

pub async fn update_script(State(app): State<Shared>, headers: HeaderMap, Path(id): Path<i64>, body: String) -> Response {
    let user = match require(&app, &headers, Role::Operator) {
        Ok(u) => u,
        Err(r) => return r,
    };
    let (name, shell, script, timeout) = match script_fields(&form(&body)) {
        Ok(x) => x,
        Err(e) => return bad(&user, &e),
    };
    match app.db.update_script(id, &name, shell, &script, timeout, now()) {
        Ok(0) => (StatusCode::NOT_FOUND, "no such script").into_response(),
        Ok(_) => {
            app.db.audit(now(), Some(user.id), None, "script.update", &format!("{id}: {name}"));
            Redirect::to(&format!("/scripts/{id}")).into_response()
        }
        Err(e) => db_error(e),
    }
}

pub async fn delete_script(State(app): State<Shared>, headers: HeaderMap, Path(id): Path<i64>) -> Response {
    let user = match require(&app, &headers, Role::Operator) {
        Ok(u) => u,
        Err(r) => return r,
    };
    let name = app.db.script(id).ok().flatten().map(|s| s.name).unwrap_or_default();
    match app.db.delete_script(id) {
        Ok(_) => {
            app.db.audit(now(), Some(user.id), None, "script.delete", &format!("{id}: {name}"));
            Redirect::to("/scripts").into_response()
        }
        Err(e) => db_error(e),
    }
}

// --- making a job ---

/// Built-in actions as the job form names them.
const ACTIONS: [(&str, &str); 5] = [
    ("reboot", "Reboot"),
    ("shutdown", "Shut down"),
    ("logoff", "Sign out user (Windows)"),
    ("lock", "Lock (Windows)"),
    ("wake", "Wake-on-LAN"),
];

#[derive(serde::Deserialize, Default)]
pub struct NewJobQuery {
    source: Option<String>,
    device: Option<String>,
}

pub async fn new_job(State(app): State<Shared>, headers: HeaderMap, Query(q): Query<NewJobQuery>) -> Response {
    let user = match require(&app, &headers, Role::Operator) {
        Ok(u) => u,
        Err(r) => return r,
    };
    let (scripts, devices, groups) = match (app.db.scripts(), app.db.devices(None), app.db.groups()) {
        (Ok(s), Ok(d), Ok(g)) => (s, d, g),
        (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => return db_error(e),
    };
    let source = q.source.unwrap_or_default();
    let mut sources = String::from(r#"<optgroup label="Scripts">"#);
    for s in &scripts {
        let v = format!("script:{}", s.id);
        let sel = if v == source { " selected" } else { "" };
        let _ = write!(sources, r#"<option value="{v}"{sel}>{} ({})</option>"#, esc(&s.name), s.shell.as_str());
    }
    sources.push_str(r#"</optgroup><optgroup label="Actions">"#);
    for (k, label) in ACTIONS {
        let v = format!("action:{k}");
        let sel = if v == source { " selected" } else { "" };
        let _ = write!(sources, r#"<option value="{v}"{sel}>{label}</option>"#);
    }
    sources.push_str("</optgroup>");
    let mut picks = String::new();
    for d in &devices {
        let checked = if q.device.as_deref() == Some(d.id.as_str()) { " checked" } else { "" };
        let _ = write!(
            picks,
            r#"<label><input type="checkbox" name="device" value="{id}"{checked}>{name} <span class="muted">{host}</span></label>"#,
            id = esc(&d.id),
            name = esc(&d.name),
            host = esc(&d.hostname),
        );
    }
    let mut group_opts = String::new();
    for g in &groups {
        let _ = write!(group_opts, r#"<option value="{}">{} ({} devices)</option>"#, g.id, esc(&g.name), g.devices);
    }
    let mut b = String::from(STYLE);
    let _ = write!(
        b,
        r#"<a class="muted" href="/jobs">← Jobs</a>
<form class="card" method="post" action="/jobs" style="display:grid;gap:12px" id="jobform">
<div class="grid"><label>Run<select name="source" id="source" required>{sources}</select></label>
<label>Job name (optional)<input name="name" maxlength="200"></label></div>
<div class="grid" id="poweropts"><label>Delay (seconds)<input name="delay_s" type="number" min="0" max="86400" value="0"></label>
<label>Message to the user<input name="message" maxlength="512"></label></div>
<fieldset><legend>On</legend>
<label><input type="radio" name="target" value="devices" checked> These devices</label>
<div class="picks">{picks}</div>
<label><input type="radio" name="target" value="group"> Group <select name="group">{group_opts}</select></label>
<label><input type="radio" name="target" value="all"> All devices ({all})</label>
</fieldset>
<fieldset><legend>When</legend>
<label><input type="radio" name="when" value="now" checked> Now</label>
<label><input type="radio" name="when" value="cron"> On a schedule (cron, UTC) <input name="cron" placeholder="0 3 * * 1-5" size="18"></label>
<label>Offline devices get it if they connect within <input name="expires_h" type="number" min="0.1" max="720" step="0.1" value="24" style="width:80px"> hours</label>
</fieldset>
<label id="confirmbox">To reboot or shut down, type the device's hostname (one device) or how many devices (several)<input name="confirm" autocomplete="off"></label>
<div class="bar"><button>Create job</button></div>
</form>
<script>
const src = document.getElementById('source');
function sync() {{
  const v = src.value;
  const power = v === 'action:reboot' || v === 'action:shutdown';
  document.getElementById('poweropts').style.display = power ? '' : 'none';
  document.getElementById('confirmbox').style.display = power ? '' : 'none';
}}
src.addEventListener('change', sync); sync();
document.querySelectorAll('.picks input').forEach(c => c.addEventListener('change', () => {{
  document.querySelector('input[name=target][value=devices]').checked = true;
}}));
</script>"#,
        all = devices.len(),
    );
    page(&user, "jobs", "New job", &b)
}

/// `POST /jobs`: makes a job from the new-job form (or a device page's quick forms) and,
/// if it's for now, fires it.
pub async fn create_job(State(app): State<Shared>, headers: HeaderMap, body: String) -> Response {
    let user = match require(&app, &headers, Role::Operator) {
        Ok(u) => u,
        Err(r) => return r,
    };
    let f = form(&body);
    let source = get(&f, "source");
    let (task, timeout_s, default_name) = if let Some(id) = source.strip_prefix("script:") {
        let Some(s) = id.parse().ok().and_then(|id| app.db.script(id).ok().flatten()) else {
            return bad(&user, "pick a script");
        };
        let name = s.name.clone();
        (Task::Script { script_id: Some(s.id), shell: s.shell, body: s.body }, s.timeout_s, name)
    } else {
        let delay_s: u32 = get(&f, "delay_s").parse().unwrap_or(0).min(86400);
        let message = Some(get(&f, "message").chars().take(512).collect::<String>()).filter(|m| !m.is_empty());
        let action = match source.strip_prefix("action:") {
            Some("reboot") => Action::Reboot { delay_s, message },
            Some("shutdown") => Action::Shutdown { delay_s, message },
            Some("logoff") => Action::Logoff,
            Some("lock") => Action::Lock,
            // The MACs and where to send them are worked out per device when it fires.
            Some("wake") => Action::WakeOnLan { macs: vec![], broadcasts: vec![] },
            _ => return bad(&user, "pick a script or an action"),
        };
        let name = action.name().to_owned();
        (Task::Action(action), ACTION_TIMEOUT, name)
    };
    let target = match get(&f, "target") {
        "group" => match get(&f, "group").parse::<i64>() {
            Ok(g) => Target::Group(Some(g)),
            Err(_) => return bad(&user, "pick a group"),
        },
        "all" => Target::All,
        _ => Target::Devices(f.iter().filter(|(k, _)| k == "device").map(|(_, v)| v.clone()).collect()),
    };
    let devices = match app.db.target_devices(&target) {
        Ok(d) => d,
        Err(e) => return db_error(e),
    };
    if devices.is_empty() {
        return bad(&user, "that selects no devices");
    }
    // Reboot and shutdown: typed confirmation, checked here as well as in the page.
    if let Task::Action(Action::Reboot { .. } | Action::Shutdown { .. }) = &task {
        let confirm = get(&f, "confirm");
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
            return bad(&user, &format!("to confirm, type {want}"));
        }
    }
    let schedule = match get(&f, "when") {
        "cron" => {
            let expr = get(&f, "cron").to_owned();
            if let Err(e) = jobs::check_cron(&expr) {
                return bad(&user, &e);
            }
            Some(expr)
        }
        _ => None,
    };
    let hours: f64 = get(&f, "expires_h").parse().unwrap_or(24.0);
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
    let name = Some(get(&f, "name").chars().take(200).collect::<String>())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| format!("{default_name} on {target_text}"));
    let job = NewJob { name: name.clone(), task, timeout_s, target, schedule: schedule.clone(), expires_after, created_by: user.id };
    let id = match app.db.add_job(&job, now()) {
        Ok(id) => id,
        Err(e) => return db_error(e),
    };
    let when = schedule.as_deref().map_or("now".to_owned(), |s| format!("cron {s}"));
    app.db.audit(now(), Some(user.id), None, "job.create", &format!("{id}: {name} ({when})"));
    if schedule.is_none() {
        if let Err(e) = jobs::fire(&app, id) {
            log::error!("firing job {id}: {e}");
        }
    }
    Redirect::to(&format!("/jobs/{id}")).into_response()
}

// --- jobs ---

fn task_text(t: &Task) -> String {
    match t {
        Task::Script { shell, .. } => format!("{} script", shell.as_str()),
        Task::Action(a) => a.name().to_owned(),
    }
}

fn target_text(app: &Shared, t: &Target) -> String {
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

pub async fn jobs_list(State(app): State<Shared>, headers: HeaderMap) -> Response {
    let user = match require(&app, &headers, Role::Viewer) {
        Ok(u) => u,
        Err(r) => return r,
    };
    let list = match app.db.jobs() {
        Ok(j) => j,
        Err(e) => return db_error(e),
    };
    let now = now();
    let mut b = String::from(STYLE);
    if user.role >= Role::Operator {
        b.push_str(r#"<div class="bar"><a class="muted" href="/jobs/new">+ New job</a><a class="muted" href="/scripts">Scripts</a></div>"#);
    }
    if list.is_empty() {
        b.push_str(r#"<p class="empty">No jobs yet. Run a script or an action from here or from a device's page.</p>"#);
    } else {
        b.push_str(r#"<div class="card tablewrap"><table><thead><tr><th>Job</th><th>Runs</th><th>On</th><th>When</th><th>Made</th></tr></thead><tbody>"#);
        for (j, n) in &list {
            let mut counts = String::new();
            for (label, v, class) in [
                ("ok", n.ok, "ok"),
                ("failed", n.failed + n.timeout, "bad"),
                ("running", n.running, "warn"),
                ("queued", n.queued, "warn"),
                ("offline", n.offline, ""),
            ] {
                if v > 0 {
                    let _ = write!(counts, r#"<span class="badge {class}">{v} {label}</span> "#);
                }
            }
            let when = match (&j.schedule, j.enabled) {
                (Some(s), true) => format!(
                    "<code>{}</code><br><span class=\"muted\">next {}</span>",
                    esc(s),
                    j.next_fire.map(iso_datetime).unwrap_or_default()
                ),
                (Some(s), false) => format!("<code>{}</code><br><span class=\"muted\">paused</span>", esc(s)),
                (None, _) => "once".into(),
            };
            let _ = write!(
                b,
                r#"<tr><td><a class="name" href="/jobs/{id}">{name}</a><br><span class="muted">{task}</span></td><td>{counts}</td><td>{on}</td><td>{when}</td><td>{made}{by}</td></tr>"#,
                id = j.id,
                name = esc(&j.name),
                task = task_text(&j.task),
                on = esc(&target_text(&app, &j.target)),
                made = ago(now - j.created),
                by = j.creator.as_deref().map(|c| format!(" · {}", esc(c))).unwrap_or_default(),
            );
        }
        b.push_str("</tbody></table></div>");
    }
    page(&user, "jobs", "Jobs", &b)
}

fn run_rows(b: &mut String, runs: &[Run], now: i64) {
    for r in runs {
        let open = if r.status == "running" || runs.len() == 1 { " open" } else { "" };
        let _ = write!(
            b,
            r#"<tr class="run" id="r-{id}"><td><a class="name" href="/devices/{dev}">{name}</a></td><td>{badge}</td><td data-exit>{exit}</td><td data-dur>{dur}</td><td>{queued}</td></tr>
<tr class="out"><td colspan="5"><details{open}><summary class="muted">Output</summary><pre id="o-{id}"{oh}>{stdout}</pre><pre class="err" id="e-{id}"{eh}>{stderr}</pre></details></td></tr>"#,
            id = r.id,
            dev = esc(&r.device_id),
            name = esc(&r.device_name),
            badge = badge(&r.status),
            exit = r.exit_code.map(|c| c.to_string()).unwrap_or_default(),
            dur = duration(r, now),
            queued = ago(now - r.queued),
            stdout = esc(&r.stdout),
            stderr = esc(&r.stderr),
            oh = if r.stdout.is_empty() { " hidden" } else { "" },
            eh = if r.stderr.is_empty() { " hidden" } else { "" },
        );
    }
}

/// Keeps a job page's runs current from `/jobs/{id}/events`. Output events carry the
/// stream's length after them, so a chunk the page already has (from the HTML or an
/// earlier event) isn't added twice.
const LIVE_JS: &str = r#"<script>
const len = s => Array.from(s).length;
const badgeClass = s => s === 'ok' ? 'ok' : (s === 'failed' || s === 'timeout') ? 'bad' : (s === 'running' || s === 'queued') ? 'warn' : '';
const es = new EventSource(location.pathname + '/events');
es.onmessage = e => {
  const m = JSON.parse(e.data);
  if (m.kind === 'new_runs') { location.reload(); return; }
  const row = document.getElementById('r-' + m.run_id);
  if (!row) { location.reload(); return; }
  if (m.kind === 'output') {
    const pre = document.getElementById((m.stream === 'stdout' ? 'o-' : 'e-') + m.run_id);
    const have = len(pre.textContent);
    if (m.end <= have) return;
    const chars = Array.from(m.data);
    pre.textContent += chars.slice(Math.max(0, chars.length - (m.end - have))).join('');
    pre.hidden = false;
  } else if (m.kind === 'status') {
    const b = row.querySelector('[data-status]');
    b.textContent = m.status; b.className = 'badge ' + badgeClass(m.status);
    row.querySelector('[data-exit]').textContent = m.exit_code ?? '';
    if (m.started && m.finished) row.querySelector('[data-dur]').textContent = (m.finished - m.started) + 's';
    for (const [k, v] of [['o-', m.stdout], ['e-', m.stderr]]) {
      const pre = document.getElementById(k + m.run_id);
      if (len(v) >= len(pre.textContent)) pre.textContent = v;
      pre.hidden = !pre.textContent;
    }
  }
};
</script>"#;

pub async fn job(State(app): State<Shared>, headers: HeaderMap, Path(id): Path<i64>) -> Response {
    let user = match require(&app, &headers, Role::Viewer) {
        Ok(u) => u,
        Err(r) => return r,
    };
    let (job, runs) = match (app.db.job(id), app.db.runs(id)) {
        (Ok(Some(j)), Ok(r)) => (j, r),
        (Ok(None), _) => return (StatusCode::NOT_FOUND, "no such job").into_response(),
        (Err(e), _) | (_, Err(e)) => return db_error(e),
    };
    let now = now();
    let mut b = String::from(STYLE);
    let when = match &job.schedule {
        Some(s) if job.enabled => format!(
            "cron <code>{}</code> (UTC), next {}",
            esc(s),
            job.next_fire.map(iso_datetime).unwrap_or_default()
        ),
        Some(s) => format!("cron <code>{}</code>, paused", esc(s)),
        None => "once".into(),
    };
    let mut acts = String::new();
    if user.role >= Role::Operator {
        let _ = write!(acts, r#"<form method="post" action="/jobs/{id}/run" onsubmit="return confirm('Run this job again on its devices now?')"><button>Run now</button></form>"#);
        if job.schedule.is_some() {
            let (path, label) = if job.enabled { ("disable", "Pause") } else { ("enable", "Resume") };
            let _ = write!(acts, r#"<form method="post" action="/jobs/{id}/{path}"><button>{label}</button></form>"#);
        }
        let _ = write!(acts, r#"<form method="post" action="/jobs/{id}/delete" onsubmit="return confirm('Delete this job and its results?')"><button class="danger">Delete</button></form>"#);
    }
    let _ = write!(
        b,
        r#"<div class="row dev"><div><a class="muted" href="/jobs">← Jobs</a><h2>{name}</h2><div class="muted">{task} · on {on} · {when} · made {made}{by}</div></div><span class="acts">{acts}</span></div>"#,
        name = esc(&job.name),
        task = task_text(&job.task),
        on = esc(&target_text(&app, &job.target)),
        made = ago(now - job.created),
        by = job.creator.as_deref().map(|c| format!(" by {}", esc(c))).unwrap_or_default(),
    );
    if let Task::Script { script_id, shell, body } = &job.task {
        let link = script_id.map(|s| format!(r#" · <a class="muted" href="/scripts/{s}">script</a>"#)).unwrap_or_default();
        let _ = write!(
            b,
            r#"<details class="card"><summary class="muted">The script ({}){link}</summary><pre>{}</pre></details>"#,
            shell.as_str(),
            esc(body)
        );
    }
    if runs.is_empty() {
        let msg = if job.schedule.is_some() { "No runs yet: it runs at the next scheduled time." } else { "No runs." };
        let _ = write!(b, r#"<p class="empty">{msg}</p>"#);
    } else {
        b.push_str(r#"<div class="card tablewrap"><table><thead><tr><th>Device</th><th>Status</th><th>Exit</th><th>Took</th><th>Queued</th></tr></thead><tbody>"#);
        run_rows(&mut b, &runs, now);
        b.push_str("</tbody></table></div>");
    }
    b.push_str(LIVE_JS);
    page(&user, "jobs", &job.name, &b)
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

async fn job_op(app: &Shared, headers: &HeaderMap, id: i64, op: &str) -> Response {
    let user = match require(app, headers, Role::Operator) {
        Ok(u) => u,
        Err(r) => return r,
    };
    let name = match app.db.job(id) {
        Ok(Some(j)) => j.name,
        Ok(None) => return (StatusCode::NOT_FOUND, "no such job").into_response(),
        Err(e) => return db_error(e),
    };
    let r = match op {
        "run" => jobs::fire(app, id).map(|_| ()).map_err(|e| e.to_string()),
        "enable" => app.db.set_job_enabled(id, true, now()).map(|_| ()).map_err(|e| e.to_string()),
        "disable" => app.db.set_job_enabled(id, false, now()).map(|_| ()).map_err(|e| e.to_string()),
        "delete" => app.db.delete_job(id).map(|_| ()).map_err(|e| e.to_string()),
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    if let Err(e) = r {
        log::error!("job {id} {op}: {e}");
        return (StatusCode::INTERNAL_SERVER_ERROR, "couldn't do that").into_response();
    }
    app.db.audit(now(), Some(user.id), None, &format!("job.{op}"), &format!("{id}: {name}"));
    if op == "delete" {
        Redirect::to("/jobs").into_response()
    } else {
        Redirect::to(&format!("/jobs/{id}")).into_response()
    }
}

pub async fn job_action(State(app): State<Shared>, headers: HeaderMap, Path((id, op)): Path<(i64, String)>) -> Response {
    job_op(&app, &headers, id, &op).await
}

// --- the device page's card ---

/// Run-a-script, power buttons and recent runs, for `/devices/{id}`.
pub fn device_card(app: &Shared, user: &User, d: &Device, online: bool) -> String {
    let now = now();
    let id = esc(&d.id);
    let mut b = String::from(STYLE);
    b.push_str(r#"<section class="card" id="jobs"><h3>Jobs</h3>"#);
    if user.role >= Role::Operator {
        let scripts = app.db.scripts().unwrap_or_default();
        if !scripts.is_empty() {
            let mut opts = String::new();
            for s in &scripts {
                let _ = write!(opts, r#"<option value="script:{}">{} ({})</option>"#, s.id, esc(&s.name), s.shell.as_str());
            }
            let _ = write!(
                b,
                r#"<form class="bar" method="post" action="/jobs"><select name="source">{opts}</select>
<input type="hidden" name="target" value="devices"><input type="hidden" name="device" value="{id}">
<input type="hidden" name="when" value="now"><input type="hidden" name="expires_h" value="24">
<button>Run script</button><a class="muted" href="/jobs/new?device={id}">More options…</a></form>"#
            );
        } else {
            b.push_str(r#"<p class="muted"><a class="muted" href="/scripts/new">Add a script</a> to run it here.</p>"#);
        }
        let windows = d.os.eq_ignore_ascii_case("windows")
            || d.inv_os.as_deref().is_some_and(|o| o.to_ascii_lowercase().contains("windows"));
        let host = esc(&d.hostname);
        let mut power = String::new();
        // Each button posts a now-job for this device. Reboot and shutdown ask for the
        // hostname; the server checks it too.
        let button = |action: &str, label: &str, check: &str| {
            format!(
                r#"<form method="post" action="/jobs" onsubmit="{check}"><input type="hidden" name="source" value="action:{action}">
<input type="hidden" name="target" value="devices"><input type="hidden" name="device" value="{id}">
<input type="hidden" name="when" value="now"><input type="hidden" name="expires_h" value="0.25">
<input type="hidden" name="delay_s" value="0"><input type="hidden" name="confirm" value=""><button>{label}</button></form>"#
            )
        };
        let typed = |what: &str| {
            format!(
                "const v = prompt('Type the hostname {host} to {what} it.'); if (v === null) return false; this.confirm.value = v; this.delay_s.value = document.getElementById('pdelay').value || 0; return true;"
            )
        };
        if online {
            power.push_str(&button("reboot", "Reboot", &typed("reboot")));
            power.push_str(&button("shutdown", "Shut down", &typed("shut down")));
            if windows {
                power.push_str(&button("logoff", "Sign out user", "return confirm('Sign out the signed-in user?')"));
                power.push_str(&button("lock", "Lock", "return confirm('Lock the screen?')"));
            }
            power.push_str(r#"<label class="muted">delay <input id="pdelay" type="number" min="0" max="86400" value="0" style="width:80px">s</label>"#);
        } else {
            power.push_str(&button("wake", "Wake (Wake-on-LAN)", "return true"));
        }
        let _ = write!(b, r#"<div class="power">{power}</div>"#);
    }
    let runs = app.db.device_runs(&d.id, 10).unwrap_or_default();
    if runs.is_empty() {
        b.push_str(r#"<p class="muted">No jobs have run here.</p>"#);
    } else {
        b.push_str(r#"<div class="tablewrap"><table><thead><tr><th>Job</th><th>Status</th><th>Exit</th><th>When</th></tr></thead><tbody>"#);
        for (r, job_id, job_name) in &runs {
            let _ = write!(
                b,
                r#"<tr><td><a class="name" href="/jobs/{job_id}">{}</a></td><td>{}</td><td>{}</td><td>{}</td></tr>"#,
                esc(job_name),
                badge(&r.status),
                r.exit_code.map(|c| c.to_string()).unwrap_or_default(),
                ago(now - r.queued),
            );
        }
        b.push_str("</tbody></table></div>");
    }
    b.push_str("</section>");
    b
}
