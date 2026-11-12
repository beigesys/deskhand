//! Jobs: a script or built-in action, run on devices once or on a cron schedule (UTC).
//! Each device gets a run; a device that's offline gets it when its agent reconnects,
//! unless the run expires first. Output streams to the job's page as it comes.

use crate::db::Db;
use crate::{now, send_to_agent, App, Shared};
use deskhand_proto::{Action, JobStatus, OutStream, ScriptShell, ServerMsg, OUTPUT_CAP};
use rusqlite::{params, OptionalExtension, Row};
use std::net::Ipv4Addr;
use std::str::FromStr;
use std::time::Duration;

/// How long a run may stay `running` past its timeout before it's given up on.
const LOST_AFTER: i64 = 300;
/// Timeout for built-in actions.
pub const ACTION_TIMEOUT: i64 = 60;

pub struct Script {
    pub id: i64,
    pub name: String,
    pub shell: ScriptShell,
    pub body: String,
    pub timeout_s: i64,
    pub creator: Option<String>,
    pub updated: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    Devices(Vec<String>),
    /// `None` once the group is deleted: then it matches nothing.
    Group(Option<i64>),
    All,
}

/// What a job runs.
#[derive(Clone, Debug)]
pub enum Task {
    Script { script_id: Option<i64>, shell: ScriptShell, body: String },
    Action(Action),
}

pub struct Job {
    pub id: i64,
    pub name: String,
    pub task: Task,
    pub timeout_s: i64,
    pub target: Target,
    pub schedule: Option<String>,
    pub next_fire: Option<i64>,
    pub last_fired: Option<i64>,
    pub expires_after: i64,
    pub enabled: bool,
    pub creator: Option<String>,
    pub created_by: Option<i64>,
    pub created: i64,
}

pub struct NewJob {
    pub name: String,
    pub task: Task,
    pub timeout_s: i64,
    pub target: Target,
    pub schedule: Option<String>,
    pub expires_after: i64,
    pub created_by: i64,
}

pub struct Run {
    pub id: i64,
    pub device_id: String,
    pub device_name: String,
    pub via: Option<String>,
    pub status: String,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub queued: i64,
    pub started: Option<i64>,
    pub finished: Option<i64>,
}

/// Run counts by status, for the jobs list.
#[derive(Default)]
pub struct Counts {
    pub queued: i64,
    pub running: i64,
    pub ok: i64,
    pub failed: i64,
    pub timeout: i64,
    pub offline: i64,
}

/// What a job page hears about while it's open.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    /// More output; `end` is the stream's length in characters after it.
    Output { job_id: i64, run_id: i64, stream: OutStream, data: String, end: i64 },
    Status {
        job_id: i64,
        run_id: i64,
        status: String,
        exit_code: Option<i32>,
        started: Option<i64>,
        finished: Option<i64>,
        stdout: String,
        stderr: String,
    },
    /// The job fired again: there are runs the page doesn't show.
    NewRuns { job_id: i64 },
}

impl Event {
    pub fn job_id(&self) -> i64 {
        match self {
            Event::Output { job_id, .. } | Event::Status { job_id, .. } | Event::NewRuns { job_id } => *job_id,
        }
    }
}

/// A cron expression as the `cron` crate wants it: five fields (minute first) get a
/// leading seconds field of 0.
fn cron_schedule(expr: &str) -> Result<cron::Schedule, String> {
    let expr = expr.trim();
    let full = match expr.split_whitespace().count() {
        5 => format!("0 {expr}"),
        6 | 7 => expr.to_owned(),
        _ => return Err("a cron expression has five fields: minute hour day month weekday".into()),
    };
    cron::Schedule::from_str(&full).map_err(|e| format!("bad cron expression: {e}"))
}

/// Checks a cron expression; the error says what's wrong.
pub fn check_cron(expr: &str) -> Result<(), String> {
    let s = cron_schedule(expr)?;
    s.upcoming(chrono::Utc).next().map(|_| ()).ok_or_else(|| "that schedule never fires".into())
}

