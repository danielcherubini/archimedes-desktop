//! The ACP session layer: spawns an agent process, speaks ACP to it, and
//! drives the session lifecycle.
//!
//! The heart of the design is the **closure-lifecycle mechanism**. The SDK's
//! `connect_with` closure *is* the connection's lifetime: "the connection
//! stays active until `main_fn` returns, then shuts down." So we never await
//! `connect_with` inline in [`SessionManager::start_session`] (it would only
//! resolve when the session closes). Instead we spawn the connection as a
//! *driver task* that owns the closure for the whole session. `close_session`
//! (or a subagent cancel) records the close kind and
//! flips a `watch` flag that makes the closure return, which drops the
//! connection and — on Unix — terminates the agent's process group;
//!
//! The same driver is used for `session/new` (start) and `session/load`
//! (resume): only the *establisher* — the future that turns a fresh
//! connection into an established session — differs.
//!
//! Multiple live sessions COEXIST (the one-live cap is lifted, ADR 0002):
//! `start_session` / `resume_session` do NOT close other live sessions; a
//! session is torn down only by an explicit `close_session`, a subagent
//! cancel, or the agent process exiting on its own.

use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use agent_client_protocol::Error as ProtocolError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{oneshot, watch, Mutex};

use agent_client_protocol::schema::v1::{
    AgentCapabilities, ClientCapabilities, ContentBlock, FileSystemCapabilities, InitializeRequest,
    NewSessionRequest, PromptRequest, ReadTextFileRequest, ReadTextFileResponse,
    RequestPermissionRequest, RequestPermissionResponse, SessionConfigId, SessionConfigOption,
    SessionId, SessionNotification, SessionUpdate, SetSessionConfigOptionRequest, StopReason,
    TextContent, WriteTextFileRequest, WriteTextFileResponse,
};
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::{
    on_receive_notification, on_receive_request, AcpAgent, AcpAgentConfig, Agent, Client,
    ConnectionTo, Responder,
};

use crate::acp::bridge::{self, PendingBridge};
use crate::acp::errors::AcpError;
use crate::acp::fs_backend::FsBackend;
use crate::acp::permission::{self, PendingPermissions};
use crate::config::{AgentEntry, ConfigError, Registry};
use crate::storage::Db;

/// Sink for outbound events (session updates, session-closed, …).
///
/// The Tauri wiring implements this with `AppHandle::emit`; tests implement
/// it with a channel so events are assertable.
pub trait EventSink: Send + Sync {
    fn emit(&self, event: &str, payload: Value);
}

/// Why a session ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClosedReason {
    /// The user (or the app) closed the session.
    User,
    /// The agent process exited on its own.
    AgentExited,
    /// The connection failed for an unexpected reason.
    Error,
}

impl ClosedReason {
    /// The string form emitted in the `session-closed` event payload.
    pub fn as_str(self) -> &'static str {
        match self {
            ClosedReason::User => "user",
            ClosedReason::AgentExited => "agent-exited",
            ClosedReason::Error => "error",
        }
    }
}

/// How a live session was (or was about to be) closed. `None` at
/// teardown time means the agent process exited on its own.
///
/// `pub(crate)`: the `ExternalClose` handle (the subagent cancel path) and
/// `SubagentCancel` (subagent.rs) carry a `CloseKind` across the module
/// boundary, so it must be visible to the whole crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CloseKind {
    /// An explicit user close (`close_session` or a subagent cancel).
    User,
}

/// A fully established session, ready to accept prompts.
///
/// `Serialize` so it can cross the IPC boundary as a command return value.
///
/// NOTE: this type must never share a module with
/// `agent_client_protocol::schema::v1::SessionInfo`; the SDK type is always
/// referenced by full path.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub session_id: SessionId,
    pub agent_id: String,
    pub cwd: PathBuf,
    pub capabilities: AgentCapabilities,
    /// The agent's session configuration options (model / thinking level
    /// selectors) from the `newSession` / `loadSession` response; `None`
    /// when the agent does not advertise any (or for stored sessions —
    /// `list_sessions` always reports `None`).
    pub config_options: Option<Vec<SessionConfigOption>>,
}

/// A live, in-memory session handle.
///
/// `session_id`, `cwd`, and `agent_id` are carried for diagnostics and for
/// resume; they are not read by the prompt path.
///
/// `pub(crate)` + `pub(crate)` fields: `subagent.rs` reads `driver.sessions`
/// entries (to clone the `cx` for the subagent's task prompt), so the
/// struct and its fields are visible to the whole crate.
#[derive(Debug)]
#[allow(dead_code)]
pub(crate) struct LiveSession {
    /// Cheap clone of the connection, shared with the driver task.
    pub(crate) cx: ConnectionTo<Agent>,
    pub(crate) session_id: SessionId,
    pub(crate) cwd: PathBuf,
    pub(crate) agent_id: String,
    /// Set to `true` to make the driver task's closure return, tearing down
    /// the connection (and the agent's process group on Unix).
    ///
    /// For a subagent session this IS the `ExternalClose`'s sender (a cancel
    /// flips it); for a main session it is the driver's internal flag.
    pub(crate) close_tx: watch::Sender<bool>,
    /// The close kind, decided by `close_session` (first-set-wins) and read
    /// by the driver task once `connect_with` returns. For a subagent session
    /// this IS the `ExternalClose`'s kind.
    pub(crate) close_kind: Arc<StdMutex<Option<CloseKind>>>,
}

/// The external-close handle: the subagent cancel path (main sessions pass
/// `None` to `drive_session`).
///
/// The dispatch worker task owns the `tx` + `kind` (it is handed to the
/// caller as a [`SubagentCancel`]); the driver task keeps the `rx` and
/// SELECTS on it in BOTH the establish phase and the block-until-close
/// phase (so a cancel during the 30 s establish window is honored, not
/// deferred), and reads the `kind` (INSTEAD of its own internal kind) for
/// the close reason (one kind, first-set-wins across the whole session).
/// The `tx` is also handed to the subagent's bridge listener, so a cancel
/// cancels the subagent's in-flight `ask` waiters via their `close_rx` arm.
#[derive(Clone)]
pub(crate) struct ExternalClose {
    /// The close flag sender (the `SubagentCancel` flips it; the bridge
    /// listener observes it; the driver task selects on its receiver).
    pub(crate) tx: watch::Sender<bool>,
    /// The close flag receiver the driver task selects on.
    pub(crate) rx: watch::Receiver<bool>,
    /// The close kind (first-set-wins): the `SubagentCancel` sets it `User`
    /// before flipping the flag; the driver task reads it for the reason.
    pub(crate) kind: Arc<StdMutex<Option<CloseKind>>>,
}

/// A cheap, `'static`-safe handle to the subagent dispatch (the `Arc` is
/// NOT a `&` — `ConnCtx` is moved into `tokio::spawn` and must be `'static`).
///
/// The parent session's bridge listener carries one so it can service
/// `dispatch_subagent` frames: `manager` is the `SubagentSessionManager`,
/// `parent_cwd` is the parent's Space folder (the subagent's cwd + fs
/// sandbox root), `parent_agent_id` is the parent's registry agent id (the
/// subagent spawns the SAME registry entry as the parent — the built-in
/// `pi` entry in production).
#[derive(Clone)]
pub struct SubagentSpawn {
    pub manager: Arc<crate::acp::subagent::SubagentSessionManager>,
    pub parent_cwd: PathBuf,
    pub parent_agent_id: String,
}

/// Accumulated `cost_update` usage (the subagent metrics source). Sums the
/// optional numeric fields across `cost_update` payloads (per-turn deltas
/// from the suite's self-usage emitter — Task 1 of this plan); an absent
/// field contributes 0. `Default` = all zeros (a session that never pushed
/// usage — the pre-Task-1 v1 state).
#[derive(Debug, Clone, Default)]
pub struct CostAccumulator {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub cost: f64,
}

