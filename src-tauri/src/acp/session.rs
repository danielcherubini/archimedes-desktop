//! The ACP session layer: spawns an agent process, speaks ACP to it, and
//! drives the session lifecycle.
//!
//! The heart of the design is the **closure-lifecycle mechanism**. The SDK's
//! `connect_with` closure *is* the connection's lifetime: "the connection
//! stays active until `main_fn` returns, then shuts down." So we never await
//! `connect_with` inline in [`SessionManager::start_session`] (it would only
//! resolve when the session closes). Instead we spawn the connection as a
//! *driver task* that owns the closure for the whole session. `close_session`
//! flips a `watch` flag that makes the closure return, which drops the
//! connection and — on Unix — terminates the agent's process group.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{oneshot, watch, Mutex};

use agent_client_protocol::schema::v1::{
    AgentCapabilities, ClientCapabilities, ContentBlock, FileSystemCapabilities, InitializeRequest,
    NewSessionRequest, PromptRequest, RequestPermissionOutcome, RequestPermissionRequest,
    RequestPermissionResponse, SessionId, SessionNotification, StopReason, TextContent,
};
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::{
    on_receive_notification, on_receive_request, AcpAgent, AcpAgentConfig, Agent, Client,
    ConnectionTo, Responder,
};

use crate::acp::errors::AcpError;
use crate::config::{ConfigError, Registry};

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
}

/// A live, in-memory session handle.
///
/// `session_id`, `cwd`, and `agent_id` are carried for diagnostics and for
/// Task 3 (permission bridge / resume); they are not read in Task 2.
#[derive(Debug)]
#[allow(dead_code)]
struct LiveSession {
    /// Cheap clone of the connection, shared with the driver task.
    cx: ConnectionTo<Agent>,
    session_id: SessionId,
    cwd: PathBuf,
    agent_id: String,
    /// Set to `true` to make the driver task's closure return, tearing down
    /// the connection (and the agent's process group on Unix).
    close_tx: watch::Sender<bool>,
}

/// Manages all live ACP sessions.
///
/// Stored in Tauri state as `tokio::sync::Mutex<SessionManager>` (Tauri wraps
/// managed state in an `Arc` internally). Commands take it as
/// `State<'_, tokio::sync::Mutex<SessionManager>>`.
pub struct SessionManager {
    /// Shared with driver tasks (the `Arc` is cloned into each `tokio::spawn`).
    sessions: Arc<Mutex<HashMap<SessionId, LiveSession>>>,
    /// Pending permission requests, keyed by session id. Always empty in
    /// Task 2 (permissions are auto-cancelled); Task 3 populates it.
    pending_permissions: Arc<Mutex<HashMap<String, oneshot::Sender<()>>>>,
    registry: Registry,
    config_dir: PathBuf,
}

