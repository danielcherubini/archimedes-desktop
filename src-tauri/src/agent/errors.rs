//! Errors for the session layer: `SessionError` is the only error type
//! (the read-only pre-approval `FsBackend` has its own local `FsError`
//! in `fs_backend.rs`).

use serde::Serialize;
/// Errors surfaced by the session layer.
///
/// Derives `Serialize` so it can cross the Tauri IPC boundary as a
/// command error.
#[derive(Debug, thiserror::Error, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionError {
    /// A filesystem read-write failure.
    #[error("i/o error: {0}")]
    Io(String),

    /// A command failed.
    #[error("command failed: {error}")]
    Command { error: String },

    /// No live session with the given id exists.
    #[error("unknown session: {id}")]
    UnknownSession { id: String },

    /// The stored session cannot be resumed (a stored row with
    /// `loadSession: false` has no native transcript to load).
    #[error("session {id} cannot be resumed")]
    NotResumable { id: String },

    /// The requested working directory does not exist (or is not a folder).
    #[error("folder not found: {path}")]
    FolderMissing { path: String },

    /// A user prompt payload failed validation (invalid image mime type,
    /// oversized image).
    #[error("invalid prompt payload: {reason}")]
    InvalidPrompt { reason: String },

    /// The turn did not settle within the settle timeout (a hung turn).
    #[error("settle timeout: {detail}")]
    SettleTimeout { detail: String },
}
