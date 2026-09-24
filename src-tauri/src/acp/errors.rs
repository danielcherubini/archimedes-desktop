//! Errors for the ACP session layer.

use serde::Serialize;

/// Errors surfaced by the ACP session layer.
///
/// Derives `Serialize` so it can cross the Tauri IPC boundary as a command
/// error.
#[derive(Debug, thiserror::Error, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AcpError {
    /// The requested `agent_id` is not present in the registry.
    #[error("unknown agent: {agent_id}")]
    UnknownAgent { agent_id: String },

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
    #[error("protocol error: {message}")]
    Protocol { message: String },

    /// No live session with the given id exists.
    #[error("unknown session: {session_id}")]
    UnknownSession { session_id: String },

    /// The agent did not advertise `agent_capabilities.load_session`, so a
    /// stored session cannot be resumed. The UI should fall back to
    /// history-only viewing.
    #[error("agent {agent_id} does not support session resume")]
    NotResumable { agent_id: String },

    /// The requested path escaped the session's sandbox root (via `..`, a
    /// symlink, or an absolute path outside the root).
    #[error("path escapes the session sandbox: {path}")]
    PathEscape { path: String },

    /// The requested working directory does not exist (or is not a folder).
    #[error("folder not found: {path}")]
    FolderMissing { path: String },

    /// A filesystem operation failed (I/O error, permission, encoding, …).
    #[error("file operation failed: {detail}")]
    Io { detail: String },

    /// A user prompt payload failed validation (invalid image mime type,
    /// oversized image).
    #[error("invalid prompt payload: {message}")]
    InvalidPrompt { message: String },
}
