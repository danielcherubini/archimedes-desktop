//! Tauri commands for the ACP session lifecycle.
//!
//! State is `tauri::State<'_, tokio::sync::Mutex<SessionManager>>` — tokio's
//! Mutex, NOT std's: these are async commands and holding a std lock across
//! `.await` is a deadlock trap.

use std::path::PathBuf;
use std::sync::Arc;

use agent_client_protocol::schema::v1::{
    AgentCapabilities, ContentBlock, PromptRequest, SessionId, TextContent,
};
use serde_json::Value;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::Mutex;

use crate::acp::{AcpError, EventSink, PermissionOutcome, SessionInfo, SessionManager};
use crate::storage::Db;

/// `EventSink` backed by `AppHandle::emit`.
///
/// Managed as Tauri state so commands (and the mock-runtime IPC test) can
/// obtain it without an `AppHandle` parameter.
#[derive(Clone)]
pub struct TauriSink<R: tauri::Runtime = tauri::Wry>(pub AppHandle<R>);

impl<R: tauri::Runtime> EventSink for TauriSink<R> {
    fn emit(&self, event: &str, payload: Value) {
        let _ = self.0.emit(event, payload);
    }
}

#[tauri::command]
pub async fn start_session(
    sink: State<'_, Arc<dyn EventSink>>,
    state: State<'_, Mutex<SessionManager>>,
    agent_id: String,
    cwd: String,
) -> Result<SessionInfo, AcpError> {
    let manager = state.inner().lock().await;
    manager
        .start_session(&agent_id, PathBuf::from(cwd), sink.inner())
        .await
}

#[tauri::command]
pub async fn send_prompt(
    state: State<'_, Mutex<SessionManager>>,
    db: State<'_, Arc<Db>>,
    session_id: String,
    text: String,
) -> Result<agent_client_protocol::schema::v1::StopReason, AcpError> {
    // Grab the connection under the manager lock, then DROP the lock before
    // awaiting the turn: the turn can block on a user-paced permission
    // prompt, and `respond_permission` needs this same lock to deliver the
    // answer. Holding it across the await would deadlock.
    let cx = {
        let manager = state.inner().lock().await;
        manager.connection(&session_id).await?
    };
    // The client owns history: record the user's message before the turn.
    // (The `SessionManager::send_prompt` method does the same for direct
    // callers; the command path never goes through that method.)
    let payload = serde_json::json!({ "text": text });
    let _ = db.record_message(&session_id, "user", None, &payload.to_string());
    let request = PromptRequest::new(
        SessionId::new(session_id.as_str()),
        vec![ContentBlock::Text(TextContent::new(text))],
    );
    let response =
        cx.send_request(request)
            .block_task()
            .await
            .map_err(|err| AcpError::Protocol {
                message: err.message,
            })?;
    Ok(response.stop_reason)
}

#[tauri::command]
pub async fn close_session(
    state: State<'_, Mutex<SessionManager>>,
    session_id: String,
) -> Result<(), AcpError> {
    let manager = state.inner().lock().await;
    manager.close_session(&session_id).await
}

/// Deliver the user's decision on a pending permission prompt to the agent.
///
/// `request_id` is the JSON-RPC id of the agent's `session/request_permission`
/// request (the one carried in the `permission-request` event). If the prompt
/// is no longer pending, this is a no-op.
#[tauri::command]
pub async fn respond_permission(
    state: State<'_, Mutex<SessionManager>>,
    session_id: String,
    request_id: String,
    outcome: PermissionOutcome,
) -> Result<(), AcpError> {
    let manager = state.inner().lock().await;
    manager
        .respond_permission(&session_id, &request_id, outcome)
        .await
}

/// Resume a stored session: spawn a fresh agent for `agent_id`, initialize
/// it, then `session/load` the given session id (the two-argument form:
/// session id + cwd).
///
/// Returns [`AcpError::NotResumable`] when the stored session's negotiated
/// capabilities (or the agent's live `initialize` response) say the agent
/// does not support `session/load` — the UI then shows the history-only
/// banner instead.
#[tauri::command]
pub async fn resume_session(
    sink: State<'_, Arc<dyn EventSink>>,
    state: State<'_, Mutex<SessionManager>>,
    db: State<'_, Arc<Db>>,
    agent_id: String,
    session_id: String,
    cwd: String,
) -> Result<SessionInfo, AcpError> {
    // Honest resume semantics, checked BEFORE spawning: if the stored
    // session's negotiated capabilities say the agent cannot load sessions,
    // there is no point launching it.
    if let Ok(rows) = db.list_sessions() {
        if let Some(row) = rows.iter().find(|r| r.id == session_id) {
            let caps: AgentCapabilities =
                serde_json::from_str(&row.capabilities_json).unwrap_or_default();
            if !caps.load_session {
                return Err(AcpError::NotResumable {
                    agent_id: agent_id.clone(),
                });
            }
        }
    }
    let manager = state.inner().lock().await;
    manager
        .resume_session(&agent_id, &session_id, PathBuf::from(cwd), sink.inner())
        .await
}
