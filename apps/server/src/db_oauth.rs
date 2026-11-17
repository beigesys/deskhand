//! What MCP clients sign in with: OAuth clients that registered themselves (RFC 7591),
//! one-minute authorization codes, access and refresh tokens, and API tokens a person
//! makes by hand. Every secret is stored as its SHA-256, like sessions.
//!
//! A *connection* is one client signed in as one person: its tokens share a `family`,
//! and revoking the connection deletes the family.

use crate::db::{token_hash, Db, Role, User};
use rusqlite::{params, OptionalExtension};

type R<T> = rusqlite::Result<T>;

/// Created on open beside the numbered migrations, so the MCP tables stay one
/// self-contained piece.
pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS oauth_clients (
    id TEXT PRIMARY KEY,
    name TEXT,
    redirect_uris TEXT NOT NULL,
    created INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS oauth_codes (
    code_hash TEXT PRIMARY KEY,
    client_id TEXT NOT NULL REFERENCES oauth_clients(id) ON DELETE CASCADE,
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    redirect_uri TEXT NOT NULL,
    challenge TEXT NOT NULL,
    scope TEXT NOT NULL,
    expires INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS oauth_tokens (
    token_hash TEXT PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind IN ('access', 'refresh', 'api')),
    family TEXT NOT NULL,
    client_id TEXT REFERENCES oauth_clients(id) ON DELETE CASCADE,
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    scope TEXT NOT NULL,
    name TEXT,
    created INTEGER NOT NULL,
    expires INTEGER,
    last_used INTEGER
);
CREATE INDEX IF NOT EXISTS oauth_tokens_family ON oauth_tokens(family);
CREATE INDEX IF NOT EXISTS oauth_tokens_user ON oauth_tokens(user_id, created);
"#;

/// Codes live a minute: the client trades one straight away.
pub const CODE_SECS: i64 = 60;
/// Access tokens: a day, then the client refreshes.
pub const ACCESS_SECS: i64 = 24 * 3600;
/// Refresh tokens: 30 days, rotated on every use.
pub const REFRESH_SECS: i64 = 30 * 24 * 3600;

pub struct Client {
    pub id: String,
    pub name: Option<String>,
    pub redirect_uris: Vec<String>,
}

/// A spent code: what was asked for when it was issued.
pub struct Redeemed {
    pub client_id: String,
    pub user_id: i64,
    pub redirect_uri: String,
    pub challenge: String,
    pub scope: String,
}

/// A token's holder, as of now.
pub struct Holder {
    pub user: User,
    pub scope: String,
    pub family: String,
}

/// One connection or API token, for the person's list.
pub struct Connection {
    pub family: String,
    pub kind: String,
    pub name: Option<String>,
    pub scope: String,
    pub created: i64,
    pub last_used: Option<i64>,
    pub expires: Option<i64>,
    pub user_id: i64,
    pub username: String,
}

impl Db {
    pub fn user_by_id(&self, id: i64) -> R<Option<User>> {
        self.c()
            .query_row("SELECT id, username, role FROM users WHERE id = ?1", [id], |r| {
                Ok(User {
                    id: r.get(0)?,
                    username: r.get(1)?,
                    role: Role::parse(&r.get::<_, String>(2)?).unwrap_or(Role::Viewer),
                })
            })
            .optional()
    }

    pub fn oauth_register(&self, id: &str, name: Option<&str>, uris: &[String], now: i64) -> R<()> {
        self.c().execute(
            "INSERT INTO oauth_clients (id, name, redirect_uris, created) VALUES (?1, ?2, ?3, ?4)",
            params![id, name, serde_json::to_string(uris).unwrap(), now],
        )?;
        Ok(())
    }

    pub fn oauth_client(&self, id: &str) -> R<Option<Client>> {
        self.c()
            .query_row("SELECT id, name, redirect_uris FROM oauth_clients WHERE id = ?1", [id], |r| {
                Ok(Client {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    redirect_uris: serde_json::from_str(&r.get::<_, String>(2)?).unwrap_or_default(),
                })
            })
            .optional()
    }

    #[allow(clippy::too_many_arguments)]
    pub fn oauth_issue_code(
        &self,
        code: &str,
        client_id: &str,
        user_id: i64,
        redirect_uri: &str,
        challenge: &str,
        scope: &str,
        now: i64,
    ) -> R<()> {
        let c = self.c();
        c.execute("DELETE FROM oauth_codes WHERE expires < ?1", [now])?;
        c.execute(
            "INSERT INTO oauth_codes (code_hash, client_id, user_id, redirect_uri, challenge, scope, expires)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![token_hash(code), client_id, user_id, redirect_uri, challenge, scope, now + CODE_SECS],
        )?;
        Ok(())
    }

    /// Spends a live code, whether or not the request then fits it: a code is good once.
    pub fn oauth_take_code(&self, code: &str, now: i64) -> R<Option<Redeemed>> {
        let c = self.c();
        let hash = token_hash(code);
        let row = c
            .query_row(
                "SELECT client_id, user_id, redirect_uri, challenge, scope FROM oauth_codes
                 WHERE code_hash = ?1 AND expires >= ?2",
                params![hash, now],
                |r| {
                    Ok(Redeemed {
                        client_id: r.get(0)?,
                        user_id: r.get(1)?,
                        redirect_uri: r.get(2)?,
                        challenge: r.get(3)?,
                        scope: r.get(4)?,
                    })
                },
            )
            .optional()?;
        c.execute("DELETE FROM oauth_codes WHERE code_hash = ?1", [hash])?;
        Ok(row)
    }

    /// Stores an access and refresh token for a connection.
    #[allow(clippy::too_many_arguments)]
    pub fn oauth_store_pair(
        &self,
        access: &str,
        refresh: &str,
        family: &str,
        client_id: &str,
        user_id: i64,
        scope: &str,
        now: i64,
    ) -> R<()> {
        let mut c = self.c();
        let tx = c.transaction()?;
        for (token, kind, life) in [(access, "access", ACCESS_SECS), (refresh, "refresh", REFRESH_SECS)] {
            tx.execute(
                "INSERT INTO oauth_tokens (token_hash, kind, family, client_id, user_id, scope, created, expires)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![token_hash(token), kind, family, client_id, user_id, scope, now, now + life],
            )?;
        }
        tx.execute(
            "DELETE FROM oauth_tokens WHERE expires IS NOT NULL AND expires < ?1",
            [now],
        )?;
        tx.commit()
    }

    /// Spends a refresh token: its connection's `(family, user, scope)` if it was live and
    /// the client's. The access tokens issued alongside it go too.
    pub fn oauth_take_refresh(&self, refresh: &str, client_id: &str, now: i64) -> R<Option<(String, i64, String)>> {
        let mut c = self.c();
        let tx = c.transaction()?;
        let row: Option<(String, i64, String)> = tx
            .query_row(
                "SELECT family, user_id, scope FROM oauth_tokens
                 WHERE token_hash = ?1 AND kind = 'refresh' AND client_id = ?2 AND expires >= ?3",
                params![token_hash(refresh), client_id, now],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        if let Some((family, ..)) = &row {
            tx.execute("DELETE FROM oauth_tokens WHERE family = ?1 AND kind IN ('access', 'refresh')", [family])?;
        }
        tx.commit()?;
        Ok(row)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn oauth_add_api_token(&self, token: &str, family: &str, user_id: i64, name: &str, scope: &str, expires: Option<i64>, now: i64) -> R<()> {
        self.c().execute(
            "INSERT INTO oauth_tokens (token_hash, kind, family, user_id, scope, name, created, expires)
             VALUES (?1, 'api', ?2, ?3, ?4, ?5, ?6, ?7)",
            params![token_hash(token), family, user_id, scope, name, now, expires],
        )?;
        Ok(())
    }

    /// Who holds a live access or API token, with the account as it is now.
    pub fn oauth_holder(&self, token: &str, now: i64) -> R<Option<Holder>> {
        let c = self.c();
        let hash = token_hash(token);
        let row = c
            .query_row(
                "SELECT u.id, u.username, u.role, t.scope, t.family, t.last_used FROM oauth_tokens t
                 JOIN users u ON u.id = t.user_id
                 WHERE t.token_hash = ?1 AND t.kind IN ('access', 'api') AND (t.expires IS NULL OR t.expires >= ?2)",
                params![hash, now],
                |r| {
                    Ok((
                        Holder {
                            user: User {
                                id: r.get(0)?,
                                username: r.get(1)?,
                                role: Role::parse(&r.get::<_, String>(2)?).unwrap_or(Role::Viewer),
                            },
                            scope: r.get(3)?,
                            family: r.get(4)?,
                        },
                        r.get::<_, Option<i64>>(5)?,
                    ))
                },
            )
            .optional()?;
        let Some((holder, last_used)) = row else {
            return Ok(None);
        };
        // A write per minute at most, not per request.
        if last_used.is_none_or(|t| now - t >= 60) {
            c.execute("UPDATE oauth_tokens SET last_used = ?1 WHERE family = ?2", params![now, holder.family])?;
        }
        Ok(Some(holder))
    }

    /// Connections and API tokens, newest first: one row per family. `user` limits it to
    /// one person's.
    pub fn oauth_connections(&self, user: Option<i64>, now: i64) -> R<Vec<Connection>> {
        let c = self.c();
        let mut s = c.prepare(
            "SELECT t.family, max(t.kind), coalesce(max(t.name), max(cl.name)), max(t.scope), min(t.created),
                    max(t.last_used), max(t.expires), t.user_id, u.username
             FROM oauth_tokens t JOIN users u ON u.id = t.user_id
             LEFT JOIN oauth_clients cl ON cl.id = t.client_id
             WHERE (?1 IS NULL OR t.user_id = ?1) AND (t.expires IS NULL OR t.expires >= ?2)
             GROUP BY t.family ORDER BY min(t.created) DESC LIMIT 500",
        )?;
        let rows = s.query_map(params![user, now], |r| {
            Ok(Connection {
                family: r.get(0)?,
                // 'refresh' > 'api' > 'access': an OAuth family reads as refresh.
                kind: match r.get::<_, String>(1)?.as_str() {
                    "api" => "api".to_owned(),
                    _ => "oauth".to_owned(),
                },
                name: r.get(2)?,
                scope: r.get(3)?,
                created: r.get(4)?,
                last_used: r.get(5)?,
                expires: r.get(6)?,
                user_id: r.get(7)?,
                username: r.get(8)?,
            })
        })?;
        rows.collect()
    }

    /// Ends a connection or API token; `user` limits it to one person's. Returns rows gone.
    pub fn oauth_revoke(&self, family: &str, user: Option<i64>) -> R<usize> {
        self.c().execute(
            "DELETE FROM oauth_tokens WHERE family = ?1 AND (?2 IS NULL OR user_id = ?2)",
            params![family, user],
        )
    }
}
