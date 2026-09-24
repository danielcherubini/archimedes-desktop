//! Errors for the session layer: `RpcError` is the only error type (the
//! ACP `AcpError` died with the pi-RPC swap; the read-only pre-approval
//! `FsBackend` has its own local `FsError` in `fs_backend.rs`).

use serde::Serialize;
/// Errors surfaced by the pi RPC session layer.
///
/// Derives `Serialize` so it can cross the Tauri IPC boundary as a command
/// error (the same set of derives + `Display`/`Error` impls as `AcpError`, so
/// it can replace `AcpError` in command signatures in the swap task).
#[derive(Debug, thiserror::Error, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RpcError {
    /// The pi process could not be spawned.
    #[error("failed to spawn pi: {0}")]
    Spawn(String),

    /// A stdin/stdout read-write failure.
    #[error("i/o error: {0}")]
    Io(String),

    /// The child died while a command was in flight (`Some(code)` = the exit
    /// code, `None` = the stream ended without a reaped code).
    #[error("pi process exited: {0:?}")]
    ProcessExited(Option<i32>),

    /// The establish (session start / resume) did not complete in time —
    /// replaces `AcpError::InitializeFailed` (the kept establish-timeout
    /// mechanism).
    #[error("establish timeout: {detail}")]
    EstablishTimeout { detail: String },

    /// A response arrived with `success: false`.
    #[error("command failed: {error}")]
    Command { error: String },

    /// A malformed JSON line on stdout (the stream is untrustworthy; the
    /// reader fails all in-flight sends and stops).
    #[error("malformed JSON on stdout: {0}")]
    Parse(String),

    /// The requested `agent_id` is not present in the registry.
    #[error("unknown agent: {id}")]
    UnknownAgent { id: String },

    /// No live session with the given id exists.
    #[error("unknown session: {id}")]
    UnknownSession { id: String },

    /// The stored session cannot be resumed (no stored pi session file).
    #[error("session {id} cannot be resumed")]
    NotResumable { id: String },

    /// The requested working directory does not exist (or is not a folder).
    #[error("folder not found: {path}")]
    FolderMissing { path: String },

    /// A user prompt payload failed validation (invalid image mime type,
    /// oversized image).
    #[error("invalid prompt payload: {reason}")]
    InvalidPrompt { reason: String },
}
