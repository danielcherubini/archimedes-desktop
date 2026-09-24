//! Tauri commands for session history: list, load, delete.
//!
//! History is owned by the client (this app): the frontend calls
//! `list_sessions` on boot to populate the session list, `load_history`
//! when a stored session is opened, and `delete_session` to remove a
//! session (and, via `ON DELETE CASCADE`, its messages).

use std::path::PathBuf;
use std::sync::Arc;

use tauri::State;

use crate::agent::{normalize_capabilities, SessionInfo};
use crate::storage::{Db, MessageRow};

/// All stored sessions, newest first, as `SessionInfo` (camelCase over IPC).
///
/// The stored `capabilities_json` is NORMALIZED on the way out (item 6b of
/// the swap plan): a pre-swap ACP row (or an unparseable blob) becomes
/// `{ "loadSession": false }` — the frontend's Resume button then stays
/// hidden and the history-only banner is the honest view.
#[tauri::command]
pub async fn list_sessions(state: State<'_, Arc<Db>>) -> Result<Vec<SessionInfo>, String> {
    let rows = state.list_sessions().map_err(|e| e.to_string())?;
    Ok(rows
        .into_iter()
        .map(|row| SessionInfo {
            session_id: row.id,
            agent_id: row.agent_id,
            cwd: PathBuf::from(row.cwd),
            capabilities: normalize_capabilities(&row.capabilities_json),
            config_options: None,
        })
        .collect())
}

/// A stored session's transcript, in insertion order.
#[tauri::command]
pub async fn load_history(
    state: State<'_, Arc<Db>>,
    session_id: String,
) -> Result<Vec<MessageRow>, String> {
    state.messages_for(&session_id).map_err(|e| e.to_string())
}

/// Delete a stored session; its messages are removed by the cascade.
#[tauri::command]
pub async fn delete_session(state: State<'_, Arc<Db>>, session_id: String) -> Result<(), String> {
    state.delete_session(&session_id).map_err(|e| e.to_string())
}
