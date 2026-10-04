//! Tauri commands for the native session lifecycle.
//!
//! State is `tauri::State<'_, Arc<SessionManager>>`: the manager is
//! `Sync` — its mutable state is `Arc<Mutex<…>>` internally, so each
//! method locks only its own map, briefly.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;
use tauri::{AppHandle, Emitter, State};

use crate::agent::{
    EventSink, ImagePayload, PermissionOutcome, SessionInfo, SessionManager, StalledInfo,
    StopReason,
};

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
    state: State<'_, Arc<SessionManager>>,
    cwd: String,
) -> Result<SessionInfo, crate::agent::SessionError> {
    state.start_session(PathBuf::from(cwd), sink.inner()).await
}

#[tauri::command]
pub async fn send_prompt(
    state: State<'_, Arc<SessionManager>>,
    session_id: String,
    text: String,
    images: Option<Vec<ImagePayload>>,
) -> Result<StopReason, crate::agent::SessionError> {
    // The manager owns the whole user-turn flow: image validation, the
    // user-row persistence (a single write — the command adds NONE), the
    // `prompt` command, and the turn's resolution.
    let images = images.unwrap_or_default();
    state
        .send_prompt_with_images(&session_id, text, images)
        .await
}

#[tauri::command]
pub async fn close_session(
    state: State<'_, Arc<SessionManager>>,
    session_id: String,
) -> Result<(), crate::agent::SessionError> {
    state.close_session(&session_id).await
}

/// Deliver the user's decision on a pending permission prompt to the agent.
///
/// `request_id` is the `id` of the agent's `extension_ui_request` dialog
/// (the one carried in the `permission-request` event). If the prompt is
/// no longer pending, this is a no-op.
///
/// The ADR 0025 Task 5 re-plumb: a SINGLE `state` (the `SessionManager`
/// routes through the `WorkerManager`'s `handle_for` — a MAIN session's
/// handle is in the `workers` map, a SUBAGENT session's handle in the
/// `drives` map — the `SubagentSessionManager`'s `respond_*` methods are
/// deleted: the child's pending maps live in its Worker).
#[tauri::command]
pub async fn respond_permission(
    state: State<'_, Arc<SessionManager>>,
    session_id: String,
    request_id: String,
    outcome: PermissionOutcome,
) -> Result<(), crate::agent::SessionError> {
    let hit = state
        .respond_permission(&session_id, &request_id, outcome)
        .await?;
    if !hit {
        eprintln!(
            "respond_permission: no pending entry for session {session_id} request {request_id}"
        );
    }
    Ok(())
}

/// Deliver the user's answer to a pending interactive request to the agent.
///
/// `request_id` is the interactive request's `id` (the one carried in the
/// `interactive-request` event). `result` is the response `result` `Value`
/// VERBATIM (no wrapper — for `ask`, the `AskResponsePayload`; for
/// `confirm`, `{confirmed}`; for `password`, `{password}`); the desktop
/// writes `{v:1, type:"response", id, result}`. If the request is no longer
/// pending, this is a no-op.
///
/// The ADR 0025 Task 5 re-plumb: a SINGLE `state` (the `SessionManager`
/// routes through the `WorkerManager`'s `handle_for` — main AND subagent
/// sessions both resolve; the `SubagentSessionManager`'s `respond_*`
/// methods are deleted: the child's pending maps live in its Worker).
#[tauri::command]
pub async fn respond_interactive_request(
    state: State<'_, Arc<SessionManager>>,
    session_id: String,
    request_id: String,
    result: Value,
) -> Result<(), crate::agent::SessionError> {
    let hit = state
        .respond_interactive_request(&session_id, &request_id, result)
        .await?;
    if !hit {
        eprintln!("respond_interactive_request: no pending entry for session {session_id} request {request_id}");
    }
    Ok(())
}

/// Resume a stored session: a fresh `AgentLoop` + `SessionStore::load_messages`
/// (resume from the `native_messages` table; the stored row's `model` /
/// `thinkingLevel` override the resolution chains).
///
/// Returns [`crate::agent::SessionError::NotResumable`] when the stored
/// row is missing — the UI then shows the history-only banner instead.
#[tauri::command]
pub async fn resume_session(
    sink: State<'_, Arc<dyn EventSink>>,
    state: State<'_, Arc<SessionManager>>,
    session_id: String,
    cwd: String,
) -> Result<SessionInfo, crate::agent::SessionError> {
    state
        .resume_session(&session_id, PathBuf::from(cwd), sink.inner())
        .await
}

/// Set a session config option (model / thinking level) on a live
/// session; returns the re-synthesized config options.
#[tauri::command]
pub async fn set_session_config_option(
    sink: State<'_, Arc<dyn EventSink>>,
    state: State<'_, Arc<SessionManager>>,
    session_id: String,
    config_id: String,
    value: String,
) -> Result<Vec<Value>, crate::agent::SessionError> {
    state
        .set_config_option(&session_id, &config_id, &value, sink.inner())
        .await
}

/// Cancel the session's in-flight prompt turn (the user pressed Esc).
/// Sets the session's `cancel_requested` flag, then cancels the session's
/// current TURN token (`handle.cancel()` — the turn stops, the session
/// STAYS ALIVE). The loop settles the turn `Cancelled` (the flag maps the
/// settle to `Cancelled`, not `EndTurn`), which completes the in-flight
/// `send_prompt` with `Cancelled` and unlocks the composer.
#[tauri::command]
pub async fn cancel_session(
    state: State<'_, Arc<SessionManager>>,
    session_id: String,
) -> Result<(), crate::agent::SessionError> {
    state.cancel_session(&session_id).await
}

/// The stalled-session query (the frontend's banner query — ADR 0025):
/// a Worker crash marks the session STALLED (the Supervisor's `stalled`
/// registry); the `session-stalled` event is the PUSH, this is the PULL
/// (the frontend reads the state from the event + this query). `Ok(None)`
/// when the session is not stalled (a query miss is a success — the
/// frontend's banner simply has nothing to show).
#[tauri::command]
pub async fn get_stalled_info(
    state: State<'_, Arc<SessionManager>>,
    session_id: String,
) -> Result<Option<StalledInfo>, String> {
    Ok(state.stalled_info(&session_id))
}
