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
//!
//! The same driver is used for `session/new` (start) and `session/load`
//! (resume): only the *establisher* — the future that turns a fresh
//! connection into an established session — differs.

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use agent_client_protocol::Error as ProtocolError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{oneshot, watch, Mutex};

use agent_client_protocol::schema::v1::{
    AgentCapabilities, ClientCapabilities, ContentBlock, FileSystemCapabilities, InitializeRequest,
    NewSessionRequest, PromptRequest, ReadTextFileRequest, ReadTextFileResponse,
    RequestPermissionRequest, RequestPermissionResponse, SessionId, SessionNotification,
    SessionUpdate, StopReason, TextContent, WriteTextFileRequest, WriteTextFileResponse,
};
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::{
    on_receive_notification, on_receive_request, AcpAgent, AcpAgentConfig, Agent, Client,
    ConnectionTo, Responder,
};

use crate::acp::errors::AcpError;
use crate::acp::fs_backend::FsBackend;
use crate::acp::permission::{self, PendingPermissions};
use crate::config::{ConfigError, Registry};
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
/// resume; they are not read by the prompt path.
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
/// `Sync` — the mutable state is `Arc<Mutex<…>>` internally, so the
/// manager is managed directly (no outer lock); each method locks only
/// its own internal maps, briefly.
pub struct SessionManager {
    /// Shared with driver tasks (the `Arc` is cloned into each `tokio::spawn`).
    sessions: Arc<Mutex<HashMap<SessionId, LiveSession>>>,
    /// Pending permission requests, keyed by `"{session_id}/{request_id}"`.
    /// Populated by the permission bridge (Task 3); the driver-task cleanup
    /// drains all entries for a closing session.
    pending_permissions: PendingPermissions,
    registry: Registry,
    config_dir: PathBuf,
    /// The app's SQLite database (Task 5); `None` in tests that do not
    /// attach one.
    db: Option<Arc<Db>>,
    /// How long the establishment phase (agent spawn + `initialize` +
    /// `session/new` or `session/load`) may run before it is cancelled.
    /// Default: 30 s.
    establish_timeout: Duration,
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
            db: None,
            establish_timeout: Duration::from_secs(30),
        })
    }

    /// Attach the persistence database. Persistence is a no-op without it.
    pub fn attach_db(&mut self, db: Arc<Db>) {
        self.db = Some(db);
    }

    /// Override the establishment timeout (default 30 s; tests shrink it
    /// so a hanging agent does not make them wait).
    pub fn set_establish_timeout(&mut self, timeout: Duration) {
        self.establish_timeout = timeout;
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
        self.sessions
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
        self.sessions.lock().await.len()
    }

    /// Record a session in the persistence layer (no-op without a database).
    fn record_session(&self, info: &SessionInfo) {
        if let Some(db) = &self.db {
            let _ = db.record_session(info);
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
        let entry = self
            .registry
            .get(agent_id)
            .ok_or_else(|| AcpError::UnknownAgent {
                agent_id: agent_id.to_string(),
            })?;

        let agent = AcpAgent::new(
            AcpAgentConfig::new(entry.command.clone())
                .args(entry.args.clone())
                .envs(entry.env.clone()),
        );
        let hint = spawn_hint(&entry.command);

        let agent_id_owned = agent_id.to_string();
        let cwd_owned = cwd.clone();

        let info = self
            .drive_session(agent, agent_id, hint, cwd, sink, move |cx| async move {
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
                    },
                ))
            })
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
        let entry = self
            .registry
            .get(agent_id)
            .ok_or_else(|| AcpError::UnknownAgent {
                agent_id: agent_id.to_string(),
            })?;

        let agent = AcpAgent::new(
            AcpAgentConfig::new(entry.command.clone())
                .args(entry.args.clone())
                .envs(entry.env.clone()),
        );
        let hint = spawn_hint(&entry.command);

        let sid = SessionId::new(session_id);
        let agent_id_owned = agent_id.to_string();
        let cwd_owned = cwd.clone();
        let db = self.db.clone();

        let info = self
            .drive_session(agent, agent_id, hint, cwd, sink, move |cx| async move {
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

                let _restored = cx
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
                    },
                ))
            })
            .await?;

        self.record_session(&info);
        Ok(info)
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
    async fn drive_session<F, Fut>(
        &self,
        agent: AcpAgent,
        agent_id: &str,
        hint: String,
        cwd: PathBuf,
        sink: &Arc<dyn EventSink>,
        establish: F,
    ) -> Result<SessionInfo, AcpError>
    where
        F: FnOnce(ConnectionTo<Agent>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<(SessionId, SessionInfo), ProtocolError>> + Send + 'static,
    {
        // Channels that carry values out of the (long-lived) closure.
        let (ready_tx, ready_rx) = oneshot::channel::<ConnectionTo<Agent>>();
        let (session_ready_tx, session_ready_rx) = oneshot::channel::<SessionInfo>();
        let (session_id_tx, session_id_rx) = oneshot::channel::<SessionId>();
        let (error_tx, mut error_rx) = oneshot::channel::<ProtocolError>();
        let (close_tx, mut close_rx) = watch::channel(false);
        // Set by the closure when the user closes the session. The driver task
        // reads it after `connect_with` returns to decide the close reason.
        // (We can't rely on the closure to report the reason: on agent death
        // the SDK's background actor fails and drops the closure first.)
        let user_closed = Arc::new(AtomicBool::new(false));
        let user_closed_for_closure = user_closed.clone();

        // Per-session transcript accumulators for the persistence hook.
        let agent_text_acc: Arc<StdMutex<HashMap<String, String>>> =
            Arc::new(StdMutex::new(HashMap::new()));
        let tool_call_state: Arc<StdMutex<HashMap<String, Value>>> =
            Arc::new(StdMutex::new(HashMap::new()));

        let sessions_arc = self.sessions.clone();
        let pending_permissions_arc = self.pending_permissions.clone();
        let establish_timeout = self.establish_timeout;
        let db = self.db.clone();
        let sink = sink.clone();
        let notify_sink = sink.clone();
        let cwd_owned = cwd.clone();

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
                    // Bounded: a slow/hanging agent must not hang the
                    // establishment indefinitely.
                    let timeout_detail = format!(
                        "agent did not answer initialize within {}s",
                        establish_timeout.as_secs()
                    );
                    let (session_id, info) = match tokio::time::timeout(
                        establish_timeout,
                        establish(cx.clone()),
                    )
                    .await
                    {
                        Ok(Ok(established)) => established,
                        Ok(Err(err)) => {
                            // Report the failure to the awaiting command,
                            // then tear the connection down.
                            error_tx.send(err).ok();
                            return Ok(());
                        }
                        Err(_) => {
                            // The detail text doubles as the mapping
                            // marker in `map_establish_error`.
                            let timeout_err =
                                agent_client_protocol::util::internal_error(&timeout_detail);
                            error_tx.send(timeout_err).ok();
                            return Ok(());
                        }
                    };

                    session_id_tx.send(session_id.clone()).ok();
                    session_ready_tx.send(info).ok();

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
                // Keys are `"{session_id}/{request_id}"` — match on the
                // trailing-slash prefix so closing "s1" does not cancel
                // the pending prompt of the longer session "s10".
                let prefix = permission::session_key_prefix(&session_id.to_string());
                pending_permissions_arc
                    .lock()
                    .await
                    .retain(|key, _| !key.starts_with(&prefix));
                sink.emit(
                    "session-closed",
                    serde_json::json!({
                        "sessionId": session_id.to_string(),
                        "reason": reason.as_str(),
                    }),
                );
            }
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
                .ok_or_else(|| AcpError::UnknownSession {
                    session_id: session_id.to_string(),
                })?
        };

        // Record the user's message in the transcript (the client owns
        // history) before the turn begins.
        if let Some(db) = &self.db {
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

    /// Deliver the user's answer to a pending permission request.
    ///
    /// Looks up the oneshot sender by the compound key
    /// `"{session_id}/{request_id}"` and sends the outcome through it. If the
    /// entry is gone (the session closed, or the prompt already resolved), this
    /// is a silent no-op.
    pub async fn respond_permission(
        &self,
        session_id: &str,
        request_id: &str,
        outcome: permission::PermissionOutcome,
    ) -> Result<(), AcpError> {
        let key = permission::permission_key(session_id, request_id);
        let sender = self.pending_permissions.lock().await.remove(&key);
        // Best-effort: if the receiver is already gone the prompt was
        // already resolved (timeout / session close), so there is nothing to
        // do.
        if let Some(sender) = sender {
            let _ = sender.send(outcome);
        }
        Ok(())
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
                .ok_or_else(|| AcpError::UnknownSession {
                    session_id: session_id.to_string(),
                })?
        };
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