impl CostAccumulator {
    /// Fold one `cost_update` payload (the wire shape: `inputTokens` /
    /// `outputTokens` / `cacheReadTokens` / `cacheWriteTokens` / `cost`,
    /// all optional) into the accumulator (absent → 0).
    pub fn add_payload(&mut self, p: &Value) {
        self.input_tokens += p.get("inputTokens").and_then(Value::as_u64).unwrap_or(0);
        self.output_tokens += p.get("outputTokens").and_then(Value::as_u64).unwrap_or(0);
        self.cache_read_tokens += p
            .get("cacheReadTokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        self.cache_write_tokens += p
            .get("cacheWriteTokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        self.cost += p.get("cost").and_then(Value::as_f64).unwrap_or(0.0);
    }
}

/// The shared session-driver state. `SessionManager` (main sessions) and
/// `SubagentSessionManager` (worker runtime) each own one.
///
/// Holds the live-session map, the pending-request maps, the establish
/// timeout, and the per-manager policy knobs (persistence, the in-memory
/// text/cost captures, and the subagent dispatch handle). `drive_session`
/// (the shared driver) is a method on this struct.
pub struct SessionDriver {
    pub(crate) sessions: Arc<Mutex<HashMap<SessionId, LiveSession>>>,
    pub(crate) pending_permissions: PendingPermissions,
    pub(crate) pending_bridge: PendingBridge,
    /// How long the establishment phase (agent spawn + `initialize` +
    /// `session/new` or `session/load`) may run before it is cancelled.
    /// Default: 30 s.
    pub(crate) establish_timeout: Duration,
    /// Persistence (main only; `None` for subagents — ephemeral, not stored).
    pub(crate) db: Option<Arc<Db>>,
    /// In-memory per-`messageId` agent-text accumulator for the FINAL
    /// OUTPUT (subagents only; `None` for main — main persists to the DB).
    pub(crate) text_capture: Option<Arc<StdMutex<HashMap<String, String>>>>,
    /// The last-seen `messageId` (updated for EVERY `agent_message_chunk`,
    /// alongside `text_capture`): a plain `HashMap` has no insertion order,
    /// so the "last message" is tracked separately, not derived from
    /// iteration order.
    pub(crate) last_message_id: Option<Arc<StdMutex<Option<String>>>>,
    /// Accumulated `cost_update` usage (subagents only; `None` for main).
    pub(crate) cost_capture: Option<Arc<StdMutex<CostAccumulator>>>,
    /// The subagent dispatch handle (main manager only — `Some`); `None`
    /// for the subagent manager itself (subagents cannot dispatch
    /// subagents — the tool is excluded from their spawn).
    pub(crate) subagent: Option<Arc<crate::acp::subagent::SubagentSessionManager>>,
}

impl SessionDriver {
    /// Create a driver (a fresh sessions map, empty pending maps, a 30 s
    /// establish timeout, no persistence / captures / subagent handle).
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            pending_permissions: Arc::new(Mutex::new(HashMap::new())),
            pending_bridge: Arc::new(Mutex::new(HashMap::new())),
            establish_timeout: Duration::from_secs(30),
            db: None,
            text_capture: None,
            last_message_id: None,
            cost_capture: None,
            subagent: None,
        }
    }