/// The first time after `t` (Unix seconds) the schedule fires.
pub fn next_fire(expr: &str, t: i64) -> Option<i64> {
    let s = cron_schedule(expr).ok()?;
    let after = chrono::DateTime::from_timestamp(t, 0)?;
    s.after(&after).next().map(|d| d.timestamp())
}

fn shell_of(s: Option<String>) -> ScriptShell {
    s.as_deref().and_then(ScriptShell::parse).unwrap_or(ScriptShell::Sh)
}

const JOB_COLS: &str = "j.id, j.name, j.script_id, j.shell, j.body, j.timeout_s, j.action, j.target_kind,
    j.target_devices, j.target_group, j.schedule, j.next_fire, j.last_fired, j.expires_after, j.enabled,
    u.username, j.created_by, j.created";

fn job_row(r: &Row) -> rusqlite::Result<Job> {
    job_row_at(r, 0)
}

/// A job from [`JOB_COLS`] starting at column `o`.
fn job_row_at(r: &Row, o: usize) -> rusqlite::Result<Job> {
    let action: Option<String> = r.get(o + 6)?;
    let task = match action.and_then(|a| serde_json::from_str::<Action>(&a).ok()) {
        Some(a) => Task::Action(a),
        None => Task::Script {
            script_id: r.get(o + 2)?,
            shell: shell_of(r.get(o + 3)?),
            body: r.get::<_, Option<String>>(o + 4)?.unwrap_or_default(),
        },
    };
    let kind: String = r.get(o + 7)?;
    let target = match kind.as_str() {
        "devices" => Target::Devices(
            r.get::<_, Option<String>>(o + 8)?
                .and_then(|j| serde_json::from_str(&j).ok())
                .unwrap_or_default(),
        ),
        "group" => Target::Group(r.get(o + 9)?),
        _ => Target::All,
    };
    Ok(Job {
        id: r.get(o)?,
        name: r.get(o + 1)?,
        task,
        timeout_s: r.get(o + 5)?,
        target,
        schedule: r.get(o + 10)?,
        next_fire: r.get(o + 11)?,
        last_fired: r.get(o + 12)?,
        expires_after: r.get(o + 13)?,
        enabled: r.get(o + 14)?,
        creator: r.get(o + 15)?,
        created_by: r.get(o + 16)?,
        created: r.get(o + 17)?,
    })
}

const RUN_COLS: &str = "r.id, r.device_id, coalesce(d.name, r.device_id), r.via, r.status, r.exit_code,
    r.stdout, r.stderr, r.queued, r.started, r.finished";

fn run_row(r: &Row) -> rusqlite::Result<Run> {
    Ok(Run {
        id: r.get(0)?,
        device_id: r.get(1)?,
        device_name: r.get(2)?,
        via: r.get(3)?,
        status: r.get(4)?,
        exit_code: r.get(5)?,
        stdout: r.get(6)?,
        stderr: r.get(7)?,
        queued: r.get(8)?,
        started: r.get(9)?,
        finished: r.get(10)?,
    })
}

type R<T> = rusqlite::Result<T>;

impl Db {
    // --- scripts ---

    pub fn scripts(&self) -> R<Vec<Script>> {
        let c = self.c();
        let mut s = c.prepare(
            "SELECT s.id, s.name, s.shell, s.body, s.timeout_s, u.username, s.updated
             FROM scripts s LEFT JOIN users u ON u.id = s.created_by ORDER BY s.name COLLATE NOCASE",
        )?;
        let rows = s.query_map([], |r| {
            Ok(Script {
                id: r.get(0)?,
                name: r.get(1)?,
                shell: shell_of(r.get(2)?),
                body: r.get(3)?,
                timeout_s: r.get(4)?,
                creator: r.get(5)?,
                updated: r.get(6)?,
            })
        })?;
        rows.collect()
    }

    pub fn script(&self, id: i64) -> R<Option<Script>> {
        Ok(self.scripts()?.into_iter().find(|s| s.id == id))
    }

