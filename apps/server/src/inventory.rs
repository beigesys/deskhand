//! Device inventory: the CSV export.

use crate::auth::require;
use crate::db::Role;
use crate::ui::db_error;
use crate::{now, Shared};
use axum::extract::State;
use axum::http::{header, HeaderMap};
use axum::response::{IntoResponse, Response};
use deskhand_proto::Inventory;

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