    /// Shared driver: spawn the agent, register the client-side backends,
    /// run the *establisher* (initialize + `session/new` or `session/load`),
    /// then block until the session closes.
    ///
    /// `establish` receives the fresh connection and must return the
    /// established `(session_id, SessionInfo)`. On failure it must return
    /// `Err` — the error is mapped to an [`AcpError`] (a
    /// "does not support session/load" marker becomes
    /// [`AcpError::NotResumable`]).
    ///
    /// `external_close` (subagents only; `None` for main) is the cancel
    /// path: the driver task selects on its receiver in BOTH the establish
    /// phase and the block-until-close phase, and reads its `kind` (instead
    /// of the internal kind) for the close reason (one kind, first-set-wins
    /// across the whole session).
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn drive_session<F, Fut>(
        &self,
        agent: AcpAgent,
        agent_id: &str,
        hint: String,
        cwd: PathBuf,
        sink: &Arc<dyn EventSink>,
        bridge_setup: Option<(String, PathBuf)>,
        external_close: Option<ExternalClose>,
        establish: F,
    ) -> Result<SessionInfo, AcpError>
    where
        F: FnOnce(ConnectionTo<Agent>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<(SessionId, SessionInfo), ProtocolError>> + Send + 'static,
    {
        // Split the external close (the subagent cancel path): the driver
        // task selects on `rx` (establish + block phases) and reads `kind`
        // for the reason; the bridge listener observes `tx` (a cancel also
        // cancels in-flight `ask` waiters). `None` for main sessions.
        let (ec_tx, ec_rx, ec_kind) = match external_close {
            Some(ec) => (Some(ec.tx), Some(ec.rx), Some(ec.kind)),
            None => (None, None, None),
        };

        // Channels that carry values out of the (long-lived) closure.
        let (ready_tx, ready_rx) = oneshot::channel::<ConnectionTo<Agent>>();
        let (session_ready_tx, session_ready_rx) = oneshot::channel::<SessionInfo>();
        let (session_id_tx, session_id_rx) = oneshot::channel::<SessionId>();
        let (error_tx, mut error_rx) = oneshot::channel::<ProtocolError>();
        let (close_tx, close_rx) = watch::channel(false);
        // The internal close kind (main sessions). The driver reads the
        // external kind INSTEAD when `external_close` is present (one kind,
        // first-set-wins across the whole session). `None` means the agent
        // process exited on its own (the close flag alone cannot carry the
        // reason: on a user close the agent process may notice the EOF and
        // exit first, so the reason must be decided by whoever closed).
        let internal_kind: Arc<StdMutex<Option<CloseKind>>> = Arc::new(StdMutex::new(None));
        let kind: Arc<StdMutex<Option<CloseKind>>> =
            ec_kind.clone().unwrap_or_else(|| internal_kind.clone());
        // A clone for the driver task (held by the task until AFTER its kind
        // read below); the original moves into the `LiveSession` value.
        let kind_for_task = kind.clone();
        // The close flag the bridge listener observes: the external close's
        // sender (subagents — a cancel cancels in-flight `ask` waiters),
        // else the driver's internal flag (main sessions).
        let listener_close_tx: &watch::Sender<bool> = ec_tx.as_ref().unwrap_or(&close_tx);

        // Bridge listener (ADR 0003): started BEFORE the driver task spawns
        // (the push retry window is only ~2 s). The anchor is the desktop's
        // own pid (`std::process::id()`, always alive). The driver-task
        // `close_tx` is passed so a session close cancels every in-flight
        // bridge request. `None` for non-bridge agents / macOS (fail-closed).
        let bridge_handle = match bridge_setup {
            Some((client_session_id, socket_path)) => {
                // The subagent dispatch handle (main only — `Some`): the
                // main session's listener services `dispatch_subagent`
                // frames. Built from the driver's `subagent` handle + the
                // session's `cwd` + `agent_id` (all in scope).
                let subagent_spawn = self.subagent.clone().map(|m| SubagentSpawn {
                    manager: m,
                    parent_cwd: cwd.clone(),
                    parent_agent_id: agent_id.to_string(),
                });
                let cost_capture = self.cost_capture.clone();
                Some(
                    bridge::start_listener(
                        client_session_id,
                        &socket_path,
                        std::process::id(),
                        sink.clone(),
                        self.pending_bridge.clone(),
                        listener_close_tx,
                        bridge::DEFAULT_BRIDGE_TIMEOUT,
                        subagent_spawn,
                        cost_capture,
                    )
                    .await
                    .map_err(|e| AcpError::SpawnFailed {
                        hint: format!("bridge listener: {e}"),
                    })?,
                )
            }
            None => None,
        };

        // Per-session transcript accumulators for the persistence hook.
        let agent_text_acc: Arc<StdMutex<HashMap<String, String>>> =
            Arc::new(StdMutex::new(HashMap::new()));
        let tool_call_state: Arc<StdMutex<HashMap<String, Value>>> =
            Arc::new(StdMutex::new(HashMap::new()));

        let sessions_arc = self.sessions.clone();
        let pending_permissions_arc = self.pending_permissions.clone();
        let pending_bridge_arc = self.pending_bridge.clone();
        let establish_timeout = self.establish_timeout;
        let db = self.db.clone();
        // The capture hooks (subagents only; `None` for main). Clones for
        // the `connect_with` notification handler.
        let text_capture = self.text_capture.clone();
        let last_message_id = self.last_message_id.clone();
        let sink = sink.clone();
        let notify_sink = sink.clone();
        let cwd_owned = cwd.clone();
        // The external close's receiver for the driver task (cloned per
        // select phase — a `None` receiver is inert).
        let ec_rx_task = ec_rx;

        // SPAWN the connection as a driver task. `connect_with` only resolves
        // when the closure returns (i.e. at session close), so it must never
        // be awaited inline here.
        tokio::spawn(async move {
            // Client-side backends for this session.
            let fs_backend = FsBackend {
                root: cwd_owned.clone(),
            };

            // Cheap clones so each handler closure can own its copy.
            let fs_read = fs_backend.clone();
            let fs_write = fs_backend.clone();
            let perm_sink = sink.clone();
            let perm_pp = pending_permissions_arc.clone();
            // A clone for the `connect_with` closure (to call `set_session_id`
            // once the ACP id is known); the original `bridge_handle` stays
            // here for the unconditional `teardown` in the cleanup below.
            let bridge_handle_cx = bridge_handle.clone();

            let builder = Client
                .builder()
                .name("archimedes-desktop")
                .on_receive_notification(
                    async move |notif: SessionNotification, _cx: ConnectionTo<Agent>| {
                        let payload = serde_json::to_value(&notif.update).unwrap_or(Value::Null);
                        let frame = serde_json::json!({
                            "sessionId": notif.session_id.to_string(),
                            "update": payload,
                        });
                        notify_sink.emit("session-update", frame);

                        // The client owns history: upsert the transcript row
                        // as the update streams in.
                        if let Some(db) = &db {
                            persist_update(
                                db,
                                &notif.session_id.to_string(),
                                &notif.update,
                                &agent_text_acc,
                                &tool_call_state,
                            );
                        }
                        // (a) Capture the agent text for the FINAL OUTPUT
                        // (subagents only; `None` for main — main persists
                        // to the DB). A plain `HashMap` has no insertion
                        // order, so the "last message" is tracked separately
                        // (`last_message_id`), not derived from iteration.
                        // The capture locks are TOLERANT of a poisoned
                        // mutex (`into_inner` — a poisoned capture degrades
                        // to its last good state, not a panic: a panic here
                        // would chain into the driver task, and on a
                        // subagent dispatch into a dropped oneshot — a
                        // crash silently reported as a "cancelled" dispatch).
                        if let Some(tc) = &text_capture {
                            if let SessionUpdate::AgentMessageChunk(chunk) = &notif.update {
                                if let ContentBlock::Text(text) = &chunk.content {
                                    if !text.text.is_empty() {
                                        let key = chunk
                                            .message_id
                                            .as_ref()
                                            .map(|m| m.to_string())
                                            .unwrap_or_else(|| "default".to_string());
                                        let mut acc = tc.lock().unwrap_or_else(|p| p.into_inner());
                                        acc.entry(key.clone()).or_default().push_str(&text.text);
                                        if let Some(lmi) = &last_message_id {
                                            *lmi.lock().unwrap_or_else(|p| p.into_inner()) =
                                                Some(key);
                                        }
                                    }
                                }
                            }
                        }
                        Ok(())
                    },
                    on_receive_notification!(),
                )
                .on_receive_request(
                    async move |req: ReadTextFileRequest,
                                responder: Responder<ReadTextFileResponse>,
                                _cx: ConnectionTo<Agent>| {
                        match fs_read.read(&req.path) {
                            Ok(content) => {
                                responder.respond(ReadTextFileResponse::new(content))?;
                            }
                            Err(e) => {
                                responder.respond_with_internal_error(e.to_string())?;
                            }
                        }
                        Ok(())
                    },
                    on_receive_request!(),
                )
                .on_receive_request(
                    async move |req: WriteTextFileRequest,
                                responder: Responder<WriteTextFileResponse>,
                                _cx: ConnectionTo<Agent>| {
                        match fs_write.write(&req.path, &req.content) {
                            Ok(()) => {
                                responder.respond(WriteTextFileResponse::new())?;
                            }
                            Err(e) => {
                                responder.respond_with_internal_error(e.to_string())?;
                            }
                        }
                        Ok(())
                    },
                    on_receive_request!(),
                )
                .on_receive_request(
                    async move |req: RequestPermissionRequest,
                                responder: Responder<RequestPermissionResponse>,
                                cx: ConnectionTo<Agent>| {
                        permission::handle_permission_request(
                            &req, responder, &cx, &perm_sink, &perm_pp,
                        )
                        .await;
                        Ok(())
                    },
                    on_receive_request!(),
                );

            let _ = builder
                .connect_with(agent, |cx: ConnectionTo<Agent>| async move {
                    // Hand the connection to the manager so it can send prompts.
                    let cx2 = cx.clone();
                    ready_tx.send(cx2).ok();

                    // Establish the session (initialize + session/new for a
                    // new session, initialize + session/load for a resume).
                    // Bounded + external-close aware: a cancel during the
                    // establish window is honored (not deferred to the
                    // timeout).
                    let timeout_detail = format!(
                        "agent did not answer initialize within {}s",
                        establish_timeout.as_secs()
                    );
                    let mut establish_rx = ec_rx_task.clone();
                    let established = tokio::select! {
                        r = tokio::time::timeout(establish_timeout, establish(cx.clone())) => {
                            match r {
                                Ok(Ok(e)) => Some(e),
                                Ok(Err(err)) => {
                                    // Report the failure to the awaiting
                                    // command, then tear the connection down.
                                    error_tx.send(err).ok();
                                    None
                                }
                                Err(_) => {
                                    // The detail text doubles as the mapping
                                    // marker in `map_establish_error`.
                                    let timeout_err =
                                        agent_client_protocol::util::internal_error(&timeout_detail);
                                    error_tx.send(timeout_err).ok();
                                    None
                                }
                            }
                        }
                        // A cancel during the establish window is honored
                        // (not deferred to the timeout); a `None` receiver
                        // is inert (main sessions).
                        _ = changed_or_inert(&mut establish_rx) => None,
                    };
                    let (session_id, info) = match established {
                        Some(e) => e,
                        None => return Ok(()),
                    };

                    session_id_tx.send(session_id.clone()).ok();
                    session_ready_tx.send(info).ok();

                    // Hand the ACP `session_id` to the bridge handle so
                    // `bridge-request`/`bridge-event` payloads carry the ACP
                    // id (bridge requests only occur mid-turn, after
                    // establish, so they always carry the ACP id).
                    if let Some(h) = &bridge_handle_cx {
                        h.set_session_id(&session_id.to_string()).await;
                    }

                    // BLOCK until close_session OR agent death OR the
                    // external close. A clean incoming EOF does NOT cancel
                    // main_fn, so select on all three.
                    let mut block_rx = ec_rx_task.clone();
                    // The internal close flag is inert for SUBAGENTS (the
                    // external close is their cancel path — the internal
                    // sender is dropped, and a dropped sender's `changed()`
                    // resolves immediately, which would tear the session
                    // down right after establishment).
                    let mut internal_rx = ec_rx_task.is_none().then_some(close_rx);
                    tokio::select! {
                        // The close kind was set by the closer before the
                        // flag send and read by the task after this returns;
                        // the closure itself does not decide the reason.
                        _ = changed_or_inert(&mut internal_rx) => {}
                        // Agent exited; the kind stays whatever the closer
                        // (if any) already set — `None` means the agent
                        // process exited on its own.
                        _ = cx.incoming_closed() => {}
                        // The external close (subagent cancel); a `None`
                        // receiver is inert (main sessions).
                        _ = changed_or_inert(&mut block_rx) => {}
                    }
                    Ok(())
                })
                .await;

            // Connection returned (closed, agent died, or error): clean up.
            // The reason comes from the close kind the closer recorded: a
            // kind set before the flag send wins; `None` means the agent
            // process exited on its own and nobody closed it.
            let kind = *kind_for_task.lock().unwrap_or_else(|p| p.into_inner());
            let reason = match kind {
                Some(CloseKind::User) => ClosedReason::User,
                None => ClosedReason::AgentExited,
            };
            if let Ok(session_id) = session_id_rx.await {
                sessions_arc.lock().await.remove(&session_id);
                // Keys are `"{session_id}/{request_id}"` — match on the
                // trailing-slash prefix so closing "s1" does not cancel
                // the pending prompt of the longer session "s10".
                let prefix = permission::session_key_prefix(&session_id.to_string());
                pending_permissions_arc
                    .lock()
                    .await
                    .retain(|key, _| !key.starts_with(&prefix));
                // Drain this session's pending bridge requests too (dropping
                // the senders cancels the spawned waiters, which write the
                // terminal `error:"cancelled"` frame). The `session_id` is
                // only known here, so this drain is nested in the guard.
                let bridge_prefix = bridge::session_key_prefix(&session_id.to_string());
                pending_bridge_arc
                    .lock()
                    .await
                    .retain(|key, _| !key.starts_with(&bridge_prefix));
                sink.emit(
                    "session-closed",
                    serde_json::json!({
                        "sessionId": session_id.to_string(),
                        "reason": reason.as_str(),
                    }),
                );
            }
            // Tear the bridge listener down UNCONDITIONALLY (do NOT nest it
            // inside the `session_id` guard, or a failed `connect_with` /
            // `session/new` would leak the listener): stop the accept loop +
            // unlink the socket (Unix; Windows pipes vanish on last close).
            bridge::teardown(bridge_handle);
        });

        // Await the connection, then the established session.
        let cx = ready_rx
            .await
            .map_err(|_| AcpError::SpawnFailed { hint: hint.clone() })?;
        let info = match session_ready_rx.await {
            Ok(info) => info,
            Err(_) => {
                // The closure never delivered an established session: either
                // the establisher failed (it sent the error first) or the
                // connection died mid-establish.
                match error_rx.try_recv() {
                    Ok(err) => return Err(map_establish_error(agent_id, err)),
                    Err(_) => {
                        return Err(AcpError::InitializeFailed {
                            detail: "agent did not complete initialize/session-new".to_string(),
                        })
                    }
                }
            }
        };

        let live = LiveSession {
            cx,
            session_id: info.session_id.clone(),
            cwd,
            agent_id: agent_id.to_string(),
            close_tx: ec_tx.unwrap_or_else(|| close_tx.clone()),
            close_kind: kind,
        };
        self.sessions
            .lock()
            .await
            .insert(info.session_id.clone(), live);

        Ok(info)
    }
}

/// A future that resolves when the (optional) close receiver's value
/// changes; inert (never resolves) when `None` — so a session with no such
/// receiver is unaffected by the select arm. Used for BOTH the internal-close
/// arm and the external-close arm:
///
/// - the EXTERNAL arm is `None` for a MAIN session (no external close — the
///   `None` receiver is inert, so the select arm never fires);
/// - the INTERNAL arm is `None` for a SUBAGENT session (its internal sender
///   is dropped — the `LiveSession` holds the external one — and a dropped
///   sender's `changed()` resolves immediately, which would tear the session
///   down right after establishment).
async fn changed_or_inert(rx: &mut Option<watch::Receiver<bool>>) {
    match rx {
        Some(rx) => {
            let _ = rx.changed().await;
        }
        None => {
            std::future::pending::<()>().await;
        }
    }
}

/// Manages all live ACP sessions (the MAIN sessions).
///
/// Owns a [`SessionDriver`] (db: attached via [`Self::attach_db`],
/// captures: `None`, subagent: injected via [`Self::set_subagent_manager`])
/// plus the agent registry + config dir. DB recording and
/// `record_session` stay here; the shared driver
/// (`drive_session`) is delegated to. (The one-live policy is LIFTED,
/// ADR 0002 — sessions coexist; a session is torn down only by an
/// explicit `close_session`, a subagent cancel, or agent death.)
///
/// `Sync` — the mutable state is `Arc<Mutex<…>>` internally, so the
/// manager is managed directly (no outer lock); each method locks only
/// its own internal maps, briefly.
pub struct SessionManager {
    driver: SessionDriver,
    registry: Registry,
    config_dir: PathBuf,
}

impl SessionManager {
    /// Create a manager, loading the agent registry from `config_dir`.
    pub fn new(config_dir: PathBuf) -> Result<Self, ConfigError> {
        let registry = Registry::load(&config_dir)?;
        Ok(Self {
            driver: SessionDriver::new(),
            registry,
            config_dir,
        })
    }

