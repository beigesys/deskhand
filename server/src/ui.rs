//! The admin pages: devices, groups, enrollment, users and the audit log. Server-rendered
//! HTML; forms post back and redirect.

use crate::auth::{base_url, hash_password, require};
use crate::db::{Role, User};
use crate::{now, random_hex, Shared};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::Form;
use std::fmt::Write;

pub fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

pub fn ago(s: i64) -> String {
    match s.max(0) {
        s @ 0..=59 => format!("{s}s ago"),
        s @ 60..=3599 => format!("{}m ago", s / 60),
        s @ 3600..=86399 => format!("{}h ago", s / 3600),
        s => format!("{}d ago", s / 86400),
    }
}

fn until(s: i64) -> String {
    match s.max(0) {
        s @ 0..=3599 => format!("in {}m", s / 60),
        s @ 3600..=86399 => format!("in {}h", s / 3600),
        s => format!("in {}d", s / 86400),
    }
}

fn db_error(e: rusqlite::Error) -> Response {
    log::error!("db: {e}");
    (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
}

/// The page frame: header with navigation for the user's role, then `body`.
fn page(user: &User, active: &str, title: &str, body: &str) -> Response {
    let mut nav = String::new();
    let mut links = vec![("devices", "/", "Devices")];
    if user.role == Role::Admin {
        links.extend([
            ("groups", "/groups", "Groups"),
            ("enroll", "/enroll", "Enroll"),
            ("users", "/users", "Users"),
            ("audit", "/audit", "Audit"),
        ]);
    }
    for (key, href, label) in links {
        let class = if key == active { " class=\"on\"" } else { "" };
        let _ = write!(nav, r#"<a href="{href}"{class}>{label}</a>"#);
    }
    Html(
        include_str!("page.html")
            .replace("{{title}}", &esc(title))
            .replace("{{nav}}", &nav)
            .replace("{{user}}", &format!("{} · {}", esc(&user.username), user.role.as_str()))
            .replace("{{body}}", body),
    )
    .into_response()
}

fn group_options(groups: &[crate::db::Group], selected: Option<i64>, none_label: &str) -> String {
    let mut o = format!(r#"<option value="">{none_label}</option>"#);
    for g in groups {
        let sel = if Some(g.id) == selected { " selected" } else { "" };
        let _ = write!(o, r#"<option value="{}"{sel}>{}</option>"#, g.id, esc(&g.name));
    }
    o
}

/// Empty form fields mean "none".
fn opt_id(s: &Option<String>) -> Option<i64> {
    s.as_deref().filter(|s| !s.is_empty()).and_then(|s| s.parse().ok())
}

// --- devices ---

#[derive(serde::Deserialize)]
pub struct DeviceFilter {
    group: Option<String>,
}

pub async fn devices(State(app): State<Shared>, headers: HeaderMap, Query(f): Query<DeviceFilter>) -> Response {
    let user = match require(&app, &headers, Role::Viewer) {
        Ok(u) => u,
        Err(r) => return r,
    };
    let group = opt_id(&f.group);
    let (devices, groups) = match (app.db.devices(group), app.db.groups()) {
        (Ok(d), Ok(g)) => (d, g),
        (Err(e), _) | (_, Err(e)) => return db_error(e),
    };
    let online: std::collections::HashMap<String, i64> = app
        .agents
        .lock()
        .unwrap()
        .iter()
        .map(|(id, a)| (id.clone(), a.since))
        .collect();
    let now = now();
    let mut b = String::new();
    let _ = write!(
        b,
        r#"<form class="bar" method="get"><select name="group" onchange="this.form.submit()">{}</select><span class="muted">{} devices, {} online</span></form>"#,
        group_options(&groups, group, "All groups"),
        devices.len(),
        devices.iter().filter(|d| online.contains_key(&d.id)).count(),
    );
    if devices.is_empty() {
        b.push_str(r#"<p class="empty">No devices yet. Create an enrollment token on the Enroll page and run the install command on a machine.</p>"#);
    }
    for d in &devices {
        let id = esc(&d.id);
        let (dot, status) = match online.get(&d.id) {
            Some(since) => ("on", format!("online {}", ago(now - since).trim_end_matches(" ago"))),
            None => ("off", format!("seen {}", ago(now - d.last_seen))),
        };
        let acts = if user.role >= Role::Operator && online.contains_key(&d.id) {
            format!(r#"<a href="/d/{id}/">Desktop</a><a href="/d/{id}/shell">Shell</a>"#)
        } else {
            String::new()
        };
        let manage = if user.role == Role::Admin {
            format!(
                r#"<details><summary>Manage</summary><div class="manage">
<form method="post" action="/devices/{id}/rename"><input name="name" value="{name}" required><button>Rename</button></form>
<form method="post" action="/devices/{id}/move"><select name="group">{groups}</select><button>Move</button></form>
<form method="post" action="/devices/{id}/delete" onsubmit="return confirm('Remove this device? Its agent will need to enroll again.')"><button class="danger">Remove</button></form>
</div></details>"#,
                name = esc(&d.name),
                groups = group_options(&groups, d.group_id, "No group"),
            )
        } else {
            String::new()
        };
        let _ = write!(
            b,
            r#"<div class="row dev"><span class="dot {dot}"></span><div><b>{name}</b><div class="muted">{host} · {os} · {group}</div></div><span class="muted">{status}</span><span class="acts">{acts}</span>{manage}</div>"#,
            name = esc(&d.name),
            host = esc(&d.hostname),
            os = esc(&d.os),
            group = esc(d.group.as_deref().unwrap_or("no group")),
        );
    }
    b.push_str(r#"<script>setInterval(()=>{if(!document.querySelector('details[open]')&&!document.querySelector('select:focus'))location.reload()},15000)</script>"#);
    page(&user, "devices", "Devices", &b)
}

#[derive(serde::Deserialize)]
pub struct NameForm {
    name: String,
}

pub async fn rename_device(
    State(app): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(f): Form<NameForm>,
) -> Response {
    let user = match require(&app, &headers, Role::Admin) {
        Ok(u) => u,
        Err(r) => return r,
    };
    let name = f.name.trim();
    if name.is_empty() {
        return Redirect::to("/").into_response();
    }
    match app.db.rename_device(&id, name) {
        Ok(1) => app.db.audit(now(), Some(user.id), Some(&id), "device.rename", name),
        Ok(_) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return db_error(e),
    }
    Redirect::to("/").into_response()
}

#[derive(serde::Deserialize)]
pub struct GroupForm {
    group: Option<String>,
}

pub async fn move_device(
    State(app): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(f): Form<GroupForm>,
) -> Response {
    let user = match require(&app, &headers, Role::Admin) {
        Ok(u) => u,
        Err(r) => return r,
    };
    let group = opt_id(&f.group);
    if let Some(g) = group {
        if !app.db.group_exists(g).unwrap_or(false) {
            return (StatusCode::BAD_REQUEST, "no such group").into_response();
        }
    }
    match app.db.move_device(&id, group) {
        Ok(1) => app.db.audit(
            now(),
            Some(user.id),
            Some(&id),
            "device.move",
            &group.map_or("no group".into(), |g| format!("group {g}")),
        ),
        Ok(_) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return db_error(e),
    }
    Redirect::to("/").into_response()
}

pub async fn delete_device(State(app): State<Shared>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    let user = match require(&app, &headers, Role::Admin) {
        Ok(u) => u,
        Err(r) => return r,
    };
    let name = app.db.device_name(&id).ok().flatten().unwrap_or_default();
    match app.db.delete_device(&id) {
        Ok(1) => {
            // Dropping its sender ends the agent's control socket.
            app.agents.lock().unwrap().remove(&id);
            app.db.audit(now(), Some(user.id), None, "device.remove", &format!("{name} ({id})"));
        }
        Ok(_) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return db_error(e),
    }
    Redirect::to("/").into_response()
}

// --- groups ---

pub async fn groups(State(app): State<Shared>, headers: HeaderMap) -> Response {
    let user = match require(&app, &headers, Role::Admin) {
        Ok(u) => u,
        Err(r) => return r,
    };
    let groups = match app.db.groups() {
        Ok(g) => g,
        Err(e) => return db_error(e),
    };
    let mut b = String::from(
        r#"<form class="bar" method="post" action="/groups"><input name="name" placeholder="New group name" required><button>Add group</button></form>"#,
    );
    if groups.is_empty() {
        b.push_str(r#"<p class="empty">No groups yet.</p>"#);
    }
    for g in &groups {
        let _ = write!(
            b,
            r#"<div class="row"><div><b>{name}</b></div><a class="muted" href="/?group={id}">{n} devices</a><form method="post" action="/groups/{id}/delete" onsubmit="return confirm('Delete this group? Its devices keep running, with no group.')"><button class="danger">Delete</button></form></div>"#,
            id = g.id,
            name = esc(&g.name),
            n = g.devices,
        );
    }
    page(&user, "groups", "Groups", &b)
}

pub async fn add_group(State(app): State<Shared>, headers: HeaderMap, Form(f): Form<NameForm>) -> Response {
    let user = match require(&app, &headers, Role::Admin) {
        Ok(u) => u,
        Err(r) => return r,
    };
    let name = f.name.trim();
    if !name.is_empty() {
        match app.db.add_group(name) {
            Ok(_) => app.db.audit(now(), Some(user.id), None, "group.add", name),
            Err(rusqlite::Error::SqliteFailure(e, _)) if e.code == rusqlite::ErrorCode::ConstraintViolation => {
                return (StatusCode::CONFLICT, "a group with that name exists").into_response()
            }
            Err(e) => return db_error(e),
        }
    }
    Redirect::to("/groups").into_response()
}

pub async fn delete_group(State(app): State<Shared>, headers: HeaderMap, Path(id): Path<i64>) -> Response {
    let user = match require(&app, &headers, Role::Admin) {
        Ok(u) => u,
        Err(r) => return r,
    };
    match app.db.delete_group(id) {
        Ok(_) => app.db.audit(now(), Some(user.id), None, "group.delete", &format!("group {id}")),
        Err(e) => return db_error(e),
    }
    Redirect::to("/groups").into_response()
}

// --- enrollment ---

fn enroll_page(app: &crate::App, headers: &HeaderMap, user: &User, new_token: Option<&str>) -> Response {
    let (tokens, groups) = match (app.db.enroll_tokens(now()), app.db.groups()) {
        (Ok(t), Ok(g)) => (t, g),
        (Err(e), _) | (_, Err(e)) => return db_error(e),
    };
    let mut b = String::new();
    if let Some(token) = new_token {
        let base = base_url(app, headers);
        let t = esc(token);
        let base = esc(&base);
        let _ = write!(
            b,
            r#"<section class="card new"><h2>New token</h2><p class="muted">Shown once. Copy it now.</p><pre>{t}</pre>
<h3>Linux (as root)</h3><pre>curl -fsSL {base}/dl/ithub-agent-linux -o /usr/local/bin/ithub-agent &amp;&amp; chmod +x /usr/local/bin/ithub-agent &amp;&amp; ithub-agent enroll --server {base} --token {t} &amp;&amp; ithub-agent</pre>
<h3>Windows (PowerShell as Administrator)</h3><pre>New-Item -ItemType Directory -Force "$env:ProgramFiles\ithub" | Out-Null; iwr {base}/dl/ithub-agent-windows.exe -OutFile "$env:ProgramFiles\ithub\ithub-agent.exe"; &amp; "$env:ProgramFiles\ithub\ithub-agent.exe" enroll --server {base} --token {t}; &amp; "$env:ProgramFiles\ithub\ithub-agent.exe"</pre></section>"#
        );
    }
    let _ = write!(
        b,
        r#"<section class="card"><h2>Create a token</h2><form class="grid" method="post" action="/enroll">
<label>Group <select name="group">{groups}</select></label>
<label>Expires in <select name="hours"><option value="1">1 hour</option><option value="24" selected>1 day</option><option value="168">7 days</option><option value="720">30 days</option></select></label>
<label>Max uses <input name="max_uses" type="number" min="1" max="100000" value="1" required></label>
<button>Create token</button></form></section>"#,
        groups = group_options(&groups, None, "No group"),
    );
    b.push_str(r#"<h2>Active tokens</h2>"#);
    if tokens.is_empty() {
        b.push_str(r#"<p class="empty">None.</p>"#);
    }
    let now = now();
    for t in &tokens {
        let _ = write!(
            b,
            r#"<div class="row"><div><b>{group}</b><div class="muted">used {uses} of {max} · expires {exp}</div></div><form method="post" action="/enroll/{id}/revoke"><button class="danger">Revoke</button></form></div>"#,
            id = t.id,
            group = esc(t.group.as_deref().unwrap_or("No group")),
            uses = t.uses,
            max = t.max_uses,
            exp = until(t.expires - now),
        );
    }
    page(user, "enroll", "Enroll", &b)
}

pub async fn enroll(State(app): State<Shared>, headers: HeaderMap) -> Response {
    match require(&app, &headers, Role::Admin) {
        Ok(u) => enroll_page(&app, &headers, &u, None),
        Err(r) => r,
    }
}

#[derive(serde::Deserialize)]
pub struct TokenForm {
    group: Option<String>,
    hours: i64,
    max_uses: i64,
}

pub async fn create_token(State(app): State<Shared>, headers: HeaderMap, Form(f): Form<TokenForm>) -> Response {
    let user = match require(&app, &headers, Role::Admin) {
        Ok(u) => u,
        Err(r) => return r,
    };
    let group = opt_id(&f.group);
    if group.is_some_and(|g| !app.db.group_exists(g).unwrap_or(false)) {
        return (StatusCode::BAD_REQUEST, "no such group").into_response();
    }
    let hours = f.hours.clamp(1, 24 * 365);
    let max_uses = f.max_uses.clamp(1, 100_000);
    let token = random_hex(20);
    let now = now();
    if let Err(e) = app.db.add_enroll_token(&token, group, now + hours * 3600, max_uses, now) {
        return db_error(e);
    }
    app.db.audit(
        now,
        Some(user.id),
        None,
        "enroll_token.create",
        &format!("{max_uses} uses, {hours}h, group {}", group.map_or("none".into(), |g| g.to_string())),
    );
    enroll_page(&app, &headers, &user, Some(&token))
}

pub async fn revoke_token(State(app): State<Shared>, headers: HeaderMap, Path(id): Path<i64>) -> Response {
    let user = match require(&app, &headers, Role::Admin) {
        Ok(u) => u,
        Err(r) => return r,
    };
    match app.db.revoke_enroll_token(id) {
        Ok(_) => app.db.audit(now(), Some(user.id), None, "enroll_token.revoke", &format!("token {id}")),
        Err(e) => return db_error(e),
    }
    Redirect::to("/enroll").into_response()
}

// --- users ---

pub async fn users(State(app): State<Shared>, headers: HeaderMap) -> Response {
    let user = match require(&app, &headers, Role::Admin) {
        Ok(u) => u,
        Err(r) => return r,
    };
    let users = match app.db.users() {
        Ok(u) => u,
        Err(e) => return db_error(e),
    };
    let roles = |sel: Role| {
        [Role::Viewer, Role::Operator, Role::Admin]
            .iter()
            .map(|r| {
                let s = if *r == sel { " selected" } else { "" };
                format!(r#"<option{s}>{}</option>"#, r.as_str())
            })
            .collect::<String>()
    };
    let mut b = format!(
        r#"<section class="card"><h2>Add a user</h2><form class="grid" method="post" action="/users">
<label>User name <input name="username" required autocomplete="off"></label>
<label>Password <input name="password" type="password" minlength="8" required autocomplete="new-password"></label>
<label>Role <select name="role">{}</select></label><button>Add user</button></form>
<p class="muted">Viewers see the device list. Operators can also open desktops and shells. Admins manage everything.</p></section>"#,
        roles(Role::Operator)
    );
    let now = now();
    for (u, created) in &users {
        let you = if u.id == user.id { r#" <span class="muted">(you)</span>"# } else { "" };
        let _ = write!(
            b,
            r#"<div class="row"><div><b>{name}</b>{you}<div class="muted">added {ago}</div></div>
<form method="post" action="/users/{id}/role"><select name="role" onchange="this.form.submit()">{roles}</select></form>
<form method="post" action="/users/{id}/delete" onsubmit="return confirm('Delete this user?')"><button class="danger">Delete</button></form></div>"#,
            id = u.id,
            name = esc(&u.username),
            ago = ago(now - created),
            roles = roles(u.role),
        );
    }
    page(&user, "users", "Users", &b)
}

#[derive(serde::Deserialize)]
pub struct NewUser {
    username: String,
    password: String,
    role: String,
}

pub async fn add_user(State(app): State<Shared>, headers: HeaderMap, Form(f): Form<NewUser>) -> Response {
    let user = match require(&app, &headers, Role::Admin) {
        Ok(u) => u,
        Err(r) => return r,
    };
    let username = f.username.trim();
    let Some(role) = Role::parse(&f.role) else {
        return (StatusCode::BAD_REQUEST, "unknown role").into_response();
    };
    if username.is_empty() || f.password.len() < 8 {
        return (StatusCode::BAD_REQUEST, "user name required and password must be 8+ characters").into_response();
    }
    match app.db.add_user(username, &hash_password(&f.password), role, now()) {
        Ok(_) => app.db.audit(now(), Some(user.id), None, "user.add", &format!("{username} as {}", role.as_str())),
        Err(rusqlite::Error::SqliteFailure(e, _)) if e.code == rusqlite::ErrorCode::ConstraintViolation => {
            return (StatusCode::CONFLICT, "that user name is taken").into_response()
        }
        Err(e) => return db_error(e),
    }
    Redirect::to("/users").into_response()
}

#[derive(serde::Deserialize)]
pub struct RoleForm {
    role: String,
}

/// Whether removing admin from `id` (by demoting or deleting) would leave no admins.
fn last_admin(app: &crate::App, id: i64) -> bool {
    let is_admin = app
        .db
        .users()
        .map(|us| us.iter().any(|(u, _)| u.id == id && u.role == Role::Admin))
        .unwrap_or(false);
    is_admin && app.db.admin_count().unwrap_or(0) <= 1
}

pub async fn set_role(
    State(app): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Form(f): Form<RoleForm>,
) -> Response {
    let user = match require(&app, &headers, Role::Admin) {
        Ok(u) => u,
        Err(r) => return r,
    };
    let Some(role) = Role::parse(&f.role) else {
        return (StatusCode::BAD_REQUEST, "unknown role").into_response();
    };
    if role != Role::Admin && last_admin(&app, id) {
        return (StatusCode::CONFLICT, "that's the last admin").into_response();
    }
    match app.db.set_role(id, role) {
        Ok(1) => app.db.audit(now(), Some(user.id), None, "user.role", &format!("user {id} -> {}", role.as_str())),
        Ok(_) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return db_error(e),
    }
    Redirect::to("/users").into_response()
}

pub async fn delete_user(State(app): State<Shared>, headers: HeaderMap, Path(id): Path<i64>) -> Response {
    let user = match require(&app, &headers, Role::Admin) {
        Ok(u) => u,
        Err(r) => return r,
    };
    if last_admin(&app, id) {
        return (StatusCode::CONFLICT, "that's the last admin").into_response();
    }
    let name = app
        .db
        .users()
        .ok()
        .and_then(|us| us.into_iter().find(|(u, _)| u.id == id).map(|(u, _)| u.username))
        .unwrap_or_default();
    match app.db.delete_user(id) {
        Ok(1) => app.db.audit(now(), Some(user.id), None, "user.delete", &name),
        Ok(_) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return db_error(e),
    }
    Redirect::to("/users").into_response()
}

// --- audit ---

pub async fn audit(State(app): State<Shared>, headers: HeaderMap) -> Response {
    let user = match require(&app, &headers, Role::Admin) {
        Ok(u) => u,
        Err(r) => return r,
    };
    let rows = match app.db.audit_rows(500) {
        Ok(r) => r,
        Err(e) => return db_error(e),
    };
    let mut b = String::from(
        r#"<div class="tablewrap"><table><thead><tr><th>When</th><th>User</th><th>Device</th><th>Action</th><th>Detail</th></tr></thead><tbody>"#,
    );
    for r in &rows {
        let _ = write!(
            b,
            r#"<tr><td data-ts="{ts}">{ts}</td><td>{user}</td><td>{dev}</td><td><code>{action}</code></td><td>{detail}</td></tr>"#,
            ts = r.ts,
            user = esc(r.user.as_deref().unwrap_or("")),
            dev = esc(r.device.as_deref().unwrap_or("")),
            action = esc(&r.action),
            detail = esc(&r.detail),
        );
    }
    b.push_str("</tbody></table></div>");
    if rows.is_empty() {
        b.push_str(r#"<p class="empty">Nothing yet.</p>"#);
    }
    b.push_str(r#"<script>for(const td of document.querySelectorAll('td[data-ts]'))td.textContent=new Date(td.dataset.ts*1000).toLocaleString()</script>"#);
    page(&user, "audit", "Audit log", &b)
}