    pub fn add_script(&self, name: &str, shell: ScriptShell, body: &str, timeout_s: i64, by: i64, now: i64) -> R<i64> {
        let c = self.c();
        c.execute(
            "INSERT INTO scripts (name, shell, body, timeout_s, created_by, created, updated) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
            params![name, shell.as_str(), body, timeout_s, by, now],
        )?;
        Ok(c.last_insert_rowid())
    }

    pub fn update_script(&self, id: i64, name: &str, shell: ScriptShell, body: &str, timeout_s: i64, now: i64) -> R<usize> {
        self.c().execute(
            "UPDATE scripts SET name = ?2, shell = ?3, body = ?4, timeout_s = ?5, updated = ?6 WHERE id = ?1",
            params![id, name, shell.as_str(), body, timeout_s, now],
        )
    }

    pub fn delete_script(&self, id: i64) -> R<usize> {
        self.c().execute("DELETE FROM scripts WHERE id = ?1", [id])
    }

    // --- jobs ---

    pub fn add_job(&self, j: &NewJob, now: i64) -> R<i64> {
        let (script_id, shell, body, action) = match &j.task {
            Task::Script { script_id, shell, body } => (*script_id, Some(shell.as_str()), Some(body.as_str()), None),
            Task::Action(a) => (None, None, None, Some(serde_json::to_string(a).expect("action json"))),
        };
        let (kind, devices, group) = match &j.target {
            Target::Devices(d) => ("devices", Some(serde_json::to_string(d).expect("ids json")), None),
            Target::Group(g) => ("group", None, *g),
            Target::All => ("all", None, None),
        };
        let next = j.schedule.as_deref().and_then(|s| next_fire(s, now));
        let c = self.c();
        c.execute(
            "INSERT INTO jobs (name, script_id, shell, body, timeout_s, action, target_kind, target_devices,
                target_group, schedule, next_fire, expires_after, created_by, created)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                j.name, script_id, shell, body, j.timeout_s, action, kind, devices, group,
                j.schedule, next, j.expires_after, j.created_by, now
            ],
        )?;
        Ok(c.last_insert_rowid())
    }

    pub fn jobs(&self) -> R<Vec<(Job, Counts)>> {
        let c = self.c();
        let mut s = c.prepare(&format!(
            "SELECT {JOB_COLS} FROM jobs j LEFT JOIN users u ON u.id = j.created_by ORDER BY j.id DESC LIMIT 500"
        ))?;
        let jobs: Vec<Job> = s.query_map([], job_row)?.collect::<R<_>>()?;
        let mut s = c.prepare("SELECT job_id, status, count(*) FROM job_runs GROUP BY job_id, status")?;
        let mut counts: std::collections::HashMap<i64, Counts> = Default::default();
        let rows = s.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?)))?;
        for row in rows {
            let (job, status, n) = row?;
            let e = counts.entry(job).or_default();
            match status.as_str() {
                "queued" => e.queued = n,
                "running" => e.running = n,
                "ok" => e.ok = n,
                "failed" => e.failed = n,
                "timeout" => e.timeout = n,
                _ => e.offline = n,
            }
        }
        Ok(jobs
            .into_iter()
            .map(|j| {
                let n = counts.remove(&j.id).unwrap_or_default();
                (j, n)
            })
            .collect())
    }

    pub fn job(&self, id: i64) -> R<Option<Job>> {
        self.c()
            .query_row(
                &format!("SELECT {JOB_COLS} FROM jobs j LEFT JOIN users u ON u.id = j.created_by WHERE j.id = ?1"),
                [id],
                job_row,
            )
            .optional()
    }

    pub fn set_job_enabled(&self, id: i64, enabled: bool, now: i64) -> R<usize> {
        // Re-enabling starts from now, not from the fire times missed while it was off.
        let next = self
            .job(id)?
            .and_then(|j| j.schedule)
            .and_then(|s| next_fire(&s, now));
        self.c().execute("UPDATE jobs SET enabled = ?2, next_fire = ?3 WHERE id = ?1", params![id, enabled, next])
    }

    pub fn delete_job(&self, id: i64) -> R<usize> {
        self.c().execute("DELETE FROM jobs WHERE id = ?1", [id])
    }

    fn due_jobs(&self, now: i64) -> R<Vec<i64>> {
        let c = self.c();
        let mut s = c.prepare(
            "SELECT id FROM jobs WHERE schedule IS NOT NULL AND enabled = 1 AND next_fire IS NOT NULL AND next_fire <= ?1",
        )?;
        let rows = s.query_map([now], |r| r.get(0))?;
        rows.collect()
    }

    fn set_fired(&self, id: i64, fired: i64, next: Option<i64>) -> R<usize> {
        self.c()
            .execute("UPDATE jobs SET last_fired = ?2, next_fire = ?3 WHERE id = ?1", params![id, fired, next])
    }

    /// The device ids a target means now.
    pub fn target_devices(&self, t: &Target) -> R<Vec<String>> {
        let c = self.c();
        let ids = match t {
            Target::Devices(ids) => {
                let mut s = c.prepare("SELECT id FROM devices WHERE id = ?1")?;
                let mut found = Vec::new();
                for id in ids {
                    if let Some(id) = s.query_row([id], |r| r.get::<_, String>(0)).optional()? {
                        found.push(id);
                    }
                }
                found
            }
            Target::Group(None) => Vec::new(),
            Target::Group(Some(g)) => {
                let mut s = c.prepare("SELECT id FROM devices WHERE group_id = ?1 ORDER BY name")?;
                let rows = s.query_map([g], |r| r.get(0))?;
                rows.collect::<R<_>>()?
            }
            Target::All => {
                let mut s = c.prepare("SELECT id FROM devices ORDER BY name")?;
                let rows = s.query_map([], |r| r.get(0))?;
                rows.collect::<R<_>>()?
            }
        };
        Ok(ids)
    }

    // --- runs ---

    fn add_run(&self, job: i64, device: &str, via: Option<&str>, now: i64, expires: i64) -> R<i64> {
        let c = self.c();
        c.execute(
            "INSERT INTO job_runs (job_id, device_id, via, status, queued, expires) VALUES (?1, ?2, ?3, 'queued', ?4, ?5)",
            params![job, device, via, now, expires],
        )?;
        Ok(c.last_insert_rowid())
    }

    pub fn runs(&self, job: i64) -> R<Vec<Run>> {
        let c = self.c();
        let mut s = c.prepare(&format!(
            "SELECT {RUN_COLS} FROM job_runs r LEFT JOIN devices d ON d.id = r.device_id
             WHERE r.job_id = ?1 ORDER BY r.id DESC LIMIT 2000"
        ))?;
        let rows = s.query_map([job], run_row)?;
        rows.collect()
    }

    /// The device's latest runs with their job names, for its page.
    pub fn device_runs(&self, device: &str, limit: i64) -> R<Vec<(Run, i64, String)>> {
        let c = self.c();
        let mut s = c.prepare(&format!(
            "SELECT {RUN_COLS}, j.id, j.name FROM job_runs r JOIN jobs j ON j.id = r.job_id
             LEFT JOIN devices d ON d.id = r.device_id
             WHERE r.device_id = ?1 ORDER BY r.id DESC LIMIT ?2"
        ))?;
        let rows = s.query_map(params![device, limit], |r| Ok((run_row(r)?, r.get(11)?, r.get(12)?)))?;
        rows.collect()
    }

    fn run(&self, id: i64) -> R<Option<(i64, Run)>> {
        self.c()
            .query_row(
                &format!(
                    "SELECT {RUN_COLS}, r.job_id FROM job_runs r LEFT JOIN devices d ON d.id = r.device_id WHERE r.id = ?1"
                ),
                [id],
                |r| Ok((r.get(11)?, run_row(r)?)),
            )
            .optional()
    }

    /// Runs waiting for this device (not yet expired), oldest first, with their jobs.
    fn queued_for(&self, device: &str, now: i64) -> R<Vec<(i64, Job)>> {
        let c = self.c();
        let mut s = c.prepare(&format!(
            "SELECT r.id, {JOB_COLS} FROM job_runs r JOIN jobs j ON j.id = r.job_id
             LEFT JOIN users u ON u.id = j.created_by
             WHERE r.device_id = ?1 AND r.via IS NULL AND r.status = 'queued' AND r.expires > ?2 ORDER BY r.id"
        ))?;
        let rows = s.query_map(params![device, now], |r| {
            Ok((r.get(0)?, job_row_at(r, 1)?))
        })?;
        rows.collect()
    }

    /// Marks a queued run as sent to its agent; false if it wasn't queued any more.
    fn mark_running(&self, run: i64, now: i64) -> R<bool> {
        Ok(self.c().execute(
            "UPDATE job_runs SET status = 'running', started = ?2 WHERE id = ?1 AND status = 'queued'",
            params![run, now],
        )? == 1)
    }

    /// Appends output from `agent` to a run it's carrying out; returns the job and the
    /// stream's new length (characters), or `None` if that agent has no such run running.
    fn append_output(&self, run: i64, agent: &str, stream: OutStream, data: &str) -> R<Option<(i64, i64)>> {
        let col = match stream {
            OutStream::Stdout => "stdout",
            OutStream::Stderr => "stderr",
        };
        let c = self.c();
        let n = c.execute(
            &format!(
                "UPDATE job_runs SET {col} = substr({col} || ?3, 1, {OUTPUT_CAP})
                 WHERE id = ?1 AND status = 'running' AND coalesce(via, device_id) = ?2"
            ),
            params![run, agent, data],
        )?;
        if n == 0 {
            return Ok(None);
        }
        c.query_row(&format!("SELECT job_id, length({col}) FROM job_runs WHERE id = ?1"), [run], |r| {
            Ok(Some((r.get(0)?, r.get(1)?)))
        })
    }

    /// Records a run's result from `agent`; false if that agent has no such run running.
    #[allow(clippy::too_many_arguments)]
    fn finish_run(
        &self,
        run: i64,
        agent: &str,
        status: &str,
        exit_code: Option<i32>,
        stdout: &str,
        stderr: &str,
        now: i64,
    ) -> R<bool> {
        Ok(self.c().execute(
            "UPDATE job_runs SET status = ?3, exit_code = ?4, stdout = substr(?5, 1, ?7), stderr = substr(?6, 1, ?7),
                finished = ?8
             WHERE id = ?1 AND status = 'running' AND coalesce(via, device_id) = ?2",
            params![run, agent, status, exit_code, stdout, stderr, OUTPUT_CAP as i64, now],
        )? == 1)
    }

    /// Ends a run without its agent (expired, lost, or couldn't be sent).
    fn fail_run(&self, run: i64, from: &str, status: &str, why: &str, now: i64) -> R<bool> {
        Ok(self.c().execute(
            "UPDATE job_runs SET status = ?3, stderr = stderr || ?4, finished = ?5 WHERE id = ?1 AND status = ?2",
            params![run, from, status, why, now],
        )? == 1)
    }

    /// Queued runs past their expiry, and running ones long past their timeout.
    fn stale_runs(&self, now: i64) -> R<(Vec<i64>, Vec<i64>)> {
        let c = self.c();
        let expired = c
            .prepare("SELECT id FROM job_runs WHERE status = 'queued' AND expires <= ?1")?
            .query_map([now], |r| r.get(0))?
            .collect::<R<_>>()?;
        let lost = c
            .prepare(
                "SELECT r.id FROM job_runs r JOIN jobs j ON j.id = r.job_id
                 WHERE r.status = 'running' AND r.started + j.timeout_s + ?2 <= ?1",
            )?
            .query_map(params![now, LOST_AFTER], |r| r.get(0))?
            .collect::<R<_>>()?;
        Ok((expired, lost))
    }
}