    /// Attach the persistence database. Persistence is a no-op without it.
    pub fn attach_db(&mut self, db: Arc<Db>) {
        self.driver.db = Some(db);
    }

    /// Override the establishment timeout (default 30 s; tests shrink it
    /// so a hanging agent does not make them wait).
    pub fn set_establish_timeout(&mut self, timeout: Duration) {
        self.driver.establish_timeout = timeout;
    }

    /// Inject the subagent manager (main only — sets the driver's
    /// `subagent` handle so the main session's bridge listener can service
    /// `dispatch_subagent` frames). The subagent manager needs nothing from
    /// the main manager; only this field points at it (intra-crate type
    /// cycles are fine in Rust).
    pub fn set_subagent_manager(&mut self, m: Arc<crate::acp::subagent::SubagentSessionManager>) {
        self.driver.subagent = Some(m);
    }

    /// The configured config directory (useful for tests and diagnostics).
    pub fn config_dir(&self) -> &PathBuf {
        &self.config_dir
    }

    /// Clone the (cheap) connection handle of a live session.
    ///
    /// Commands that must NOT hold the manager lock across an await (e.g.
    /// `send_prompt`, whose turn can span a user-paced permission prompt)
    /// use this to grab the connection, drop the lock, and then drive the
    /// request.
    pub async fn connection(&self, session_id: &str) -> Result<ConnectionTo<Agent>, AcpError> {
        let sid = SessionId::new(session_id);
        self.driver
            .sessions
            .lock()
            .await
            .get(&sid)
            .map(|live| live.cx.clone())
            .ok_or_else(|| AcpError::UnknownSession {
                session_id: session_id.to_string(),
            })
    }

    /// Number of live sessions.
    pub async fn session_count(&self) -> usize {
        self.driver.sessions.lock().await.len()
    }

    /// The configured agents (consumed by the `list_agents` command).
    pub fn agents(&self) -> &[AgentEntry] {
        &self.registry.agents
    }

