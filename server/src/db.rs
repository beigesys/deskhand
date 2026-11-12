//! The server's SQLite database: users, sessions, groups, devices, enrollment tokens and
//! the audit log. One connection behind a mutex; every query here is small.

use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use std::sync::Mutex;

pub struct Db(Mutex<Connection>);

/// Migrations, applied in order; `PRAGMA user_version` counts how many have run.
const MIGRATIONS: &[&str] = &[r#"
CREATE TABLE users (
    id INTEGER PRIMARY KEY,
    username TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    role TEXT NOT NULL CHECK (role IN ('admin', 'operator', 'viewer')),
    created INTEGER NOT NULL
);
CREATE TABLE sessions (
    token_hash TEXT PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    expires INTEGER NOT NULL
);
CREATE TABLE groups (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE
);
CREATE TABLE devices (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    hostname TEXT NOT NULL,
    os TEXT NOT NULL,
    group_id INTEGER REFERENCES groups(id) ON DELETE SET NULL,
    secret_hash TEXT NOT NULL,
    enrolled_at INTEGER NOT NULL,
    last_seen INTEGER NOT NULL
);
CREATE TABLE enroll_tokens (
    id INTEGER PRIMARY KEY,
    token_hash TEXT NOT NULL UNIQUE,
    group_id INTEGER REFERENCES groups(id) ON DELETE SET NULL,
    expires INTEGER NOT NULL,
    max_uses INTEGER NOT NULL,
    uses INTEGER NOT NULL DEFAULT 0,
    created INTEGER NOT NULL
);
CREATE TABLE audit (
    id INTEGER PRIMARY KEY,
    ts INTEGER NOT NULL,
    user_id INTEGER,
    device_id TEXT,
    action TEXT NOT NULL,
    detail TEXT NOT NULL DEFAULT ''
);
CREATE INDEX audit_ts ON audit(ts);
"#, r#"
ALTER TABLE users ADD COLUMN email TEXT;
CREATE UNIQUE INDEX users_email ON users(email COLLATE NOCASE) WHERE email IS NOT NULL;
CREATE TABLE user_identities (
    provider TEXT NOT NULL,
    subject TEXT NOT NULL,
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    email TEXT,
    created INTEGER NOT NULL,
    PRIMARY KEY (provider, subject)
);
"#, r#"
ALTER TABLE devices ADD COLUMN inv_os TEXT;
ALTER TABLE devices ADD COLUMN inv_os_version TEXT;
ALTER TABLE devices ADD COLUMN inv_serial TEXT;
ALTER TABLE devices ADD COLUMN inv_ram INTEGER;
ALTER TABLE devices ADD COLUMN inv_last_boot INTEGER;
ALTER TABLE devices ADD COLUMN inv_at INTEGER;
CREATE INDEX devices_inv_os ON devices(inv_os, inv_os_version);
CREATE INDEX devices_inv_serial ON devices(inv_serial);
CREATE INDEX devices_inv_ram ON devices(inv_ram);
CREATE INDEX devices_inv_last_boot ON devices(inv_last_boot);
CREATE TABLE inventory (
    id INTEGER PRIMARY KEY,
    device_id TEXT NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    ts INTEGER NOT NULL,
    json TEXT NOT NULL
);
CREATE INDEX inventory_device ON inventory(device_id, id);
"#, r#"
CREATE TABLE scripts (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    shell TEXT NOT NULL CHECK (shell IN ('powershell', 'cmd', 'bash', 'sh')),
    body TEXT NOT NULL,
    timeout_s INTEGER NOT NULL,
    created_by INTEGER REFERENCES users(id) ON DELETE SET NULL,
    created INTEGER NOT NULL,
    updated INTEGER NOT NULL
);
CREATE TABLE jobs (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    -- What runs: a copy of the script as it was when the job was made, or a built-in
    -- action as JSON.
    script_id INTEGER REFERENCES scripts(id) ON DELETE SET NULL,
    shell TEXT,
    body TEXT,
    timeout_s INTEGER NOT NULL,
    action TEXT,
    -- Who: 'devices' (target_devices, a JSON array of ids), 'group' or 'all'.
    target_kind TEXT NOT NULL CHECK (target_kind IN ('devices', 'group', 'all')),
    target_devices TEXT,
    target_group INTEGER REFERENCES groups(id) ON DELETE SET NULL,
    -- When: NULL runs once, at creation; otherwise a cron expression (UTC).
    schedule TEXT,
    next_fire INTEGER,
    last_fired INTEGER,
    -- How long a run waits for an offline device.
    expires_after INTEGER NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    created_by INTEGER REFERENCES users(id) ON DELETE SET NULL,
    created INTEGER NOT NULL
);
CREATE INDEX jobs_due ON jobs(next_fire) WHERE schedule IS NOT NULL AND enabled = 1;
CREATE TABLE job_runs (
    id INTEGER PRIMARY KEY,
    job_id INTEGER NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
    device_id TEXT NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    -- The agent that carries the run out when it isn't the device itself (wake-on-LAN).
    via TEXT,
    status TEXT NOT NULL CHECK (status IN ('queued', 'running', 'ok', 'failed', 'timeout', 'offline')),
    exit_code INTEGER,
    stdout TEXT NOT NULL DEFAULT '',
    stderr TEXT NOT NULL DEFAULT '',
    queued INTEGER NOT NULL,
    expires INTEGER NOT NULL,
    started INTEGER,
    finished INTEGER
);
CREATE INDEX job_runs_job ON job_runs(job_id);
CREATE INDEX job_runs_device ON job_runs(device_id, status);
CREATE INDEX job_runs_status ON job_runs(status);
"#];

/// Inventory snapshots kept per device.
const INVENTORY_HISTORY: i64 = 30;

/// SHA-256 in hex, for tokens and secrets: they're random and long, so a fast hash is
/// enough (passwords get argon2).
pub fn token_hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

#[derive(Clone)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub role: Role,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Role {
    Viewer,
    Operator,
    Admin,
}