// --- the engine ---

fn send_status(app: &App, run_id: i64) {
    if let Ok(Some((job_id, r))) = app.db.run(run_id) {
        let _ = app.job_events.send(Event::Status {
            job_id,
            run_id,
            status: r.status,
            exit_code: r.exit_code,
            started: r.started,
            finished: r.finished,
            stdout: r.stdout,
            stderr: r.stderr,
        });
    }
}

/// The message that carries a run out on its agent.
fn message(run_id: i64, job: &Job) -> ServerMsg {
    match &job.task {
        Task::Script { shell, body, .. } => ServerMsg::RunJob {
            run_id,
            shell: *shell,
            body: body.clone(),
            timeout_s: job.timeout_s.max(1) as u64,
        },
        Task::Action(a) => ServerMsg::RunAction { run_id, action: a.clone() },
    }
}

/// Sends a device its queued runs: when a job fires and whenever its agent connects.
pub fn dispatch(app: &App, device: &str) {
    let now = now();
    let queued = match app.db.queued_for(device, now) {
        Ok(q) => q,
        Err(e) => return log::error!("queued runs for {device}: {e}"),
    };
    for (run_id, job) in queued {
        // Mark first: the result can't then arrive for a run that still looks queued.
        if !app.db.mark_running(run_id, now).unwrap_or(false) {
            continue;
        }
        if send_to_agent(app, device, message(run_id, &job)) {
            send_status(app, run_id);
        } else {
            // Went offline in between: back in the queue.
            let _ = app.db.c().execute(
                "UPDATE job_runs SET status = 'queued', started = NULL WHERE id = ?1 AND status = 'running'",
                [run_id],
            );
            break;
        }
    }
}

