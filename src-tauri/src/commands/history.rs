//! Tauri commands for session history: list, load, archive, delete.
//!
//! History is owned by the client (this app): the frontend calls
//! `list_sessions` on boot to populate the session list, `load_history`
//! when a stored session is opened, `set_session_archived` to archive /
//! unarchive a session (ADR 0016), and `delete_session` to remove a
//! session (and, via `ON DELETE CASCADE`, its messages).

use std::sync::Arc;

use tauri::State;

use crate::agent::{SessionInfo, SessionManager};
use crate::storage::{Db, MessageRow};

/// All stored sessions, newest first, as `SessionInfo` (camelCase over IPC).
///
/// `include_archived` (ADR 0016, `Option` → the frontend may omit the
/// argument): `false` (the default) hides archived sessions; `true`
/// returns them with the `archived` flag set (the frontend splits the
/// boot list client-side).
///
/// Each row carries `config_options` synthesized from the stored
/// `model` / `thinkingLevel` (the same resolution as the resume — the
/// frontend's `SessionConfigSelect` renders them in the stored session's
/// composer, disabled) and `context_usage` read from the row's
/// `context_usage_json` (the last known usage — the frontend's store
/// drops it on close, so the row is the source of truth for the stored
/// session's context bar). Delegates to `SessionManager::list_sessions`
/// (the manager's `effective_catalog` — the user's providers, not the
/// test-only static catalog).
#[tauri::command]
pub async fn list_sessions(
    state: State<'_, Arc<SessionManager>>,
    include_archived: Option<bool>,
) -> Result<Vec<SessionInfo>, String> {
    state
        .list_sessions(include_archived.unwrap_or(false))
        .await
        .map_err(|e| e.to_string())
}

/// A stored session's transcript, in insertion order.
#[tauri::command]
pub async fn load_history(
    state: State<'_, Arc<Db>>,
    session_id: String,
) -> Result<Vec<MessageRow>, String> {
    state.messages_for(&session_id).map_err(|e| e.to_string())
}

/// Archive (or unarchive) a stored session (ADR 0016): sets the
/// `sessions.archived` flag. The transcript is NOT touched.
#[tauri::command]
pub async fn set_session_archived(
    state: State<'_, Arc<Db>>,
    session_id: String,
    archived: bool,
) -> Result<bool, String> {
    state
        .set_session_archived(&session_id, archived)
        .map_err(|e| e.to_string())
}

/// Delete a stored session; its messages are removed by the cascade.
#[tauri::command]
pub async fn delete_session(state: State<'_, Arc<Db>>, session_id: String) -> Result<(), String> {
    state.delete_session(&session_id).map_err(|e| e.to_string())
}