impl Role {
    pub fn parse(s: &str) -> Option<Role> {
        match s {
            "admin" => Some(Role::Admin),
            "operator" => Some(Role::Operator),
            "viewer" => Some(Role::Viewer),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Admin => "admin",
            Role::Operator => "operator",
            Role::Viewer => "viewer",
        }
    }
}

pub struct Device {
    pub id: String,
    pub name: String,
    pub hostname: String,
    pub os: String,
    pub group_id: Option<i64>,
    pub group: Option<String>,
    pub last_seen: i64,
    /// From the latest inventory.
    pub inv_os: Option<String>,
    pub inv_os_version: Option<String>,
    pub inv_last_boot: Option<i64>,
}

pub struct Group {
    pub id: i64,
    pub name: String,
    pub devices: i64,
}

pub struct EnrollToken {
    pub id: i64,
    pub group: Option<String>,
    pub expires: i64,
    pub max_uses: i64,
    pub uses: i64,
}

pub struct AuditRow {
    pub ts: i64,
    pub user: Option<String>,
    pub device: Option<String>,
    pub action: String,
    pub detail: String,
}

type R<T> = rusqlite::Result<T>;

impl Db {
    pub fn open(path: &str) -> R<Db> {
        let c = Connection::open(path)?;
        c.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 5000;")?;
        let version: usize = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        for (i, m) in MIGRATIONS.iter().enumerate().skip(version) {
            c.execute_batch(&format!("BEGIN; {m}; PRAGMA user_version = {}; COMMIT;", i + 1))?;
        }
        Ok(Db(Mutex::new(c)))
    }

    pub(crate) fn c(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.0.lock().unwrap()
    }

    // --- users and sessions ---

    pub fn user_count(&self) -> R<i64> {
        self.c().query_row("SELECT count(*) FROM users", [], |r| r.get(0))
    }

    pub fn add_user(&self, username: &str, password_hash: &str, role: Role, now: i64) -> R<i64> {
        let c = self.c();
        c.execute(
            "INSERT INTO users (username, password_hash, role, created) VALUES (?1, ?2, ?3, ?4)",
            params![username, password_hash, role.as_str(), now],
        )?;
        Ok(c.last_insert_rowid())
    }

    /// The user and their password hash, for signing in.
    pub fn user_by_name(&self, username: &str) -> R<Option<(User, String)>> {
        self.c()
            .query_row(
                "SELECT id, username, role, password_hash FROM users WHERE username = ?1",
                [username],
                |r| {
                    Ok((
                        User {
                            id: r.get(0)?,
                            username: r.get(1)?,
                            role: Role::parse(&r.get::<_, String>(2)?).unwrap_or(Role::Viewer),
                        },
                        r.get(3)?,
                    ))
                },
            )
            .optional()
    }