/// Fires a job: a run for each device it targets, sent to those online now.
pub fn fire(app: &App, job_id: i64) -> Result<usize, String> {
    let job = app.db.job(job_id).map_err(|e| e.to_string())?.ok_or("no such job")?;
    let devices = app.db.target_devices(&job.target).map_err(|e| e.to_string())?;
    let now = now();
    let expires = now + job.expires_after.max(60);
    let what = match &job.task {
        Task::Script { .. } => "script".to_owned(),
        Task::Action(a) => a.name().to_owned(),
    };
    for device in &devices {
        if let Task::Action(Action::WakeOnLan { .. }) = &job.task {
            wake(app, &job, device, now, expires);
        } else {
            match app.db.add_run(job.id, device, None, now, expires) {
                Ok(_) => {}
                Err(e) => {
                    log::error!("adding a run of job {job_id} for {device}: {e}");
                    continue;
                }
            }
        }
        app.db.audit(now, job.created_by, Some(device), "job.run", &format!("job {} ({}): {what}", job.id, job.name));
    }
    let _ = app.job_events.send(Event::NewRuns { job_id });
    for device in &devices {
        dispatch(app, device);
    }
    Ok(devices.len())
}

/// An IPv4 address with its prefix length from `a.b.c.d/n`.
fn parse_cidr(s: &str) -> Option<(Ipv4Addr, u8)> {
    let (ip, len) = s.split_once('/')?;
    let ip: Ipv4Addr = ip.parse().ok()?;
    let len: u8 = len.parse().ok()?;
    (len <= 32).then_some((ip, len))
}

