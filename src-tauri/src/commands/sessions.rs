//! Tauri commands for the ACP session lifecycle.
//!
//! State is `tauri::State<'_, tokio::sync::Mutex<SessionManager>>` — tokio's
//! Mutex, NOT std's: these are async commands and holding a std lock across
//! `.await` is a deadlock trap.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::Mutex;

use crate::acp::{AcpError, EventSink, SessionInfo, SessionManager};
use agent_client_protocol::schema::v1::StopReason;

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
) -> Result<StopReason, AcpError> {
    let manager = state.inner().lock().await;
    manager.send_prompt(&session_id, text).await
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