impl SessionManager {
    /// Create a manager, loading the agent registry from `config_dir`.
    pub fn new(config_dir: PathBuf) -> Result<Self, ConfigError> {
        let registry = Registry::load(&config_dir)?;
        Ok(Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            pending_permissions: Arc::new(Mutex::new(HashMap::new())),
            registry,
            config_dir,
        })
    }

    /// The configured config directory (useful for tests and diagnostics).
    pub fn config_dir(&self) -> &PathBuf {
        &self.config_dir
    }

    /// Number of live sessions.
    pub async fn session_count(&self) -> usize {
        self.sessions.lock().await.len()
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
        let entry = self
            .registry
            .get(agent_id)
            .ok_or_else(|| AcpError::UnknownAgent(agent_id.to_string()))?;

        let agent = AcpAgent::new(
            AcpAgentConfig::new(entry.command.clone())
                .args(entry.args.clone())
                .envs(entry.env.clone()),
        );
        let hint = spawn_hint(&entry.command);

        // Channels that carry values out of the (long-lived) closure.
        let (ready_tx, ready_rx) = oneshot::channel::<ConnectionTo<Agent>>();
        let (session_ready_tx, session_ready_rx) = oneshot::channel::<SessionInfo>();
        let (session_id_tx, session_id_rx) = oneshot::channel::<SessionId>();
        let (close_tx, mut close_rx) = watch::channel(false);
        // Set by the closure when the user closes the session. The driver task
        // reads it after `connect_with` returns to decide the close reason.
        // (We can't rely on the closure to report the reason: on agent death
        // the SDK's background actor fails and drops the closure first.)
        let user_closed = Arc::new(AtomicBool::new(false));
        let user_closed_for_closure = user_closed.clone();

        let sessions_arc = self.sessions.clone();
        let pending_permissions_arc = self.pending_permissions.clone();
        let sink = sink.clone();
        let notify_sink = sink.clone();
        let agent_id_owned = agent_id.to_string();
        let cwd_owned = cwd.clone();

        // SPAWN the connection as a driver task. `connect_with` only resolves
        // when the closure returns (i.e. at session close), so it must never
        // be awaited inline here.
        tokio::spawn(async move {
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
                        Ok(())
                    },
                    on_receive_notification!(),
                )
                .on_receive_request(
                    async move |_req: RequestPermissionRequest,
                                responder: Responder<RequestPermissionResponse>,
                                _cx: ConnectionTo<Agent>| {
                        // Task 2: auto-respond Cancelled. Task 3 replaces this
                        // with the real permission bridge.
                        responder.respond(RequestPermissionResponse::new(
                            RequestPermissionOutcome::Cancelled,
                        ))?;
                        Ok(())
                    },
                    on_receive_request!(),
                );

            let _ = builder
                .connect_with(agent, |cx: ConnectionTo<Agent>| async move {
                    // Hand the connection to the manager so it can send prompts.
                    let cx2 = cx.clone();
                    ready_tx.send(cx2).ok();

                    let init = cx
                        .send_request(
                            InitializeRequest::new(ProtocolVersion::V1).client_capabilities(
                                ClientCapabilities::default()
                                    .fs(FileSystemCapabilities::default()
                                        .read_text_file(true)
                                        .write_text_file(true))
                                    .terminal(true),
                            ),
                        )
                        .block_task()
                        .await?;

                    let new_session = cx
                        .send_request(NewSessionRequest::new(cwd_owned.clone()))
                        .block_task()
                        .await?;

                    session_id_tx.send(new_session.session_id.clone()).ok();
                    session_ready_tx
                        .send(SessionInfo {
                            session_id: new_session.session_id.clone(),
                            agent_id: agent_id_owned.clone(),
                            cwd: cwd_owned.clone(),
                            capabilities: init.agent_capabilities,
                        })
                        .ok();

                    // BLOCK until close_session OR agent death. A clean
                    // incoming EOF does NOT cancel main_fn, so select on both.
                    tokio::select! {
                        _ = close_rx.changed() => {
                            user_closed_for_closure.store(true, Ordering::SeqCst);
                        }
                        _ = cx.incoming_closed() => {
                            // Agent exited; leave `user_closed` false.
                        }
                    }
                    Ok(())
                })
                .await;

            // Connection returned (closed, agent died, or error): clean up.
            // The reason is derived from whether the user explicitly closed:
            // the closure sets the flag on a user close; on agent death the
            // closure is dropped before it can set it.
            let reason = if user_closed.load(Ordering::SeqCst) {
                ClosedReason::User
            } else {
                ClosedReason::AgentExited
            };
            if let Ok(session_id) = session_id_rx.await {
                sessions_arc.lock().await.remove(&session_id);
                pending_permissions_arc
                    .lock()
                    .await
                    .retain(|key, _| !key.starts_with(&session_id.to_string()));
                sink.emit(
                    "session-closed",
                    serde_json::json!({
                        "sessionId": session_id.to_string(),
                        "reason": reason.as_str(),
                    }),
                );
            }
        });

        // Await the connection, then the established session. The session id
        // only exists after `session/new`, so the store happens after both.
        let cx = ready_rx
            .await
            .map_err(|_| AcpError::SpawnFailed { hint: hint.clone() })?;
        let info = session_ready_rx
            .await
            .map_err(|_| AcpError::InitializeFailed {
                detail: "agent did not complete initialize/session-new".to_string(),
            })?;

        let live = LiveSession {
            cx,
            session_id: info.session_id.clone(),
            cwd,
            agent_id: agent_id.to_string(),
            close_tx,
        };
        self.sessions
            .lock()
            .await
            .insert(info.session_id.clone(), live);

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
            let sessions = self.sessions.lock().await;
            sessions
                .get(&sid)
                .map(|live| live.cx.clone())
                .ok_or_else(|| AcpError::UnknownSession(session_id.to_string()))?
        };

        let request = PromptRequest::new(sid, vec![ContentBlock::Text(TextContent::new(text))]);
        let response = cx
            .send_request(request)
            .block_task()
            .await
            .map_err(|err| AcpError::Protocol(err.message))?;
        Ok(response.stop_reason)
    }

    /// Close a live session.
    ///
    /// `async` because it must lock the sessions map to find the session.
    /// Sets the close flag; the driver task performs the map removal and the
    /// `session-closed` emit.
    pub async fn close_session(&self, session_id: &str) -> Result<(), AcpError> {
        let sid = SessionId::new(session_id);
        // Clone just the close flag's sender (it is cheaply cloneable).
        let close_tx = {
            let sessions = self.sessions.lock().await;
            sessions
                .get(&sid)
                .map(|live| live.close_tx.clone())
                .ok_or_else(|| AcpError::UnknownSession(session_id.to_string()))?
        };
        close_tx
            .send(true)
            .map_err(|_| AcpError::Protocol("session already closed".to_string()))?;
        Ok(())
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
