//! Runs the server binary against a fresh database and checks `/api/v1` end to end:
//! sign-in, roles, the Origin check and the main endpoints.

use serde_json::{json, Value};
use std::process::{Child, Command};
use std::time::Duration;

struct Server {
    child: Child,
    base: String,
    _dir: TempDir,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct TempDir(std::path::PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn start() -> Server {
    let port = free_port();
    let dir = std::env::temp_dir().join(format!("deskhand-api-test-{port}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let child = Command::new(env!("CARGO_BIN_EXE_deskhand-server"))
        .env("DESKHAND_ADDR", format!("127.0.0.1:{port}"))
        .env("DESKHAND_DB", dir.join("t.db"))
        .env("DESKHAND_ADMIN_PASSWORD", "admin-password")
        .env("DESKHAND_AGENT_SOURCE", "local")
        .env("RUST_LOG", "warn")
        .spawn()
        .unwrap();
    let base = format!("http://127.0.0.1:{port}");
    for _ in 0..100 {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Server { child, base, _dir: TempDir(dir) }
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder().http_status_as_error(false).max_redirects(0).build().into()
}

/// A signed-in client: its session cookie, sent by hand.
struct Client {
    base: String,
    cookie: Option<String>,
}

impl Client {
    fn anon(s: &Server) -> Client {
        Client { base: s.base.clone(), cookie: None }
    }

    fn login(s: &Server, user: &str, pw: &str) -> Client {
        let mut c = Client::anon(s);
        let (status, _) = c.post("/api/v1/login", json!({ "username": user, "password": pw }));
        assert_eq!(status, 200, "login as {user}");
        assert!(c.cookie.is_some());
        c
    }

    fn send(&mut self, method: &str, path: &str, body: Option<Value>, origin: Option<&str>) -> (u16, Value) {
        let url = format!("{}{path}", self.base);
        let a = agent();
        let mut req = match method {
            "GET" => a.get(&url).force_send_body(),
            "POST" => a.post(&url),
            "PUT" => a.put(&url),
            "DELETE" => a.delete(&url).force_send_body(),
            _ => unreachable!(),
        };
        if let Some(c) = &self.cookie {
            req = req.header("cookie", c);
        }
        if let Some(o) = origin {
            req = req.header("origin", o);
        }
        let resp = match body {
            Some(b) => req.send_json(b),
            None => req.send_empty(),
        }
        .unwrap();
        let status = resp.status().as_u16();
        if let Some(set) = resp.headers().get("set-cookie").and_then(|v| v.to_str().ok()) {
            let pair = set.split(';').next().unwrap().to_owned();
            self.cookie = (!pair.ends_with('=')).then_some(pair);
        }
        let mut resp = resp;
        let text = resp.body_mut().read_to_string().unwrap_or_default();
        (status, serde_json::from_str(&text).unwrap_or(Value::String(text)))
    }

    fn get(&mut self, path: &str) -> (u16, Value) {
        self.send("GET", path, None, None)
    }
    fn post(&mut self, path: &str, body: Value) -> (u16, Value) {
        self.send("POST", path, Some(body), None)
    }
    fn put(&mut self, path: &str, body: Value) -> (u16, Value) {
        self.send("PUT", path, Some(body), None)
    }
    fn del(&mut self, path: &str) -> (u16, Value) {
        self.send("DELETE", path, None, None)
    }
}

#[test]
fn api_v1() {
    let s = start();

    // Signed out: 401 with a JSON error; the auth info is public.
    let mut anon = Client::anon(&s);
    let (st, body) = anon.get("/api/v1/me");
    assert_eq!(st, 401);
    assert_eq!(body["error"], "sign in first");
    let (st, body) = anon.get("/api/v1/auth");
    assert_eq!(st, 200);
    assert_eq!(body["password_login"], true);
    assert_eq!(body["setup_required"], false);
    assert_eq!(anon.get("/api/v1/devices").0, 401);
    let (st, _) = anon.post("/api/v1/login", json!({ "username": "admin", "password": "nope" }));
    assert_eq!(st, 401);

    let mut admin = Client::login(&s, "admin", "admin-password");
    let (st, me) = admin.get("/api/v1/me");
    assert_eq!((st, me["role"].as_str()), (200, Some("admin")));

    // Groups, then a token for one.
    let (st, g) = admin.post("/api/v1/groups", json!({ "name": "Office" }));
    assert_eq!(st, 200);
    let gid = g["id"].as_i64().unwrap();
    assert_eq!(admin.post("/api/v1/groups", json!({ "name": "Office" })).0, 409);
    assert_eq!(admin.post("/api/v1/groups", json!({ "name": "  " })).0, 400);
    let (st, groups) = admin.get("/api/v1/groups");
    assert_eq!(st, 200);
    assert_eq!(groups["items"][0]["name"], "Office");
    let (st, tok) = admin.post("/api/v1/enroll-tokens", json!({ "group_id": gid, "hours": 24, "max_uses": 3 }));
    assert_eq!(st, 200);
    assert_eq!(tok["token"].as_str().unwrap().len(), 40);
    let token = tok["token"].as_str().unwrap();
    let base = admin.base.clone();
    assert_eq!(tok["linux"].as_str().unwrap(), format!("curl -fsSL {base}/install.sh | sudo sh -s -- {token}"));
    assert_eq!(tok["windows"].as_str().unwrap(), format!("irm {base}/install.ps1 | iex; Install-Deskhand -Token {token}"));
    assert!(!tok.to_string().contains("deskhand.sh"));
    assert_eq!(tok["binaries"].as_array().unwrap().len(), 4);
    let (_, tokens) = admin.get("/api/v1/enroll-tokens");
    assert_eq!(tokens["items"].as_array().unwrap().len(), 1);

    // Enroll a device with it (the agent's own endpoint), then see it listed.
    let (st, enrolled) = anon.post("/api/enroll", json!({ "token": tok["token"], "hostname": "test-host", "os": "linux" }));
    assert_eq!(st, 200);
    let dev = enrolled["device_id"].as_str().unwrap().to_owned();
    let (st, devices) = admin.get("/api/v1/devices");
    assert_eq!(st, 200);
    assert_eq!(devices["items"][0]["hostname"], "test-host");
    assert_eq!(devices["items"][0]["online"], false);
    assert_eq!(devices["items"][0]["group"], "Office");
    assert_eq!(devices["total"], 1);
    assert_eq!(admin.get("/api/v1/devices?q=nomatch").1["items"].as_array().unwrap().len(), 0);
    assert_eq!(admin.get("/api/v1/devices?q=TEST-h").1["items"].as_array().unwrap().len(), 1);
    assert_eq!(admin.get("/api/v1/devices?status=online").1["items"].as_array().unwrap().len(), 0);
    assert_eq!(admin.get("/api/v1/devices?status=offline").1["items"].as_array().unwrap().len(), 1);
    assert_eq!(admin.get(&format!("/api/v1/devices?group={gid}")).1["total"], 1);
    assert_eq!(admin.get("/api/v1/devices?group=none").1["total"], 0);
    assert_eq!(admin.get(&format!("/api/v1/devices?ids={dev}")).1["total"], 1);
    let (_, summary) = admin.get("/api/v1/devices/summary");
    assert_eq!((summary["total"].as_i64(), summary["online"].as_i64()), (Some(1), Some(0)));
    let (st, detail) = admin.get(&format!("/api/v1/devices/{dev}"));
    assert_eq!(st, 200);
    assert!(detail["inventory"].is_null());
    assert_eq!(admin.post(&format!("/api/v1/devices/{dev}/rename"), json!({ "name": "Front desk" })).0, 200);
    assert_eq!(admin.get(&format!("/api/v1/devices/{dev}")).1["device"]["name"], "Front desk");
    assert_eq!(admin.post(&format!("/api/v1/devices/{dev}/move"), json!({ "group_id": null })).0, 200);
    assert_eq!(admin.post(&format!("/api/v1/devices/{dev}/refresh"), json!({})).0, 409);
    assert_eq!(admin.get("/api/v1/devices/nope").0, 404);

    // Scripts and jobs.
    let (st, sc) = admin.post("/api/v1/scripts", json!({ "name": "Uptime", "shell": "bash", "body": "uptime", "timeout_s": 30 }));
    assert_eq!(st, 200);
    let sid = sc["id"].as_i64().unwrap();
    assert_eq!(admin.post("/api/v1/scripts", json!({ "name": "x", "shell": "zsh", "body": "x", "timeout_s": 1 })).0, 400);
    assert_eq!(admin.put(&format!("/api/v1/scripts/{sid}"), json!({ "name": "Uptime 2", "shell": "sh", "body": "uptime", "timeout_s": 30 })).0, 200);
    let (_, script) = admin.get(&format!("/api/v1/scripts/{sid}"));
    assert_eq!((script["name"].as_str(), script["shell"].as_str()), (Some("Uptime 2"), Some("sh")));
    let (st, job) = admin.post("/api/v1/jobs", json!({ "script_id": sid, "devices": [dev] }));
    assert_eq!(st, 200, "{job}");
    let jid = job["id"].as_i64().unwrap();
    let (st, jd) = admin.get(&format!("/api/v1/jobs/{jid}"));
    assert_eq!(st, 200);
    assert_eq!(jd["counts"]["queued"], 1);
    assert_eq!(jd["job"]["kind"], "script");
    let (_, runs) = admin.get(&format!("/api/v1/jobs/{jid}/runs"));
    assert_eq!(runs["items"][0]["status"], "queued");
    assert_eq!(admin.get(&format!("/api/v1/jobs/{jid}/runs?status=ok")).1["items"].as_array().unwrap().len(), 0);
    // Reboot without the typed confirmation is refused; with the hostname it's accepted.
    let (st, e) = admin.post("/api/v1/jobs", json!({ "action": "reboot", "devices": [dev] }));
    assert_eq!(st, 400);
    assert!(e["error"].as_str().unwrap().contains("confirm"));
    assert_eq!(admin.post("/api/v1/jobs", json!({ "action": "reboot", "devices": [dev], "confirm": "test-host" })).0, 200);
    assert_eq!(admin.post("/api/v1/jobs", json!({ "script_id": sid, "all": true, "cron": "not cron" })).0, 400);
    let (st, cronjob) = admin.post("/api/v1/jobs", json!({ "script_id": sid, "all": true, "cron": "0 3 * * *" }));
    assert_eq!(st, 200);
    let cid = cronjob["id"].as_i64().unwrap();
    assert_eq!(admin.post(&format!("/api/v1/jobs/{cid}/disable"), json!({})).0, 200);
    assert_eq!(admin.get(&format!("/api/v1/jobs/{cid}")).1["job"]["enabled"], false);
    let (_, jobs) = admin.get("/api/v1/jobs");
    assert_eq!(jobs["items"].as_array().unwrap().len(), 3);
    let (_, p1) = admin.get("/api/v1/jobs?limit=2");
    assert_eq!(p1["items"].as_array().unwrap().len(), 2);
    let cur = p1["next_cursor"].as_str().unwrap().to_owned();
    let (_, p2) = admin.get(&format!("/api/v1/jobs?limit=2&cursor={cur}"));
    assert_eq!(p2["items"].as_array().unwrap().len(), 1);
    assert!(p2["next_cursor"].is_null());
    assert_eq!(admin.get("/api/v1/scripts?q=uptime").1["items"].as_array().unwrap().len(), 1);
    assert_eq!(admin.get("/api/v1/scripts?q=nope").1["items"].as_array().unwrap().len(), 0);
    assert_eq!(admin.del(&format!("/api/v1/jobs/{cid}")).0, 200);

    // Users: a viewer and an operator.
    assert_eq!(admin.post("/api/v1/users", json!({ "username": "vera", "password": "viewer-pass", "role": "viewer" })).0, 200);
    assert_eq!(admin.post("/api/v1/users", json!({ "username": "otto", "password": "operator-pass", "role": "operator", "email": "Otto@Example.com" })).0, 200);
    assert_eq!(admin.post("/api/v1/users", json!({ "username": "short", "password": "x", "role": "viewer" })).0, 400);
    assert_eq!(admin.post("/api/v1/users", json!({ "username": "vera", "password": "viewer-pass", "role": "viewer" })).0, 409);
    let (_, users) = admin.get("/api/v1/users");
    let otto = users["items"].as_array().unwrap().iter().find(|u| u["username"] == "otto").unwrap().clone();
    assert_eq!(otto["email"], "otto@example.com");

    let mut viewer = Client::login(&s, "vera", "viewer-pass");
    assert_eq!(viewer.get("/api/v1/devices").0, 200);
    assert_eq!(viewer.get("/api/v1/jobs").0, 200);
    assert_eq!(viewer.post("/api/v1/scripts", json!({ "name": "x", "shell": "sh", "body": "x", "timeout_s": 1 })).0, 403);
    assert_eq!(viewer.post("/api/v1/jobs", json!({ "script_id": sid, "devices": [dev] })).0, 403);
    assert_eq!(viewer.post(&format!("/api/v1/devices/{dev}/refresh"), json!({})).0, 403);
    assert_eq!(viewer.get("/api/v1/users").0, 403);
    assert_eq!(viewer.get("/api/v1/audit").0, 403);
    assert_eq!(viewer.post("/api/v1/groups", json!({ "name": "x" })).0, 403);

    let mut operator = Client::login(&s, "otto", "operator-pass");
    assert_eq!(operator.post("/api/v1/jobs", json!({ "script_id": sid, "devices": [dev] })).0, 200);
    assert_eq!(operator.post(&format!("/api/v1/devices/{dev}/rename"), json!({ "name": "x" })).0, 403);
    assert_eq!(operator.del(&format!("/api/v1/devices/{dev}")).0, 403);
    assert_eq!(operator.get("/api/v1/enroll-tokens").0, 403);

    // Origin: a cross-site mutation is refused even with a valid session; same-site passes.
    let (st, _) = admin.send("POST", "/api/v1/groups", Some(json!({ "name": "Evil" })), Some("http://evil.example"));
    assert_eq!(st, 403);
    let host = s.base.trim_start_matches("http://").to_owned();
    let (st, _) = admin.send("POST", "/api/v1/groups", Some(json!({ "name": "Lab" })), Some(&format!("http://{host}")));
    assert_eq!(st, 200);

    // Last admin can't be demoted or deleted.
    let admin_id = me["id"].as_i64().unwrap();
    assert_eq!(admin.post(&format!("/api/v1/users/{admin_id}/role"), json!({ "role": "viewer" })).0, 409);
    assert_eq!(admin.del(&format!("/api/v1/users/{admin_id}")).0, 409);

    // Audit: filtered and paged.
    let (st, audit) = admin.get("/api/v1/audit?limit=2");
    assert_eq!(st, 200);
    assert_eq!(audit["items"].as_array().unwrap().len(), 2);
    let last = audit["items"][1]["id"].as_i64().unwrap();
    let next = audit["next_cursor"].as_str().unwrap().to_owned();
    let (_, page2) = admin.get(&format!("/api/v1/audit?limit=2&cursor={next}"));
    assert!(page2["items"][0]["id"].as_i64().unwrap() < last);
    let (_, logins) = admin.get("/api/v1/audit?action=login.failed");
    assert!(logins["items"].as_array().unwrap().iter().all(|r| r["action"] == "login.failed"));
    assert!(!logins["items"].as_array().unwrap().is_empty());
    let (_, by_admin) = admin.get(&format!("/api/v1/audit?user={admin_id}&limit=200"));
    assert!(by_admin["items"].as_array().unwrap().iter().all(|r| r["user"] == "admin"));
    let (_, actions) = admin.get("/api/v1/audit/actions?q=enroll");
    assert!(actions["items"].as_array().unwrap().iter().any(|a| a["id"] == "device.enroll"));

    // Device delete, then sign out.
    assert_eq!(admin.del(&format!("/api/v1/devices/{dev}")).0, 200);
    assert_eq!(admin.get("/api/v1/devices").1["items"].as_array().unwrap().len(), 0);
    assert_eq!(admin.post("/api/v1/logout", json!({})).0, 200);
    assert_eq!(admin.get("/api/v1/me").0, 401);

    // The classic pages answer under /classic; the app owns the root.
    assert_eq!(anon.get("/classic/jobs").0, 303);
    let (st, html) = anon.get("/devices/anything");
    assert_eq!(st, 200);
    assert!(html.as_str().unwrap().contains("<div id=\"root\">"));
    assert_eq!(anon.get("/missing.js").0, 404);
    assert_eq!(anon.get("/api/v1/nope").0, 404);
}

/// 5,000 devices: the device picker's first page and a narrowing search stay fast.
#[test]
fn device_picker_at_5000() {
    let s = start();
    let mut admin = Client::login(&s, "admin", "admin-password");
    // Seed straight into the database the server uses (WAL, so it sees them at once).
    {
        let db = rusqlite::Connection::open(s._dir.0.join("t.db")).unwrap();
        db.execute_batch("BEGIN").unwrap();
        let mut ins = db
            .prepare(
                "INSERT INTO devices (id, name, hostname, os, secret_hash, enrolled_at, last_seen, inv_ip, inv_serial, inv_os)
                 VALUES (?1, ?2, ?2, 'windows', 'x', 0, 0, ?3, ?4, 'Windows 11 Pro')",
            )
            .unwrap();
        for i in 0..5000 {
            ins.execute(rusqlite::params![
                format!("seed{i:05}"),
                format!("PC-{i:05}"),
                format!("10.{}.{}.{}", i / 65536, (i / 256) % 256, i % 256),
                format!("SN{i:07}")
            ])
            .unwrap();
        }
        drop(ins);
        db.execute_batch("COMMIT").unwrap();
    }
    let timed = |c: &mut Client, path: &str| {
        let t = std::time::Instant::now();
        let r = c.get(path);
        (r, t.elapsed())
    };
    let ((st, first), took) = timed(&mut admin, "/api/v1/devices?limit=25");
    assert_eq!(st, 200);
    assert_eq!(first["items"].as_array().unwrap().len(), 25);
    assert_eq!(first["total"], 5000);
    assert_eq!(first["items"][0]["name"], "PC-00000");
    assert!(took.as_millis() < 300, "first page took {took:?}");
    // The next page continues where the first stopped.
    let cur = first["next_cursor"].as_str().unwrap().to_owned();
    let ((_, second), took) = timed(&mut admin, &format!("/api/v1/devices?limit=25&cursor={cur}"));
    assert_eq!(second["items"][0]["name"], "PC-00025");
    assert!(took.as_millis() < 300, "second page took {took:?}");
    // Search narrows: by name, IP and serial.
    let ((_, hits), took) = timed(&mut admin, "/api/v1/devices?limit=25&q=PC-0420");
    assert_eq!(hits["total"], 10);
    assert!(took.as_millis() < 300, "search took {took:?}");
    assert_eq!(admin.get("/api/v1/devices?q=10.0.16.1").1["items"][0]["name"], "PC-04097");
    assert_eq!(admin.get("/api/v1/devices?q=SN0004999").1["total"], 1);
    // The last page has no cursor.
    let (_, tail) = admin.get("/api/v1/devices?limit=25&q=PC-0499");
    assert_eq!(tail["total"], 10);
    assert!(tail["next_cursor"].is_null());
}
