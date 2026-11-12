//! Messages on the agent's control socket (`/agent/connect`), as JSON text frames.
//!
//! The agent dials out to the server, so machines behind NAT need no open ports. Each
//! browser request for a device becomes a stream: the server sends `Open`, and the agent
//! opens `/agent/stream/{sid}` and pipes it, byte for byte, to its local voxl.

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
    Open { sid: String },
}