fn mask(len: u8) -> u32 {
    if len == 0 { 0 } else { u32::MAX << (32 - len) }
}

/// Bridges, VPNs and container networks: the same addresses show up on unrelated
/// machines (every Docker host has 172.17.0.1/16), so they say nothing about who shares
/// a LAN.
fn virtual_nic(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    ["docker", "br-", "veth", "virbr", "vmnet", "vboxnet", "vethernet", "tailscale", "zt", "wg", "tun", "tap", "utun", "cni", "flannel", "cali", "podman", "lxc", "lxd"]
        .iter()
        .any(|p| n.starts_with(p))
}

/// IPv4 addresses (with prefix) on real interfaces.
fn lan_ips(inv: &deskhand_proto::Inventory) -> Vec<(Ipv4Addr, u8)> {
    inv.network
        .iter()
        .filter(|n| !virtual_nic(&n.name))
        .flat_map(|n| n.ips.iter().filter_map(|s| parse_cidr(s)))
        .collect()
}

/// The target's wakeable interfaces: (MAC, network, prefix length, broadcast address),
/// for real IPv4 LANs (not loopback, link-local, virtual, or host routes like a VPN's /32).
fn lan_interfaces(inv: &deskhand_proto::Inventory) -> Vec<(String, u32, u8, Ipv4Addr)> {
    let mut out = Vec::new();
    for nic in inv.network.iter().filter(|n| !virtual_nic(&n.name)) {
        let Some(mac) = nic.mac.as_deref().filter(|m| !m.is_empty() && *m != "00:00:00:00:00:00") else {
            continue;
        };
        for (ip, len) in nic.ips.iter().filter_map(|s| parse_cidr(s)) {
            if ip.is_loopback() || ip.is_link_local() || !(8..=30).contains(&len) {
                continue;
            }
            let net = u32::from(ip) & mask(len);
            out.push((mac.to_owned(), net, len, Ipv4Addr::from(net | !mask(len))));
        }
    }
    out
}

