//! Paged, searchable lists for pickers and big tables: keyset pagination (never OFFSET)
//! over indexed sort keys, LIKE search in SQL, and `ids` to resolve a picker's selection.

use crate::db::{Db, Group, Role, User};
use crate::jobs::{Counts, Job, Run, Script};
use rusqlite::params;

type R<T> = rusqlite::Result<T>;

/// `%q%` for LIKE … ESCAPE '\', or None for no search.
pub fn like(q: Option<&str>) -> Option<String> {
    let q = q?.trim();
    (!q.is_empty()).then(|| format!("%{}%", q.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")))
}

impl Db {
    /// Groups in name order after `(name, id)`.
    pub fn groups_page(&self, q: Option<&str>, ids: Option<&str>, after: Option<(String, i64)>, limit: i64) -> R<Vec<Group>> {
        let c = self.c();
        let (an, ai) = after.map_or((None, None), |(n, i)| (Some(n), Some(i)));
        let mut s = c.prepare(
            "SELECT g.id, g.name, (SELECT count(*) FROM devices d WHERE d.group_id = g.id) FROM groups g
             WHERE (?1 IS NULL OR g.name LIKE ?1 ESCAPE '\\')
               AND (?2 IS NULL OR g.id IN (SELECT value FROM json_each(?2)))
               AND (?3 IS NULL OR g.name > ?3 COLLATE NOCASE OR (g.name = ?3 COLLATE NOCASE AND g.id > ?4))
             ORDER BY g.name COLLATE NOCASE, g.id LIMIT ?5",
        )?;
        let rows = s.query_map(params![like(q), ids, an, ai, limit], |r| {
            Ok(Group { id: r.get(0)?, name: r.get(1)?, devices: r.get(2)? })
        })?;
        rows.collect()
    }

    /// Users in name order after `(username, id)`, with their email.
    pub fn users_page(
        &self,
        q: Option<&str>,
        ids: Option<&str>,
        after: Option<(String, i64)>,
        limit: i64,
    ) -> R<Vec<(User, i64, Option<String>)>> {
        let c = self.c();
        let (an, ai) = after.map_or((None, None), |(n, i)| (Some(n), Some(i)));
        let mut s = c.prepare(
            "SELECT id, username, role, created, email FROM users
             WHERE (?1 IS NULL OR username LIKE ?1 ESCAPE '\\' OR email LIKE ?1 ESCAPE '\\')
               AND (?2 IS NULL OR id IN (SELECT value FROM json_each(?2)))
               AND (?3 IS NULL OR username > ?3 COLLATE NOCASE OR (username = ?3 COLLATE NOCASE AND id > ?4))
             ORDER BY username COLLATE NOCASE, id LIMIT ?5",
        )?;
        let rows = s.query_map(params![like(q), ids, an, ai, limit], |r| {
            Ok((
                User { id: r.get(0)?, username: r.get(1)?, role: Role::parse(&r.get::<_, String>(2)?).unwrap_or(Role::Viewer) },
                r.get(3)?,
                r.get(4)?,
            ))
        })?;
        rows.collect()
    }

    /// Linked identities of these users (a JSON array of ids): (user id, provider, email).
    pub fn identities_of(&self, ids: &str) -> R<Vec<(i64, String, Option<String>)>> {
        let c = self.c();
        let mut s = c.prepare(
            "SELECT user_id, provider, email FROM user_identities
             WHERE user_id IN (SELECT value FROM json_each(?1)) ORDER BY provider",
        )?;
        let rows = s.query_map([ids], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        rows.collect()
    }

    /// Scripts (without their bodies) in name order after `(name, id)`.
    pub fn scripts_page(&self, q: Option<&str>, ids: Option<&str>, after: Option<(String, i64)>, limit: i64) -> R<Vec<Script>> {
        let c = self.c();
        let (an, ai) = after.map_or((None, None), |(n, i)| (Some(n), Some(i)));
        let mut s = c.prepare(
            "SELECT s.id, s.name, s.shell, s.timeout_s, u.username, s.updated FROM scripts s
             LEFT JOIN users u ON u.id = s.created_by
             WHERE (?1 IS NULL OR s.name LIKE ?1 ESCAPE '\\')
               AND (?2 IS NULL OR s.id IN (SELECT value FROM json_each(?2)))
               AND (?3 IS NULL OR s.name > ?3 COLLATE NOCASE OR (s.name = ?3 COLLATE NOCASE AND s.id > ?4))
             ORDER BY s.name COLLATE NOCASE, s.id LIMIT ?5",
        )?;
        let rows = s.query_map(params![like(q), ids, an, ai, limit], |r| {
            Ok(Script {
                id: r.get(0)?,
                name: r.get(1)?,
                shell: deskhand_proto::ScriptShell::parse(&r.get::<_, String>(2)?).unwrap_or(deskhand_proto::ScriptShell::Sh),
                body: String::new(),
                timeout_s: r.get(3)?,
                creator: r.get(4)?,
                updated: r.get(5)?,
            })
        })?;
        rows.collect()
    }

    /// Jobs newest first, before job `before`, with run counts.
    pub fn jobs_page(&self, q: Option<&str>, ids: Option<&str>, before: Option<i64>, limit: i64) -> R<Vec<(Job, Counts)>> {
        let c = self.c();
        let mut s = c.prepare(&format!(
            "SELECT {} FROM jobs j LEFT JOIN users u ON u.id = j.created_by
             WHERE (?1 IS NULL OR j.name LIKE ?1 ESCAPE '\\')
               AND (?2 IS NULL OR j.id IN (SELECT value FROM json_each(?2)))
               AND (?3 IS NULL OR j.id < ?3)
             ORDER BY j.id DESC LIMIT ?4",
            crate::jobs::JOB_COLS
        ))?;
        let jobs: Vec<Job> = s.query_map(params![like(q), ids, before, limit], crate::jobs::job_row)?.collect::<R<_>>()?;
        let ids = serde_json::to_string(&jobs.iter().map(|j| j.id).collect::<Vec<_>>()).unwrap();
        let mut s = c.prepare(
            "SELECT job_id, status, count(*) FROM job_runs WHERE job_id IN (SELECT value FROM json_each(?1))
             GROUP BY job_id, status",
        )?;
        let mut counts: std::collections::HashMap<i64, Counts> = Default::default();
        for row in s.query_map([ids], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?)))? {
            let (job, status, n) = row?;
            let e = counts.entry(job).or_default();
            match status.as_str() {
                "queued" => e.queued = n,
                "running" => e.running = n,
                "ok" => e.ok = n,
                "failed" => e.failed = n,
                "timeout" => e.timeout = n,
                _ => e.offline += n,
            }
        }
        Ok(jobs.into_iter().map(|j| {
            let n = counts.remove(&j.id).unwrap_or_default();
            (j, n)
        }).collect())
    }

    /// A job's runs newest first, before run `before`, optionally of one status or
    /// searching the device name.
    pub fn runs_page(&self, job: i64, status: Option<&str>, q: Option<&str>, before: Option<i64>, limit: i64) -> R<Vec<Run>> {
        let c = self.c();
        let mut s = c.prepare(&format!(
            "SELECT {} FROM job_runs r LEFT JOIN devices d ON d.id = r.device_id
             WHERE r.job_id = ?1 AND (?2 IS NULL OR r.status = ?2)
               AND (?3 IS NULL OR d.name LIKE ?3 ESCAPE '\\' OR d.hostname LIKE ?3 ESCAPE '\\')
               AND (?4 IS NULL OR r.id < ?4)
             ORDER BY r.id DESC LIMIT ?5",
            crate::jobs::RUN_COLS
        ))?;
        let rows = s.query_map(params![job, status, like(q), before, limit], crate::jobs::run_row)?;
        rows.collect()
    }

    /// A job's run counts by status.
    pub fn run_counts(&self, job: i64) -> R<Vec<(String, i64)>> {
        let c = self.c();
        let mut s = c.prepare("SELECT status, count(*) FROM job_runs WHERE job_id = ?1 GROUP BY status ORDER BY status")?;
        let rows = s.query_map([job], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect()
    }

    /// Distinct audit actions matching `q`, after `after`.
    pub fn audit_actions_page(&self, q: Option<&str>, after: Option<&str>, limit: i64) -> R<Vec<String>> {
        let c = self.c();
        let mut s = c.prepare(
            "SELECT DISTINCT action FROM audit WHERE (?1 IS NULL OR action LIKE ?1 ESCAPE '\\')
               AND (?2 IS NULL OR action > ?2) ORDER BY action LIMIT ?3",
        )?;
        let rows = s.query_map(params![like(q), after, limit], |r| r.get(0))?;
        rows.collect()
    }
}
