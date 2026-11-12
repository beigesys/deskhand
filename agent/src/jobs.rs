//! Jobs: scripts the server sends (`RunJob`) and built-in actions (`RunAction`). Results go
//! out on a channel that outlives the control socket, so a job that finishes while the
//! agent is reconnecting still reports.

use ithub_proto::{Action, AgentMsg, JobStatus, OutStream, ScriptShell, OUTPUT_CAP};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::{mpsc, Semaphore};

/// Jobs running at once; more wait their turn.
const CONCURRENT: usize = 4;
/// How often output is sent while a job runs.
const FLUSH_EVERY: Duration = Duration::from_millis(250);

pub struct Runner {
    out: mpsc::UnboundedSender<AgentMsg>,
    slots: Arc<Semaphore>,
}

impl Runner {
    pub fn new(out: mpsc::UnboundedSender<AgentMsg>) -> Runner {
        Runner { out, slots: Arc::new(Semaphore::new(CONCURRENT)) }
    }

    pub fn run_script(&self, run_id: i64, shell: ScriptShell, body: String, timeout_s: u64) {
        let out = self.out.clone();
        let slots = self.slots.clone();
        tokio::spawn(async move {
            let _slot = slots.acquire_owned().await;
            let msg = match script(run_id, shell, &body, timeout_s, &out).await {
                Ok(m) => m,
                Err(e) => result(run_id, JobStatus::Failed, None, String::new(), e),
            };
            let _ = out.send(msg);
        });
    }

    pub fn run_action(&self, run_id: i64, action: Action) {
        let out = self.out.clone();
        tokio::spawn(async move {
            let msg = match action::run(&action).await {
                Ok(stdout) => result(run_id, JobStatus::Ok, Some(0), stdout, String::new()),
                Err(e) => result(run_id, JobStatus::Failed, None, String::new(), e),
            };
            let _ = out.send(msg);
        });
    }
}

fn result(run_id: i64, status: JobStatus, exit_code: Option<i32>, stdout: String, stderr: String) -> AgentMsg {
    AgentMsg::JobResult { run_id, status, exit_code, stdout, stderr }
}

/// The program and arguments that run a script file, and the file's extension.
fn interpreter(shell: ScriptShell, path: &str) -> (&'static str, Vec<String>, &'static str) {
    match shell {
        ScriptShell::Powershell => (
            if cfg!(windows) { "powershell.exe" } else { "pwsh" },
            ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", path]
                .map(String::from)
                .to_vec(),
            "ps1",
        ),
        ScriptShell::Cmd => ("cmd.exe", ["/d", "/c", path].map(String::from).to_vec(), "cmd"),
        ScriptShell::Bash => ("bash", vec![path.to_owned()], "sh"),
        ScriptShell::Sh => ("sh", vec![path.to_owned()], "sh"),
    }
}

