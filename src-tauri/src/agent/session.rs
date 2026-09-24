//! The pi RPC session layer: spawns a `pi --mode rpc` process, speaks the
//! pi JSONL RPC protocol to it, and drives the session lifecycle.
//!
//! The heart of the design is the **driver-task mechanism**. The pi child
//! is long-lived (it lives for the session), so we never await it inline in
//! [`SessionManager::start_session`] / [`Self::resume_session`]. Instead we
//! spawn a *driver task* that owns a handle to the child for the whole
//! session: it runs the *establisher* (the `get_state` round-trip that turns
//! a fresh child into an established session), then blocks until the session
//! closes, streaming `session-update` events as pi's events arrive.
//! `close_session` (or a subagent cancel) flips a `watch` flag that makes
//! the driver task return; dropping the last handle closes the child's
//! stdin (a clean pi shutdown) and reaps the process.
//!
//! The same driver is used for a new session (establisher = `get_state`)
//! and a resume (establisher = `get_state` + `get_messages` replay — the
//! child is spawned with `--session <file>` so `get_messages` returns the
//! loaded session's transcript).
//!
//! Multiple live sessions COEXIST (the one-live cap is lifted, ADR 0002):
//! `start_session` / `resume_session` do NOT close other live sessions; a
//! session is torn down only by an explicit `close_session`, a subagent
//! cancel, or the agent process exiting on its own.
//!
//! **The `session-update` contract is FROZEN** (ADR 0009): [`normalize`]
//! maps pi events onto the exact JSON envelopes the frontend consumes
//! (`agent_message_chunk` / `agent_thought_chunk` / `tool_call` /
//! `tool_call_update` / `session_info_update` / `config_option_update`), so
//! the frontend needs no changes for the ACP → pi-RPC swap.

use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::{oneshot, watch, Mutex};

use crate::agent::bridge::{self, PendingBridge};
use crate::agent::errors::RpcError;
use crate::agent::permission::{self, PendingPermissions};
use crate::agent::rpc::{PiRpc, PiRpcHandle, RpcEvent};
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

/// Why a prompt turn ended.
///
/// A LOCAL enum (the ACP crate type dies with the swap): the strings are
/// exactly what the frontend expects (`end_turn` / `max_tokens` / `refusal`
/// / `max_turn_requests` / `cancelled`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// The model finished its turn normally.
    EndTurn,
    /// The model hit its output token limit.
    MaxTokens,
    /// The model (or the provider) refused the prompt.
    Refusal,
    /// The agent hit its max turn-request limit.
    MaxTurnRequests,
    /// The user (or the app) cancelled the turn.
    Cancelled,
}

/// One image attachment over IPC. The frontend sends CAMEL CASE
/// (`mimeType`, `sizeBytes`) — Tauri camel-cases only the TOP-LEVEL command
/// args, so this nested struct renames explicitly.
///
/// (Moved from `agent/prompt.rs`, which died with the ACP swap — the
/// `ImagePayload` / `MAX_IMAGE_BYTES` / validation / `user_message_payload`
/// helpers all live here now.)
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImagePayload {
    pub mime_type: String,
    /// base64, WITHOUT a `data:` prefix.
    pub data: String,
    pub name: String,
    pub size_bytes: u64,
}

/// 10 MiB — mirrors the frontend cap (ADR 0008).
pub const MAX_IMAGE_BYTES: u64 = 10 * 1024 * 1024;

/// 8 images per message — mirrors the frontend `MAX_CHAT_ATTACHMENTS` (ADR
/// 0008): re-validated here so hand-rolled IPC cannot bloat the DB
/// out-of-band (worst case 8 × 10 MiB raw ≈ 108 MB of base64 persisted per
/// message — base64 expands each image ~4/3×).
pub const MAX_IMAGE_COUNT: usize = 8;

/// 255 bytes — the OS per-component filename limit: `name` is written verbatim
/// to SQLite, so re-validated here so hand-rolled IPC cannot bloat a row
/// out-of-band. (The frontend's `name` comes from a real OS filename and is
/// already ≤255 bytes on all major OSes.)
pub const MAX_IMAGE_NAME_LEN: usize = 255;

/// Deliberately NARROWER than `image/*`: the destination is LLM vision APIs
/// (png/jpeg/gif/webp only — an SVG would fail the whole turn at the
/// provider). Mirrors the frontend `SUPPORTED_IMAGE_TYPES`.
const SUPPORTED_IMAGE_TYPES: &[&str] = &["image/png", "image/jpeg", "image/gif", "image/webp"];

/// Validate image attachments (guards against hand-rolled IPC): the count ≤
/// `MAX_IMAGE_COUNT`; `name` ≤ `MAX_IMAGE_NAME_LEN` bytes; `mime_type` in
/// `SUPPORTED_IMAGE_TYPES`; `data` with ≤2 trailing `=` padding chars;
/// decoded size (the padding-stripped `len * 3 / 4` estimate — `size_bytes`
/// is NOT trusted) ≤ `MAX_IMAGE_BYTES`.
fn validate_images(images: &[ImagePayload]) -> Result<(), RpcError> {
    if images.len() > MAX_IMAGE_COUNT {
        return Err(RpcError::InvalidPrompt {
            reason: format!("at most {MAX_IMAGE_COUNT} images per message"),
        });
    }
    for img in images {
        if img.name.len() > MAX_IMAGE_NAME_LEN {
            return Err(RpcError::InvalidPrompt {
                reason: "image name exceeds 255 bytes".to_string(),
            });
        }
        if !SUPPORTED_IMAGE_TYPES.contains(&img.mime_type.as_str()) {
            return Err(RpcError::InvalidPrompt {
                reason: format!(
                    "unsupported image type: {} (expected png, jpeg, gif, webp)",
                    img.mime_type
                ),
            });
        }
        let trimmed = img.data.trim_end_matches('=');
        // Legitimate base64 has 0–2 trailing `=` padding. Rejecting >2 keeps
        // the stripped-size estimate a real bound on the persisted data
        // (arbitrary `=` padding would otherwise bypass the size cap).
        let padding = img.data.len() - trimmed.len();
        if padding > 2 {
            return Err(RpcError::InvalidPrompt {
                reason: "malformed base64 image data".to_string(),
            });
        }
        let decoded_bytes = (trimmed.len() as u64) * 3 / 4;
        if decoded_bytes > MAX_IMAGE_BYTES {
            return Err(RpcError::InvalidPrompt {
                reason: "image exceeds the 10 MiB limit".to_string(),
            });
        }
    }
    Ok(())
}

/// The user-message payload persisted in `messages.payload_json` (ADR 0008):
/// `{ "text": ..., "images": [{ name, mimeType, sizeBytes, data }] }` —
/// the `images` key is OMITTED when empty (pre-feature rows stay `{"text"}`).
pub fn user_message_payload(text: &str, images: &[ImagePayload]) -> Value {
    if images.is_empty() {
        json!({ "text": text })
    } else {
        json!({
            "text": text,
            "images": images.iter().map(|img| {
                json!({
                    "name": img.name,
                    "mimeType": img.mime_type,
                    "sizeBytes": img.size_bytes,
                    "data": img.data,
                })
            }).collect::<Vec<_>>(),
        })
    }
}

/// A fully established session, ready to accept prompts.
///
/// `Serialize` so it can cross the IPC boundary as a command return value.
///
/// `capabilities` is the pi session's capability envelope (item 1 of the
/// swap plan): `piSessionId` / `piSessionFile`? / `model`? /
/// `thinkingLevel` / `loadSession` / `promptCapabilities` — the `model` key
/// is ABSENT when `get_state` reports no model, and `piSessionFile` is
/// ABSENT when the session has no file (`--no-session` runs; the session
/// is unresumable → `loadSession: false`). The two trailing keys are
/// load-bearing: `loadSession` gates the frontend's Resume button and
/// `promptCapabilities.image` gates image sending (fail-closed).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub session_id: String,
    pub agent_id: String,
    pub cwd: PathBuf,
    pub capabilities: Value,
    /// The session's configuration options (model / thinking level
    /// selectors) synthesized from `get_state` + `get_available_models` +
    /// `get_available_thinking_levels`; `None` when the agent advertised
    /// nothing (or for stored sessions — `list_sessions` always reports
    /// `None`).
    pub config_options: Option<Vec<Value>>,
}

/// A live, in-memory session handle.
///
/// `session_id`, `cwd`, and `agent_id` are carried for diagnostics and for
/// resume; they are not read by the prompt path.
///
/// `pub(crate)` + `pub(crate)` fields: `subagent.rs` reads `driver.sessions`
/// entries (to clone the `cx` for the subagent's task prompt), so the
/// struct and its fields are visible to the whole crate.
///
/// (No `Debug` derive — `PiRpcHandle` does not implement `Debug`.)
#[allow(dead_code)]
pub(crate) struct LiveSession {
    /// Cheap clone of the pi RPC handle, shared with the driver task.
    pub(crate) handle: PiRpcHandle,
    pub(crate) session_id: String,
    pub(crate) cwd: PathBuf,
    pub(crate) agent_id: String,
    /// Set to `true` to make the driver task's loop return, tearing the
    /// session down (dropping the handle closes the child's stdin).
    ///
    /// For a subagent session this IS the `ExternalClose`'s sender (a cancel
    /// flips it); for a main session it is the driver's internal flag.
    pub(crate) close_tx: watch::Sender<bool>,
    /// The close kind, decided by `close_session` (first-set-wins) and read
    /// by the driver task once the loop returns. For a subagent session
    /// this IS the `ExternalClose`'s kind.
    pub(crate) close_kind: Arc<StdMutex<Option<CloseKind>>>,
    pub(crate) thought_state: Arc<StdMutex<ThoughtState>>,
    /// The in-flight turn's resolver: `send_prompt` stores a sender here
    /// (last-wins), the driver resolves it on `agent_settled` (a
    /// `cancel_requested` flag maps a late settle to `Cancelled`).
    pub(crate) pending_turn: Arc<StdMutex<Option<oneshot::Sender<StopReason>>>>,
    /// Set by `cancel_session` BEFORE the `abort` is sent: a late
    /// `agent_settled` after an abort maps to `Cancelled`, not `EndTurn`.
    pub(crate) cancel_requested: Arc<StdMutex<bool>>,
    /// The most recent turn settle, watched by `SessionDriver::wait_for_settle`
    /// (the subagent's prompt wait): the driver sends `(seq, reason)` on
    /// `agent_settled`; the sequence number forces `changed()` to fire on
    /// every settle (a watch coalesces equal values). The initial `(0, …)`
    /// means "no settle yet"; a DROPPED sender (the driver task ended —
    /// a teardown without a settle) resolves `changed()` too.
    pub(crate) settle_rx: watch::Receiver<(u64, StopReason)>,
    /// This driver's generation (assigned from the driver's counter at
    /// registration): a SUPERSEDED driver (a resume overwrote this entry
    /// under the same session id) must not clobber the replacement's
    /// entry on teardown — the removal is guarded by this token.
    pub(crate) generation: u64,
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
    pub manager: Arc<crate::agent::subagent::SubagentSessionManager>,
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

/// Per-session thinking-persistence state: the accumulated text of the
/// OPEN segment, the segment's message key (or `None` when the last
/// update was not a continuing thought chunk), and the next segment
/// number. `current` is reset when a new segment starts (a closed
/// segment is never re-appended — see the segmentation rule above).
#[derive(Debug, Default)]
pub(crate) struct ThoughtState {
    current: String,
    open_key: Option<String>,
    next: u32,
}

/// Per-session turn state for the event normalizer.
///
/// `msg_counter` is the `messageId` source: it starts at 0 and is advanced
/// ONLY when a `message_start` carries an `assistant` message (the wire's
/// `message_start` carries ANY `AgentMessage` — user and tool-result
/// messages included — so a role-blind counter would number the first
/// assistant chunk `m2` and make the `get_messages` replay numbering
/// irreproducible). `current_message_id` is `format!("m{}", counter)`.
///
/// `toolcall_args` is the accumulating partial-args buffer per tool-call id
/// (the `toolcall_delta` frames are JSON fragments; `toolcall_end` carries
/// the full `arguments` object and the buffer entry is dropped).
#[derive(Debug, Default)]
pub struct TurnState {
    pub msg_counter: u64,
    pub current_message_id: Option<String>,
    pub toolcall_args: HashMap<String, String>,
}

/// The shared session-driver state. `SessionManager` (main sessions) and
/// `SubagentSessionManager` (worker runtime) each own one.
///
/// Holds the live-session map, the pending-request maps, the establish
/// timeout, and the per-manager policy knobs (persistence, the in-memory
/// text/cost captures, and the subagent dispatch handle). `drive_session`
/// (the shared driver) is a method on this struct.
pub struct SessionDriver {
    pub(crate) sessions: Arc<Mutex<HashMap<String, LiveSession>>>,
    pub(crate) pending_permissions: PendingPermissions,
    pub(crate) pending_bridge: PendingBridge,
    /// How long the establishment phase (agent spawn + `get_state`
    /// establisher) may run before it is cancelled. Default: 30 s.
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
    pub(crate) subagent: Option<Arc<crate::agent::subagent::SubagentSessionManager>>,
    /// The live-session generation counter (monotonic; each
    /// `drive_session` registration takes the next value — the teardown
    /// guard).
    pub(crate) generation_counter: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

impl SessionDriver {
    /// Create a driver (a fresh sessions map, empty pending maps, a 30 s
    /// establish timeout, no persistence / captures / subagent handle).
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            generation_counter: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
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

