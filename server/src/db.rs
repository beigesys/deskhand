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
"#];

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

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
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

    fn c(&self) -> std::sync::MutexGuard<'_, Connection> {
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
            "SELECT d.id, d.name, d.hostname, d.os, d.group_id, g.name, d.last_seen
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
            })
        })?;
        rows.collect()
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