    /// Record a session in the persistence layer (no-op without a database).
    fn record_session(&self, info: &SessionInfo) {
        if let Some(db) = &self.driver.db {
            let _ = db.record_session(info);
            // A start/resume updates or creates the space row (and `resume`
            // re-touches `last_opened_at`): a space is born/touched when a
            // conversation starts or resumes in it.
            let _ = db.upsert_space(&info.cwd.display().to_string());
        }
    }

    /// Spawn an agent, initialize it, create a session, and register it.
    ///
    /// Returns the [`SessionInfo`] once the session is established. The
    /// connection is driven by a background task that lives for the session's
    /// lifetime; it is torn down by [`Self::close_session`] or agent death.
    pub async fn start_session(
        &self,
        agent_id: &str,
        cwd: PathBuf,
        sink: &Arc<dyn EventSink>,
    ) -> Result<SessionInfo, AcpError> {
        // Canonicalize BEFORE the registry lookup and before the space row is
        // touched: the spaces join key is the canonicalized cwd, so a
        // `~/x` / symlink spelling must not produce a different row.
        let cwd = std::fs::canonicalize(&cwd).map_err(|_| AcpError::FolderMissing {
            path: cwd.display().to_string(),
        })?;

        let entry = self
            .registry
            .get(agent_id)
            .ok_or_else(|| AcpError::UnknownAgent {
                agent_id: agent_id.to_string(),
            })?;

        // Bridge wiring (ADR 0003): for a bridge agent (on a platform where
        // the bridge is available — NOT macOS), set the 4 bridge env vars
        // and pass the (client session id, socket path) to the driver so it
        // starts the peer-verified listener before the spawn returns. For a
        // NEW session the client session id is a fresh UUID (the ACP
        // `session_id` is agent-generated and does not exist yet).
        let client_session_id = uuid::Uuid::new_v4().to_string();
        let (agent_env, bridge_setup) = match bridge_spawn_setup(entry, &client_session_id) {
            Some((env, sid, socket_path)) => (env, Some((sid, socket_path))),
            None => (entry.env.clone(), None),
        };

        let agent = AcpAgent::new(
            AcpAgentConfig::new(entry.command.clone())
                .args(entry.args.clone())
                .envs(agent_env),
        );
        let hint = spawn_hint(&entry.command);

        let agent_id_owned = agent_id.to_string();
        let cwd_owned = cwd.clone();

        let info = self
            .driver
            .drive_session(
                agent,
                agent_id,
                hint,
                cwd,
                sink,
                bridge_setup,
                None,
                move |cx| async move {
                    let init = cx
                        .send_request(
                            InitializeRequest::new(ProtocolVersion::V1).client_capabilities(
                                ClientCapabilities::default()
                                    .fs(FileSystemCapabilities::default()
                                        .read_text_file(true)
                                        .write_text_file(true))
                                    .terminal(false),
                            ),
                        )
                        .block_task()
                        .await?;

                    let new_session = cx
                        .send_request(NewSessionRequest::new(cwd_owned.clone()))
                        .block_task()
                        .await?;

                    Ok((
                        new_session.session_id.clone(),
                        SessionInfo {
                            session_id: new_session.session_id.clone(),
                            agent_id: agent_id_owned,
                            cwd: cwd_owned,
                            capabilities: init.agent_capabilities,
                            config_options: new_session.config_options.clone(),
                        },
                    ))
                },
            )
            .await?;

        self.record_session(&info);
        Ok(info)
    }

    /// Resume a stored session: spawn a fresh agent for `agent_id`,
    /// initialize it (same client capabilities as a new session), then
    /// `session/load` the given session id — the two-argument form
    /// (`session_id` + `cwd`) — and consume the returned
    /// `RestoreSessionBuilder` exactly like the session builder
    /// (`.block_task().start_session().await`).
    ///
    /// The driver-task lifecycle is shared verbatim with
    /// [`Self::start_session`] (same [`LiveSession`] storage, same
    /// `select!` on close / agent death, same cleanup and `session-closed`
    /// emit); only the `NewSessionRequest` is replaced by `session/load`.
    ///
    /// If the agent does not advertise `agent_capabilities.load_session`,
    /// this returns [`AcpError::NotResumable`] and the caller should fall
    /// back to history-only viewing.
    pub async fn resume_session(
        &self,
        agent_id: &str,
        session_id: &str,
        cwd: PathBuf,
        sink: &Arc<dyn EventSink>,
    ) -> Result<SessionInfo, AcpError> {
        // Canonicalize BEFORE the registry lookup (same rationale as
        // `start_session`): everything downstream (the `session/load` cwd,
        // `SessionInfo.cwd`, the space join key) uses the canonical path.
        let cwd = std::fs::canonicalize(&cwd).map_err(|_| AcpError::FolderMissing {
            path: cwd.display().to_string(),
        })?;

        let entry = self
            .registry
            .get(agent_id)
            .ok_or_else(|| AcpError::UnknownAgent {
                agent_id: agent_id.to_string(),
            })?;

        // Bridge wiring (ADR 0003): same as `start_session`, but the client
        // session id is the STORED `session_id` (a resume re-uses it, so the
        // agent's `session` push echoes the same id the desktop set).
        let (agent_env, bridge_setup) = match bridge_spawn_setup(entry, session_id) {
            Some((env, sid, socket_path)) => (env, Some((sid, socket_path))),
            None => (entry.env.clone(), None),
        };

        let agent = AcpAgent::new(
            AcpAgentConfig::new(entry.command.clone())
                .args(entry.args.clone())
                .envs(agent_env),
        );
        let hint = spawn_hint(&entry.command);

        let sid = SessionId::new(session_id);
        let agent_id_owned = agent_id.to_string();
        let cwd_owned = cwd.clone();
        let db = self.driver.db.clone();

        let info = self
            .driver
            .drive_session(
                agent,
                agent_id,
                hint,
                cwd,
                sink,
                bridge_setup,
                None,
                move |cx| async move {
                    let init = cx
                        .send_request(
                            InitializeRequest::new(ProtocolVersion::V1).client_capabilities(
                                ClientCapabilities::default()
                                    .fs(FileSystemCapabilities::default()
                                        .read_text_file(true)
                                        .write_text_file(true))
                                    .terminal(false),
                            ),
                        )
                        .block_task()
                        .await?;

                    if !init.agent_capabilities.load_session {
                        // Honest resume semantics: an agent that cannot load a
                        // session must not pretend to. The UI shows the
                        // history-only banner instead.
                        return Err(agent_client_protocol::util::internal_error(
                            "agent does not support session/load",
                        ));
                    }

                    // The restored transcript is replaced by the agent's replay,
                    // which doubles as the authoritative history: clear the
                    // stored rows BEFORE `session/load` so a replay reusing a
                    // known `messageId` overwrites (rather than clobbers) and a
                    // replay under a new id does not duplicate the stored text.
                    if let Some(db) = &db {
                        let _ = db.clear_messages_for(&sid.to_string());
                    }

                    let restored = cx
                        .load_session(sid.clone(), cwd_owned.as_path())
                        .block_task()
                        .start_session()
                        .await?;

                    Ok((
                        sid.clone(),
                        SessionInfo {
                            session_id: sid,
                            agent_id: agent_id_owned,
                            cwd: cwd_owned,
                            capabilities: init.agent_capabilities,
                            config_options: restored.response().config_options.clone(),
                        },
                    ))
                },
            )
            .await?;

        self.record_session(&info);
        Ok(info)
    }

