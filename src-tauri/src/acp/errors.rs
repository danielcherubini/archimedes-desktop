//! Errors for the ACP session layer.

use serde::Serialize;

/// Errors surfaced by the ACP session layer.
///
/// Derives `Serialize` so it can cross the Tauri IPC boundary as a command
/// error.
#[derive(Debug, thiserror::Error, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AcpError {
    /// The requested `agent_id` is not present in the registry.
    #[error("unknown agent: {0}")]
    UnknownAgent(String),

    /// The agent process could not be spawned. `hint` explains how to fix it
    /// (e.g. install `pi` / `pi-acp`).
    #[error("failed to spawn agent: {hint}")]
    SpawnFailed {
        /// Human-readable remediation hint.
        hint: String,
    },

    /// The agent was spawned but `initialize` or `session/new` did not
    /// complete.
    #[error("initialize failed: {detail}")]
    InitializeFailed {
        /// What went wrong during initialization.
        detail: String,
    },

    /// A protocol-level error (the agent returned a JSON-RPC error, or the
    /// connection failed).
    #[error("protocol error: {0}")]
    Protocol(String),

    /// No live session with the given id exists.
    #[error("unknown session: {0}")]
    UnknownSession(String),

    /// The requested path escaped the session's sandbox root (via `..`, a
    /// symlink, or an absolute path outside the root).
    #[error("path escapes the session sandbox: {0}")]
    PathEscape(String),

    /// A filesystem operation failed (I/O error, permission, encoding, …).
    #[error("file operation failed: {0}")]
    Io(String),
}
