//! The permission bridge: turns the agent's `session/request_permission`
//! request into a UI prompt, and delivers the user's answer back to the agent.
//!
//! The SDK runs every handler callback on a single event-loop task, and while a
//! handler is running no new messages are processed. A permission prompt is
//! *user-paced* — the user might take minutes to answer — so the handler must
//! **not** await it. Instead the handler:
//!
//!   1. emits a `permission-request` Tauri event (the UI shows the prompt),
//!   2. registers a oneshot sender in the manager's `pending_permissions` map
//!      (keyed by `"{session_id}/{request_id}"`), and
//!   3. `cx.spawn`s a task that owns the responder + oneshot receiver, awaits
//!      the answer (bounded by a 300 s timeout), and responds to the agent.
//!
//! The handler then returns immediately.
//!
//! The compound key is what lets the driver-task cleanup (Task 2) drain every
//! entry for a closing session: dropping the senders makes the spawned tasks
//! observe a `Canceled` oneshot and cancel promptly instead of waiting out the
//! full 300 s timeout.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use agent_client_protocol::schema::v1::{
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SelectedPermissionOutcome,
};
use agent_client_protocol::{Agent, ConnectionTo, Responder};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::{oneshot, Mutex};

use crate::acp::session::EventSink;

/// The user's decision on a permission prompt, as chosen via the
/// `respond_permission` Tauri command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionOutcome {
    /// The user selected one of the offered options; `option_id` is its id.
    Selected { option_id: String },
    /// The user dismissed the prompt (or it timed out / the session closed).
    Cancelled,
}

/// The manager's map of pending permission senders.
///
/// Keyed by `"{session_id}/{request_id}"` so the driver-task cleanup can drain
/// all entries for a closing session at once (dropping the senders cancels the
/// spawned tasks).
pub type PendingPermissions = Arc<Mutex<HashMap<String, oneshot::Sender<PermissionOutcome>>>>;

/// Build the compound key for a pending permission.
pub fn permission_key(session_id: &str, request_id: &str) -> String {
    format!("{session_id}/{request_id}")
}

/// How long a permission prompt stays open before it auto-cancels.
const PERMISSION_TIMEOUT: Duration = Duration::from_secs(300);

/// Handle an incoming `session/request_permission` without blocking the event
/// loop. See the module docs for the full flow.
pub async fn handle_permission_request(
    req: &RequestPermissionRequest,
    responder: Responder<RequestPermissionResponse>,
    cx: &ConnectionTo<Agent>,
    sink: &Arc<dyn EventSink>,
    pending_permissions: &PendingPermissions,
) {
    let request_id = responder.id().to_string();
    let session_id = req.session_id.to_string();
    let key = permission_key(&session_id, &request_id);

    // (a) Tell the UI to show the prompt.
    let payload = json!({
        "sessionId": session_id,
        "requestId": request_id,
        "request": serde_json::to_value(req).unwrap_or(Value::Null),
    });
    sink.emit("permission-request", payload);

    // (b) Register the oneshot the user's answer will flow through.
    let (tx, rx) = oneshot::channel();
    {
        let mut map = pending_permissions.lock().await;
        map.insert(key.clone(), tx);
    }

    // (c) Spawn the waiter. It owns the responder and the receiver.
    let key_owned = key.clone();
    let pp = pending_permissions.clone();

    let spawn_result = cx.spawn(async move {
        // Await the user's answer, a timeout, or a Canceled oneshot (the
        // session closed before the user answered).
        let outcome = match tokio::time::timeout(PERMISSION_TIMEOUT, rx).await {
            Ok(Ok(outcome)) => outcome,
            _ => PermissionOutcome::Cancelled,
        };

        // Map to the ACP response.
        let response = match outcome {
            PermissionOutcome::Selected { option_id } => RequestPermissionResponse::new(
                RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(option_id)),
            ),
            PermissionOutcome::Cancelled => {
                RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled)
            }
        };

        // Remove the entry (best-effort; it may already have been drained by a
        // session close).
        {
            let mut map = pp.lock().await;
            map.remove(&key_owned);
        }

        // Respond exactly once, best-effort. A spawned task must return
        // Ok(()) on every path: returning Err would tear down the whole
        // connection. Dropping the responder instead would leave the agent's
        // request hanging forever.
        let _ = responder.respond(response);
        Ok(())
    });

    if let Err(err) = spawn_result {
        // Spawning failed (the connection is already shutting down): the
        // responder was consumed by the spawn call, so we can no longer
        // respond. Clean up the map entry; the agent's request will be
        // dropped along with the connection.
        {
            let mut map = pending_permissions.lock().await;
            map.remove(&key);
        }
        eprintln!("archimedes: failed to spawn permission waiter: {err}");
    }
}