    pub fn users(&self) -> R<Vec<(User, i64)>> {
        let c = self.c();
        let mut s = c.prepare("SELECT id, username, role, created FROM users ORDER BY username")?;
        let rows = s.query_map([], |r| {
            Ok((
                User {
                    id: r.get(0)?,
                    username: r.get(1)?,
                    role: Role::parse(&r.get::<_, String>(2)?).unwrap_or(Role::Viewer),
                },
                r.get(3)?,
            ))
        })?;
        rows.collect()
    }

    pub fn set_role(&self, id: i64, role: Role) -> R<usize> {
        self.c().execute("UPDATE users SET role = ?1 WHERE id = ?2", params![role.as_str(), id])
    }

    pub fn delete_user(&self, id: i64) -> R<usize> {
        self.c().execute("DELETE FROM users WHERE id = ?1", [id])
    }

    pub fn admin_count(&self) -> R<i64> {
        self.c().query_row("SELECT count(*) FROM users WHERE role = 'admin'", [], |r| r.get(0))
    }

    pub fn add_session(&self, token: &str, user_id: i64, expires: i64) -> R<()> {
        self.c().execute(
            "INSERT INTO sessions (token_hash, user_id, expires) VALUES (?1, ?2, ?3)",
            params![token_hash(token), user_id, expires],
        )?;
        Ok(())
    }

    /// The session's user if it hasn't expired, sliding its expiry forward to `extend_to`
    /// when that's at least an hour later.
    pub fn session_user(&self, token: &str, now: i64, extend_to: i64) -> R<Option<User>> {
        let c = self.c();
        let hash = token_hash(token);
        let row = c
            .query_row(
                "SELECT u.id, u.username, u.role, s.expires FROM sessions s JOIN users u ON u.id = s.user_id
                 WHERE s.token_hash = ?1 AND s.expires > ?2",
                params![hash, now],
                |r| {
                    Ok((
                        User {
                            id: r.get(0)?,
                            username: r.get(1)?,
                            role: Role::parse(&r.get::<_, String>(2)?).unwrap_or(Role::Viewer),
                        },
                        r.get::<_, i64>(3)?,
                    ))
                },
            )
            .optional()?;
        let Some((user, expires)) = row else {
            return Ok(None);
        };
        if extend_to - expires >= 3600 {
            c.execute("UPDATE sessions SET expires = ?1 WHERE token_hash = ?2", params![extend_to, hash])?;
        }
        Ok(Some(user))
    }

    pub fn delete_session(&self, token: &str) -> R<()> {
        self.c().execute("DELETE FROM sessions WHERE token_hash = ?1", [token_hash(token)])?;
        Ok(())
    }

    pub fn purge_sessions(&self, now: i64) -> R<()> {
        self.c().execute("DELETE FROM sessions WHERE expires <= ?1", [now])?;
        Ok(())
    }

    pub fn set_email(&self, id: i64, email: Option<&str>) -> R<usize> {
        self.c().execute("UPDATE users SET email = ?2 WHERE id = ?1", params![id, email])
    }

    /// Each user's email, by user id.
    pub fn user_emails(&self) -> R<std::collections::HashMap<i64, String>> {
        let c = self.c();
        let mut s = c.prepare("SELECT id, email FROM users WHERE email IS NOT NULL")?;
        let rows = s.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect()
    }

    fn user_where(&self, sql: &str, p: impl rusqlite::Params) -> R<Option<User>> {
        self.c()
            .query_row(sql, p, |r| {
                Ok(User {
                    id: r.get(0)?,
                    username: r.get(1)?,
                    role: Role::parse(&r.get::<_, String>(2)?).unwrap_or(Role::Viewer),
                })
            })
            .optional()
    }

    // --- single sign-on identities ---

    pub fn identity_user(&self, provider: &str, subject: &str) -> R<Option<User>> {
        self.user_where(
            "SELECT u.id, u.username, u.role FROM user_identities i JOIN users u ON u.id = i.user_id
             WHERE i.provider = ?1 AND i.subject = ?2",
            params![provider, subject],
        )
    }