    /// Shared driver: run the *establisher* against the (already spawned)
    /// pi child, then stream the session's events until it closes.
    ///
    /// `handle` is a cheap clone of the pi RPC handle (the caller spawned
    /// the `PiRpc` — a dropped `PiRpc` wrapper does NOT kill the child:
    /// the child lives while ANY handle does). `establisher` receives the
    /// handle and must return the established `SessionInfo`; on failure it
    /// returns `Err` (the error is reported to the awaiting command; the
    /// establish-timeout marker is applied here, not in the establisher).
    ///
    /// `external_close` (subagents only; `None` for main) is the cancel
    /// path: the driver task selects on its receiver in BOTH the establish
    /// phase and the block-until-close phase, and reads its `kind` (instead
    /// of the internal kind) for the close reason (one kind, first-set-wins
    /// across the whole session).
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn drive_session<Establisher, EstablisherFut>(
        &self,
        handle: PiRpcHandle,
        agent_id: &str,
        hint: String,
        cwd: PathBuf,
        sink: &Arc<dyn EventSink>,
        bridge_setup: Option<(String, PathBuf)>,
        external_close: Option<ExternalClose>,
        establisher: Establisher,
    ) -> Result<SessionInfo, RpcError>
    where
        Establisher: FnOnce(PiRpcHandle) -> EstablisherFut + Send + 'static,
        EstablisherFut: Future<Output = Result<SessionInfo, RpcError>> + Send + 'static,
    {
        // Split the external close (the subagent cancel path): the driver
        // task selects on `rx` (establish + block phases) and reads `kind`
        // for the reason; the bridge listener observes `tx` (a cancel also
        // cancels in-flight `ask` waiters). `None` for main sessions.
        let (ec_tx, ec_rx, ec_kind) = match external_close {
            Some(ec) => (Some(ec.tx), Some(ec.rx), Some(ec.kind)),
            None => (None, None, None),
        };

        // Channels that carry values out of the (long-lived) driver task.
        let (session_ready_tx, session_ready_rx) = oneshot::channel::<SessionInfo>();
        let (error_tx, mut error_rx) = oneshot::channel::<RpcError>();
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
        // The turn-settle watch: the driver sends `(seq, reason)` on
        // `agent_settled` (the sequence number forces `changed()` to fire
        // on every settle — a watch coalesces equal values); the initial
        // `(0, …)` means "no settle yet". `wait_for_settle` (the subagent's
        // prompt wait) reads the receiver; a DROPPED sender (a teardown
        // without a settle) resolves `changed()` too.
        let (settle_tx, settle_rx) = watch::channel((0u64, StopReason::EndTurn));

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
                    .map_err(|e| RpcError::Io(format!("bridge listener: {e}")))?,
                )
            }
            None => None,
        };

        // Per-session transcript accumulators for the persistence hook.
        let agent_text_acc: Arc<StdMutex<HashMap<String, String>>> =
            Arc::new(StdMutex::new(HashMap::new()));
        let tool_call_state: Arc<StdMutex<HashMap<String, Value>>> =
            Arc::new(StdMutex::new(HashMap::new()));
        let thought_state: Arc<StdMutex<ThoughtState>> =
            Arc::new(StdMutex::new(ThoughtState::default()));
        // The normalizer's per-session turn state (the `messageId` counter +
        // the tool-call partial-args buffers).
        let turn_state: Arc<StdMutex<TurnState>> = Arc::new(StdMutex::new(TurnState::default()));
        // The settle sequence for the turn-settle watch (moved into the
        // driver task; incremented on every `agent_settled`).
        let mut settle_seq = 0u64;
        let _thought_state_task = thought_state.clone();

        let sessions_arc = self.sessions.clone();
        let pending_permissions_arc = self.pending_permissions.clone();
        let pending_bridge_arc = self.pending_bridge.clone();
        let establish_timeout = self.establish_timeout;
        let db = self.db.clone();
        // The capture hooks (subagents only; `None` for main). Clones for
        // the driver task's event handler.
        let text_capture = self.text_capture.clone();
        let last_message_id = self.last_message_id.clone();
        let thought_capture = thought_state.clone();
        let sink = sink.clone();
        // The external close's receiver for the driver task (cloned per
        // select phase — a `None` receiver is inert).
        let ec_rx_task = ec_rx;