/// Runs one script to completion or its timeout, streaming its output.
async fn script(
    run_id: i64,
    shell: ScriptShell,
    body: &str,
    timeout_s: u64,
    out: &mpsc::UnboundedSender<AgentMsg>,
) -> Result<AgentMsg, String> {
    let ext = interpreter(shell, "").2;
    let path = std::env::temp_dir().join(format!("ithub-job-{run_id}-{}.{ext}", crate::random_hex(4)));
    // cmd wants CRLF; the rest take the body as written.
    let text = if shell == ScriptShell::Cmd { body.replace("\r\n", "\n").replace('\n', "\r\n") } else { body.to_owned() };
    std::fs::write(&path, text).map_err(|e| format!("writing the script: {e}"))?;
    let _cleanup = RemoveOnDrop(path.clone());
    let (program, args, _) = interpreter(shell, &path.to_string_lossy());

    let mut cmd = Command::new(program);
    cmd.args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    cmd.process_group(0);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    let mut child = cmd.spawn().map_err(|e| format!("starting {program}: {e}"))?;
    let tree = Tree::new(&child);

    let (chunk_tx, mut chunk_rx) = mpsc::unbounded_channel::<(OutStream, Vec<u8>)>();
    let readers = [
        child.stdout.take().map(|r| tokio::spawn(read_all(r, OutStream::Stdout, chunk_tx.clone()))),
        child.stderr.take().map(|r| tokio::spawn(read_all(r, OutStream::Stderr, chunk_tx.clone()))),
    ];
    drop(chunk_tx);

    let mut outputs = [Output::default(), Output::default()];
    let mut flush = tokio::time::interval(FLUSH_EVERY);
    let deadline = tokio::time::sleep(Duration::from_secs(timeout_s.max(1)));
    tokio::pin!(deadline);
    let mut timed_out = false;
    let mut exit = None;
    // Until the process exits (or the timeout), gather output and send it on a timer.
    loop {
        tokio::select! {
            s = child.wait(), if exit.is_none() => {
                exit = Some(s.map_err(|e| e.to_string())?);
                break;
            }
            _ = &mut deadline => {
                timed_out = true;
                tree.kill();
                let _ = child.kill().await;
                break;
            }
            Some((which, bytes)) = chunk_rx.recv() => outputs[which as usize].push(&bytes),
            _ = flush.tick() => send_pending(run_id, &mut outputs, out),
        }
    }
    // Take what's left in the pipes; something the script left running in the background
    // may hold them open, so don't wait long.
    let drain = async {
        while let Some((which, bytes)) = chunk_rx.recv().await {
            outputs[which as usize].push(&bytes);
        }
    };
    let _ = tokio::time::timeout(Duration::from_secs(2), drain).await;
    for r in readers.into_iter().flatten() {
        r.abort();
    }
    send_pending(run_id, &mut outputs, out);

    let [stdout, stderr] = outputs.map(Output::finish);
    if timed_out {
        return Ok(result(run_id, JobStatus::Timeout, None, stdout, stderr));
    }
    let code = exit.and_then(|s| s.code());
    let status = if code == Some(0) { JobStatus::Ok } else { JobStatus::Failed };
    Ok(result(run_id, status, code, stdout, stderr))
}

async fn read_all(
    mut r: impl tokio::io::AsyncRead + Unpin,
    which: OutStream,
    tx: mpsc::UnboundedSender<(OutStream, Vec<u8>)>,
) {
    let mut buf = vec![0u8; 16 * 1024];
    loop {
        match r.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if tx.send((which, buf[..n].to_vec())).is_err() {
                    break;
                }
            }
        }
    }
}

/// One stream's output: everything kept (up to the cap) and what hasn't been sent yet.
#[derive(Default)]
struct Output {
    kept: Vec<u8>,
    /// Kept bytes already sent as `JobOutput`.
    sent: usize,
    dropped: usize,
}

impl Output {
    fn push(&mut self, bytes: &[u8]) {
        let room = OUTPUT_CAP.saturating_sub(self.kept.len());
        let take = room.min(bytes.len());
        self.kept.extend_from_slice(&bytes[..take]);
        self.dropped += bytes.len() - take;
    }

    /// The unsent bytes that form whole UTF-8 characters; a character split across reads
    /// waits for its other half.
    fn take_pending(&mut self) -> Option<String> {
        let pending = &self.kept[self.sent..];
        if pending.is_empty() {
            return None;
        }
        let end = match std::str::from_utf8(pending) {
            Ok(_) => pending.len(),
            // Incomplete at the end: hold it back. Invalid in the middle: send it replaced.
            Err(e) if e.error_len().is_none() => e.valid_up_to(),
            Err(_) => pending.len(),
        };
        if end == 0 {
            return None;
        }
        let s = String::from_utf8_lossy(&pending[..end]).into_owned();
        self.sent += end;
        Some(s)
    }

    fn finish(self) -> String {
        let mut s = String::from_utf8_lossy(&self.kept).into_owned();
        if self.dropped > 0 {
            s.push_str(&format!("\n[ithub: {} more bytes not kept]", self.dropped));
        }
        s
    }
}

fn send_pending(run_id: i64, outputs: &mut [Output; 2], out: &mpsc::UnboundedSender<AgentMsg>) {
    for (i, stream) in [OutStream::Stdout, OutStream::Stderr].into_iter().enumerate() {
        if let Some(data) = outputs[i].take_pending() {
            let _ = out.send(AgentMsg::JobOutput { run_id, stream, data });
        }
    }
}

