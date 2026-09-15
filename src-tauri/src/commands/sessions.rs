//! Tauri commands for the ACP session lifecycle.
//!
//! State is `tauri::State<'_, tokio::sync::Mutex<SessionManager>>` — tokio's
//! Mutex, NOT std's: these are async commands and holding a std lock across
//! `.await` is a deadlock trap.

use std::path::PathBuf;
use std::sync::Arc;

use agent_client_protocol::schema::v1::{ContentBlock, PromptRequest, SessionId, TextContent};
use serde_json::Value;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::Mutex;

use crate::acp::{AcpError, EventSink, PermissionOutcome, SessionInfo, SessionManager};

/// `EventSink` backed by `AppHandle::emit`.
#[derive(Clone)]
struct TauriSink(AppHandle);

impl EventSink for TauriSink {
    fn emit(&self, event: &str, payload: Value) {
        let _ = self.0.emit(event, payload);
    }
}

#[tauri::command]
pub async fn start_session(
    app: AppHandle,
    state: State<'_, Mutex<SessionManager>>,
    agent_id: String,
    cwd: String,
) -> Result<SessionInfo, AcpError> {
    let manager = state.inner().lock().await;
    let sink: Arc<dyn EventSink> = Arc::new(TauriSink(app));
    manager
        .start_session(&agent_id, PathBuf::from(cwd), &sink)
        .await
}

#[tauri::command]
pub async fn send_prompt(
    state: State<'_, Mutex<SessionManager>>,
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
    let request = PromptRequest::new(
        SessionId::new(session_id.as_str()),
        vec![ContentBlock::Text(TextContent::new(text))],
    );
    let response = cx
        .send_request(request)
        .block_task()
        .await
        .map_err(|err| AcpError::Protocol { message: err.message })?;
    Ok(response.stop_reason)
}

#[tauri::command]
pub async fn close_session(
    _app: AppHandle,
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
