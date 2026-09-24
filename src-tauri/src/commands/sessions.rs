//! Tauri commands for the pi-RPC session lifecycle.
//!
//! State is `tauri::State<'_, Arc<SessionManager>>`: the manager is
//! `Sync` — its mutable state is `Arc<Mutex<…>>` internally, so each
//! method locks only its own map, briefly.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;
use tauri::{AppHandle, Emitter, State};

use crate::agent::{
    EventSink, ImagePayload, PermissionOutcome, SessionInfo, SessionManager, StopReason,
    SubagentSessionManager,
};
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
    state: State<'_, Arc<SessionManager>>,
    agent_id: String,
    cwd: String,
) -> Result<SessionInfo, crate::agent::RpcError> {
    state
        .start_session(&agent_id, PathBuf::from(cwd), sink.inner())
        .await
}

#[tauri::command]
pub async fn send_prompt(
    state: State<'_, Arc<SessionManager>>,
    session_id: String,
    text: String,
    images: Option<Vec<ImagePayload>>,
) -> Result<StopReason, crate::agent::RpcError> {
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
) -> Result<(), crate::agent::RpcError> {
    state.close_session(&session_id).await
}

/// Deliver the user's decision on a pending permission prompt to the agent.
///
/// `request_id` is the `id` of the agent's `extension_ui_request` dialog
/// (the one carried in the `permission-request` event). If the prompt is
/// no longer pending, this is a no-op.
#[tauri::command]
pub async fn respond_permission(
    state: State<'_, Arc<SessionManager>>,
    subagent_state: State<'_, Arc<SubagentSessionManager>>,
    session_id: String,
    request_id: String,
    outcome: PermissionOutcome,
) -> Result<(), crate::agent::RpcError> {
    // Main manager first; a miss (no entry) routes to the subagent manager
    // (the subagent's own listener's map — the `session_id` is the
    // subagent's id). The existing "silent no-op when gone" semantics
    // stay: a miss on both managers is a no-op (success either way) — but
    // it is LOGGED (a double-miss is a routing/id mismatch, and a silent
    // success would make it invisible). `||` short-circuits: the subagent
    // lookup runs only when the main manager missed.
    let main_hit = state
        .respond_permission(&session_id, &request_id, outcome.clone())
        .await?;
    let sub_hit = main_hit
        || subagent_state
            .respond_permission(&session_id, &request_id, outcome)
            .await;
    if !sub_hit {
        eprintln!("respond_permission: no pending entry for session {session_id} request {request_id} (main and subagent managers)");
    }
    Ok(())
}

/// Deliver the user's answer to a pending bridge request to the agent.
///
/// `request_id` is the bridge request's `id` (the one carried in the
/// `bridge-request` event). `result` is the response `result` `Value`
/// VERBATIM (no wrapper — for `ask`, the `AskResponsePayload`; for
/// `confirm`, `{confirmed}`; for `password`, `{password}`); the desktop
/// writes `{v:1, type:"response", id, result}`. If the request is no longer
/// pending, this is a no-op.
#[tauri::command]
pub async fn respond_bridge_request(
    state: State<'_, Arc<SessionManager>>,
    subagent_state: State<'_, Arc<SubagentSessionManager>>,
    session_id: String,
    request_id: String,
    result: Value,
) -> Result<(), crate::agent::RpcError> {
    // Main manager first; a miss (no entry) routes to the subagent manager
    // (the subagent's own listener's map — the `session_id` is the
    // subagent's id). The existing "silent no-op when gone" semantics
    // stay: a miss on both managers is a no-op (success either way) — but
    // it is LOGGED (a double-miss is a routing/id mismatch, and a silent
    // success would make it invisible). `||` short-circuits: the subagent
    // lookup runs only when the main manager missed.
    let main_hit = state
        .respond_bridge_request(&session_id, &request_id, result.clone())
        .await?;
    let sub_hit = main_hit
        || subagent_state
            .respond_bridge_request(&session_id, &request_id, result)
            .await;
    if !sub_hit {
        eprintln!("respond_bridge_request: no pending entry for session {session_id} request {request_id} (main and subagent managers)");
    }
    Ok(())
}

/// Resume a stored session: spawn a fresh agent for `agent_id` with
/// `--session <stored piSessionFile>` (the stored session's pi session
/// file), `get_state` it, and replay the stored transcript from
/// `get_messages`.
///
/// Returns [`crate::agent::RpcError::NotResumable`] when the stored
/// session's capabilities say it cannot be resumed (no `piSessionFile`,
/// or a legacy row with `loadSession: false`) — the UI then shows the
/// history-only banner instead.
#[tauri::command]
pub async fn resume_session(
    sink: State<'_, Arc<dyn EventSink>>,
    state: State<'_, Arc<SessionManager>>,
    db: State<'_, Arc<Db>>,
    agent_id: String,
    session_id: String,
    cwd: String,
) -> Result<SessionInfo, crate::agent::RpcError> {
    // Honest resume semantics, checked BEFORE spawning: if the stored
    // session's capabilities lack a `piSessionFile`, there is no point
    // launching the agent. (The manager's `resume_session` re-checks —
    // this pre-check exists so the command fails fast with the right
    // error kind.)
    if let Ok(Some(row)) = db.session(&session_id) {
        let caps: Value = serde_json::from_str(&row.capabilities_json).unwrap_or(Value::Null);
        if !caps
            .get("loadSession")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return Err(crate::agent::RpcError::NotResumable {
                id: session_id.clone(),
            });
        }
    }
    state
        .resume_session(&agent_id, &session_id, PathBuf::from(cwd), sink.inner())
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
) -> Result<Vec<Value>, crate::agent::RpcError> {
    state
        .set_config_option(&session_id, &config_id, &value, sink.inner())
        .await
}

/// Cancel the session's in-flight prompt turn (the user pressed Esc).
/// The agent resolves the open `prompt` with an `abort` (the `prompt`
/// response arrives and `agent_settled` fires), which completes the
/// in-flight `send_prompt` with `Cancelled` and unlocks the composer.
#[tauri::command]
pub async fn cancel_session(
    state: State<'_, Arc<SessionManager>>,
    session_id: String,
) -> Result<(), crate::agent::RpcError> {
    state.cancel_session(&session_id).await
}
