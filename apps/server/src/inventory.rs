//! Device inventory: the detail page, on-demand refresh and the CSV export.

use crate::auth::require;
use crate::db::Role;
use crate::ui::{ago, db_error, esc, page};
use crate::{now, send_to_agent, Shared};
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use deskhand_proto::{Inventory, ServerMsg};
use std::fmt::Write;

fn gib(bytes: u64) -> String {
    let g = bytes as f64 / (1u64 << 30) as f64;
    if g >= 100.0 {
        format!("{g:.0} GiB")
    } else {
        format!("{g:.1} GiB")
    }
}

fn duration(s: u64) -> String {
    let (d, h, m) = (s / 86400, s % 86400 / 3600, s % 3600 / 60);
    match (d, h) {
        (0, 0) => format!("{m}m"),
        (0, _) => format!("{h}h {m}m"),
        _ => format!("{d}d {h}h"),
    }
}

fn opt(v: &Option<String>) -> String {
    match v {
        Some(s) => esc(s),
        None => r#"<span class="muted">unknown</span>"#.into(),
    }
}

/// A timestamp the browser shows in local time.
fn when(ts: i64) -> String {
    format!(r#"<span data-ts="{ts}">{}</span>"#, crate::iso_datetime(ts))
}

fn badge(ok: Option<bool>, yes: &str, no: &str) -> String {
    match ok {
        Some(true) => format!(r#"<span class="badge ok">{yes}</span>"#),
        Some(false) => format!(r#"<span class="badge bad">{no}</span>"#),
        None => r#"<span class="badge">unknown</span>"#.into(),
    }
}

fn meter(used: u64, total: u64) -> String {
    let pct = if total > 0 { (used as f64 / total as f64 * 100.0).round() as u64 } else { 0 };
    let class = if pct >= 90 { " bad" } else if pct >= 75 { " warn" } else { "" };
    format!(r#"<span class="meter{class}"><span style="width:{pct}%"></span></span> {pct}%"#)
}

fn kv(b: &mut String, rows: &[(&str, String)]) {
    b.push_str(r#"<table class="kv"><tbody>"#);
    for (k, v) in rows {
        let _ = write!(b, "<tr><th>{k}</th><td>{v}</td></tr>");
    }
    b.push_str("</tbody></table>");
}

#[derive(serde::Deserialize)]
pub struct DetailQuery {
    /// Set after a refresh: the inventory time to wait past.
    since: Option<i64>,
    #[serde(default)]
    tries: u32,
}

pub async fn detail(
    State(app): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<DetailQuery>,
) -> Response {
    let user = match require(&app, &headers, Role::Viewer) {
        Ok(u) => u,
        Err(r) => return r,
    };
    let device = match app.db.device(&id) {
        Ok(Some(d)) => d,
        Ok(None) => return (StatusCode::NOT_FOUND, "no such device").into_response(),
        Err(e) => return db_error(e),
    };
    let snapshot = match app.db.inventory(&id) {
        Ok(s) => s,
        Err(e) => return db_error(e),
    };
    let snapshots = app.db.inventory_count(&id).unwrap_or(0);
    let online = app.agents.lock().unwrap().contains_key(&id);
    let now = now();
    let eid = esc(&id);

    let mut b = String::new();
    let status = if online {
        r#"<span class="dot on"></span> online"#.to_owned()
    } else {
        format!(r#"<span class="dot off"></span> seen {}"#, ago(now - device.last_seen))
    };
    let mut acts = String::new();
    if user.role >= Role::Operator && online {
        let _ = write!(
            acts,
            r#"<a href="/d/{eid}/">Desktop</a><a href="/d/{eid}/shell">Shell</a><form method="post" action="/devices/{eid}/refresh"><button>Refresh inventory</button></form>"#
        );
    }
    let collected = match &snapshot {
        Some((ts, _)) => format!("inventory from {} · {snapshots} kept", ago(now - ts)),
        None => "no inventory yet".into(),
    };
    let _ = write!(
        b,
        r#"<div class="row dev"><div><a class="muted" href="/">← Devices</a><h2>{name}</h2><div class="muted">{status} · {group} · {collected}</div></div><span class="acts">{acts}</span></div>"#,
        name = esc(&device.name),
        group = esc(device.group.as_deref().unwrap_or("no group")),
    );

    // After a refresh, reload until the new snapshot lands (or give up after ~20s).
    let current = snapshot.as_ref().map_or(0, |(ts, _)| *ts);
    if let Some(since) = q.since {
        if current <= since && online && q.tries < 10 {
            let _ = write!(
                b,
                r#"<p class="muted">Collecting inventory…</p><script>setTimeout(()=>location.replace('/devices/{eid}?since={since}&tries={}'),2000)</script>"#,
                q.tries + 1
            );
        } else if current <= since {
            b.push_str(r#"<p class="muted">The device didn't send a new inventory yet.</p>"#);
        }
    }

    b.push_str(&crate::jobs_ui::device_card(&app, &user, &device, online));
    let Some((_, inv)) = snapshot else {
        b.push_str(r#"<p class="empty">The agent sends its inventory when it connects. Agents older than this server don't send one.</p>"#);
        return page(&user, "devices", &device.name, &b);
    };
    b.push_str(&sections(&inv));
    page(&user, "devices", &device.name, &b)
}

fn sections(inv: &Inventory) -> String {
    let mut b = String::from(
        r##"<nav class="tabs"><a href="#overview">Overview</a><a href="#hardware">Hardware</a><a href="#network">Network</a><a href="#software">Software</a><a href="#security">Security</a></nav>"##,
    );

    // Overview
    b.push_str(r#"<section class="card" id="overview"><h2>Overview</h2>"#);
    let os = [&inv.os.name, &inv.os.version]
        .iter()
        .filter_map(|v| v.as_deref())
        .collect::<Vec<_>>()
        .join(" ");
    let users = if inv.logged_in_users.is_empty() {
        r#"<span class="muted">nobody</span>"#.to_owned()
    } else {
        esc(&inv.logged_in_users.join(", "))
    };
    kv(
        &mut b,
        &[
            ("Hostname", opt(&inv.hostname)),
            ("OS", if os.is_empty() { opt(&None) } else { esc(&os) }),
            ("Build", opt(&inv.os.build)),
            ("Kernel", opt(&inv.os.kernel)),
            ("Architecture", opt(&inv.os.arch)),
            ("Domain", opt(&inv.domain)),
            ("Logged in", users),
            ("Uptime", inv.uptime_secs.map_or_else(|| opt(&None), duration)),
            ("Last boot", inv.last_boot.map_or_else(|| opt(&None), when)),
            ("Public IP", opt(&inv.public_ip)),
            ("Collected", when(inv.collected_at)),
        ],
    );
    b.push_str("</section>");

    // Hardware
    b.push_str(r#"<section class="card" id="hardware"><h2>Hardware</h2>"#);
    let cores = match (inv.cpu.cores, inv.cpu.threads) {
        (Some(c), Some(t)) => format!("{c} cores, {t} threads"),
        (None, Some(t)) => format!("{t} threads"),
        (Some(c), None) => format!("{c} cores"),
        (None, None) => opt(&None),
    };
    let ram = match (inv.memory.used, inv.memory.total) {
        (Some(u), Some(t)) => format!("{} of {} {}", gib(u), gib(t), meter(u, t)),
        (_, Some(t)) => gib(t),
        _ => opt(&None),
    };
    kv(
        &mut b,
        &[
            ("Manufacturer", opt(&inv.hardware.manufacturer)),
            ("Model", opt(&inv.hardware.model)),
            ("Serial", opt(&inv.hardware.serial)),
            ("CPU", opt(&inv.cpu.model)),
            ("Cores", cores),
            ("Memory", ram),
        ],
    );
    b.push_str(r#"<h3>Disks</h3>"#);
    if inv.disks.is_empty() {
        b.push_str(r#"<p class="empty">None reported.</p>"#);
    } else {
        b.push_str(r#"<div class="tablewrap"><table><thead><tr><th>Mount</th><th>File system</th><th>Size</th><th>Free</th><th>Used</th></tr></thead><tbody>"#);
        for d in &inv.disks {
            let _ = write!(
                b,
                "<tr><td><code>{}</code></td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
                esc(&d.mount),
                esc(d.fs.as_deref().unwrap_or("")),
                gib(d.total),
                gib(d.free),
                meter(d.total.saturating_sub(d.free), d.total),
            );
        }
        b.push_str("</tbody></table></div>");
    }
    b.push_str("</section>");

    // Network
    b.push_str(r#"<section class="card" id="network"><h2>Network</h2>"#);
    if inv.network.is_empty() {
        b.push_str(r#"<p class="empty">None reported.</p>"#);
    } else {
        b.push_str(r#"<div class="tablewrap"><table><thead><tr><th>Interface</th><th>MAC</th><th>Addresses</th></tr></thead><tbody>"#);
        for n in &inv.network {
            let _ = write!(
                b,
                "<tr><td>{}</td><td><code>{}</code></td><td>{}</td></tr>",
                esc(&n.name),
                esc(n.mac.as_deref().unwrap_or("")),
                n.ips.iter().map(|i| format!("<code>{}</code>", esc(i))).collect::<Vec<_>>().join("<br>"),
            );
        }
        b.push_str("</tbody></table></div>");
    }
    b.push_str("</section>");

    // Software
    let _ = write!(
        b,
        r#"<section class="card" id="software"><h2>Software <span class="muted">{}</span></h2>"#,
        inv.software.len()
    );
    if inv.software.is_empty() {
        b.push_str(r#"<p class="empty">None reported.</p>"#);
    } else {
        b.push_str(r#"<input id="swq" type="search" placeholder="Filter by name, version or publisher" autocomplete="off"><div class="tablewrap"><table id="sw"><thead><tr><th>Name</th><th>Version</th><th>Publisher</th><th>Installed</th></tr></thead><tbody>"#);
        for s in &inv.software {
            let _ = write!(
                b,
                "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
                esc(&s.name),
                esc(s.version.as_deref().unwrap_or("")),
                esc(s.publisher.as_deref().unwrap_or("")),
                esc(s.install_date.as_deref().unwrap_or("")),
            );
        }
        b.push_str(r#"</tbody></table></div><script>{const q=document.getElementById('swq'),rows=[...document.querySelectorAll('#sw tbody tr')];q.addEventListener('input',()=>{const t=q.value.toLowerCase();for(const r of rows)r.hidden=t&&!r.textContent.toLowerCase().includes(t)})}</script>"#);
    }
    b.push_str("</section>");

    // Security
    let s = &inv.security;
    b.push_str(r#"<section class="card" id="security"><h2>Security</h2>"#);
    let encryption = match &s.encryption {
        None => r#"<span class="badge">unknown</span> <span class="muted">(the agent needs admin rights to read BitLocker)</span>"#.to_owned(),
        Some(v) if v.is_empty() => r#"<span class="badge warn">no encrypted volumes found</span>"#.to_owned(),
        Some(v) => v
            .iter()
            .map(|e| {
                let class = match e.status.as_str() {
                    "on" | "luks" | "luks1" | "luks2" => "ok",
                    "off" => "bad",
                    _ => "",
                };
                format!(r#"<span class="badge {class}">{} {}</span>"#, esc(&e.volume), esc(&e.status))
            })
            .collect::<Vec<_>>()
            .join(" "),
    };
    let av = match &s.antivirus {
        None => opt(&None),
        Some(a) => {
            let age = match a.signature_age_days {
                Some(d) if d > 7 => format!(r#" <span class="badge warn">signatures {d} days old</span>"#),
                Some(d) => format!(r#" <span class="muted">signatures {d} days old</span>"#),
                None => String::new(),
            };
            format!(
                "{} {} {}{age}",
                esc(&a.name),
                badge(a.enabled, "enabled", "disabled"),
                badge(a.realtime, "real-time on", "real-time off"),
            )
        }
    };
    let firewall = if s.firewall.is_empty() {
        r#"<span class="badge">none found</span>"#.to_owned()
    } else {
        s.firewall
            .iter()
            .map(|f| format!("{} {}", esc(&f.profile), badge(f.enabled, "on", "off")))
            .collect::<Vec<_>>()
            .join(" &nbsp; ")
    };
    let reboot = match s.pending_reboot {
        Some(true) => r#"<span class="badge warn">reboot pending</span>"#.to_owned(),
        Some(false) => r#"<span class="badge ok">no</span>"#.to_owned(),
        None => r#"<span class="badge">unknown</span>"#.to_owned(),
    };
    kv(
        &mut b,
        &[("Disk encryption", encryption), ("Antivirus", av), ("Firewall", firewall), ("Pending reboot", reboot)],
    );
    b.push_str("</section>");
    b.push_str(r#"<script>for(const e of document.querySelectorAll('[data-ts]'))e.textContent=new Date(e.dataset.ts*1000).toLocaleString()</script>"#);
    b
}

/// `POST /devices/{id}/refresh`: asks the agent for a fresh inventory.
pub async fn refresh(State(app): State<Shared>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    let user = match require(&app, &headers, Role::Operator) {
        Ok(u) => u,
        Err(r) => return r,
    };
    let since = app.db.inventory(&id).ok().flatten().map_or(0, |(ts, _)| ts);
    if !send_to_agent(&app, &id, ServerMsg::RefreshInventory) {
        return (StatusCode::CONFLICT, "device offline").into_response();
    }
    app.db.audit(now(), Some(user.id), Some(&id), "device.inventory_refresh", "");
    Redirect::to(&format!("/devices/{id}?since={since}")).into_response()
}

/// A CSV field: quoted when needed, and with a leading `'` on anything a spreadsheet would
/// run as a formula.
fn field(s: &str) -> String {
    let s = if s.starts_with(['=', '+', '-', '@', '\t', '\r']) { format!("'{s}") } else { s.to_owned() };
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s
    }
}

/// `GET /devices.csv`: the device list with key inventory fields.
pub async fn csv(State(app): State<Shared>, headers: HeaderMap) -> Response {
    if let Err(r) = require(&app, &headers, Role::Viewer) {
        return r;
    }
    let (devices, invs) = match (app.db.devices(None), app.db.latest_inventories()) {
        (Ok(d), Ok(i)) => (d, i),
        (Err(e), _) | (_, Err(e)) => return db_error(e),
    };
    let online: std::collections::HashSet<String> = app.agents.lock().unwrap().keys().cloned().collect();
    let mut out = String::from(
        "name,hostname,group,online,last_seen,os,os_version,os_build,manufacturer,model,serial,cpu,cores,ram_gib,disk_gib,disk_free_gib,last_boot,domain,public_ip,logged_in_users,software_count,inventory_at\r\n",
    );
    let empty = Inventory::default();
    for d in &devices {
        let inv = invs.get(&d.id).unwrap_or(&empty);
        let has = invs.contains_key(&d.id);
        let o = |v: &Option<String>| v.clone().unwrap_or_default();
        let gb = |n: u64| format!("{:.1}", n as f64 / (1u64 << 30) as f64);
        let disk_total: u64 = inv.disks.iter().map(|d| d.total).sum();
        let disk_free: u64 = inv.disks.iter().map(|d| d.free).sum();
        let row = [
            d.name.clone(),
            d.hostname.clone(),
            d.group.clone().unwrap_or_default(),
            online.contains(&d.id).to_string(),
            crate::iso_datetime(d.last_seen),
            o(&inv.os.name),
            o(&inv.os.version),
            o(&inv.os.build),
            o(&inv.hardware.manufacturer),
            o(&inv.hardware.model),
            o(&inv.hardware.serial),
            o(&inv.cpu.model),
            inv.cpu.cores.map(|c| c.to_string()).unwrap_or_default(),
            inv.memory.total.map(gb).unwrap_or_default(),
            if has { gb(disk_total) } else { String::new() },
            if has { gb(disk_free) } else { String::new() },
            inv.last_boot.map(crate::iso_datetime).unwrap_or_default(),
            o(&inv.domain),
            o(&inv.public_ip),
            inv.logged_in_users.join(" "),
            if has { inv.software.len().to_string() } else { String::new() },
            if has { crate::iso_datetime(inv.collected_at) } else { String::new() },
        ];
        out.push_str(&row.iter().map(|f| field(f)).collect::<Vec<_>>().join(","));
        out.push_str("\r\n");
    }
    (
        [
            (header::CONTENT_TYPE, "text/csv; charset=utf-8".to_owned()),
            (header::CONTENT_DISPOSITION, format!("attachment; filename=\"devices-{}.csv\"", &crate::iso_datetime(now())[..10])),
        ],
        out,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::field;

    #[test]
    fn csv_fields() {
        assert_eq!(field("plain"), "plain");
        assert_eq!(field("a,b"), "\"a,b\"");
        assert_eq!(field("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(field("=cmd()"), "'=cmd()");
        assert_eq!(field("-1"), "'-1");
    }

    #[test]
    fn iso() {
        assert_eq!(crate::iso_datetime(0), "1970-01-01T00:00:00Z");
        assert_eq!(crate::iso_datetime(1_762_905_600), "2025-11-12T00:00:00Z");
        assert_eq!(crate::iso_datetime(951_782_400), "2000-02-29T00:00:00Z");
    }
}
