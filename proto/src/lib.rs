//! Messages on the agent's control socket (`/agent/connect`), as JSON text frames.
//!
//! The agent dials out to the server, so machines behind NAT need no open ports. Each
//! browser request for a device becomes a stream: the server sends `Open`, and the agent
//! opens `/agent/stream/{sid}` and pipes it, byte for byte, to its local voxl (a desktop
//! stream) or to a new terminal (a shell stream).

use serde::{Deserialize, Serialize};

/// Agent to server.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentMsg {
    /// First message after connecting.
    Hello {
        id: String,
        hostname: String,
        os: String,
        /// voxl's password, which the server sends as a Bearer token on each request.
        voxl_token: String,
        /// The agent's version (`CARGO_PKG_VERSION`); empty from agents older than this field.
        #[serde(default)]
        version: String,
    },
    /// What the device is: sent after Hello, every few hours, and when asked.
    Inventory { inventory: Box<Inventory> },
    /// Output from a running job, as it comes. `data` is UTF-8 (invalid bytes replaced).
    JobOutput { run_id: i64, stream: OutStream, data: String },
    /// A job or action finished. `stdout`/`stderr` are the whole output, up to
    /// [`OUTPUT_CAP`] bytes each.
    JobResult {
        run_id: i64,
        status: JobStatus,
        exit_code: Option<i32>,
        stdout: String,
        stderr: String,
    },
}

/// Most output kept per stream of a job run, in bytes.
pub const OUTPUT_CAP: usize = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutStream {
    Stdout,
    Stderr,
}

/// How a job run ended on the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    /// Exit code 0 (or the action was carried out).
    Ok,
    Failed,
    /// Killed, with everything it started, after `timeout_s`.
    Timeout,
}

/// What runs a job's script.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScriptShell {
    Powershell,
    Cmd,
    Bash,
    Sh,
}

impl ScriptShell {
    pub const ALL: [ScriptShell; 4] = [ScriptShell::Powershell, ScriptShell::Cmd, ScriptShell::Bash, ScriptShell::Sh];

    pub fn as_str(self) -> &'static str {
        match self {
            ScriptShell::Powershell => "powershell",
            ScriptShell::Cmd => "cmd",
            ScriptShell::Bash => "bash",
            ScriptShell::Sh => "sh",
        }
    }

    pub fn parse(s: &str) -> Option<ScriptShell> {
        ScriptShell::ALL.into_iter().find(|x| x.as_str() == s)
    }
}

/// Built-in actions, run through the same pipeline as scripts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Action {
    Reboot {
        #[serde(default)]
        delay_s: u32,
        #[serde(default)]
        message: Option<String>,
    },
    Shutdown {
        #[serde(default)]
        delay_s: u32,
        #[serde(default)]
        message: Option<String>,
    },
    /// Signs out the console user (Windows).
    Logoff,
    /// Locks the console session (Windows).
    Lock,
    /// Sent to another agent on the target's network: it broadcasts a magic packet for
    /// each MAC to each address (UDP port 9).
    WakeOnLan { macs: Vec<String>, broadcasts: Vec<String> },
}

impl Action {
    pub fn name(&self) -> &'static str {
        match self {
            Action::Reboot { .. } => "reboot",
            Action::Shutdown { .. } => "shutdown",
            Action::Logoff => "logoff",
            Action::Lock => "lock",
            Action::WakeOnLan { .. } => "wake",
        }
    }
}

/// Server to agent.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMsg {
    /// Open a stream for this id.
    Open {
        sid: String,
        #[serde(default)]
        kind: StreamKind,
    },
    /// Collect and send the inventory now.
    RefreshInventory,
    /// Run a script; answered with `JobOutput`s and one `JobResult`.
    RunJob { run_id: i64, shell: ScriptShell, body: String, timeout_s: u64 },
    /// Carry out a built-in action; answered with one `JobResult`.
    RunAction { run_id: i64, action: Action },
}