        // SPAWN the driver task. The pi child is long-lived (it lives for
        // the session), so the task must never be awaited inline here.
        // The handle is CHEAP to clone (an `Arc`); the child lives while
        // ANY clone is alive — the task takes a clone, the original moves
        // into the `LiveSession` value below.
        // The session's generation token (captured by the driver task —
        // the teardown guard: a SUPERSEDED driver (a resume overwrote this
        // entry under the same session id) must not clobber the
        // replacement's entry).
        let live_generation = self
            .generation_counter
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let handle_task = handle.clone();
        tokio::spawn(async move {
            // The event / extension-UI / exit streams (a fresh receiver per
            // call — the queues buffer until a consumer attaches).
            let handle = handle_task;
            let mut events_rx = handle.events();
            let mut ui_rx = handle.extension_ui();
            let mut exited_rx = handle.exited();

            // Establish the session (the `get_state` round-trip for a new
            // session; `get_state` + `get_messages` replay for a resume).
            // Bounded + external-close aware: a cancel during the establish
            // window is honored (not deferred to the timeout).
            let timeout_detail = format!(
                "agent did not answer get_state within {}s",
                establish_timeout.as_secs()
            );
            let mut establish_rx = ec_rx_task.clone();
            let established = tokio::select! {
                r = tokio::time::timeout(establish_timeout, establisher(handle.clone())) => {
                    match r {
                        Ok(Ok(info)) => Some(info),
                        Ok(Err(err)) => {
                            // Report the failure to the awaiting command,
                            // then tear the child down.
                            error_tx.send(err).ok();
                            None
                        }
                        Err(_) => {
                            error_tx
                                .send(RpcError::EstablishTimeout {
                                    detail: timeout_detail,
                                })
                                .ok();
                            None
                        }
                    }
                }
                // A cancel during the establish window is honored (not
                // deferred to the timeout); a `None` receiver is inert
                // (main sessions).
                _ = changed_or_inert(&mut establish_rx) => None,
            };
            let info = match established {
                Some(info) => info,
                // The establisher failed (it sent the error first) or a
                // cancel won the race: tear the child down and exit.
                None => {
                    bridge::teardown(bridge_handle);
                    return;
                }
            };

            session_ready_tx.send(info.clone()).ok();

            // Hand the pi `session_id` to the bridge handle so
            // `bridge-request`/`bridge-event` payloads carry the pi id
            // (bridge requests only occur mid-turn, after establish, so
            // they always carry the pi id).
            if let Some(h) = &bridge_handle {
                h.set_session_id(&info.session_id).await;
            }

            // BLOCK until close_session OR agent death OR the external
            // close, streaming the session's events as they arrive.
            let mut internal_rx = ec_rx_task.is_none().then_some(close_rx);
            // The internal close flag is inert for SUBAGENTS (the external
            // close is their cancel path — the internal sender is dropped,
            // and a dropped sender's `changed()` resolves immediately, which
            // would tear the session down right after establishment).
            let mut block_rx = ec_rx_task.clone();
            loop {
                tokio::select! {
                    ev = events_rx.recv() => {
                        let Some(ev) = ev else {
                            // The reader task ended (the child's stdout
                            // closed — treat it as an exit).
                            break;
                        };
                        // A settled turn resolves the pending prompt (a
                        // `cancel_requested` flag maps it to `Cancelled`) AND
                        // records the settle on the watch (the subagent's
                        // prompt wait — `wait_for_settle`).
                        if matches!(ev, RpcEvent::agent_settled) {
                            settle_seq += 1;
                            let reason = {
                                let sessions = sessions_arc.lock().await;
                                if let Some(live) = sessions.get(&info.session_id) {
                                    let cancelled = *live
                                        .cancel_requested
                                        .lock()
                                        .unwrap_or_else(|p| p.into_inner());
                                    if cancelled {
                                        StopReason::Cancelled
                                    } else {
                                        StopReason::EndTurn
                                    }
                                } else {
                                    StopReason::EndTurn
                                }
                            };
                            let _ = settle_tx.send((settle_seq, reason));
                            resolve_pending_turn(&sessions_arc, &info.session_id).await;
                        }
                        // A thinking-level change re-synthesizes the config
                        // options (the normalizer has no model list — the
                        // fetch is async, so it lives here, not in the
                        // pure normalizer).
                        if matches!(ev, RpcEvent::thinking_level_changed { .. }) {
                            resynthesize_config_options(&handle, &info.session_id, &sink).await;
                            continue;
                        }
                        let updates = {
                            let mut turn =
                                turn_state.lock().unwrap_or_else(|p| p.into_inner());
                            normalize(&ev, &mut turn)
                        };
                        for update in updates {
                            let frame = json!({
                                "sessionId": info.session_id,
                                "update": update,
                            });
                            sink.emit("session-update", frame);
                            // The client owns history: upsert the transcript
                            // row as the update streams in.
                            if let Some(db) = &db {
                                persist_update(
                                    db,
                                    &info.session_id,
                                    &update,
                                    &agent_text_acc,
                                    &tool_call_state,
                                    &thought_capture,
                                );
                            }
                            // (a) Capture the agent text for the FINAL
                            // OUTPUT (subagents only; `None` for main —
                            // main persists to the DB). Fed from the
                            // normalized `agent_message_chunk` frames keyed
                            // by the derived `messageId`.
                            if let Some(tc) = &text_capture {
                                if update
                                    .get("sessionUpdate")
                                    .and_then(Value::as_str)
                                    == Some("agent_message_chunk")
                                {
                                    if let Some(text) = update
                                        .get("content")
                                        .and_then(|c| c.get("text"))
                                        .and_then(Value::as_str)
                                    {
                                        if !text.is_empty() {
                                            let key = update
                                                .get("messageId")
                                                .and_then(Value::as_str)
                                                .unwrap_or("default")
                                                .to_string();
                                            let mut acc = tc
                                                .lock()
                                                .unwrap_or_else(|p| p.into_inner());
                                            acc.entry(key.clone()).or_default().push_str(text);
                                            if let Some(lmi) = &last_message_id {
                                                *lmi.lock().unwrap_or_else(
                                                    |p| p.into_inner(),
                                                ) = Some(key);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    req = ui_rx.recv() => {
                        let Some(req) = req else {
                            break;
                        };
                        // The permission gate (the bundled gate extension's
                        // `ctx.ui.confirm` / `ctx.ui.select` dialogs).
                        permission::handle_extension_ui_request(
                            &info.session_id, req, &handle, &sink, &pending_permissions_arc,
                        )
                        .await;
                    }
                    // The child exited (reaped by the exit-watcher task).
                    _ = exited_rx.changed() => break,
                    // The internal close flag (main sessions).
                    _ = changed_or_inert(&mut internal_rx) => break,
                    // The external close (subagent cancel); a `None`
                    // receiver is inert (main sessions).
                    _ = changed_or_inert(&mut block_rx) => break,
                }
            }

            // The session is over: clean up. The reason comes from the close
            // kind the closer recorded: a kind set before the flag send
            // wins; `None` means the agent process exited on its own and
            // nobody closed it.
            //
            // Close the child's stdin EXPLICITLY (idempotent — `close_session`
            // may have done it already): dropping this task's handle clone
            // is NOT enough to close the stdin, because the exit-watcher
            // task holds its own `Arc<Inner>` clone for the whole
            // `child.wait()` — and `wait()` only returns once the child
            // exits, which (for a well-behaved agent) only happens on the
            // stdin EOF. Without this explicit close the cycle would leak
            // the agent process (and the exit-watcher task) forever.
            handle.close().await;
            let kind = *kind_for_task.lock().unwrap_or_else(|p| p.into_inner());
            let reason = match kind {
                Some(CloseKind::User) => ClosedReason::User,
                None => ClosedReason::AgentExited,
            };
            // Guard by the generation token (captured above): a
            // SUPERSEDED driver (a resume overwrote this entry under the
            // same session id) must not clobber the replacement's entry.
            {
                let mut sessions = sessions_arc.lock().await;
                if sessions
                    .get(&info.session_id)
                    .is_some_and(|l| l.generation == live_generation)
                {
                    sessions.remove(&info.session_id);
                }
            }
            // Keys are `"{session_id}/{request_id}"` — match on the
            // trailing-slash prefix so closing "s1" does not cancel
            // the pending prompt of the longer session "s10".
            let prefix = permission::session_key_prefix(&info.session_id);
            pending_permissions_arc
                .lock()
                .await
                .retain(|key, _| !key.starts_with(&prefix));
            // Drain this session's pending bridge requests too (dropping
            // the senders cancels the spawned waiters, which write the
            // terminal `error:"cancelled"` frame).
            let bridge_prefix = bridge::session_key_prefix(&info.session_id);
            pending_bridge_arc
                .lock()
                .await
                .retain(|key, _| !key.starts_with(&bridge_prefix));
            sink.emit(
                "session-closed",
                json!({
                    "sessionId": info.session_id,
                    "reason": reason.as_str(),
                }),
            );
            // Tear the bridge listener down UNCONDITIONALLY (do NOT nest it
            // inside the session guard, or a failed establisher would leak
            // the listener): stop the accept loop + unlink the socket
            // (Unix; Windows pipes vanish on last close).
            bridge::teardown(bridge_handle);
        });

        // Await the established session.
        let info = match session_ready_rx.await {
            Ok(info) => info,
            Err(_) => {
                // The driver never delivered an established session: either
                // the establisher failed (it sent the error first) or the
                // child died mid-establish.
                match error_rx.try_recv() {
                    Ok(err) => return Err(err),
                    Err(_) => {
                        return Err(RpcError::EstablishTimeout {
                            detail: format!("agent did not complete establish ({hint})"),
                        })
                    }
                }
            }
        };

        let live = LiveSession {
            handle,
            generation: live_generation,
            session_id: info.session_id.clone(),
            cwd,
            agent_id: agent_id.to_string(),
            close_tx: ec_tx.unwrap_or_else(|| close_tx.clone()),
            close_kind: kind,
            thought_state,
            pending_turn: Arc::new(StdMutex::new(None)),
            cancel_requested: Arc::new(StdMutex::new(false)),
            settle_rx,
        };
        self.sessions
            .lock()
            .await
            .insert(live.session_id.clone(), live);

        Ok(info)
    }
    /// Await the session's next turn settle (the driver's `agent_settled` watch).
    ///
    /// UNBOUNDED (like the ACP subagent prompt await): the wait ends on the
    /// turn's `agent_settled` OR on a teardown (the driver task ending DROPS
    /// the watch sender, which resolves `changed()`). `Ok(reason)` when a
    /// settle was recorded (the `cancel_requested` flag already mapped it to
    /// `Cancelled`); `Err` when the session vanished or the agent died without
    /// settling.
    pub async fn wait_for_settle(&self, session_id: &str) -> Result<StopReason, RpcError> {
        let mut rx = {
            let sessions = self.sessions.lock().await;
            sessions
                .get(session_id)
                .map(|l| l.settle_rx.clone())
                .ok_or_else(|| RpcError::UnknownSession {
                    id: session_id.to_string(),
                })?
        };
        // `borrow_and_update` marks the current value as seen: `changed()` then
        // fires only on a LATER change (or a sender drop).
        let (seq, _) = *rx.borrow_and_update();
        if seq == 0 {
            let _ = rx.changed().await;
        }
        let (seq, reason) = *rx.borrow();
        if seq == 0 {
            // The sender was dropped without a settle (the agent died
            // mid-turn, or the session was torn down).
            Err(RpcError::ProcessExited(None))
        } else {
            Ok(reason)
        }
    }
}

/// Resolve the session's pending turn (a `send_prompt` awaiting its
/// `agent_settled`): a `cancel_requested` flag maps the settle to
/// `Cancelled`, else `EndTurn`. A no-op when no turn is in flight.
async fn resolve_pending_turn(
    sessions: &Arc<Mutex<HashMap<String, LiveSession>>>,
    session_id: &str,
) {
    if let Some(live) = sessions.lock().await.get(session_id) {
        if let Some(tx) = live
            .pending_turn
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
        {
            let cancelled = *live
                .cancel_requested
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            let _ = tx.send(if cancelled {
                StopReason::Cancelled
            } else {
                StopReason::EndTurn
            });
        }
    }
}

/// Re-synthesize the session's config options after a `thinking_level_changed`
/// (the normalizer has no model list — the fetch is async, so this lives in
/// the driver task): `get_state` (the current model + level) +
/// `get_available_models` + `get_available_thinking_levels` → emit a
/// `config_option_update` with the fresh options. A no-op (silently) when
/// any fetch fails (a config refresh is a convenience, not a correctness
/// signal).
async fn resynthesize_config_options(
    handle: &PiRpcHandle,
    session_id: &str,
    sink: &Arc<dyn EventSink>,
) {
    let Ok(state) = handle.send(json!({ "type": "get_state" })).await else {
        return;
    };
    let models = handle
        .send(json!({ "type": "get_available_models" }))
        .await
        .ok()
        .and_then(|v| v.get("models").cloned());
    let levels = handle
        .send(json!({ "type": "get_available_thinking_levels" }))
        .await
        .ok()
        .and_then(|v| v.get("levels").cloned());
    if let Some(opts) = synthesize_config_options(&state, models.as_ref(), levels.as_ref()) {
        sink.emit(
            "session-update",
            json!({
                "sessionId": session_id,
                "update": { "sessionUpdate": "config_option_update", "configOptions": opts },
            }),
        );
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
    /// The installed gate extension's path (`None` when the install
    /// failed — the spawn then skips the gate args/env, and the session
    /// runs ungated rather than broken).
    gate_path: Option<PathBuf>,
}

impl SessionManager {
    /// The configured config directory (useful for tests and diagnostics).
    pub fn config_dir(&self) -> &PathBuf {
        &self.config_dir
    }
    /// Create a manager, loading the agent registry from `config_dir`.
    pub fn new(config_dir: PathBuf) -> Result<Self, ConfigError> {
        let registry = Registry::load(&config_dir)?;
        // Install the bundled gate extension (idempotent; both managers
        // install the same file — the write is skipped when it matches).
        // A failure is NON-fatal: the session runs ungated rather than
        // broken at startup.
        let gate_path = match crate::agent::gate::install_gate_extension(&config_dir) {
            Ok(path) => Some(path),
            Err(e) => {
                eprintln!("gate extension install failed: {e} (sessions run ungated)");
                None
            }
        };
        Ok(Self {
            driver: SessionDriver::new(),
            registry,
            config_dir,
            gate_path,
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
    pub fn set_subagent_manager(&mut self, m: Arc<crate::agent::subagent::SubagentSessionManager>) {
        self.driver.subagent = Some(m);
    }

    /// Reset the session's open thinking segment at a prompt boundary. The
    /// frontend's `addUserMessage` starts a new thinking block on a user
    /// message, but `persist_update` never sees user messages (they are
    /// recorded by the `send_prompt` paths, not the event normalizer) —
    /// so the Rust accumulator must be reset here, not in `persist_update`.
    pub async fn begin_user_turn(&self, session_id: &str) {
        if let Some(live) = self.driver.sessions.lock().await.get(session_id) {
            live.thought_state
                .lock()
                .expect("thought state poisoned")
                .open_key = None;
        }
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

    /// Spawn a pi agent, establish the session (`get_state`), and register
    /// it.
    ///
    /// Returns the [`SessionInfo`] once the session is established. The
    /// child is driven by a background task that lives for the session's
    /// lifetime; it is torn down by [`Self::close_session`] or agent death.
    pub async fn start_session(
        &self,
        agent_id: &str,
        cwd: PathBuf,
        sink: &Arc<dyn EventSink>,
    ) -> Result<SessionInfo, RpcError> {
        // Canonicalize BEFORE the registry lookup and before the space row is
        // touched: the spaces join key is the canonicalized cwd, so a
        // `~/x` / symlink spelling must not produce a different row.
        let cwd = std::fs::canonicalize(&cwd).map_err(|_| RpcError::FolderMissing {
            path: cwd.display().to_string(),
        })?;

        let entry = self
            .registry
            .get(agent_id)
            .ok_or_else(|| RpcError::UnknownAgent {
                id: agent_id.to_string(),
            })?;

        // Bridge wiring (ADR 0003): for a bridge agent (on a platform where
        // the bridge is available — NOT macOS), set the 4 bridge env vars
        // and pass the (client session id, socket path) to the driver so it
        // starts the peer-verified listener before the spawn returns. For a
        // NEW session the client session id is a fresh UUID (the pi
        // `sessionId` does not exist until the session is established).
        let client_session_id = uuid::Uuid::new_v4().to_string();
        let (agent_env, bridge_setup) = match bridge_spawn_setup(entry, &client_session_id) {
            Some((env, sid, socket_path)) => (env, Some((sid, socket_path))),
            None => (entry.env.clone(), None),
        };

        // The gate injection (Task 4): `-e <gate.ts>` + `PI_ARCHIMEDES_GATE=1`
        // (the extension is inert without the env var).
        let args = crate::agent::gate::gate_spawn_args(self.gate_path.as_deref(), &entry.args);
        let mut agent_env = agent_env;
        if self.gate_path.is_some() {
            crate::agent::gate::gate_env(&mut agent_env);
        }
        let rpc = PiRpc::spawn(&entry.command, &args, &agent_env, &cwd)?;
        let handle = rpc.handle();

        let agent_id_owned = agent_id.to_string();
        let cwd_owned = cwd.clone();

        let info = self
            .driver
            .drive_session(
                handle,
                agent_id,
                spawn_hint(&entry.command),
                cwd,
                sink,
                bridge_setup,
                None,
                move |handle: PiRpcHandle| async move {
                    // The establisher: `get_state` (the session's identity)
                    // + the config-option sources (models / levels —
                    // lenient: a missing source just means no selectors).
                    let state = handle.send(json!({ "type": "get_state" })).await?;
                    let session_id = state
                        .get("sessionId")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let models = handle
                        .send(json!({ "type": "get_available_models" }))
                        .await
                        .ok()
                        .and_then(|v| v.get("models").cloned());
                    let levels = handle
                        .send(json!({ "type": "get_available_thinking_levels" }))
                        .await
                        .ok()
                        .and_then(|v| v.get("levels").cloned());
                    Ok(SessionInfo {
                        session_id,
                        agent_id: agent_id_owned,
                        cwd: cwd_owned,
                        capabilities: build_capabilities(&state),
                        config_options: synthesize_config_options(
                            &state,
                            models.as_ref(),
                            levels.as_ref(),
                        ),
                    })
                },
            )
            .await?;

        self.record_session(&info);
        Ok(info)
    }

    /// Resume a stored session: spawn a fresh pi for `agent_id` WITH
    /// `--session <stored pi session file>` (the child loads the stored
    /// session at startup), then establish (`get_state` + `get_messages`
    /// replay — `get_messages` returns the LOADED session's transcript).
    ///
    /// The driver-task lifecycle is shared verbatim with
    /// [`Self::start_session`]; only the establisher differs.
    ///
    /// Returns [`RpcError::NotResumable`] when the stored session has no
    /// pi session file (a legacy ACP row, or a `--no-session` run — the UI
    /// shows the history-only banner instead).
    pub async fn resume_session(
        &self,
        agent_id: &str,
        session_id: &str,
        cwd: PathBuf,
        sink: &Arc<dyn EventSink>,
    ) -> Result<SessionInfo, RpcError> {
        // Canonicalize BEFORE the registry lookup (same rationale as
        // `start_session`): everything downstream (the spawn cwd,
        // `SessionInfo.cwd`, the space join key) uses the canonical path.
        let cwd = std::fs::canonicalize(&cwd).map_err(|_| RpcError::FolderMissing {
            path: cwd.display().to_string(),
        })?;

        let entry = self
            .registry
            .get(agent_id)
            .ok_or_else(|| RpcError::UnknownAgent {
                id: agent_id.to_string(),
            })?;

        // Read the stored capability envelope (the `piSessionFile` is the
        // `--session` argument). A missing row / unparseable envelope /
        // absent `piSessionFile` is unresumable (a legacy ACP row or a
        // `--no-session` run).
        let caps_json = self
            .driver
            .db
            .as_ref()
            .and_then(|db| db.session(session_id).ok().flatten())
            .map(|row| row.capabilities_json)
            .ok_or_else(|| RpcError::NotResumable {
                id: session_id.to_string(),
            })?;
        let caps: Value = serde_json::from_str(&caps_json).unwrap_or(Value::Null);
        let session_file = caps
            .get("piSessionFile")
            .and_then(Value::as_str)
            .ok_or_else(|| RpcError::NotResumable {
                id: session_id.to_string(),
            })?;

        // The restored transcript is replaced by the agent's replay, which
        // doubles as the authoritative history: clear the stored rows
        // BEFORE the spawn so a replay reusing a known `messageId`
        // overwrites (rather than clobbers) and a replay under a new id
        // does not duplicate the stored text.
        if let Some(db) = &self.driver.db {
            let _ = db.clear_messages_for(session_id);
        }

        // Bridge wiring (ADR 0003): same as `start_session`, but the client
        // session id is the STORED `session_id` (a resume re-uses it, so
        // the agent's `session` push echoes the same id the desktop set).
        let (agent_env, bridge_setup) = match bridge_spawn_setup(entry, session_id) {
            Some((env, sid, socket_path)) => (env, Some((sid, socket_path))),
            None => (entry.env.clone(), None),
        };

        // The resume spawn LOADS the pi session: `--session <file>` (the
        // stored `piSessionFile`) so `get_messages` returns the loaded
        // transcript, not an empty fresh session.
        let mut args = entry.args.clone();
        args.push("--session".to_string());
        args.push(session_file.to_string());

        // The gate injection (Task 4): `-e <gate.ts>` + `PI_ARCHIMEDES_GATE=1`.
        let args = crate::agent::gate::gate_spawn_args(self.gate_path.as_deref(), &args);
        let mut agent_env = agent_env;
        if self.gate_path.is_some() {
            crate::agent::gate::gate_env(&mut agent_env);
        }
        let rpc = PiRpc::spawn(&entry.command, &args, &agent_env, &cwd)?;
        let handle = rpc.handle();

        let agent_id_owned = agent_id.to_string();
        let cwd_owned = cwd.clone();
        let db = self.driver.db.clone();
        let session_id_owned = session_id.to_string();

        let info = self
            .driver
            .drive_session(
                handle,
                agent_id,
                spawn_hint(&entry.command),
                cwd,
                sink,
                bridge_setup,
                None,
                move |handle: PiRpcHandle| async move {
                    // The establisher: `get_state` (now reflects the loaded
                    // session) + `get_messages` (the loaded transcript) →
                    // replay the stored rows (user rows persisted directly;
                    // assistant / toolResult messages through the
                    // normalizer per the replay-feed rule).
                    let state = handle.send(json!({ "type": "get_state" })).await?;
                    let messages = handle.send(json!({ "type": "get_messages" })).await?;
                    if let Some(db) = &db {
                        replay_messages(db, &session_id_owned, &messages);
                    }
                    let models = handle
                        .send(json!({ "type": "get_available_models" }))
                        .await
                        .ok()
                        .and_then(|v| v.get("models").cloned());
                    let levels = handle
                        .send(json!({ "type": "get_available_thinking_levels" }))
                        .await
                        .ok()
                        .and_then(|v| v.get("levels").cloned());
                    Ok(SessionInfo {
                        session_id: session_id_owned,
                        agent_id: agent_id_owned,
                        cwd: cwd_owned,
                        capabilities: build_capabilities(&state),
                        config_options: synthesize_config_options(
                            &state,
                            models.as_ref(),
                            levels.as_ref(),
                        ),
                    })
                },
            )
            .await?;

        self.record_session(&info);
        Ok(info)
    }

    /// Send a prompt to a live session and wait for the turn to finish.
    ///
    /// Returns the [`StopReason`] the turn resolved to (the frontend needs
    /// the turn-completion signal).
    pub async fn send_prompt(
        &self,
        session_id: &str,
        text: String,
    ) -> Result<StopReason, RpcError> {
        self.send_prompt_with_images(session_id, text, Vec::new())
            .await
    }

    /// Like `send_prompt`, but with image attachments: validates them
    /// (`validate_images`), appends pi `image` content (text first), and
    /// persists `{ "text", "images": [...] }` in the transcript (the
    /// `images` key omitted when empty — ADR 0008).
    ///
    /// The user-row persistence lives SOLELY here (the command delegates to
    /// this method and adds NO persistence of its own — a double write
    /// would show a duplicate user bubble). The turn resolves on
    /// `agent_settled` (the driver's `pending_turn` oneshot): a
    /// `success: false` prompt response maps to `Refusal`, a cancel maps to
    /// `Cancelled` (the `cancel_requested` flag), everything else to
    /// `EndTurn`.
    pub async fn send_prompt_with_images(
        &self,
        session_id: &str,
        text: String,
        images: Vec<ImagePayload>,
    ) -> Result<StopReason, RpcError> {
        // Clone just the (cheap) handle, not the whole LiveSession.
        let (handle, pending_turn, cancel_requested) = {
            let sessions = self.driver.sessions.lock().await;
            let live = sessions
                .get(session_id)
                .map(|l| {
                    (
                        l.handle.clone(),
                        l.pending_turn.clone(),
                        l.cancel_requested.clone(),
                    )
                })
                .ok_or_else(|| RpcError::UnknownSession {
                    id: session_id.to_string(),
                })?;
            live
        };

        // Validate the images FIRST: a rejected payload must NOT be written
        // to the transcript and must NOT start a user turn — validating
        // before persisting is what keeps the "cap bounds DB growth"
        // guarantee real.
        validate_images(&images)?;

        // Record the user's message in the transcript (the client owns
        // history) before the turn begins.
        self.begin_user_turn(session_id).await;
        if let Some(db) = &self.driver.db {
            let payload = user_message_payload(&text, &images);
            let _ = db.record_message(session_id, "user", None, &payload.to_string());
        }

        // Build the prompt command: the text message + the image content
        // (pi's `ImageContent` = `{type: "image", data, mimeType}` — the
        // `ImagePayload` maps onto it verbatim; the `name` / `sizeBytes`
        // are transcript-only, not wire fields).
        let mut command = json!({ "type": "prompt", "message": text });
        if !images.is_empty() {
            command["images"] = Value::Array(
                images
                    .iter()
                    .map(|img| {
                        json!({
                            "type": "image",
                            "data": img.data,
                            "mimeType": img.mime_type,
                        })
                    })
                    .collect(),
            );
        }
        // A prompt while the session is already streaming is a STEER (the
        // turn continues with the new input — the frontend's composer is
        // locked until the turn resolves, so a concurrent send means the
        // previous turn is still running).
        if let Ok(state) = handle.send(json!({ "type": "get_state" })).await {
            if state.get("isStreaming").and_then(Value::as_bool) == Some(true) {
                command["streamingBehavior"] = json!("steer");
            }
        }

        // Store the turn's resolver (last-wins: a replaced turn's sender is
        // dropped → its `send_prompt` resolves `Cancelled` below) and reset
        // the cancel flag (a fresh turn is not a cancel).
        let (tx, rx) = oneshot::channel::<StopReason>();
        {
            let mut slot = pending_turn.lock().unwrap_or_else(|p| p.into_inner());
            *slot = Some(tx);
        }
        *cancel_requested.lock().unwrap_or_else(|p| p.into_inner()) = false;

        // Send the prompt. The response arrives after preflight (start of
        // turn); a `success: false` response is a REFUSAL — emit the error
        // as a chunk (the user sees it) and resolve the turn `Refusal`
        // without waiting for a settle that never comes.
        match handle.send(command).await {
            Ok(_) => {}
            Err(RpcError::Command { error }) => {
                let mut slot = pending_turn.lock().unwrap_or_else(|p| p.into_inner());
                if let Some(tx) = slot.take() {
                    let _ = tx.send(StopReason::Refusal);
                }
                return Err(RpcError::Command { error });
            }
            Err(e) => return Err(e),
        }

        // Await the turn (unbounded — the turn can block on a user-paced
        // permission prompt). A dropped resolver (replaced by a new prompt,
        // or the session died) maps to `Cancelled`.
        match rx.await {
            Ok(reason) => Ok(reason),
            Err(_) => Ok(StopReason::Cancelled),
        }
    }

    /// Cancel the session's in-flight prompt turn (the pi `abort` command —
    /// the agent aborts the turn and settles it). The `cancel_requested`
    /// flag is set BEFORE the `abort` is sent so a fast settle maps to
    /// `Cancelled`, not `EndTurn`. Fire-and-forget on the agent side: a
    /// no-op if there is no in-flight turn.
    pub async fn cancel_session(&self, session_id: &str) -> Result<(), RpcError> {
        let (handle, cancel_requested) = {
            let sessions = self.driver.sessions.lock().await;
            let live = sessions
                .get(session_id)
                .map(|l| (l.handle.clone(), l.cancel_requested.clone()))
                .ok_or_else(|| RpcError::UnknownSession {
                    id: session_id.to_string(),
                })?;
            live
        };
        *cancel_requested.lock().unwrap_or_else(|p| p.into_inner()) = true;
        handle.send(json!({ "type": "abort" })).await?;
        Ok(())
    }
    /// Set a session config option (the model or the thinking level) on a
    /// live session.
    ///
    /// Sends the pi command (`set_model` — the value is parsed as
    /// `"<provider>/<modelId>"`; `set_thinking_level`), then re-synthesizes
    /// the config options from the fresh `get_state` and emits a
    /// `config_option_update` (pi does not emit one itself — the client
    /// owns the frame).
    pub async fn set_config_option(
        &self,
        session_id: &str,
        config_id: &str,
        value: &str,
        sink: &Arc<dyn EventSink>,
    ) -> Result<Vec<Value>, RpcError> {
        let handle = {
            let sessions = self.driver.sessions.lock().await;
            sessions
                .get(session_id)
                .map(|l| l.handle.clone())
                .ok_or_else(|| RpcError::UnknownSession {
                    id: session_id.to_string(),
                })?
        };

        // The config id → the pi command. `model` values are
        // `"<provider>/<modelId>"` (the synthesizer's option values).
        let command = match config_id {
            "model" => {
                let (provider, model_id) =
                    value
                        .split_once('/')
                        .ok_or_else(|| RpcError::InvalidPrompt {
                            reason: format!(
                                "invalid model value: {value} (expected provider/modelId)"
                            ),
                        })?;
                json!({ "type": "set_model", "provider": provider, "modelId": model_id })
            }
            "thought_level" => json!({ "type": "set_thinking_level", "level": value }),
            other => {
                return Err(RpcError::Command {
                    error: format!("unknown config option: {other}"),
                })
            }
        };
        handle.send(command).await?;

        // Re-synthesize + emit (the agent does not emit a
        // `config_option_update` itself).
        let state = handle.send(json!({ "type": "get_state" })).await?;
        let models = handle
            .send(json!({ "type": "get_available_models" }))
            .await
            .ok()
            .and_then(|v| v.get("models").cloned());
        let levels = handle
            .send(json!({ "type": "get_available_thinking_levels" }))
            .await
            .ok()
            .and_then(|v| v.get("levels").cloned());
        let options = synthesize_config_options(&state, models.as_ref(), levels.as_ref())
            .ok_or_else(|| RpcError::Command {
                error: "no config options available".to_string(),
            })?;
        sink.emit(
            "session-update",
            json!({
                "sessionId": session_id,
                "update": { "sessionUpdate": "config_option_update", "configOptions": options },
            }),
        );
        Ok(options)
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
    ) -> Result<bool, RpcError> {
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
    ) -> Result<bool, RpcError> {
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
    /// `session-closed` emit. The handle's `close` is idempotent (the
    /// driver teardown may also call it): closing the child's stdin is the
    /// clean pi shutdown (its `onInputEnd` → exit 0).
    pub async fn close_session(&self, session_id: &str) -> Result<(), RpcError> {
        let (close_tx, close_kind, handle) = {
            let sessions = self.driver.sessions.lock().await;
            let live = sessions
                .get(session_id)
                .map(|l| (l.close_tx.clone(), l.close_kind.clone(), l.handle.clone()))
                .ok_or_else(|| RpcError::UnknownSession {
                    id: session_id.to_string(),
                })?;
            live
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
        close_tx
            .send(true)
            .map_err(|_| RpcError::Io("session already closed".to_string()))?;
        // Close the child's stdin (idempotent — the driver teardown may
        // close it too): a clean pi shutdown.
        handle.close().await;
        Ok(())
    }
}

/// The session's capability envelope (item 1 of the swap plan) built from a
/// `get_state` payload:
///
/// ```json
/// {
///   "piSessionId": "<sessionId>",
///   "piSessionFile": "<sessionFile>",        // ABSENT when get_state omits it
///   "model": "<provider>/<modelId>",        // ABSENT when the model is absent
///   "thinkingLevel": "<level>",
///   "loadSession": <sessionFile was present>,
///   "promptCapabilities": { "image": true, "audio": false, "embeddedContext": false }
/// }
/// ```
///
/// The two trailing keys are load-bearing, not decoration: the frontend's
/// Resume button gates on `loadSession === true` and image sending
/// fail-closes on `promptCapabilities.image === true`.
fn build_capabilities(state: &Value) -> Value {
    let mut caps = json!({
        "piSessionId": state.get("sessionId").cloned().unwrap_or(Value::Null),
        "promptCapabilities": {
            "image": true,
            "audio": false,
            "embeddedContext": false,
        },
    });
    if let Some(file) = state.get("sessionFile") {
        caps["piSessionFile"] = file.clone();
    }
    // `model` is OPTIONAL in `RpcSessionState` (absent for a session with no
    // model — e.g. a fresh `--no-session` run before the first model
    // selection): the key is omitted, not nulled.
    if let Some(model) = state.get("model") {
        let provider = model
            .get("provider")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let id = model.get("id").and_then(Value::as_str).unwrap_or_default();
        caps["model"] = Value::String(format!("{provider}/{id}"));
    }
    if let Some(level) = state.get("thinkingLevel") {
        caps["thinkingLevel"] = level.clone();
    }
    // `loadSession` is the frontend's Resume-button gate: a session with no
    // file (`--no-session`) is unresumable.
    caps["loadSession"] = json!(state.get("sessionFile").is_some());
    caps
}

/// Synthesize the session's config options (the model / thinking-level
/// selectors) from a `get_state` payload + the available models / levels.
///
/// The field names mirror the frontend's `SessionConfigOption` type
/// (`tauri.ts`); the frontend also supports GROUPED options, but the
/// synthesizer emits flat lists only (parity with what the agent
/// advertises). `None` when there is nothing to synthesize (no model AND
/// no levels).
fn synthesize_config_options(
    state: &Value,
    models: Option<&Value>,
    levels: Option<&Value>,
) -> Option<Vec<Value>> {
    let mut out: Vec<Value> = Vec::new();
    if let Some(models) = models.and_then(Value::as_array) {
        // The current model is `"<provider>/<modelId>"` (the option value
        // form); absent → no model selector (nothing to select).
        if let Some(current) = state.get("model").and_then(|m| {
            m.get("provider")
                .and_then(Value::as_str)
                .zip(m.get("id").and_then(Value::as_str))
                .map(|(p, i)| format!("{p}/{i}"))
        }) {
            let options: Vec<Value> = models
                .iter()
                .filter_map(|m| {
                    m.get("provider")
                        .and_then(Value::as_str)
                        .zip(m.get("id").and_then(Value::as_str))
                        .map(|(p, i)| {
                            json!({
                                "value": format!("{p}/{i}"),
                                "name": m.get("name").and_then(Value::as_str).unwrap_or(i),
                            })
                        })
                })
                .collect();
            out.push(json!({
                "id": "model",
                "name": "Model",
                "category": "model",
                "type": "select",
                "currentValue": current,
                "options": options,
            }));
        }
    }
    if let Some(levels) = levels.and_then(Value::as_array) {
        let current = state
            .get("thinkingLevel")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let options: Vec<Value> = levels
            .iter()
            .filter_map(|l| l.as_str().map(|s| s.to_string()))
            .map(|s| {
                // The display name is the capitalized level
                // (`"medium"` → `"Medium"`).
                let mut name = s.clone();
                if let Some(c0) = name.chars().next() {
                    name = format!("{}{}", c0.to_uppercase(), &name[c0.len_utf8()..]);
                }
                json!({ "value": s, "name": name })
            })
            .collect();
        out.push(json!({
            "id": "thought_level",
            "name": "Thinking",
            "category": "thought_level",
            "type": "select",
            "currentValue": current,
            "options": options,
        }));
    }
    (!out.is_empty()).then_some(out)
}

/// Map one pi event onto the FROZEN `session-update` JSON the frontend
/// consumes (the ACP-era envelope shapes — `agent_message_chunk` /
/// `agent_thought_chunk` / `tool_call` / `tool_call_update` /
/// `session_info_update` / `config_option_update`). A pure function (no
/// I/O): the driver feeds it the event + the session's `TurnState` and
/// emits / persists the frames it returns.
///
/// No-frame events are bookkeeping (`agent_start` / `agent_end` /
/// `turn_start` / `turn_end` / `queue_update` / `entry_appended` /
/// `bash_execution_update` / `message_end` / `text_end` / `thinking_end` —
/// the delta stream already delivered the content, and the authoritative
/// `message_end` text is NOT re-emitted) or the turn's resolution signal
/// (`agent_settled` — the driver resolves the pending turn, not a frame).
fn normalize(e: &RpcEvent, st: &mut TurnState) -> Vec<Value> {
    match e {
        // `message_start` carries ANY `AgentMessage` (user / assistant /
        // toolResult): advance the counter ONLY for `assistant` messages
        // (a role-blind counter would number the first assistant chunk
        // `m2` and make the `get_messages` replay numbering irreproducible).
        RpcEvent::message_start { message } => {
            if message.get("role").and_then(Value::as_str) == Some("assistant") {
                st.msg_counter += 1;
                st.current_message_id = Some(format!("m{}", st.msg_counter));
            }
            Vec::new()
        }
        RpcEvent::message_update {
            assistant_message_event: ev,
            ..
        } => {
            let mid = st
                .current_message_id
                .clone()
                .unwrap_or_else(|| "default".to_string());
            match ev.get("type").and_then(Value::as_str) {
                Some("text_delta") => vec![json!({
                    "sessionUpdate": "agent_message_chunk",
                    "content": { "type": "text", "text": ev.get("delta") },
                    "messageId": mid,
                })],
                Some("thinking_delta") => vec![json!({
                    "sessionUpdate": "agent_thought_chunk",
                    "content": { "type": "text", "text": ev.get("delta") },
                    "messageId": mid,
                })],
                // The `*_end` / `*_start` frames carry the authoritative
                // (already streamed) content — no frame (re-emitting would
                // double the text).
                Some("text_start")
                | Some("text_end")
                | Some("thinking_start")
                | Some("thinking_end") => Vec::new(),
                Some("toolcall_start") => vec![json!({
                    "sessionUpdate": "tool_call",
                    "toolCallId": ev.get("id"),
                    "title": ev.get("toolName"),
                    "status": "in_progress",
                    "rawInput": {},
                })],
                // Accumulate the partial-args JSON fragments; a complete
                // object is sent as `rawInput`, an incomplete one as
                // `partialArgs` (the adapter-era behavior, kept for the
                // streaming tool-call frames).
                Some("toolcall_delta") => {
                    let Some(id) = ev.get("id").and_then(Value::as_str) else {
                        return Vec::new();
                    };
                    let delta = ev.get("delta").and_then(Value::as_str).unwrap_or_default();
                    st.toolcall_args
                        .entry(id.to_string())
                        .or_default()
                        .push_str(delta);
                    let acc = st.toolcall_args.get(id).unwrap();
                    match serde_json::from_str::<Value>(acc) {
                        Ok(v) => vec![json!({
                            "sessionUpdate": "tool_call_update",
                            "toolCallId": id,
                            "rawInput": v,
                        })],
                        Err(_) => vec![json!({
                            "sessionUpdate": "tool_call_update",
                            "toolCallId": id,
                            "partialArgs": acc,
                        })],
                    }
                }
                // The full `arguments` object (the wire `toolCall` field —
                // `{id, name, arguments}`); clear the partial-args buffer.
                Some("toolcall_end") => {
                    let Some(tc) = ev.get("toolCall") else {
                        return Vec::new();
                    };
                    let Some(id) = tc.get("id").and_then(Value::as_str) else {
                        return Vec::new();
                    };
                    st.toolcall_args.remove(id);
                    vec![json!({
                        "sessionUpdate": "tool_call_update",
                        "toolCallId": id,
                        "rawInput": tc.get("arguments"),
                        "status": "in_progress",
                    })]
                }
                _ => Vec::new(),
            }
        }
        RpcEvent::tool_execution_start { tool_call_id, .. } => vec![json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": tool_call_id,
            "status": "in_progress",
        })],
        // `rawOutput` is persisted-only (the frontend's `AcpSessionUpdate`
        // has no `rawOutput` field — it lands in the persisted payload via
        // `merge_json` and is harmless).
        RpcEvent::tool_execution_update {
            tool_call_id,
            partial_result,
            ..
        } => vec![json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": tool_call_id,
            "rawOutput": partial_result,
        })],
        RpcEvent::tool_execution_end {
            tool_call_id,
            result,
            is_error,
            ..
        } => vec![json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": tool_call_id,
            "status": if *is_error { "failed" } else { "completed" },
            "rawOutput": result,
        })],
        RpcEvent::session_info_changed { name } => vec![json!({
            "sessionUpdate": "session_info_update",
            "title": name,
        })],
        // Bookkeeping / de-structured one-liners (the adapter's parity; the
        // strings are stable for tests). They carry a `messageId` of
        // `"system"` (a dedicated key — never mixed into a real message's
        // accumulated text).
        RpcEvent::compaction_start { .. } => vec![system_chunk("Compacting context…")],
        RpcEvent::compaction_end { .. } => vec![system_chunk("Compaction finished")],
        RpcEvent::auto_retry_start {
            attempt,
            max_attempts,
            ..
        } => vec![system_chunk(format!(
            "Retrying (attempt {attempt}/{max_attempts}…)"
        ))],
        RpcEvent::auto_retry_end { success, .. } => {
            vec![if *success {
                system_chunk("Retry succeeded")
            } else {
                system_chunk("Retry failed")
            }]
        }
        RpcEvent::extension_error { error, .. } => {
            vec![system_chunk(format!("Extension error: {error}"))]
        }
        // Bookkeeping (no frame): the turn's resolution signal
        // (`agent_settled` — the driver resolves the pending turn), the
        // turn / message boundaries, the queue / entry / bash bookkeeping,
        // and unknown event types (permissive — debug-logged by the
        // reader, never an error).
        _ => Vec::new(),
    }
}

/// A one-line `agent_message_chunk` with the dedicated `"system"` messageId
/// (the bookkeeping one-liners — never mixed into a real message's
/// accumulated text).
fn system_chunk(text: impl Into<String>) -> Value {
    let text = text.into();
    json!({
        "sessionUpdate": "agent_message_chunk",
        "content": { "type": "text", "text": text },
        "messageId": "system",
    })
}

/// Replay a stored transcript (`get_messages` payload) through the
/// normalizer (the resume establisher's persistence step).
///
/// Per the replay-feed rule, the replay synthesizes the DELTA events the
/// live stream would have produced (a literal replay fed as
/// `message_end` would produce ZERO frames — the no-re-emit rule is
/// live-stream-only, and `message_end` is never fed):
///
/// - an **assistant** message → a `message_start` (advancing the counter
///   per the role rule) + one `message_update` per content block: a
///   `text_delta` with the block's full text as a SINGLE delta, a
///   `thinking_delta` per thinking block, and `toolcall_start` +
///   `toolcall_end` (arguments = the block's `arguments` object) per
///   toolCall. No `message_end` / `text_end` / `thinking_end`.
/// - a **toolResult** message → a `tool_execution_end` (`toolCallId` from
///   the message's own `toolCallId`, `result` = the content, `isError`
///   from the message).
/// - a **user** message → persisted DIRECTLY as a `kind: "user"` row with
///   the `{ "text": …, "images": […]? }` payload shape (the normalizer has
///   no user-message input) — an explicit improvement over the ACP replay,
///   which dropped user rows.
fn replay_messages(db: &Db, session_id: &str, messages: &Value) {
    let Some(msgs) = messages.get("messages").and_then(Value::as_array) else {
        return;
    };
    let mut turn = TurnState::default();
    let text_acc = StdMutex::new(HashMap::new());
    let tool_state = StdMutex::new(HashMap::new());
    let thought = StdMutex::new(ThoughtState::default());

    for msg in msgs {
        let role = msg.get("role").and_then(Value::as_str).unwrap_or_default();
        match role {
            "user" => {
                let payload = user_replay_payload(msg.get("content").unwrap_or(&Value::Null));
                let _ = db.record_message(session_id, "user", None, &payload.to_string());
            }
            "assistant" => {
                // The `message_start` (advances the counter per the role
                // rule — the replay uses the SAME rule in message order so
                // the live and replay `messageId`s match).
                let start: RpcEvent =
                    serde_json::from_value(json!({ "type": "message_start", "message": msg }))
                        .expect("message_start is always parseable");
                let _ = normalize(&start, &mut turn);
                for block in msg
                    .get("content")
                    .and_then(Value::as_array)
                    .unwrap_or(&Vec::new())
                {
                    let block_type = block
                        .get("type")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let update = match block_type {
                        "text" => json!({
                            "type": "message_update",
                            "usage": null,
                            "assistantMessageEvent": {
                                "type": "text_delta",
                                "contentIndex": 0,
                                "delta": block.get("text"),
                            },
                        }),
                        "thinking" => json!({
                            "type": "message_update",
                            "usage": null,
                            "assistantMessageEvent": {
                                "type": "thinking_delta",
                                "contentIndex": 0,
                                "delta": block.get("thinking"),
                            },
                        }),
                        "toolCall" => {
                            // `toolcall_start` + `toolcall_end` (the block IS
                            // the wire `toolCall` object: `{id, name,
                            // arguments}`).
                            let start: RpcEvent = serde_json::from_value(json!({
                                "type": "message_update",
                                "usage": null,
                                "assistantMessageEvent": {
                                    "type": "toolcall_start",
                                    "contentIndex": 0,
                                    "id": block.get("id"),
                                    "toolName": block.get("name"),
                                },
                            }))
                            .expect("toolcall_start is always parseable");
                            let end: RpcEvent = serde_json::from_value(json!({
                                "type": "message_update",
                                "usage": null,
                                "assistantMessageEvent": { "type": "toolcall_end", "toolCall": block },
                            }))
                            .expect("toolcall_end is always parseable");
                            for u in normalize(&start, &mut turn) {
                                persist_update(
                                    db,
                                    session_id,
                                    &u,
                                    &text_acc,
                                    &tool_state,
                                    &thought,
                                );
                            }
                            for u in normalize(&end, &mut turn) {
                                persist_update(
                                    db,
                                    session_id,
                                    &u,
                                    &text_acc,
                                    &tool_state,
                                    &thought,
                                );
                            }
                            continue;
                        }
                        _ => continue,
                    };
                    let ev: RpcEvent =
                        serde_json::from_value(update).expect("message_update is always parseable");
                    for u in normalize(&ev, &mut turn) {
                        persist_update(db, session_id, &u, &text_acc, &tool_state, &thought);
                    }
                }
                // NO `message_end` / `text_end` / `thinking_end` (the
                // authoritative content is not re-emitted).
            }
            "toolResult" => {
                let ev: RpcEvent = serde_json::from_value(json!({
                    "type": "tool_execution_end",
                    "toolCallId": msg.get("toolCallId"),
                    "toolName": msg.get("toolName"),
                    "result": msg.get("content"),
                    "isError": msg.get("isError").cloned().unwrap_or(Value::Bool(false)),
                }))
                .expect("tool_execution_end is always parseable");
                for u in normalize(&ev, &mut turn) {
                    persist_update(db, session_id, &u, &text_acc, &tool_state, &thought);
                }
            }
            _ => {}
        }
    }
}

/// The `kind: "user"` replay payload from a stored user message's content
/// (`string | (TextContent | ImageContent)[]`): `{ "text": … }` (the
/// `images` key omitted when absent) — the same shape `user_message_payload`
/// writes at `send_prompt` time (the frontend's `rowToMessages` reads it).
/// A stored image block is `{type, data, mimeType}` (no `name` /
/// `sizeBytes` on the wire — they are omitted).
fn user_replay_payload(content: &Value) -> Value {
    if let Some(s) = content.as_str() {
        return json!({ "text": s });
    }
    let mut text = String::new();
    let mut images: Vec<Value> = Vec::new();
    for block in content.as_array().unwrap_or(&Vec::new()) {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                text.push_str(
                    block
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                );
            }
            Some("image") => images.push(json!({
                "mimeType": block.get("mimeType"),
                "data": block.get("data"),
            })),
            _ => {}
        }
    }
    if images.is_empty() {
        json!({ "text": text })
    } else {
        json!({ "text": text, "images": images })
    }
}

/// Normalize a stored `capabilities_json` before it reaches the frontend
/// (the `list_sessions` path — NOT `load_history`, which returns raw
/// `MessageRow`s and never touches `capabilities_json`): an envelope that
/// (a) fails to parse as JSON, or (b) parses but lacks the `loadSession`
/// key (a pre-swap ACP row) is normalized to `loadSession: false` — so a
/// legacy row shows NO Resume button (the history-only banner is a
/// FRONTEND-side decision driven by `capabilities.loadSession`; there is no
/// error-kind matching in the frontend, so the normalized capabilities are
/// the honest path).
pub fn normalize_capabilities(raw: &str) -> Value {
    let v: Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(_) => return json!({ "loadSession": false }),
    };
    if v.get("loadSession").is_some() {
        v
    } else {
        json!({ "loadSession": false })
    }
}

/// Persist one `session-update` (the FROZEN JSON shapes the normalizer
/// emits) into the transcript.
///
/// The persistence semantics are the ACP-era ones, unchanged: upsert keys
/// (`(session_id, kind, message_key)`), agent-text accumulation per
/// `messageId`, thought segmentation (`{messageId}#{segment}` boundaries),
/// tool-call `merge_json` shallow-merge. (The ACP `content:
/// ToolCallContent[]` diff channel has no RPC input in Phase 1 — pi's tool
/// results carry no diff-structured content — so the `has_diff` branch is
/// gone with the crate types.)
fn persist_update(
    db: &Db,
    session_id: &str,
    update: &Value,
    agent_text_acc: &StdMutex<HashMap<String, String>>,
    tool_call_state: &StdMutex<HashMap<String, Value>>,
    thought_state: &StdMutex<ThoughtState>,
) {
    let kind = update.get("sessionUpdate").and_then(Value::as_str);
    match kind {
        Some("agent_thought_chunk") => {
            let Some(text) = update
                .get("content")
                .and_then(|c| c.get("text"))
                .and_then(Value::as_str)
            else {
                return;
            };
            if text.is_empty() {
                return;
            }
            let key = update
                .get("messageId")
                .and_then(Value::as_str)
                .unwrap_or("default")
                .to_string();
            let mut state = thought_state.lock().expect("thought state poisoned");
            if state.open_key.as_deref() != Some(key.as_str()) {
                // New thinking segment: a new messageId, or an intervening
                // segmenting update (see the rule above) cleared `open_key`.
                state.next += 1;
                state.open_key = Some(key);
                state.current = String::new();
            }
            state.current.push_str(text);
            let row_key = format!("{}#{}", state.open_key.as_ref().unwrap(), state.next);
            let payload = json!({ "text": state.current });
            let _ = db.record_message(
                session_id,
                "agent-thought",
                Some(&row_key),
                &payload.to_string(),
            );
        }
        Some("agent_message_chunk") => {
            let Some(text) = update
                .get("content")
                .and_then(|c| c.get("text"))
                .and_then(Value::as_str)
            else {
                return;
            };
            if text.is_empty() {
                return;
            }
            thought_state
                .lock()
                .expect("thought state poisoned")
                .open_key = None;
            let key = update
                .get("messageId")
                .and_then(Value::as_str)
                .unwrap_or("default")
                .to_string();
            let mut acc = agent_text_acc
                .lock()
                .expect("agent-text accumulator poisoned");
            let entry = acc.entry(key.clone()).or_default();
            entry.push_str(text);
            let payload = json!({ "text": entry });
            let _ = db.record_message(session_id, "agent-text", Some(&key), &payload.to_string());
        }
        Some("tool_call") => {
            let Some(key) = update
                .get("toolCallId")
                .and_then(Value::as_str)
                .map(str::to_string)
            else {
                return;
            };
            thought_state
                .lock()
                .expect("thought state poisoned")
                .open_key = None;
            let mut state = tool_call_state.lock().expect("tool-call state poisoned");
            state.insert(key.clone(), update.clone());
            let _ = db.record_message(
                session_id,
                "tool-call",
                Some(&key),
                &state[&key].to_string(),
            );
        }
        Some("tool_call_update") => {
            let Some(key) = update
                .get("toolCallId")
                .and_then(Value::as_str)
                .map(str::to_string)
            else {
                return;
            };
            let mut state = tool_call_state.lock().expect("tool-call state poisoned");
            let is_new = !state.contains_key(&key);
            let entry = state
                .entry(key.clone())
                .or_insert_with(|| Value::Object(Default::default()));
            merge_json(entry, update);
            let _ = db.record_message(session_id, "tool-call", Some(&key), &entry.to_string());
            drop(state);
            // A first-seen tool-call update segments the thought stream
            // (the same rule as the ACP `has_diff` branch — a new tool
            // result interrupts the run).
            if is_new {
                thought_state
                    .lock()
                    .expect("thought state poisoned")
                    .open_key = None;
            }
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

/// Build a remediation hint for a spawn failure, mentioning the pi install
/// path.
fn spawn_hint(command: &str) -> String {
    format!(
        "could not spawn '{}'. If this is the 'pi' agent, make sure `pi` is \
         installed and on PATH (e.g. `npm install -g \
         @earendil-works/pi-coding-agent`), then retry.",
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
mod normalize_tests {
    use serde_json::json;

    use super::{build_capabilities, normalize, synthesize_config_options, TurnState};
    use crate::agent::rpc::RpcEvent;

    fn ev(v: serde_json::Value) -> RpcEvent {
        serde_json::from_value(v).expect("event should parse")
    }

    /// The `messageId` role rule: a USER `message_start` does NOT advance
    /// the counter; the following ASSISTANT `message_start` numbers the
    /// first assistant chunk `m1` (a role-blind counter would make it
    /// `m2` and break the replay numbering).
    #[test]
    fn message_id_advances_only_for_assistant_messages() {
        let mut st = TurnState::default();
        let user = ev(json!({
            "type": "message_start",
            "message": { "role": "user", "content": [{ "type": "text", "text": "hi" }] },
        }));
        assert!(normalize(&user, &mut st).is_empty());
        assert_eq!(
            st.msg_counter, 0,
            "a user message_start must NOT advance the counter"
        );
        assert!(st.current_message_id.is_none());

        let assistant = ev(json!({
            "type": "message_start",
            "message": { "role": "assistant", "content": [] },
        }));
        assert!(normalize(&assistant, &mut st).is_empty());
        assert_eq!(st.msg_counter, 1);
        assert_eq!(st.current_message_id.as_deref(), Some("m1"));

        let delta = ev(json!({
            "type": "message_update",
            "usage": null,
            "assistantMessageEvent": { "type": "text_delta", "contentIndex": 0, "delta": "Hel" },
        }));
        let frames = normalize(&delta, &mut st);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0]["sessionUpdate"], "agent_message_chunk");
        assert_eq!(
            frames[0]["messageId"], "m1",
            "the first assistant chunk keys m1"
        );
        assert_eq!(frames[0]["content"]["text"], "Hel");
    }

    /// A second assistant message advances the counter to `m2`; the
    /// `*_end` / `*_start` frames carry NO frame (the delta stream already
    /// delivered the content — re-emitting would double the text).
    #[test]
    fn end_and_start_frames_emit_nothing() {
        let mut st = TurnState::default();
        let _ = normalize(
            &ev(json!({ "type": "message_start", "message": { "role": "assistant" } })),
            &mut st,
        );
        let _ = normalize(
            &ev(json!({ "type": "message_start", "message": { "role": "assistant" } })),
            &mut st,
        );
        assert_eq!(st.current_message_id.as_deref(), Some("m2"));

        for t in ["text_end", "thinking_end", "text_start", "thinking_start"] {
            let frames = normalize(
                &ev(json!({
                    "type": "message_update",
                    "usage": null,
                    "assistantMessageEvent": { "type": t, "contentIndex": 0, "content": "x", "delta": "x" },
                })),
                &mut st,
            );
            assert!(frames.is_empty(), "{t} must emit no frame");
        }
    }

    /// `thinking_delta` → `agent_thought_chunk` (the same `messageId` keying
    /// as text deltas).
    #[test]
    fn thinking_delta_maps_to_agent_thought_chunk() {
        let mut st = TurnState::default();
        let _ = normalize(
            &ev(json!({ "type": "message_start", "message": { "role": "assistant" } })),
            &mut st,
        );
        let frames = normalize(
            &ev(json!({
                "type": "message_update",
                "usage": null,
                "assistantMessageEvent": { "type": "thinking_delta", "contentIndex": 0, "delta": "hmm" },
            })),
            &mut st,
        );
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0]["sessionUpdate"], "agent_thought_chunk");
        assert_eq!(frames[0]["content"]["text"], "hmm");
        assert_eq!(frames[0]["messageId"], "m1");
    }

    /// The tool-call frame sequence: `toolcall_start` → `tool_call` (empty
    /// `rawInput`), `toolcall_delta` → `tool_call_update` (partial args as
    /// `partialArgs` while incomplete, `rawInput` once the accumulated
    /// string parses as JSON), `toolcall_end` → `tool_call_update` with the
    /// full `arguments` object + the buffer cleared.
    #[test]
    fn toolcall_frames_accumulate_args() {
        let mut st = TurnState::default();
        let _ = normalize(
            &ev(json!({ "type": "message_start", "message": { "role": "assistant" } })),
            &mut st,
        );

        let start = normalize(
            &ev(json!({
                "type": "message_update",
                "usage": null,
                "assistantMessageEvent": { "type": "toolcall_start", "contentIndex": 0, "id": "tc1", "toolName": "bash" },
            })),
            &mut st,
        );
        assert_eq!(start[0]["sessionUpdate"], "tool_call");
        assert_eq!(start[0]["toolCallId"], "tc1");
        assert_eq!(start[0]["title"], "bash");
        assert_eq!(start[0]["rawInput"], json!({}));

        // An incomplete JSON fragment → `partialArgs` (the adapter's
        // behavior — the frontend shows the raw string while parsing).
        let partial = normalize(
            &ev(json!({
                "type": "message_update",
                "usage": null,
                "assistantMessageEvent": { "type": "toolcall_delta", "contentIndex": 0, "id": "tc1", "delta": "{\"cmd\":" },
            })),
            &mut st,
        );
        assert_eq!(partial[0]["partialArgs"], "{\"cmd\":");
        assert!(partial[0].get("rawInput").is_none());

        // The fragment completes → `rawInput` (the accumulated string
        // parses as JSON).
        let done = normalize(
            &ev(json!({
                "type": "message_update",
                "usage": null,
                "assistantMessageEvent": { "type": "toolcall_delta", "contentIndex": 0, "id": "tc1", "delta": "\"ls\"}" },
            })),
            &mut st,
        );
        assert_eq!(done[0]["rawInput"], json!({ "cmd": "ls" }));

        // `toolcall_end` → the full `arguments` object + the buffer
        // cleared (a later delta for the same id starts fresh).
        let end = normalize(
            &ev(json!({
                "type": "message_update",
                "usage": null,
                "assistantMessageEvent": {
                    "type": "toolcall_end",
                    "toolCall": { "id": "tc1", "name": "bash", "arguments": { "cmd": "ls" } },
                },
            })),
            &mut st,
        );
        assert_eq!(end[0]["rawInput"], json!({ "cmd": "ls" }));
        assert_eq!(end[0]["status"], "in_progress");
        assert!(
            !st.toolcall_args.contains_key("tc1"),
            "the buffer must be cleared"
        );
    }

    /// `tool_execution_*` → `tool_call_update` (in_progress / partial
    /// `rawOutput` / completed-or-failed + `rawOutput`).
    #[test]
    fn tool_execution_frames_map_to_updates() {
        let mut st = TurnState::default();
        let start = normalize(
            &ev(
                json!({ "type": "tool_execution_start", "toolCallId": "tc1", "toolName": "bash", "args": {} }),
            ),
            &mut st,
        );
        assert_eq!(
            start[0],
            json!({ "sessionUpdate": "tool_call_update", "toolCallId": "tc1", "status": "in_progress" })
        );

        let update = normalize(
            &ev(
                json!({ "type": "tool_execution_update", "toolCallId": "tc1", "toolName": "bash", "args": {}, "partialResult": "out" }),
            ),
            &mut st,
        );
        assert_eq!(update[0]["rawOutput"], "out");

        let end_ok = normalize(
            &ev(
                json!({ "type": "tool_execution_end", "toolCallId": "tc1", "toolName": "bash", "result": "done", "isError": false }),
            ),
            &mut st,
        );
        assert_eq!(end_ok[0]["status"], "completed");
        assert_eq!(end_ok[0]["rawOutput"], "done");

        let end_err = normalize(
            &ev(
                json!({ "type": "tool_execution_end", "toolCallId": "tc1", "toolName": "bash", "result": "boom", "isError": true }),
            ),
            &mut st,
        );
        assert_eq!(end_err[0]["status"], "failed");
    }

    /// The bookkeeping one-liners (stable strings for tests) + the
    /// no-frame bookkeeping events (`agent_settled` / `turn_end` / …).
    #[test]
    fn bookkeeping_events_and_one_liners() {
        let mut st = TurnState::default();
        let compacting = normalize(
            &ev(json!({ "type": "compaction_start", "reason": "auto" })),
            &mut st,
        );
        assert_eq!(compacting[0]["content"]["text"], "Compacting context…");
        assert_eq!(
            compacting[0]["messageId"], "system",
            "the one-liners key the dedicated system id"
        );

        let done = normalize(
            &ev(
                json!({ "type": "compaction_end", "reason": "auto", "aborted": false, "willRetry": false }),
            ),
            &mut st,
        );
        assert_eq!(done[0]["content"]["text"], "Compaction finished");

        let retry = normalize(
            &ev(
                json!({ "type": "auto_retry_start", "attempt": 1, "maxAttempts": 3, "delayMs": 1000, "errorMessage": "x" }),
            ),
            &mut st,
        );
        assert_eq!(retry[0]["content"]["text"], "Retrying (attempt 1/3…)");

        for t in [
            "agent_start",
            "agent_end",
            "agent_settled",
            "turn_start",
            "queue_update",
            "bash_execution_update",
        ] {
            let v = match t {
                "turn_start" => json!({ "type": t }),
                "agent_end" => json!({ "type": t, "messages": [], "willRetry": false }),
                _ => {
                    json!({ "type": t, "toolCallId": "x", "toolName": "bash", "args": {}, "partialResult": "p", "result": "r", "isError": false, "steering": [], "followUp": [], "entry": {}, "id": "x", "delta": "d", "message": { "role": "assistant" }, "toolResults": [] })
                }
            };
            assert!(
                normalize(&ev(v), &mut st).is_empty(),
                "{t} must emit no frame"
            );
        }
    }

    /// The capability envelope: `piSessionFile` / `model` keys ABSENT when
    /// `get_state` omits them; `loadSession: false` when the session file is
    /// absent; the load-bearing `promptCapabilities` always present.
    #[test]
    fn capabilities_envelope_shape() {
        let full = build_capabilities(&json!({
            "sessionId": "s1",
            "sessionFile": "/tmp/s1.jsonl",
            "model": { "provider": "fake", "id": "m1", "name": "M1" },
            "thinkingLevel": "medium",
        }));
        assert_eq!(full["piSessionId"], "s1");
        assert_eq!(full["piSessionFile"], "/tmp/s1.jsonl");
        assert_eq!(full["model"], "fake/m1");
        assert_eq!(full["thinkingLevel"], "medium");
        assert_eq!(full["loadSession"], true);
        assert_eq!(
            full["promptCapabilities"],
            json!({ "image": true, "audio": false, "embeddedContext": false })
        );

        // A `--no-session` run (no sessionFile, no model): the keys are
        // ABSENT (not nulled) and the session is unresumable.
        let bare = build_capabilities(&json!({ "sessionId": "s2" }));
        assert!(bare.get("piSessionFile").is_none(), "no file → key absent");
        assert!(bare.get("model").is_none(), "no model → key absent");
        assert_eq!(bare["loadSession"], false, "no file → unresumable");
    }

    /// The config synthesizer: model + thinking selectors (the flat shape
    /// the frontend's `SessionConfigOption` reads); `None` when there is
    /// nothing to synthesize.
    #[test]
    fn config_synthesizer_shape() {
        let state = json!({
            "model": { "provider": "fake", "id": "m1" },
            "thinkingLevel": "medium",
        });
        let models = json!([{ "provider": "fake", "id": "m1", "name": "M1" }, { "provider": "fake", "id": "m2", "name": "M2" }]);
        let levels = json!(["off", "medium"]);
        let opts = synthesize_config_options(&state, Some(&models), Some(&levels)).unwrap();
        assert_eq!(opts.len(), 2);
        assert_eq!(opts[0]["id"], "model");
        assert_eq!(opts[0]["currentValue"], "fake/m1");
        assert_eq!(opts[0]["options"].as_array().unwrap().len(), 2);
        assert_eq!(opts[1]["id"], "thought_level");
        assert_eq!(opts[1]["currentValue"], "medium");
        assert_eq!(
            opts[1]["options"][0]["name"], "Off",
            "levels are capitalized"
        );

        // No model AND no levels → `None`.
        assert!(synthesize_config_options(&json!({}), None, None).is_none());
    }
}

#[cfg(test)]
mod session_tests {
    use super::*;
    use crate::agent::permission::PermissionOutcome;
    use crate::storage::Db;
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

    /// The `fake_pi` binary path. These tests spawn it DIRECTLY (no copy):
    /// they never reap processes by binary path (the driver kills via the
    /// child handle `PiRpc` owns), so a shared path cannot false-positive —
    /// and a copy races the kernel's ETXTBSY check (the copy's write-fd
    /// can still be in flight when the forked child execs).
    fn fake_pi_bin() -> PathBuf {
        PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/target/debug/fake_pi"))
    }

    /// Write an `agents.json` with a single `fake` entry pointing at
    /// `fake_pi` + the given mode env vars (e.g. `FAKE_PI_PROMPT=1`).
    fn write_agents_json_pi(dir: &Path, env: &[(&str, &str)]) {
        // `env` is a JSON OBJECT (a `BTreeMap` on the wire) — the slice
        // form would serialize as an array of pairs and fail to
        // deserialize.
        let env_map: serde_json::Map<String, serde_json::Value> = env
            .iter()
            .map(|(k, v)| (k.to_string(), serde_json::Value::String(v.to_string())))
            .collect();
        let agents = serde_json::json!({
            "agents": [{
                "id": "fake",
                "name": "Fake Pi",
                "command": fake_pi_bin(),
                "args": [],
                "env": env_map,
            }]
        });
        std::fs::write(dir.join("agents.json"), agents.to_string()).unwrap();
    }

    fn open_db(dir: &Path) -> std::sync::Arc<Db> {
        std::sync::Arc::new(Db::open(&dir.join("archimedes.db")).expect("db should open"))
    }

    /// (start) `start_session` + `send_prompt` against `fake_pi`
    /// (`FAKE_PI_PROMPT=1`): the turn streams 2 `agent_message_chunk`
    /// frames keyed `m1` (the `messageId` role rule — the user
    /// `message_start` does not advance the counter), resolves `EndTurn`,
    /// persists ONE accumulated `agent-text` row ("Hello"), and stores a
    /// `capabilities_json` carrying the load-bearing keys
    /// (`piSessionFile` + `loadSession: true` + `promptCapabilities.image:
    /// true`).
    #[tokio::test]
    async fn start_session_streams_and_persists() {
        let dir = temp_config_dir();
        write_agents_json_pi(&dir, &[("FAKE_PI_PROMPT", "1")]);
        let db = open_db(&dir);
        let mut manager = SessionManager::new(dir.clone()).unwrap();
        manager.attach_db(db.clone());
        let (tx, mut rx) = mpsc::unbounded_channel();
        let sink: Arc<dyn EventSink> = Arc::new(TestSink { tx });

        let info = crate::test_support::run_with_retry(|| {
            manager.start_session("fake", dir.clone(), &sink)
        })
        .await
        .unwrap();

        // The capability envelope (item 1 — the load-bearing keys). The
        // session id is UNIQUE per process (mirroring real pi — the fake
        // derives it from the process's time + pid).
        assert!(
            info.session_id.starts_with("fake-pi-"),
            "a fresh session gets a unique pi id, got {}",
            info.session_id
        );
        assert_eq!(
            info.capabilities["piSessionId"],
            Value::String(info.session_id.clone())
        );
        assert_eq!(
            info.capabilities["piSessionFile"],
            "/tmp/fake-pi-session.jsonl"
        );
        assert_eq!(info.capabilities["model"], "fake/fake-model");
        assert_eq!(info.capabilities["loadSession"], true);
        assert_eq!(
            info.capabilities["promptCapabilities"]["image"], true,
            "the image-send gate key must be present (fail-closed)"
        );
        // The config options (the synthesizer — 2 selectors).
        assert_eq!(info.config_options.as_ref().unwrap().len(), 2);

        // The turn: 2 `agent_message_chunk` frames keyed `m1`, then
        // `EndTurn`.
        let reason = manager
            .send_prompt(&info.session_id, "hi".to_string())
            .await
            .unwrap();
        assert_eq!(reason, StopReason::EndTurn);

        let mut chunks: Vec<Value> = Vec::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while std::time::Instant::now() < deadline && chunks.len() < 2 {
            if let Ok(msg) = tokio::time::timeout(Duration::from_millis(500), rx.recv()).await {
                let msg = msg.unwrap();
                if msg["event"] == "session-update" {
                    let update = &msg["payload"]["update"];
                    if update["sessionUpdate"] == "agent_message_chunk" {
                        chunks.push(update.clone());
                    }
                }
            }
        }
        assert_eq!(chunks.len(), 2, "two text deltas (Hel + lo)");
        assert_eq!(
            chunks[0]["messageId"], "m1",
            "the role rule keys the first assistant chunk m1"
        );
        assert_eq!(chunks[1]["messageId"], "m1");
        assert_eq!(chunks[0]["content"]["text"], "Hel");
        assert_eq!(chunks[1]["content"]["text"], "lo");

        // Persistence: ONE `agent-text` row with the accumulated "Hello" +
        // the `user` row written by `send_prompt`.
        let rows = db
            .messages_for(&info.session_id)
            .expect("messages_for should work");
        let agent_rows: Vec<_> = rows.iter().filter(|r| r.kind == "agent-text").collect();
        assert_eq!(agent_rows.len(), 1, "two chunks, one messageId → one row");
        assert_eq!(agent_rows[0].message_key.as_deref(), Some("m1"));
        assert_eq!(agent_rows[0].payload_json, r#"{"text":"Hello"}"#);
        let user_rows: Vec<_> = rows.iter().filter(|r| r.kind == "user").collect();
        assert_eq!(user_rows.len(), 1, "exactly ONE user row (no double write)");
        assert_eq!(user_rows[0].payload_json, r#"{"text":"hi"}"#);

        // The stored `capabilities_json` (the sessions row).
        let stored = db
            .session(&info.session_id)
            .expect("session should work")
            .unwrap();
        let stored_caps: Value = serde_json::from_str(&stored.capabilities_json).unwrap();
        assert_eq!(stored_caps["loadSession"], true);
        assert_eq!(stored_caps["promptCapabilities"]["image"], true);
        assert_eq!(stored_caps["piSessionFile"], "/tmp/fake-pi-session.jsonl");

        let _ = manager.close_session(&info.session_id).await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// (resume) `resume_session` against a pre-seeded stored row
    /// (`capabilities_json` = the item-1 shape with a `piSessionFile`):
    /// the stored rows are cleared then re-populated from `get_messages` —
    /// 2+ rows, including a `kind: "user"` row (the explicit improvement
    /// over the ACP replay, which dropped user rows) and an accumulated
    /// `agent-text` row keyed by the SAME `messageId` rule. A legacy row
    /// WITHOUT `loadSession` → `list_sessions`-normalized capabilities have
    /// `loadSession: false` (item 6b) and `resume_session` →
    /// `NotResumable`.
    #[tokio::test]
    async fn resume_replays_messages() {
        let dir = temp_config_dir();
        write_agents_json_pi(&dir, &[]);
        let db = open_db(&dir);
        let mut manager = SessionManager::new(dir.clone()).unwrap();
        manager.attach_db(db.clone());
        let (tx, _rx) = mpsc::unbounded_channel();
        let sink: Arc<dyn EventSink> = Arc::new(TestSink { tx });

        // Pre-seed the stored row: the item-1 capability envelope (a
        // resumption of a session with a file).
        db.record_session(&SessionInfo {
            session_id: "resume-1".to_string(),
            agent_id: "fake".to_string(),
            cwd: dir.clone(),
            capabilities: serde_json::json!({
                "piSessionId": "resume-1",
                // The file's STEM is the loaded session's id (the fake
                // models real pi: `--session <file>` → the file's stem) —
                // it must match the stored id for the resume to round-trip.
                "piSessionFile": "/tmp/resume-1.jsonl",
                "model": "fake/fake-model",
                "thinkingLevel": "off",
                "loadSession": true,
                "promptCapabilities": { "image": true, "audio": false, "embeddedContext": false },
            }),
            config_options: None,
        })
        .expect("record_session should succeed");
        // Two stored rows the resume must CLEAR (the replay re-populates).
        db.record_message("resume-1", "agent-text", Some("m1"), r#"{"text":"stale"}"#)
            .expect("record_message should succeed");
        db.record_message("resume-1", "user", None, r#"{"text":"stale-user"}"#)
            .expect("record_message should succeed");

        let info = crate::test_support::run_with_retry(|| {
            manager.resume_session("fake", "resume-1", dir.clone(), &sink)
        })
        .await
        .unwrap();
        assert_eq!(info.session_id, "resume-1");

        // The replay re-populated the transcript: 2+ rows, including a
        // `kind: "user"` row with the `{"text": "hello"}` payload (the
        // stored "stale" rows are gone — the clear happened first).
        let rows = db
            .messages_for("resume-1")
            .expect("messages_for should work");
        assert!(
            rows.len() >= 2,
            "the replay re-populated the transcript: {rows:?}"
        );
        let user_rows: Vec<_> = rows.iter().filter(|r| r.kind == "user").collect();
        assert_eq!(user_rows.len(), 1, "exactly ONE user row (the replay's)");
        assert_eq!(user_rows[0].payload_json, r#"{"text":"hello"}"#);
        let agent_rows: Vec<_> = rows.iter().filter(|r| r.kind == "agent-text").collect();
        assert_eq!(
            agent_rows.len(),
            1,
            "one assistant message → one agent-text row"
        );
        assert_eq!(
            agent_rows[0].message_key.as_deref(),
            Some("m1"),
            "the replay uses the same messageId rule as the live stream"
        );
        assert_eq!(agent_rows[0].payload_json, r#"{"text":"world"}"#);
        assert!(
            !rows.iter().any(|r| r.payload_json.contains("stale")),
            "the stale rows were cleared"
        );

        // (item 6b) A legacy row WITHOUT `loadSession` → normalized
        // capabilities have `loadSession: false` + `resume_session` →
        // `NotResumable`.
        db.record_session(&SessionInfo {
            session_id: "legacy-1".to_string(),
            agent_id: "fake".to_string(),
            cwd: dir.clone(),
            capabilities: serde_json::json!({ "promptCapabilities": { "image": true } }),
            config_options: None,
        })
        .expect("record_session should succeed");
        assert_eq!(
            normalize_capabilities(&db.session("legacy-1").unwrap().unwrap().capabilities_json)
                ["loadSession"],
            false,
            "a legacy row (no loadSession key) normalizes to loadSession: false"
        );
        let result = manager
            .resume_session("fake", "legacy-1", dir.clone(), &sink)
            .await;
        assert!(
            matches!(result, Err(RpcError::NotResumable { .. })),
            "a legacy row is NotResumable, got: {result:?}"
        );

        let _ = manager.close_session(&info.session_id).await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// (cancel) `send_prompt` + `cancel_session` (`FAKE_PI_WAIT_ABORT=1` —
    /// the turn stays in-flight until the `abort`): the `abort` settles
    /// the turn and the in-flight `send_prompt` resolves `Cancelled` (the
    /// `cancel_requested` flag — set BEFORE the abort).
    #[tokio::test]
    async fn cancel_resolves_cancelled() {
        let dir = temp_config_dir();
        write_agents_json_pi(&dir, &[("FAKE_PI_WAIT_ABORT", "1")]);
        let manager = SessionManager::new(dir.clone()).unwrap();
        let (tx, _rx) = mpsc::unbounded_channel();
        let sink: Arc<dyn EventSink> = Arc::new(TestSink { tx });

        let info = crate::test_support::run_with_retry(|| {
            manager.start_session("fake", dir.clone(), &sink)
        })
        .await
        .unwrap();

        // The prompt + the cancel, CONCURRENTLY (`join!` — the `send_prompt`
        // awaits the turn's `agent_settled`, which the fake emits on the
        // `abort` the cancel sends; the `cancel_requested` flag set BEFORE
        // the abort maps the settle to `Cancelled`).
        let sid = info.session_id.clone();
        let (reason, cancel_res) = tokio::join!(
            async { manager.send_prompt(&sid, "hi".to_string()).await },
            async {
                // A head start for the prompt (the fake answers the prompt
                // preflight immediately, then waits for the abort).
                tokio::time::sleep(Duration::from_millis(100)).await;
                manager.cancel_session(&sid).await
            },
        );
        assert_eq!(cancel_res, Ok(()), "cancel should succeed");
        assert_eq!(reason, Ok(StopReason::Cancelled));

        let _ = manager.close_session(&info.session_id).await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// (config) `set_config_option("model", "fake/fake-model-2")`: the
    /// `set_model` command reaches the agent (the fake echoes the requested
    /// provider/modelId), the response's re-synthesized options come back
    /// (`currentValue` moved to `fake/fake-model-2`), AND a
    /// `config_option_update` event arrives with the same options.
    #[tokio::test]
    async fn set_config_option_roundtrip() {
        let dir = temp_config_dir();
        write_agents_json_pi(&dir, &[]);
        let manager = SessionManager::new(dir.clone()).unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let sink: Arc<dyn EventSink> = Arc::new(TestSink { tx });

        let info = crate::test_support::run_with_retry(|| {
            manager.start_session("fake", dir.clone(), &sink)
        })
        .await
        .unwrap();

        let updated = manager
            .set_config_option(&info.session_id, "model", "fake/fake-model-2", &sink)
            .await
            .unwrap();
        let model = updated
            .iter()
            .find(|o| o["id"] == "model")
            .expect("the model selector");
        assert_eq!(
            model["currentValue"], "fake/fake-model-2",
            "the echoed model becomes the current value"
        );

        // The `config_option_update` event (the client owns the frame — pi
        // does not emit one itself).
        let mut found = false;
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline && !found {
            if let Ok(msg) = tokio::time::timeout(Duration::from_millis(500), rx.recv()).await {
                let msg = msg.unwrap();
                if msg["event"] == "session-update"
                    && msg["payload"]["update"]["sessionUpdate"] == "config_option_update"
                {
                    found = true;
                    let opts = &msg["payload"]["update"]["configOptions"];
                    let model = opts
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|o| o["id"] == "model")
                        .unwrap();
                    assert_eq!(model["currentValue"], "fake/fake-model-2");
                }
            }
        }
        assert!(found, "the config_option_update event was not emitted");

        let _ = manager.close_session(&info.session_id).await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// (permission) `FAKE_PI_GATE=1`: the `prompt` turn fires the gate
    /// dialog (`extension_ui_request` `confirm` → a `permission-request`
    /// event with the `[Allow, Block]` options) → `respond_permission`
    /// (`Selected("allow")` → `confirmed: true`) → the turn still resolves
    /// `EndTurn` (the fake only settles after the response).
    #[tokio::test]
    async fn permission_gate_roundtrip() {
        let dir = temp_config_dir();
        write_agents_json_pi(&dir, &[("FAKE_PI_GATE", "1")]);
        let manager = SessionManager::new(dir.clone()).unwrap();
        // The injection (Task 4): `new` installed the bundled gate
        // extension (the spawn passes `-e <path>` + `PI_ARCHIMEDES_GATE=1`;
        // the fake's `FAKE_PI_GATE=1` mode plays the extension's part —
        // it emits the `extension_ui_request` the extension would cause).
        let gate_file = dir.join("pi-gate").join("gate.ts");
        assert!(
            gate_file.exists(),
            "SessionManager::new installs the gate extension"
        );
        let (tx, mut rx) = mpsc::unbounded_channel();
        let sink: Arc<dyn EventSink> = Arc::new(TestSink { tx });

        let info = crate::test_support::run_with_retry(|| {
            manager.start_session("fake", dir.clone(), &sink)
        })
        .await
        .unwrap();

        // The prompt + the gate answer, CONCURRENTLY (`join!` — the
        // `send_prompt` awaits the turn's `agent_settled`, which the fake
        // emits only AFTER the client's `confirmed: true` response; the
        // `permission-request` event arrives mid-turn, and the pending
        // entry is registered BEFORE the event is emitted, so
        // `respond_permission` finds it by the time the event is seen).
        let sid = info.session_id.clone();
        let (reason, answer) = tokio::join!(
            async { manager.send_prompt(&sid, "hi".to_string()).await },
            async {
                // The `permission-request` event (the synthesized shape —
                // the `request` sub-object mirrors the frontend's
                // `PermissionRequest` type; `confirm` frames offer
                // `[Allow, Block]`).
                let mut request_id = None;
                let deadline = std::time::Instant::now() + Duration::from_secs(5);
                while std::time::Instant::now() < deadline && request_id.is_none() {
                    if let Ok(msg) =
                        tokio::time::timeout(Duration::from_millis(500), rx.recv()).await
                    {
                        let msg = msg.unwrap();
                        if msg["event"] == "permission-request" {
                            let request = &msg["payload"]["request"];
                            let options: Vec<String> = request["options"]
                                .as_array()
                                .unwrap()
                                .iter()
                                .map(|o| o["optionId"].as_str().unwrap().to_string())
                                .collect();
                            assert_eq!(
                                options,
                                vec!["allow", "reject"],
                                "confirm frames offer [Allow, Block]"
                            );
                            assert_eq!(request["sessionId"], sid);
                            // The tool name is in the title (the frontend's
                            // `PermissionPrompt` renders it).
                            let title = request["toolCall"]["title"].as_str().unwrap_or("");
                            assert!(
                                title.contains("bash"),
                                "the title carries the gated tool name, got {title:?}"
                            );
                            request_id =
                                Some(msg["payload"]["requestId"].as_str().unwrap().to_string());
                        }
                    }
                }
                let request_id = request_id.expect("the permission-request event was not emitted");
                // The user allows → the fake receives `confirmed: true`
                // and settles the turn.
                let hit = manager
                    .respond_permission(
                        &sid,
                        &request_id,
                        PermissionOutcome::Selected {
                            option_id: "allow".to_string(),
                        },
                    )
                    .await
                    .unwrap();
                assert!(hit, "the pending entry must be resolved");
                Ok::<(), RpcError>(())
            },
        );
        assert!(answer.is_ok());
        assert_eq!(
            reason,
            Ok(StopReason::EndTurn),
            "the turn settles after the allowed gate"
        );

        let _ = manager.close_session(&info.session_id).await;
        let _ = std::fs::remove_dir_all(&dir);
    }
}