    pub fn user_by_email(&self, email: &str) -> R<Option<User>> {
        self.user_where("SELECT id, username, role FROM users WHERE email = ?1 COLLATE NOCASE", [email])
    }

    pub fn username_taken(&self, username: &str) -> R<bool> {
        self.c()
            .query_row("SELECT count(*) FROM users WHERE username = ?1", [username], |r| r.get::<_, i64>(0))
            .map(|n| n > 0)
    }

    pub fn link_identity(&self, user_id: i64, provider: &str, subject: &str, email: Option<&str>, now: i64) -> R<()> {
        self.c().execute(
            "INSERT INTO user_identities (provider, subject, user_id, email, created) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (provider, subject) DO UPDATE SET email = excluded.email",
            params![provider, subject, user_id, email, now],
        )?;
        Ok(())
    }

    /// Linked identities as (user id, provider, email), for the users page.
    pub fn identities(&self) -> R<Vec<(i64, String, Option<String>)>> {
        let c = self.c();
        let mut s = c.prepare("SELECT user_id, provider, email FROM user_identities ORDER BY provider")?;
        let rows = s.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        rows.collect()
    }

    /// Unlinks a user's sign-in with `provider`; the number of identities removed.
    pub fn unlink_identity(&self, user_id: i64, provider: &str) -> R<usize> {
        self.c().execute("DELETE FROM user_identities WHERE user_id = ?1 AND provider = ?2", params![user_id, provider])
    }

    // --- groups ---

    pub fn groups(&self) -> R<Vec<Group>> {
        let c = self.c();
        let mut s = c.prepare(
            "SELECT g.id, g.name, (SELECT count(*) FROM devices d WHERE d.group_id = g.id)
             FROM groups g ORDER BY g.name",
        )?;
        let rows = s.query_map([], |r| Ok(Group { id: r.get(0)?, name: r.get(1)?, devices: r.get(2)? }))?;
        rows.collect()
    }

    pub fn add_group(&self, name: &str) -> R<i64> {
        let c = self.c();
        c.execute("INSERT INTO groups (name) VALUES (?1)", [name])?;
        Ok(c.last_insert_rowid())
    }

    pub fn delete_group(&self, id: i64) -> R<usize> {
        self.c().execute("DELETE FROM groups WHERE id = ?1", [id])
    }

    pub fn group_exists(&self, id: i64) -> R<bool> {
        self.c().query_row("SELECT count(*) FROM groups WHERE id = ?1", [id], |r| r.get::<_, i64>(0)).map(|n| n > 0)
    }

    // --- devices ---

    pub fn devices(&self, group: Option<i64>) -> R<Vec<Device>> {
        let c = self.c();
        let mut s = c.prepare(
            "SELECT d.id, d.name, d.hostname, d.os, d.group_id, g.name, d.last_seen,
                    d.inv_os, d.inv_os_version, d.inv_last_boot
             FROM devices d LEFT JOIN groups g ON g.id = d.group_id
             WHERE ?1 IS NULL OR d.group_id = ?1
             ORDER BY d.name COLLATE NOCASE",
        )?;
        let rows = s.query_map([group], |r| {
            Ok(Device {
                id: r.get(0)?,
                name: r.get(1)?,
                hostname: r.get(2)?,
                os: r.get(3)?,
                group_id: r.get(4)?,
                group: r.get(5)?,
                last_seen: r.get(6)?,
                inv_os: r.get(7)?,
                inv_os_version: r.get(8)?,
                inv_last_boot: r.get(9)?,
            })
        })?;
        rows.collect()
    }

    pub fn device(&self, id: &str) -> R<Option<Device>> {
        Ok(self.devices(None)?.into_iter().find(|d| d.id == id))
    }

    // --- inventory ---