/// A wake-on-LAN run for `device`: carried out by an online agent in the same group with
/// an address on one of the device's LANs, which broadcasts the magic packet there.
fn wake(app: &App, job: &Job, device: &str, now: i64, expires: i64) {
    let fail = |why: &str| {
        if let Ok(run) = app.db.add_run(job.id, device, None, now, expires) {
            let _ = app.db.mark_running(run, now);
            let _ = app.db.fail_run(run, "running", "failed", why, now);
        }
    };
    let target = match app.db.device(device) {
        Ok(Some(d)) => d,
        _ => return fail("device not found"),
    };
    let Some((_, inv)) = app.db.inventory(device).ok().flatten() else {
        return fail("no inventory for this device yet, so its MAC address isn't known");
    };
    let lans = lan_interfaces(&inv);
    if lans.is_empty() {
        return fail("the device's inventory shows no wired or wireless IPv4 network with a MAC address");
    }
    let online: Vec<String> = app.agents.lock().unwrap().keys().filter(|id| *id != device).cloned().collect();
    for relay in online {
        let Ok(Some(r)) = app.db.device(&relay) else { continue };
        if r.group_id != target.group_id {
            continue;
        }
        let Some((_, rinv)) = app.db.inventory(&relay).ok().flatten() else { continue };
        let relay_ips = lan_ips(&rinv);
        // The target's LANs this relay has an address on.
        let shared: Vec<&(String, u32, u8, Ipv4Addr)> = lans
            .iter()
            .filter(|(_, net, len, _)| relay_ips.iter().any(|(ip, _)| u32::from(*ip) & mask(*len) == *net))
            .collect();
        if shared.is_empty() {
            continue;
        }
        let mut macs: Vec<String> = shared.iter().map(|l| l.0.clone()).collect();
        macs.sort();
        macs.dedup();
        let mut broadcasts: Vec<String> = shared.iter().map(|l| l.3.to_string()).collect();
        broadcasts.sort();
        broadcasts.dedup();
        broadcasts.push("255.255.255.255".into());
        let Ok(run) = app.db.add_run(job.id, device, Some(&relay), now, expires) else { return };
        let _ = app.db.mark_running(run, now);
        let msg = ServerMsg::RunAction { run_id: run, action: Action::WakeOnLan { macs, broadcasts } };
        if send_to_agent(app, &relay, msg) {
            let _ = app.db.c().execute(
                "UPDATE job_runs SET stdout = ?2 WHERE id = ?1",
                params![run, format!("via {}\n", r.name)],
            );
        } else {
            let _ = app.db.fail_run(run, "running", "failed", "the relay agent went offline", now);
        }
        return;
    }
    fail("no online agent in the same group shares a network with this device");
}