    /// Send a prompt to a live session and wait for the turn to finish.
    ///
    /// Returns the [`StopReason`] the agent reported (the frontend needs the
    /// turn-completion signal).
    pub async fn send_prompt(
        &self,
        session_id: &str,
        text: String,
    ) -> Result<StopReason, AcpError> {
        let sid = SessionId::new(session_id);
        // Clone just the (cheap) connection handle, not the whole LiveSession.
        let cx = {
            let sessions = self.driver.sessions.lock().await;
            sessions
                .get(&sid)
                .map(|live| live.cx.clone())
                .ok_or_else(|| AcpError::UnknownSession {
                    session_id: session_id.to_string(),
                })?
        };

        // Record the user's message in the transcript (the client owns
        // history) before the turn begins.
        if let Some(db) = &self.driver.db {
            let payload = serde_json::json!({ "text": text });
            let _ = db.record_message(session_id, "user", None, &payload.to_string());
        }

        let request = PromptRequest::new(sid, vec![ContentBlock::Text(TextContent::new(text))]);
        let response =
            cx.send_request(request)
                .block_task()
                .await
                .map_err(|err| AcpError::Protocol {
                    message: err.message,
                })?;
        Ok(response.stop_reason)
    }

    /// Set a session config option (e.g. the model) on a live session.
    ///
    /// Clones the (cheap) connection handle, drops the lock, sends
    /// `session/set_config_option`, and returns the agent's updated
    /// `configOptions` (the agent also emits a `config_option_update`
    /// notification — the two paths converge to the same state).
    pub async fn set_config_option(
        &self,
        session_id: &str,
        config_id: &str,
        value: &str,
    ) -> Result<Vec<SessionConfigOption>, AcpError> {
        let sid = SessionId::new(session_id);
        let cx = {
            let sessions = self.driver.sessions.lock().await;
            sessions
                .get(&sid)
                .map(|live| live.cx.clone())
                .ok_or_else(|| AcpError::UnknownSession {
                    session_id: session_id.to_string(),
                })?
        };
        // `SessionConfigId` has `From` ONLY for `Arc<str>` / `String` /
        // `&'static str` — a borrowed `&str` does NOT convert; wrap it.
        let request =
            SetSessionConfigOptionRequest::new(sid, SessionConfigId::new(config_id), value);
        let response =
            cx.send_request(request)
                .block_task()
                .await
                .map_err(|err| AcpError::Protocol {
                    message: err.message,
                })?;
        Ok(response.config_options)
    }