struct RemoveOnDrop(std::path::PathBuf);

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// The job's process and everything it starts: a process group on Unix, a job object on
/// Windows, so a timeout kills the whole tree.
struct Tree {
    #[cfg(unix)]
    pgid: Option<i32>,
    /// The job object's handle as a number: a HANDLE (a raw pointer) isn't Send, and the
    /// tree lives across awaits.
    #[cfg(windows)]
    job: Option<usize>,
}

#[cfg(windows)]
fn handle(h: usize) -> windows::Win32::Foundation::HANDLE {
    windows::Win32::Foundation::HANDLE(h as *mut core::ffi::c_void)
}

#[cfg(unix)]
impl Tree {
    fn new(child: &tokio::process::Child) -> Tree {
        // process_group(0) made the child the leader of a group with its own pid.
        Tree { pgid: child.id().map(|p| p as i32) }
    }

    fn kill(&self) {
        if let Some(pgid) = self.pgid {
            unsafe {
                libc::killpg(pgid, libc::SIGKILL);
            }
        }
    }
}

#[cfg(windows)]
impl Tree {
    fn new(child: &tokio::process::Child) -> Tree {
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::System::JobObjects::{AssignProcessToJobObject, CreateJobObjectW};
        let Some(process) = child.raw_handle() else {
            return Tree { job: None };
        };
        unsafe {
            let Ok(job) = CreateJobObjectW(None, None) else {
                return Tree { job: None };
            };
            // Children the script starts from here on join the job; anything it started
            // in the instant before this call doesn't.
            let _ = AssignProcessToJobObject(job, HANDLE(process));
            Tree { job: Some(job.0 as usize) }
        }
    }

    fn kill(&self) {
        if let Some(job) = self.job {
            unsafe {
                let _ = windows::Win32::System::JobObjects::TerminateJobObject(handle(job), 1);
            }
        }
    }
}

#[cfg(windows)]
impl Drop for Tree {
    /// Without KILL_ON_JOB_CLOSE, closing the job leaves anything a finished script
    /// started in the background running.
    fn drop(&mut self) {
        if let Some(job) = self.job.take() {
            unsafe {
                let _ = windows::Win32::Foundation::CloseHandle(handle(job));
            }
        }
    }
}

#[cfg(not(any(unix, windows)))]
impl Tree {
    fn new(_: &tokio::process::Child) -> Tree {
        Tree {}
    }
    fn kill(&self) {}
}

mod action {
    use ithub_proto::Action;

    /// `ITHUB_AGENT_DRY_RUN_POWER=1`: log power actions instead of doing them (testing).
    fn dry_run() -> bool {
        std::env::var("ITHUB_AGENT_DRY_RUN_POWER").is_ok_and(|v| v == "1")
    }

    /// Carries out the action; the text on success says what was done.
    pub async fn run(action: &Action) -> Result<String, String> {
        match action {
            Action::WakeOnLan { macs, broadcasts } => wake(macs, broadcasts).await,
            Action::Reboot { delay_s, message } | Action::Shutdown { delay_s, message } => {
                let reboot = matches!(action, Action::Reboot { .. });
                let what = if reboot { "reboot" } else { "shutdown" };
                if dry_run() {
                    log::warn!("dry run: would {what} in {delay_s}s ({message:?})");
                    return Ok(format!("dry run: {what} in {delay_s}s"));
                }
                power(reboot, *delay_s, message.as_deref()).await?;
                Ok(format!("{what} in {delay_s}s"))
            }
            Action::Logoff | Action::Lock => {
                let what = action.name();
                if dry_run() {
                    log::warn!("dry run: would {what}");
                    return Ok(format!("dry run: {what}"));
                }
                session(action)?;
                Ok(format!("{what} done"))
            }
        }
    }

    #[cfg(windows)]
    async fn power(reboot: bool, delay_s: u32, message: Option<&str>) -> Result<(), String> {
        let mut cmd = tokio::process::Command::new("shutdown.exe");
        cmd.arg(if reboot { "/r" } else { "/s" }).args(["/t", &delay_s.min(315_360_000).to_string()]);
        if let Some(m) = message.filter(|m| !m.is_empty()) {
            cmd.args(["/c", &m.chars().take(512).collect::<String>()]);
        }
        let s = cmd.status().await.map_err(|e| format!("shutdown.exe: {e}"))?;
        s.success().then_some(()).ok_or_else(|| format!("shutdown.exe exited with {s}"))
    }