/// Output from an agent for one of its runs.
pub fn on_output(app: &App, agent: &str, run_id: i64, stream: OutStream, data: String) {
    match app.db.append_output(run_id, agent, stream, &data) {
        Ok(Some((job_id, end))) => {
            let _ = app.job_events.send(Event::Output { job_id, run_id, stream, data, end });
        }
        Ok(None) => log::warn!("agent {agent} sent output for run {run_id}, which it isn't running"),
        Err(e) => log::error!("saving output of run {run_id}: {e}"),
    }
}

/// A run's result from an agent.
pub fn on_result(app: &App, agent: &str, run_id: i64, status: JobStatus, exit_code: Option<i32>, stdout: String, stderr: String) {
    let status = match status {
        JobStatus::Ok => "ok",
        JobStatus::Failed => "failed",
        JobStatus::Timeout => "timeout",
    };
    // A wake-on-LAN run keeps the "via" line it started with.
    let stdout = match app.db.run(run_id) {
        Ok(Some((_, r))) if r.via.is_some() => format!("{}{stdout}", r.stdout),
        _ => stdout,
    };
    match app.db.finish_run(run_id, agent, status, exit_code, &stdout, &stderr, now()) {
        Ok(true) => send_status(app, run_id),
        Ok(false) => log::warn!("agent {agent} sent a result for run {run_id}, which it isn't running"),
        Err(e) => log::error!("saving result of run {run_id}: {e}"),
    }
}

/// Every few seconds: fire due scheduled jobs, expire runs that waited too long for an
/// offline device, and give up on runs whose agent never answered.
pub async fn scheduler(app: Shared) {
    let mut tick = tokio::time::interval(Duration::from_secs(5));
    loop {
        tick.tick().await;
        let now = now();
        for id in app.db.due_jobs(now).unwrap_or_default() {
            let next = app.db.job(id).ok().flatten().and_then(|j| j.schedule).and_then(|s| next_fire(&s, now));
            let _ = app.db.set_fired(id, now, next);
            match fire(&app, id) {
                Ok(n) => log::info!("scheduled job {id} fired for {n} devices"),
                Err(e) => log::error!("scheduled job {id}: {e}"),
            }
        }
        let Ok((expired, lost)) = app.db.stale_runs(now) else { continue };
        for run in expired {
            if app.db.fail_run(run, "queued", "offline", "the device didn't come online before the run expired", now).unwrap_or(false) {
                send_status(&app, run);
            }
        }
        for run in lost {
            if app.db.fail_run(run, "running", "failed", "\n[deskhand: no result from the agent]", now).unwrap_or(false) {
                send_status(&app, run);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cron_five_fields() {
        assert!(check_cron("* * * * *").is_ok());
        assert!(check_cron("0 3 * * 1-5").is_ok());
        assert!(check_cron("bogus").is_err());
        assert!(check_cron("61 * * * *").is_err());
        // 2026-01-01T00:00:30Z: next minute boundary.
        assert_eq!(next_fire("* * * * *", 1767225630), Some(1767225660));
    }

    #[test]
    fn lans() {
        let inv = deskhand_proto::Inventory {
            network: vec![
                deskhand_proto::Nic { name: "lo".into(), mac: None, ips: vec!["127.0.0.1/8".into()] },
                deskhand_proto::Nic {
                    name: "eth0".into(),
                    mac: Some("aa:bb:cc:dd:ee:ff".into()),
                    ips: vec!["192.168.1.20/24".into(), "fe80::1/64".into()],
                },
                deskhand_proto::Nic { name: "ts".into(), mac: Some("11:22:33:44:55:66".into()), ips: vec!["100.64.0.5/32".into()] },
                deskhand_proto::Nic { name: "docker0".into(), mac: Some("02:42:ac:11:00:01".into()), ips: vec!["172.17.0.1/16".into()] },
            ],
            ..Default::default()
        };
        let l = lan_interfaces(&inv);
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].0, "aa:bb:cc:dd:ee:ff");
        assert_eq!(l[0].3, Ipv4Addr::new(192, 168, 1, 255));
    }
}