/// What a stream connects to on the agent.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamKind {
    /// voxl's HTTP and WebSocket server, as raw bytes.
    #[default]
    Desktop,
    /// A terminal: binary frames are its bytes both ways, and text frames from the
    /// server are [`ShellControl`] JSON.
    Shell { cols: u16, rows: u16 },
}

/// Control messages on a shell stream, as text frames.
#[derive(Debug, Serialize, Deserialize)]
pub struct ShellControl {
    /// New size, as `[cols, rows]`.
    pub resize: Option<(u16, u16)>,
}

/// `POST /api/enroll` body: an enrollment token from the dashboard, traded once for this
/// device's id and secret.
#[derive(Debug, Serialize, Deserialize)]
pub struct EnrollRequest {
    pub token: String,
    pub hostname: String,
    pub os: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct EnrollResponse {
    pub device_id: String,
    /// Sent as `Authorization: Bearer {device_id}:{secret}` on every agent socket.
    pub secret: String,
}

/// A device's hardware, software and security state. Anything a probe couldn't find is
/// `None` (or empty): every field is best effort.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Inventory {
    /// Unix seconds, by the agent's clock.
    pub collected_at: i64,
    pub hostname: Option<String>,
    /// e.g. `domain: corp.example.com` or `workgroup: WORKGROUP`.
    pub domain: Option<String>,
    pub os: OsInfo,
    pub uptime_secs: Option<u64>,
    /// Unix seconds.
    pub last_boot: Option<i64>,
    /// Users with a session on the machine now.
    pub logged_in_users: Vec<String>,
    pub hardware: Hardware,
    pub cpu: Cpu,
    pub memory: Memory,
    pub disks: Vec<Disk>,
    pub network: Vec<Nic>,
    /// The address the server sees the agent connect from; filled in by the server.
    pub public_ip: Option<String>,
    pub software: Vec<Software>,
    pub security: Security,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct OsInfo {
    /// e.g. `Windows 11 Pro`, `Ubuntu`.
    pub name: Option<String>,
    pub version: Option<String>,
    pub build: Option<String>,
    pub kernel: Option<String>,
    pub arch: Option<String>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Hardware {
    pub manufacturer: Option<String>,
    pub model: Option<String>,
    pub serial: Option<String>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Cpu {
    pub model: Option<String>,
    pub cores: Option<u32>,
    pub threads: Option<u32>,
}

/// Bytes.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Memory {
    pub total: Option<u64>,
    pub used: Option<u64>,
}

/// Sizes in bytes.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Disk {
    pub mount: String,
    pub fs: Option<String>,
    pub total: u64,
    pub free: u64,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Nic {
    pub name: String,
    pub mac: Option<String>,
    /// Addresses with their prefix, e.g. `192.168.1.20/24`.
    pub ips: Vec<String>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Software {
    pub name: String,
    pub version: Option<String>,
    pub publisher: Option<String>,
    /// As the OS reports it (`YYYYMMDD` on Windows, a date on Linux).
    pub install_date: Option<String>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Security {
    /// Encrypted volumes: BitLocker per volume on Windows, LUKS mappings on Linux.
    /// `None` when the agent couldn't tell (e.g. BitLocker needs admin).
    pub encryption: Option<Vec<Encryption>>,
    pub antivirus: Option<Antivirus>,
    /// Firewall per profile (Windows: Domain/Private/Public; Linux: ufw or firewalld).
    pub firewall: Vec<Firewall>,
    pub pending_reboot: Option<bool>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Encryption {
    pub volume: String,
    /// e.g. `on`, `off`, `luks`.
    pub status: String,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Antivirus {
    pub name: String,
    pub enabled: Option<bool>,
    pub realtime: Option<bool>,
    pub signature_age_days: Option<u32>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Firewall {
    pub profile: String,
    pub enabled: Option<bool>,
}