    /// Saves a snapshot, updates the device's summary columns and keeps the newest
    /// [`INVENTORY_HISTORY`].
    pub fn save_inventory(&self, device: &str, inv: &deskhand_proto::Inventory, now: i64) -> R<()> {
        let json = serde_json::to_string(inv).expect("inventory json");
        let mut c = self.c();
        let tx = c.transaction()?;
        tx.execute("INSERT INTO inventory (device_id, ts, json) VALUES (?1, ?2, ?3)", params![device, now, json])?;
        tx.execute(
            "UPDATE devices SET inv_os = ?2, inv_os_version = ?3, inv_serial = ?4, inv_ram = ?5,
                    inv_last_boot = ?6, inv_at = ?7 WHERE id = ?1",
            params![
                device,
                inv.os.name,
                inv.os.version,
                inv.hardware.serial,
                inv.memory.total.map(|n| n as i64),
                inv.last_boot,
                now
            ],
        )?;
        tx.execute(
            "DELETE FROM inventory WHERE device_id = ?1 AND id NOT IN
             (SELECT id FROM inventory WHERE device_id = ?1 ORDER BY id DESC LIMIT ?2)",
            params![device, INVENTORY_HISTORY],
        )?;
        tx.commit()
    }

    /// The newest snapshot and when the server got it.
    pub fn inventory(&self, device: &str) -> R<Option<(i64, deskhand_proto::Inventory)>> {
        let row: Option<(i64, String)> = self
            .c()
            .query_row(
                "SELECT ts, json FROM inventory WHERE device_id = ?1 ORDER BY id DESC LIMIT 1",
                [device],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        Ok(row.map(|(ts, j)| (ts, serde_json::from_str(&j).unwrap_or_default())))
    }

    /// Every device's newest snapshot, by device id, for the CSV export.
    pub fn latest_inventories(&self) -> R<std::collections::HashMap<String, deskhand_proto::Inventory>> {
        let c = self.c();
        let mut s = c.prepare(
            "SELECT device_id, json FROM inventory WHERE id IN (SELECT max(id) FROM inventory GROUP BY device_id)",
        )?;
        let rows = s.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        rows.map(|r| r.map(|(id, j)| (id, serde_json::from_str(&j).unwrap_or_default()))).collect()
    }

    /// How many snapshots the device has, for the detail page.
    pub fn inventory_count(&self, device: &str) -> R<i64> {
        self.c().query_row("SELECT count(*) FROM inventory WHERE device_id = ?1", [device], |r| r.get(0))
    }

    pub fn device_name(&self, id: &str) -> R<Option<String>> {
        self.c().query_row("SELECT name FROM devices WHERE id = ?1", [id], |r| r.get(0)).optional()
    }

    pub fn device_secret_hash(&self, id: &str) -> R<Option<String>> {
        self.c()
            .query_row("SELECT secret_hash FROM devices WHERE id = ?1", [id], |r| r.get(0))
            .optional()
    }

    /// Spends one use of an enrollment token and adds the device, in one transaction.
    /// `None` when the token is unknown, expired or used up.
    pub fn enroll(
        &self,
        token: &str,
        device_id: &str,
        secret_hash: &str,
        hostname: &str,
        os: &str,
        now: i64,
    ) -> R<Option<()>> {
        let mut c = self.c();
        let tx = c.transaction()?;
        let row: Option<(i64, Option<i64>)> = tx
            .query_row(
                "SELECT id, group_id FROM enroll_tokens WHERE token_hash = ?1 AND expires > ?2 AND uses < max_uses",
                params![token_hash(token), now],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((token_id, group_id)) = row else {
            return Ok(None);
        };
        tx.execute("UPDATE enroll_tokens SET uses = uses + 1 WHERE id = ?1", [token_id])?;
        tx.execute(
            "INSERT INTO devices (id, name, hostname, os, group_id, secret_hash, enrolled_at, last_seen)
             VALUES (?1, ?2, ?2, ?3, ?4, ?5, ?6, ?6)",
            params![device_id, hostname, os, group_id, secret_hash, now],
        )?;
        tx.commit()?;
        Ok(Some(()))
    }

    pub fn device_seen(&self, id: &str, hostname: Option<&str>, os: Option<&str>, now: i64) -> R<()> {
        self.c().execute(
            "UPDATE devices SET last_seen = ?2, hostname = coalesce(?3, hostname), os = coalesce(?4, os) WHERE id = ?1",
            params![id, now, hostname, os],
        )?;
        Ok(())
    }

    pub fn rename_device(&self, id: &str, name: &str) -> R<usize> {
        self.c().execute("UPDATE devices SET name = ?2 WHERE id = ?1", params![id, name])
    }

    pub fn move_device(&self, id: &str, group: Option<i64>) -> R<usize> {
        self.c().execute("UPDATE devices SET group_id = ?2 WHERE id = ?1", params![id, group])
    }

    pub fn delete_device(&self, id: &str) -> R<usize> {
        self.c().execute("DELETE FROM devices WHERE id = ?1", [id])
    }

    // --- enrollment tokens ---

    pub fn add_enroll_token(&self, token: &str, group: Option<i64>, expires: i64, max_uses: i64, now: i64) -> R<()> {
        self.c().execute(
            "INSERT INTO enroll_tokens (token_hash, group_id, expires, max_uses, created) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![token_hash(token), group, expires, max_uses, now],
        )?;
        Ok(())
    }

    pub fn enroll_tokens(&self, now: i64) -> R<Vec<EnrollToken>> {
        let c = self.c();
        let mut s = c.prepare(
            "SELECT t.id, g.name, t.expires, t.max_uses, t.uses FROM enroll_tokens t
             LEFT JOIN groups g ON g.id = t.group_id
             WHERE t.expires > ?1 AND t.uses < t.max_uses ORDER BY t.created DESC",
        )?;
        let rows = s.query_map([now], |r| {
            Ok(EnrollToken {
                id: r.get(0)?,
                group: r.get(1)?,
                expires: r.get(2)?,
                max_uses: r.get(3)?,
                uses: r.get(4)?,
            })
        })?;
        rows.collect()
    }

    pub fn revoke_enroll_token(&self, id: i64) -> R<usize> {
        self.c().execute("DELETE FROM enroll_tokens WHERE id = ?1", [id])
    }

    // --- audit ---

    pub fn audit(&self, ts: i64, user: Option<i64>, device: Option<&str>, action: &str, detail: &str) {
        let r = self.c().execute(
            "INSERT INTO audit (ts, user_id, device_id, action, detail) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![ts, user, device, action, detail],
        );
        if let Err(e) = r {
            log::error!("audit write failed: {e}");
        }
    }

    /// A page of the audit log, newest first: rows older than `before` (an audit id), with
    /// the action starting with `action` and the user name / device / detail containing `q`.
    pub fn audit_page(&self, before: Option<i64>, limit: i64, action: Option<&str>, q: Option<&str>) -> R<Vec<(i64, AuditRow)>> {
        let c = self.c();
        let mut s = c.prepare(
            "SELECT a.id, a.ts, u.username, coalesce(d.name, a.device_id), a.action, a.detail FROM audit a
             LEFT JOIN users u ON u.id = a.user_id LEFT JOIN devices d ON d.id = a.device_id
             WHERE (?1 IS NULL OR a.id < ?1)
               AND (?2 IS NULL OR a.action LIKE ?2 || '%')
               AND (?3 IS NULL OR u.username LIKE '%' || ?3 || '%' OR d.name LIKE '%' || ?3 || '%'
                    OR a.detail LIKE '%' || ?3 || '%')
             ORDER BY a.id DESC LIMIT ?4",
        )?;
        let rows = s.query_map(params![before, action, q, limit], |r| {
            Ok((
                r.get(0)?,
                AuditRow { ts: r.get(1)?, user: r.get(2)?, device: r.get(3)?, action: r.get(4)?, detail: r.get(5)? },
            ))
        })?;
        rows.collect()
    }

    /// The distinct actions in the audit log, for the filter.
    pub fn audit_actions(&self) -> R<Vec<String>> {
        let c = self.c();
        let mut s = c.prepare("SELECT DISTINCT action FROM audit ORDER BY action")?;
        let rows = s.query_map([], |r| r.get(0))?;
        rows.collect()
    }

    pub fn audit_rows(&self, limit: i64) -> R<Vec<AuditRow>> {
        let c = self.c();
        let mut s = c.prepare(
            "SELECT a.ts, u.username, coalesce(d.name, a.device_id), a.action, a.detail FROM audit a
             LEFT JOIN users u ON u.id = a.user_id LEFT JOIN devices d ON d.id = a.device_id
             ORDER BY a.id DESC LIMIT ?1",
        )?;
        let rows = s.query_map([limit], |r| {
            Ok(AuditRow {
                ts: r.get(0)?,
                user: r.get(1)?,
                device: r.get(2)?,
                action: r.get(3)?,
                detail: r.get(4)?,
            })
        })?;
        rows.collect()
    }
}
