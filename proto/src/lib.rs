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
    },
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