    /// Unix: tell logged-in users, wait out the delay in the background, then power off.
    #[cfg(not(windows))]
    async fn power(reboot: bool, delay_s: u32, message: Option<&str>) -> Result<(), String> {
        if let Some(m) = message.filter(|m| !m.is_empty()) {
            let _ = tokio::process::Command::new("wall").arg(m).status().await;
        }
        let flag = if reboot { "-r" } else { "-h" };
        if delay_s == 0 {
            return run_shutdown(flag).await;
        }
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(delay_s.into())).await;
            if let Err(e) = run_shutdown(flag).await {
                log::error!("{e}");
            }
        });
        Ok(())
    }

    #[cfg(not(windows))]
    async fn run_shutdown(flag: &str) -> Result<(), String> {
        let s = tokio::process::Command::new("shutdown")
            .args([flag, "now"])
            .status()
            .await
            .map_err(|e| format!("shutdown: {e}"))?;
        s.success().then_some(()).ok_or_else(|| format!("shutdown exited with {s}"))
    }

    #[cfg(windows)]
    fn session(action: &Action) -> Result<(), String> {
        use windows::Win32::System::RemoteDesktop::{WTSGetActiveConsoleSessionId, WTSLogoffSession, WTS_CURRENT_SERVER_HANDLE};
        use windows::Win32::System::Shutdown::LockWorkStation;
        unsafe {
            match action {
                // Works as SYSTEM or as the console user.
                Action::Logoff => WTSLogoffSession(Some(WTS_CURRENT_SERVER_HANDLE), WTSGetActiveConsoleSessionId(), false)
                    .map_err(|e| format!("log off: {e}")),
                // Only from inside the user's session: the agent as a service will need its
                // session helper for this.
                Action::Lock => LockWorkStation().map_err(|e| format!("lock: {e}")),
                _ => unreachable!(),
            }
        }
    }

    #[cfg(not(windows))]
    fn session(action: &Action) -> Result<(), String> {
        Err(format!("{} is only supported on Windows", action.name()))
    }

    /// A magic packet: six 0xFF, then the MAC sixteen times.
    pub fn magic_packet(mac: &str) -> Result<[u8; 102], String> {
        let bytes: Vec<u8> = mac
            .split([':', '-'])
            .map(|h| u8::from_str_radix(h, 16))
            .collect::<Result<_, _>>()
            .map_err(|_| format!("bad MAC {mac:?}"))?;
        if bytes.len() != 6 {
            return Err(format!("bad MAC {mac:?}"));
        }
        let mut p = [0xFFu8; 102];
        for i in 0..16 {
            p[6 + i * 6..12 + i * 6].copy_from_slice(&bytes);
        }
        Ok(p)
    }

    async fn wake(macs: &[String], broadcasts: &[String]) -> Result<String, String> {
        let sock = tokio::net::UdpSocket::bind("0.0.0.0:0").await.map_err(|e| e.to_string())?;
        sock.set_broadcast(true).map_err(|e| e.to_string())?;
        let mut sent = Vec::new();
        for mac in macs {
            let p = magic_packet(mac)?;
            for b in broadcasts {
                // An address with a port ("127.0.0.1:9999") is used as given (testing).
                let to = if b.contains(':') { b.clone() } else { format!("{b}:9") };
                match sock.send_to(&p, &to).await {
                    Ok(_) => sent.push(format!("{mac} -> {to}")),
                    Err(e) => log::warn!("wake {mac} -> {to}: {e}"),
                }
            }
        }
        if sent.is_empty() {
            return Err("no magic packet could be sent".into());
        }
        Ok(format!("sent magic packets: {}", sent.join(", ")))
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn magic_packet() {
            let p = super::magic_packet("aa:bb:cc:dd:ee:0f").unwrap();
            assert_eq!(&p[..6], &[0xFF; 6]);
            assert_eq!(&p[6..12], &[0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0x0f]);
            assert_eq!(&p[96..], &[0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0x0f]);
            assert!(super::magic_packet("aa:bb").is_err());
        }
    }
}