    /// Deliver the user's answer to a pending permission request.
    ///
    /// Looks up the oneshot sender by the compound key
    /// `"{session_id}/{request_id}"` and sends the outcome through it. If the
    /// entry is gone (the session closed, or the prompt already resolved), this
    /// is a silent no-op. Returns `true` when an entry was resolved (the caller
    /// can then route a miss to the subagent manager).
    pub async fn respond_permission(
        &self,
        session_id: &str,
        request_id: &str,
        outcome: permission::PermissionOutcome,
    ) -> Result<bool, AcpError> {
        let key = permission::permission_key(session_id, request_id);
        let sender = self.driver.pending_permissions.lock().await.remove(&key);
        // Best-effort: if the receiver is already gone the prompt was
        // already resolved (timeout / session close), so there is nothing to
        // do.
        match sender {
            Some(sender) => {
                let _ = sender.send(outcome);
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// Deliver the user's answer to a pending bridge request.
    ///
    /// Looks up the oneshot sender by the compound key
    /// `"{session_id}/{request_id}"` and sends the `result` `Value` verbatim
    /// (no wrapper — for `password`, `{password}`; for `confirm`,
    /// `{confirmed}`; for `ask`, the `AskResponsePayload`). If the entry is
    /// gone (the session closed, or the request already resolved), this is a
    /// silent no-op. Returns `true` when an entry was resolved (the caller
    /// can then route a miss to the subagent manager).
    pub async fn respond_bridge_request(
        &self,
        session_id: &str,
        request_id: &str,
        result: serde_json::Value,
    ) -> Result<bool, AcpError> {
        let key = bridge::bridge_key(session_id, request_id);
        let sender = self.driver.pending_bridge.lock().await.remove(&key);
        // Best-effort: if the receiver is already gone the request was
        // already resolved (timeout / session close), so there is nothing to
        // do.
        match sender {
            Some(sender) => {
                let _ = sender.send(result);
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// Close a live session.
    ///
    /// `async` because it must lock the sessions map to find the session.
    /// Records the `User` close kind (first-set-wins) and sends the close
    /// flag; the driver task performs the map removal and the
    /// `session-closed` emit.
    pub async fn close_session(&self, session_id: &str) -> Result<(), AcpError> {
        let sid = SessionId::new(session_id);
        // Clone just the close flag's sender and the shared close kind (both
        // cheaply cloneable).
        let (close_tx, close_kind) = {
            let sessions = self.driver.sessions.lock().await;
            sessions
                .get(&sid)
                .map(|live| (live.close_tx.clone(), live.close_kind.clone()))
                .ok_or_else(|| AcpError::UnknownSession {
                    session_id: session_id.to_string(),
                })?
        };
        // Decide the kind BEFORE starting the close. First-set-wins: a kind
        // already present means the close is in progress (another setter won
        // the race) or the reason is already decided. The kind mutex is never
        // held by anyone who also holds the close flag's future, so the set
        // and the send can run safely in this order.
        if let Ok(mut kind) = close_kind.lock() {
            if kind.is_none() {
                *kind = Some(CloseKind::User);
            }
        }
        close_tx.send(true).map_err(|_| AcpError::Protocol {
            message: "session already closed".to_string(),
        })?;
        Ok(())
    }
}

/// Map an establisher failure to an [`AcpError`]. The
/// "does not support session/load" marker becomes [`AcpError::NotResumable`];
/// the establishment-timeout marker becomes [`AcpError::InitializeFailed`].
fn map_establish_error(agent_id: &str, err: ProtocolError) -> AcpError {
    let data = err.data.as_ref().and_then(Value::as_str);
    if data == Some("agent does not support session/load") {
        return AcpError::NotResumable {
            agent_id: agent_id.to_string(),
        };
    }
    if data.is_some_and(|d| d.starts_with("agent did not answer initialize within")) {
        return AcpError::InitializeFailed {
            detail: data.unwrap().to_string(),
        };
    }
    AcpError::Protocol {
        message: err.message,
    }
}

/// Persist one session update into the transcript (see the module docs for
/// the upsert semantics).
fn persist_update(
    db: &Db,
    session_id: &str,
    update: &SessionUpdate,
    agent_text_acc: &StdMutex<HashMap<String, String>>,
    tool_call_state: &StdMutex<HashMap<String, Value>>,
) {
    match update {
        SessionUpdate::AgentMessageChunk(chunk) => {
            if let ContentBlock::Text(text) = &chunk.content {
                if text.text.is_empty() {
                    return;
                }
                let key = chunk
                    .message_id
                    .as_ref()
                    .map(|m| m.to_string())
                    .unwrap_or_else(|| "default".to_string());
                let mut acc = agent_text_acc
                    .lock()
                    .expect("agent-text accumulator poisoned");
                let entry = acc.entry(key.clone()).or_default();
                entry.push_str(&text.text);
                let payload = serde_json::json!({ "text": entry });
                let _ =
                    db.record_message(session_id, "agent-text", Some(&key), &payload.to_string());
            }
        }
        SessionUpdate::ToolCall(tool_call) => {
            let key = tool_call.tool_call_id.to_string();
            let mut state = tool_call_state.lock().expect("tool-call state poisoned");
            state.insert(
                key.clone(),
                serde_json::to_value(tool_call).unwrap_or(Value::Null),
            );
            let _ = db.record_message(
                session_id,
                "tool-call",
                Some(&key),
                &state[&key].to_string(),
            );
        }
        SessionUpdate::ToolCallUpdate(tool_call_update) => {
            let key = tool_call_update.tool_call_id.to_string();
            let mut state = tool_call_state.lock().expect("tool-call state poisoned");
            let patch = serde_json::to_value(tool_call_update).unwrap_or(Value::Null);
            let entry = state
                .entry(key.clone())
                .or_insert_with(|| Value::Object(Default::default()));
            merge_json(entry, &patch);
            let _ = db.record_message(session_id, "tool-call", Some(&key), &entry.to_string());
        }
        _ => {}
    }
}

/// Shallow-merge `patch` into `base`: non-null fields of `patch` win.
fn merge_json(base: &mut Value, patch: &Value) {
    if let (Some(base), Some(patch)) = (base.as_object_mut(), patch.as_object()) {
        for (k, v) in patch {
            if !v.is_null() {
                base.insert(k.clone(), v.clone());
            }
        }
    }
}

/// Build a remediation hint for a spawn failure, mentioning the pi / pi-acp
/// install path.
fn spawn_hint(command: &str) -> String {
    format!(
        "could not spawn '{}'. If this is the 'pi' agent, make sure `pi` and the \
         `pi-acp` adapter are installed and on PATH (e.g. `npm install -g pi-acp`), \
         then retry.",
        command
    )
}

/// A per-spawn randomized bridge socket path (ADR 0003).
///
/// On Linux: a `bridge-<uuid>.sock` under a `0700` dir named
/// `archimedes-bridge-<uid>` — under `XDG_RUNTIME_DIR` when set (a
/// per-user, `0700` dir the system manages), else the temp dir.
/// **Fail-closed:** the dir is verified to be owned by the current uid
/// (and chmodded `0700`) BEFORE the socket is bound into it — a dir
/// pre-created by ANOTHER user (a pre-squat of the guessable name in a
/// shared temp dir) makes the bridge unavailable for this spawn
/// (`None`) rather than the desktop binding its socket inside an
/// attacker-owned dir. The uid in the dir name is a hint, not a
/// guarantee — the ownership check is the gate. A symlinked dir path is
/// also rejected (chmod/uid checks follow symlinks and would validate
/// the target instead). On Windows: the bare
/// name `bridge-<uuid>` (the listener prefixes `\\.\\pipe\\`; the dir is
/// per-user already). On macOS the bridge is unavailable, so this
/// returns a placeholder that is never used (`bridge::available()` is
/// `false`).
pub(crate) fn bridge_socket_path() -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::MetadataExt;
        use std::path::Path;
        let uid = unsafe { libc::getuid() };
        // Prefer `XDG_RUNTIME_DIR` (per-user, `0700`, managed by the
        // system) over the shared temp dir when it is set. A relative
        // `XDG_RUNTIME_DIR` is treated as UNSET (XDG spec) — using it as
        // is would create the dir relative to the desktop's CWD.
        let base = std::env::var("XDG_RUNTIME_DIR")
            .ok()
            .filter(|p| !p.is_empty())
            .filter(|p| Path::new(p).is_absolute())
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let dir = base.join(format!("archimedes-bridge-{uid}"));
        // Fail closed on any setup error: the bridge is a per-spawn
        // convenience — a failed socket dir must not abort the spawn.
        if std::fs::create_dir_all(&dir).is_err() {
            return None;
        }
        // `set_permissions` / `metadata` FOLLOW symlinks: a local attacker
        // who can write the base dir can pre-create `dir` as a symlink to
        // a victim-owned directory — the chmod + uid check would then pass
        // against the TARGET (chmodding an attacker-chosen victim-owned
        // dir to 0700, a local DoS) and the socket would bind inside an
        // attacker-chosen location. Reject a symlinked path before
        // trusting the dir (fail-closed).
        if std::fs::symlink_metadata(&dir)
            .ok()
            .is_some_and(|m| m.file_type().is_symlink())
        {
            return None;
        }
        if std::fs::set_permissions(&dir, std::os::unix::fs::PermissionsExt::from_mode(0o700))
            .is_err()
        {
            // EPERM: the dir pre-exists owned by ANOTHER user (a
            // pre-squat) — do not bind a socket inside a foreign dir.
            return None;
        }
        // Defense in depth: even after the chmod, verify the dir is
        // actually owned by us (the real gate against a foreign dir).
        let meta = std::fs::metadata(&dir).ok()?;
        if meta.uid() != uid {
            return None;
        }
        Some(dir.join(format!("bridge-{}.sock", uuid::Uuid::new_v4())))
    }
    #[cfg(windows)]
    {
        Some(PathBuf::from(format!("bridge-{}", uuid::Uuid::new_v4())))
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        // macOS (bridge unavailable): a placeholder, never used (the
        // listener is not started — `bridge::available()` is `false`).
        Some(std::env::temp_dir().join(format!("bridge-{}.sock", uuid::Uuid::new_v4())))
    }
}

/// Build the 4 bridge env vars (ADR 0003) and the `(client session id,
/// socket path)` the driver uses to start the listener. Returns `None` when
/// the agent is not a bridge agent OR the bridge is unavailable on this
/// platform (macOS — fail-closed).
pub(crate) fn bridge_spawn_setup(
    entry: &AgentEntry,
    session_id: &str,
) -> Option<(BTreeMap<String, String>, String, PathBuf)> {
    if !entry.bridge || !bridge::available() {
        return None;
    }
    // Fail-closed: the socket dir could not be set up safely (e.g.
    // pre-squatted by another user) — spawn WITHOUT the bridge.
    let socket_path = bridge_socket_path()?;
    let mut env = entry.env.clone();
    env.insert("PI_ARCHIMEDES_BRIDGE".to_string(), "1".to_string());
    env.insert(
        "PI_ARCHIMEDES_BRIDGE_SESSION".to_string(),
        session_id.to_string(),
    );
    env.insert(
        "PI_ARCHIMEDES_BRIDGE_SERVER_PID".to_string(),
        std::process::id().to_string(),
    );
    env.insert(
        "PI_ARCHIMEDES_BRIDGE_SOCKET".to_string(),
        socket_path.to_string_lossy().to_string(),
    );
    Some((env, session_id.to_string(), socket_path))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::CostAccumulator;

    /// Two payloads with one ABSENT field each → the sums (an absent field
    /// contributes 0; payload 1 has NO `cacheReadTokens` / `cacheWriteTokens`).
    #[test]
    fn add_payload_sums_fields_across_payloads() {
        let mut acc = CostAccumulator::default();
        acc.add_payload(
            &json!({ "source": "main", "inputTokens": 100, "outputTokens": 50, "cost": 0.001 }),
        );
        acc.add_payload(
            &json!({ "source": "main", "inputTokens": 200, "outputTokens": 25, "cacheReadTokens": 10, "cost": 0.002 }),
        );
        assert_eq!(acc.input_tokens, 300, "inputTokens should SUM (100 + 200)");
        assert_eq!(acc.output_tokens, 75, "outputTokens should SUM (50 + 25)");
        assert_eq!(acc.cache_read_tokens, 10, "the absent field contributes 0");
        assert_eq!(acc.cache_write_tokens, 0, "the absent field contributes 0");
        assert!(
            (acc.cost - 0.003).abs() < 1e-9,
            "cost should SUM (0.001 + 0.002 = 0.003), got {}",
            acc.cost
        );
    }

    /// An all-absent payload (only `source`) → NO change to the accumulator.
    #[test]
    fn add_payload_all_absent_is_a_no_op() {
        let mut acc = CostAccumulator::default();
        acc.add_payload(&json!({ "source": "main" }));
        assert_eq!(acc.input_tokens, 0);
        assert_eq!(acc.output_tokens, 0);
        assert_eq!(acc.cache_read_tokens, 0);
        assert_eq!(acc.cache_write_tokens, 0);
        assert_eq!(acc.cost, 0.0);
    }
}

#[cfg(test)]
mod session_tests {
    use super::*;
    use agent_client_protocol::schema::v1::{
        SessionConfigKind, SessionConfigOptionCategory, SessionConfigSelectOptions,
    };
    use std::path::Path;

    use tokio::sync::mpsc;

    pub struct TestSink {
        tx: mpsc::UnboundedSender<Value>,
    }

    impl EventSink for TestSink {
        fn emit(&self, event: &str, payload: Value) {
            let _ = self
                .tx
                .send(serde_json::json!({ "event": event, "payload": payload }));
        }
    }

    pub fn temp_config_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir(&dir).unwrap();
        dir
    }

    pub fn unique_fake_agent(dir: &Path) -> PathBuf {
        let fake = dir.join("fake_agent");
        let bin = PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/target/debug/fake_agent"
        ));
        std::fs::copy(&bin, &fake).expect("failed to copy fake_agent");
        fake
    }

    pub fn write_agents_json_cmd(cmd: &Path, dir: &Path, mode: Option<&str>) {
        let agents = serde_json::json!({
            "agents": [{
                "id": "fake",
                "name": "Fake Agent",
                "command": cmd,
                "args": mode.map(|m| vec![m]).unwrap_or_default(),
                "env": {}
            }]
        });
        std::fs::write(dir.join("agents.json"), agents.to_string()).unwrap();
    }

    async fn start_with_retry<F, Fut>(mut attempt_fn: F) -> Result<SessionInfo, AcpError>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = Result<SessionInfo, AcpError>>,
    {
        let mut last_err = None;
        for _ in 0..3 {
            match attempt_fn().await {
                Ok(info) => return Ok(info),
                Err(AcpError::SpawnFailed { .. }) => {
                    last_err = Some(AcpError::SpawnFailed {
                        hint: "spawn failed".to_string(),
                    });
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    continue;
                }
                Err(e) => return Err(e),
            }
        }
        Err(last_err.unwrap_or_else(|| AcpError::Protocol {
            message: "unreachable".to_string(),
        }))
    }

    #[tokio::test]
    async fn start_session_returns_config_options_from_new_session_response() {
        let dir = temp_config_dir();
        let cmd = unique_fake_agent(&dir);
        write_agents_json_cmd(&cmd, &dir, None);
        let manager = SessionManager::new(dir.clone()).unwrap();
        let (tx, _rx) = mpsc::unbounded_channel();
        let sink: Arc<dyn EventSink> = Arc::new(TestSink { tx });

        let info = start_with_retry(|| manager.start_session("fake", dir.clone(), &sink))
            .await
            .unwrap();

        assert!(info.config_options.is_some());
        let opts = info.config_options.unwrap();
        assert_eq!(opts.len(), 2);

        let model = opts
            .iter()
            .find(|o| o.category == Some(SessionConfigOptionCategory::Model))
            .unwrap();
        match &model.kind {
            SessionConfigKind::Select(s) => {
                assert_eq!(s.current_value.to_string(), "acme/alpha");
                match &s.options {
                    SessionConfigSelectOptions::Ungrouped(o) => assert_eq!(o.len(), 3),
                    _ => panic!("expected ungrouped"),
                }
            }
            _ => panic!("expected select"),
        }

        let thought = opts
            .iter()
            .find(|o| o.category == Some(SessionConfigOptionCategory::ThoughtLevel))
            .unwrap();
        match &thought.kind {
            SessionConfigKind::Select(s) => {
                assert_eq!(s.current_value.to_string(), "medium");
                match &s.options {
                    SessionConfigSelectOptions::Ungrouped(o) => assert_eq!(o.len(), 6),
                    _ => panic!("expected ungrouped"),
                }
            }
            _ => panic!("expected select"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn resume_session_returns_config_options_from_load_session_response() {
        let dir = temp_config_dir();
        let cmd = unique_fake_agent(&dir);
        write_agents_json_cmd(&cmd, &dir, Some("resume"));
        let manager = SessionManager::new(dir.clone()).unwrap();
        let (tx, _rx) = mpsc::unbounded_channel();
        let sink: Arc<dyn EventSink> = Arc::new(TestSink { tx });

        let info = start_with_retry(|| {
            manager.resume_session("fake", "fake-session-1", dir.clone(), &sink)
        })
        .await
        .unwrap();

        assert!(info.config_options.is_some());
        let opts = info.config_options.unwrap();
        assert_eq!(opts.len(), 2);

        let model = opts
            .iter()
            .find(|o| o.category == Some(SessionConfigOptionCategory::Model))
            .unwrap();
        match &model.kind {
            SessionConfigKind::Select(s) => {
                assert_eq!(s.current_value.to_string(), "acme/alpha");
                match &s.options {
                    SessionConfigSelectOptions::Ungrouped(o) => assert_eq!(o.len(), 3),
                    _ => panic!("expected ungrouped"),
                }
            }
            _ => panic!("expected select"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// (set) `set_config_option` round-trips: the request reaches the agent,
    /// the response's updated `configOptions` come back (model current value
    /// moved to `acme/beta`, the thinking entry unchanged), AND the agent's
    /// `config_option_update` notification arrives as a `session-update`
    /// event with the same updated options.
    #[tokio::test]
    async fn set_config_option_round_trips_and_notifies() {
        let dir = temp_config_dir();
        let cmd = unique_fake_agent(&dir);
        write_agents_json_cmd(&cmd, &dir, None);
        let manager = SessionManager::new(dir.clone()).unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let sink: Arc<dyn EventSink> = Arc::new(TestSink { tx });

        let info = start_with_retry(|| manager.start_session("fake", dir.clone(), &sink))
            .await
            .unwrap();

        let updated = manager
            .set_config_option(&info.session_id.to_string(), "model", "acme/beta")
            .await
            .unwrap();

        assert_eq!(updated.len(), 2);
        let model = updated
            .iter()
            .find(|o| o.category == Some(SessionConfigOptionCategory::Model))
            .unwrap();
        match &model.kind {
            SessionConfigKind::Select(s) => {
                assert_eq!(s.current_value.to_string(), "acme/beta");
            }
            _ => panic!("expected select"),
        }

        // Wait for the notification
        let mut found = false;
        for _ in 0..10 {
            if let Ok(msg) = tokio::time::timeout(Duration::from_millis(500), rx.recv()).await {
                let msg = msg.unwrap();
                if msg["event"] == "session-update" {
                    let payload = &msg["payload"]["update"];
                    if payload["sessionUpdate"] == "config_option_update" {
                        found = true;
                        assert_eq!(
                            payload["configOptions"],
                            serde_json::to_value(&updated).unwrap()
                        );
                        break;
                    }
                }
            }
        }
        assert!(found, "notification not received");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// (error) `set_config_option_error` mode: the agent's rejection maps to
    /// `AcpError::Protocol`.
    #[tokio::test]
    async fn set_config_option_rejection_maps_to_protocol_error() {
        let dir = temp_config_dir();
        let cmd = unique_fake_agent(&dir);
        write_agents_json_cmd(&cmd, &dir, Some("set_config_option_error"));
        let manager = SessionManager::new(dir.clone()).unwrap();
        let (tx, _rx) = mpsc::unbounded_channel();
        let sink: Arc<dyn EventSink> = Arc::new(TestSink { tx });

        let info = start_with_retry(|| manager.start_session("fake", dir.clone(), &sink))
            .await
            .unwrap();

        let result = manager
            .set_config_option(&info.session_id.to_string(), "model", "acme/beta")
            .await;

        assert!(matches!(result, Err(AcpError::Protocol { .. })));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// (unknown) `set_config_option` on an unknown session id maps to
    /// `AcpError::UnknownSession`.
    #[tokio::test]
    async fn set_config_option_unknown_session() {
        let dir = temp_config_dir();
        let manager = SessionManager::new(dir.clone()).unwrap();

        let result = manager
            .set_config_option("nope", "model", "acme/beta")
            .await;

        assert!(matches!(result, Err(AcpError::UnknownSession { .. })));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
