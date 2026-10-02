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

use std::collections::{BTreeMap, HashMap, HashSet};
use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::{oneshot, watch, Mutex};

use crate::agent::bridge::{self, CachedPassword, PendingBridge, PendingSudo, SudoRunner};
use crate::agent::errors::RpcError;
use crate::agent::harness::{
    build_main_prompt, discover_models, merge_catalog, seed_from_pi_config, AgentLoop, ControlCmd,
    Model, ModelCatalog, OpenAiCompatibleProvider, Prompt, PromptContext, Provider,
    ProviderDiscovery, RetryPolicy, SessionStore, SudoDeps, DEFAULT_CONTEXT_WINDOW,
};
use crate::agent::permission::{self, PendingPermissions};
use crate::agent::rpc::{PiRpc, PiRpcHandle, RpcEvent};
use crate::agent::todo::TodoStore;
use crate::agent::tools::ImageRef;
use crate::commands::settings::{load_settings, write_settings};
use crate::config::{AgentEntry, AgentKind, ConfigError, Registry};
use crate::storage::Db;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

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
    /// The desktop's archived flag (ADR 0016). `false` for a newly
    /// started or ephemeral session; the resume paths and
    /// `list_sessions` read it from the stored row.
    pub archived: bool,
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
    /// The session backend (the pi RPC handle for an EXTERNAL session; the
    /// in-process `AgentLoop` handle for a NATIVE session) — cheap clone,
    /// shared with the driver task.
    pub(crate) handle: SessionBackend,
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

/// The session backend (the `SessionDriver` generalization, Task 7): an
/// EXTERNAL session is a `PiRpc` subprocess (the existing path — unchanged);
/// a NATIVE session is an in-process `AgentLoop` tokio task (the harness,
/// Tasks 4–6) driven through the `NativeHandle`.
#[derive(Clone)]
pub(crate) enum SessionBackend {
    /// The pi RPC handle (the external path — `pi --mode rpc`).
    Pi(PiRpcHandle),
    /// The in-process `AgentLoop` handle (the native path).
    Native(NativeHandle),
}

/// The native session's config state (the `set_config_option` re-synthesizer
/// source — the loop's own `model` / `thinking_level` live INSIDE the spawned
/// task, so the handle mirrors the applied config: `start_native_session`
/// initializes it, `set_config_option` updates it when a change is applied).
#[derive(Clone)]
pub(crate) struct NativeConfigState {
    pub(crate) model: Model,
    pub(crate) thinking_level: Option<String>,
}

/// A cheap, `'static`-safe handle to the native `AgentLoop` task (the
/// NATIVE counterpart of `PiRpcHandle`): `prompt_tx` / `control_tx` clone
/// the loop's channels and `cancel` is the loop's cancellation token. The
/// loop's `RpcEvent` `Receiver` is NOT held here (a `tokio` mpsc `Receiver`
/// is not `Clone`) — `drive_native_session` takes it by move (the driver
/// task consumes it to watch `agent_settled`; the loop has ALREADY run the
/// events through the `normalize` + `persist_update` pipeline in its
/// `emit`).
#[derive(Clone)]
pub(crate) struct NativeHandle {
    prompt_tx: mpsc::Sender<Prompt>,
    control_tx: mpsc::Sender<ControlCmd>,
    /// The SESSION teardown token (the loop's `run()` exits on it — a
    /// `close_session` tears the loop down; the driver tears the session
    /// down when the loop task ends).
    cancel: CancellationToken,
    /// The current TURN's cancel token (SHARED with the loop — the loop
    /// arms a fresh one per prompt; a `cancel_session` Stop cancels the
    /// CURRENT turn only — finding 8c).
    turn_cancel: Arc<StdMutex<CancellationToken>>,
    /// The session's config state (see `NativeConfigState`).
    state: Arc<StdMutex<NativeConfigState>>,
    /// The loop task's `AbortHandle` (set after `tokio::spawn` — a test-only
    /// seam to kill the loop task DIRECTLY, without cancelling any token, so
    /// the `settle_tx` sender drops with NO pending settle (a deterministic
    /// `changed()` `Err` → the teardown resolves `pending_turn` `Cancelled`).
    loop_task: Arc<StdMutex<Option<tokio::task::AbortHandle>>>,
}

impl NativeHandle {
    fn new(
        prompt_tx: mpsc::Sender<Prompt>,
        control_tx: mpsc::Sender<ControlCmd>,
        cancel: CancellationToken,
        turn_cancel: Arc<StdMutex<CancellationToken>>,
        model: Model,
        thinking_level: Option<String>,
    ) -> Self {
        Self {
            prompt_tx,
            control_tx,
            cancel,
            turn_cancel,
            state: Arc::new(StdMutex::new(NativeConfigState {
                model,
                thinking_level,
            })),
            loop_task: Arc::new(StdMutex::new(None)),
        }
    }

    /// (test-only) Kill the loop task DIRECTLY (a `JoinHandle::abort` — the
    /// task dies with NO token cancelled, so it emits NO final settle; the
    /// `settle_tx` sender drops unseen → `changed()` returns `Err`
    /// deterministically). Distinct from `close` (which cancels the turn +
    /// session tokens and may let the loop settle first).
    #[cfg(test)]
    fn abort_loop_task(&self) {
        if let Some(abort) = self.loop_task.lock().unwrap().clone() {
            abort.abort();
        }
    }

    /// Store the loop task's `AbortHandle` (called after `tokio::spawn`).
    fn set_loop_task(&self, handle: tokio::task::AbortHandle) {
        *self.loop_task.lock().unwrap() = Some(handle);
    }

    /// Queue a prompt (best-effort — a full / closed queue is dropped,
    /// mirroring `AgentLoop::send_prompt`).
    fn send_prompt(&self, text: &str, images: &[ImageRef]) -> bool {
        self.prompt_tx
            .try_send(Prompt {
                text: text.to_string(),
                images: images.to_vec(),
            })
            .is_ok()
    }

    /// Stop the in-flight turn (the `cancel_session` Stop — finding 8c:
    /// the native Stop matches the external `abort`: the loop settles
    /// the turn `Cancelled` and STAYS ALIVE — a new prompt reuses the
    /// session; only a `close_session` (`close`) tears the loop down).
    fn cancel(&self) {
        self.turn_cancel
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .cancel();
    }

    /// Tear the loop down (the `close_session` teardown — the prompt
    /// queue + the in-flight turn stop; the driver teardown cancels too
    /// — idempotent).
    fn close(&self) {
        self.turn_cancel
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .cancel();
        self.cancel.cancel();
    }

    /// The current config state (the `set_config_option` re-synthesizer).
    fn config_state(&self) -> Arc<StdMutex<NativeConfigState>> {
        self.state.clone()
    }

    /// The loop's control channel sender (the `set_config_option` native
    /// branch queues `SetModel` / `SetThinkingLevel` through it — the loop
    /// applies them via `set_model` / `set_thinking_level` when idle).
    fn control_tx_clone(&self) -> mpsc::Sender<ControlCmd> {
        self.control_tx.clone()
    }
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
///
/// `announced_tool_calls` is the set of ids a `tool_call` (ANNOUNCE) frame
/// has already been emitted for (a `toolcall_start` with a non-empty
/// `toolName`, or the `toolcall_end` / `tool_execution_start` fallbacks).
/// The first frame for an id MUST be a `tool_call` (with a real `title`) —
/// a `tool_call_update` for an id the frontend never saw would create a
/// message with `title = toolCallId` (e.g. `chatcmpl-tool-…`), and a second
/// `tool_call` frame would APPEND a duplicate row.
#[derive(Debug, Default)]
pub struct TurnState {
    pub msg_counter: u64,
    pub current_message_id: Option<String>,
    pub toolcall_args: HashMap<String, String>,
    pub announced_tool_calls: HashSet<String>,
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
    /// The shared todo store (Phase 2, Task 1 — the `todo_update`
    /// handler's store; the main and subagent managers each get their
    /// OWN store, mirroring how `pending_bridge` is split across the
    /// two managers).
    pub(crate) todo_store: Arc<TodoStore>,
    /// The pending `sudo_exec` sub-prompt oneshots (Phase 2, Task 1 —
    /// one entry per sub-prompt, keyed `"{sid}/{id}:confirm"` /
    /// `"{sid}/{id}:password"`).
    pub(crate) pending_sudo: PendingSudo,
    /// The per-session sudo credential cache (Phase 2, Task 1 — the
    /// suite's `credentialCache`: in-memory only, keyed by session id,
    /// cleared in the driver-task teardown alongside the `pending_bridge`
    /// prefix drain — mirroring the suite's cache cleared at every
    /// session boundary). It MUST live in shared per-session state (the
    /// `ConnCtx` is per-connection, rebuilt per frame — it cannot hold
    /// the cache).
    pub(crate) sudo_password: Arc<Mutex<HashMap<String, CachedPassword>>>,
    /// The `sudo -S` execution seam (Phase 2, Task 1 — the real runner
    /// in production; tests inject a fake via the `start_listener`
    /// parameter).
    pub(crate) runner: Arc<dyn SudoRunner>,
    /// How long the establishment phase (agent spawn + `get_state`
    /// establisher) may run before it is cancelled. Default: 30 s.
    pub(crate) establish_timeout: Duration,
    /// How long to wait for a turn to settle before reporting a failure. A
    /// hung turn (a prompt that never settles) is torn down + reported
    /// failed after this, so a subagent can't linger forever (the
    /// zombie-subagent fix). Defaults to 30 minutes.
    pub(crate) settle_timeout: Duration,
    /// Transcript persistence (main only; `None` for subagents —
    /// ephemeral, not stored). Gates `persist_update` only; the trust
    /// lookup is `trust_db` (below — main AND subagent).
    pub(crate) db: Option<Arc<Db>>,
    /// The trust lookup source (ADR 0010): the `space_trusted` lookup the
    /// permission gate uses to auto-confirm a TRUSTED Space's `confirm`.
    /// Independent of `db` — `db` gates TRANSCRIPT PERSISTENCE (main only;
    /// `None` for subagents, which are ephemeral), while `trust_db` is the
    /// trust lookup for BOTH main and subagent Sessions (`None` = fail-
    /// closed: the gate prompts, today's flow).
    pub(crate) trust_db: Option<Arc<Db>>,
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
    /// establish timeout, no persistence / trust db / captures / subagent
    /// handle).
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            generation_counter: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            pending_permissions: Arc::new(Mutex::new(HashMap::new())),
            pending_bridge: Arc::new(Mutex::new(HashMap::new())),
            todo_store: Arc::new(TodoStore::new()),
            pending_sudo: Arc::new(Mutex::new(HashMap::new())),
            sudo_password: Arc::new(Mutex::new(HashMap::new())),
            runner: Arc::new(bridge::RealSudoRunner),
            establish_timeout: Duration::from_secs(30),
            settle_timeout: Duration::from_secs(30 * 60),
            db: None,
            trust_db: None,
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
                // The Phase 2 method-aware handler state (`todo_update` /
                // `sudo_exec` — the desktop answers them itself): the
                // shared todo store, the sudo sub-prompt oneshots, the
                // per-session sudo credential cache, and the `sudo -S`
                // execution seam.
                let todo_store = self.todo_store.clone();
                let pending_sudo = self.pending_sudo.clone();
                let sudo_password = self.sudo_password.clone();
                let runner = self.runner.clone();
                Some(
                    bridge::start_listener(
                        client_session_id,
                        &socket_path,
                        std::process::id(),
                        &cwd,
                        sink.clone(),
                        self.pending_bridge.clone(),
                        listener_close_tx,
                        bridge::DEFAULT_BRIDGE_TIMEOUT,
                        subagent_spawn,
                        cost_capture,
                        todo_store,
                        pending_sudo,
                        sudo_password,
                        runner,
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
        // The Phase 2 shared state for the driver-task teardown (the
        // `sudo_password` cache + the `todo_store` entry are removed by the
        // BARE session id; the `pending_sudo` oneshots are drained with the
        // `pending_bridge` prefix drain below — their keys are compound).
        let pending_sudo_arc = self.pending_sudo.clone();
        let sudo_password_arc = self.sudo_password.clone();
        let todo_store_arc = self.todo_store.clone();
        let establish_timeout = self.establish_timeout;
        let db = self.db.clone();
        // The trust lookup source (ADR 0010 — the permission gate's
        // `space_trusted` lookup; independent of `db`, which gates transcript
        // persistence only).
        let trust_db = self.trust_db.clone();
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
                            // (finding 6) The `pending_turn` take + the settle
                            // watch send happen under ONE `sessions` lock,
                            // the watch send LAST (the `wait_for_settle`
                            // race — see the native driver's settle arm).
                            let _reason = {
                                let sessions = sessions_arc.lock().await;
                                let reason = if let Some(live) = sessions.get(&info.session_id) {
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
                                };
                                if let Some(live) = sessions.get(&info.session_id) {
                                    if let Some(tx) = live
                                        .pending_turn
                                        .lock()
                                        .unwrap_or_else(|p| p.into_inner())
                                        .take()
                                    {
                                        let _ = tx.send(reason);
                                    }
                                }
                                let _ = settle_tx.send((settle_seq, reason));
                                reason
                            };
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
                        // `trust_db` + `cwd` let a TRUSTED Space's `confirm`
                        // short-circuit (ADR 0010) — one call site serves both
                        // main and subagent Sessions: the main manager's
                        // `attach_db` sets `trust_db` (the same db), and the
                        // subagent manager threads it through `new` (its `db`
                        // stays `None` — ephemeral, no transcript persistence).
                        permission::handle_extension_ui_request(
                            &info.session_id, req, &handle, &sink, &pending_permissions_arc,
                            trust_db.as_ref(), &info.cwd,
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
            // Resolve the pending turn (an in-flight `send_prompt`
            // awaiting its `agent_settled`) with `Cancelled` BEFORE the
            // session is removed (finding 3a — the agent died / the
            // session was torn down mid-turn, so the turn will never
            // settle; without this the `send_prompt`'s unbounded
            // `rx.await` hangs forever).
            {
                let mut sessions = sessions_arc.lock().await;
                if sessions
                    .get(&info.session_id)
                    .is_some_and(|l| l.generation == live_generation)
                {
                    if let Some(live) = sessions.get(&info.session_id) {
                        if let Some(tx) = live
                            .pending_turn
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .take()
                        {
                            let _ = tx.send(StopReason::Cancelled);
                        }
                    }
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
            // Phase 2: drain this session's pending `sudo_exec` sub-prompts
            // too (dropping the senders cancels the in-flight confirm /
            // password waiters). The `pending_sudo` entries are COMPOUND-keyed
            // (`"{sid}/{id}:confirm"`), so the trailing-slash prefix is correct
            // there.
            pending_sudo_arc
                .lock()
                .await
                .retain(|key, _| !key.starts_with(&bridge_prefix));
            // Clear the cached sudo password (the suite's `credentialCache`
            // is cleared at every session boundary — a stale credential must
            // not survive the session, and a resume under the same id must
            // re-prompt, not silently reuse it). The cache is keyed by the
            // BARE session id (NOT compound — `cache.get(sid)`), so it is
            // removed by the bare key: a `"{sid}/"` prefix would never match
            // `"{sid}"` and the plaintext credential would leak.
            sudo_password_arc.lock().await.remove(&info.session_id);
            // Same boundary for the todo store (a resumed session must not
            // read the previous incarnation's todos; the map must not grow
            // one entry per session forever).
            todo_store_arc.remove(&info.session_id);
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
            // The EXTERNAL backend (the `PiRpc` handle — the task keeps its
            // own clone; the child lives while ANY clone is alive).
            handle: SessionBackend::Pi(handle),
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
    /// BOUNDED by `settle_timeout` (a hung turn can't linger forever — the
    /// zombie-subagent fix): the wait ends on a settle AT OR AFTER the turn
    /// being waited for (resolves `Ok(reason)` — the `cancel_requested` flag
    /// already mapped a cancel to `Cancelled`), on a teardown (the driver
    /// task ending DROPS the watch sender, which resolves `changed()` as
    /// `Err` → `Err(ProcessExited)` — the agent died mid-turn, or the
    /// session was torn down), or on the settle timeout (a hung turn →
    /// `Err(SettleTimeout)` — the caller's cancel tears the session down).
    ///
    /// STALE-SETTLE GUARD: the wait is pinned to the channel's current
    /// version (`mark_unchanged`) ONLY while a turn is IN FLIGHT (the
    /// `pending_turn` slot is occupied — `send_prompt` sets it before the
    /// dispatch and the driver clears it on `agent_settled`). With a turn
    /// in flight, a recorded settle is a PREVIOUS turn's (stale) — the
    /// mark makes `changed()` resolve only on a NEW settle. With NO turn
    /// in flight (the caller's turn already settled — the subagent
    /// dispatches a raw prompt, which does NOT occupy `pending_turn`, then
    /// awaits the settle: a fast turn settles before the await), the mark
    /// is SKIPPED: the clone inherits the stored receiver's initial version,
    /// so `changed()` resolves immediately with the latest settle (the fast
    /// turn's — not hung on a new settle that never comes).
    pub async fn wait_for_settle(&self, session_id: &str) -> Result<StopReason, RpcError> {
        let mut rx = {
            let sessions = self.sessions.lock().await;
            let live = sessions
                .get(session_id)
                .ok_or_else(|| RpcError::UnknownSession {
                    id: session_id.to_string(),
                })?;
            // A `send_prompt` turn is in flight (the slot is occupied —
            // set before the dispatch, cleared on `agent_settled`).
            let turn_in_flight = live
                .pending_turn
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .is_some();
            let mut rx = live.settle_rx.clone();
            // (finding 6) The `mark_unchanged` runs under the SAME
            // `sessions` lock as the `pending_turn` snapshot (the race: a
            // settle that lands between the snapshot and the mark would be
            // marked seen and never resolve the wait — a 30-min spurious
            // `SettleTimeout`). It is applied ONLY when a turn is in flight
            // (a no-turn-in-flight `wait_for_settle` must resolve with the
            // LATEST settle — the fast-turn contract — not be pinned to the
            // current version). The driver's settle arm takes the slot +
            // sends the watch under the same lock (watch send last), so the
            // two critical sections are ordered: a settle that lands after
            // the mark is a NEW version (resolves), and one that lands
            // before empties the slot (no mark → resolve with the latest).
            if turn_in_flight {
                rx.mark_unchanged();
            }
            rx
        };
        // A bounded wait: a hung turn (no settle within `settle_timeout`)
        // resolves `Err(SettleTimeout)`; a teardown (sender dropped)
        // resolves `Err(ProcessExited)`; a settle resolves `Ok(reason)`.
        match tokio::time::timeout(self.settle_timeout, rx.changed()).await {
            Ok(Ok(_)) => {} // a settle was recorded (fall through)
            Ok(Err(_)) => {
                // The sender was dropped without a settle (the agent died
                // mid-turn, or the session was torn down).
                return Err(RpcError::ProcessExited(None));
            }
            Err(_elapsed) => {
                // The settle timed out (a hung turn — the caller's cancel
                // tears the session down; see the dispatch).
                return Err(RpcError::SettleTimeout {
                    detail: format!(
                        "the turn did not settle within {}s",
                        self.settle_timeout.as_secs()
                    ),
                });
            }
        }
        let (seq, reason) = *rx.borrow();
        if seq == 0 {
            Err(RpcError::ProcessExited(None))
        } else {
            Ok(reason)
        }
    }

    /// Shared driver for a NATIVE session (the `AgentLoop` variant of
    /// [`Self::drive_session`]).
    ///
    /// The loop task (spawned by the caller) emits `RpcEvent`s on
    /// `events_rx` (moved in — a `tokio` mpsc `Receiver` is not `Clone`)
    /// AND writes the settle watch on every `agent_settled` (finding 3 —
    /// the RELIABLE settle: a full / slow `events` mpsc can drop the raw
    /// event, but a watch send is never dropped). The loop has ALREADY
    /// run the events through the `normalize` + `persist_update` pipeline
    /// (its `emit`), so the driver does NOT re-normalize: it watches the
    /// settle (the `pending_turn` + the `settle_tx` watch, populated
    /// IDENTICALLY to the external path so `wait_for_settle` works
    /// unchanged) and blocks until close (or the loop task ending — a
    /// `close_session` / teardown).
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn drive_native_session(
        &self,
        handle: NativeHandle,
        events_rx: mpsc::Receiver<RpcEvent>,
        loop_settle_rx: watch::Receiver<u64>,
        agent_id: &str,
        cwd: PathBuf,
        sink: &Arc<dyn EventSink>,
        info: SessionInfo,
    ) -> Result<SessionInfo, RpcError> {
        let (close_tx, mut close_rx) = watch::channel(false);
        let kind: Arc<StdMutex<Option<CloseKind>>> = Arc::new(StdMutex::new(None));
        // The turn-settle watch (the external path's `settle_tx` — the
        // initial `(0, …)` means "no settle yet").
        let (settle_tx, settle_rx) = watch::channel((0u64, StopReason::EndTurn));
        let sessions_arc = self.sessions.clone();
        let pending_permissions_arc = self.pending_permissions.clone();
        let pending_bridge_arc = self.pending_bridge.clone();
        let pending_sudo_arc = self.pending_sudo.clone();
        let sudo_password_arc = self.sudo_password.clone();
        let todo_store_arc = self.todo_store.clone();
        // The session's generation token (the teardown guard: a SUPERSEDED
        // driver — a resume overwrote this entry under the same session id —
        // must not clobber the replacement's entry).
        let live_generation = self
            .generation_counter
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let backend = handle.clone();
        let mut events_rx = events_rx;
        // The loop's settle watch receiver (finding 3 — moved into the
        // driver task; the `LiveSession`'s `settle_rx` above is the
        // driver's OWN watch, which the driver task UPDATES on settle).
        let mut loop_settle_rx = loop_settle_rx;
        // Clones for the driver task (held by the task until AFTER its kind
        // read below); the originals move into the `LiveSession` value.
        let info_task = info.clone();
        let kind_for_task = kind.clone();
        let sink = sink.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    ev = events_rx.recv() => {
                        if ev.is_none() {
                            // The loop task ended (a close / teardown) —
                            // tear the session down.
                            break;
                        }
                        // The `events` mpsc is a LIVENESS signal only:
                        // a full / slow channel can DROP an
                        // `agent_settled` delivery (finding 3), so the
                        // settle is NOT taken from it — the settle watch
                        // arm below is the reliable one (a watch send is
                        // never dropped).
                    }
                    // A settled turn (the loop's settle watch — RELIABLE:
                        // a watch send is never dropped, so a full / slow
                        // `events` mpsc cannot lose it, finding 3): resolves
                        // the pending prompt (a `cancel_requested` flag maps
                        // it to `Cancelled`) AND records the settle on the
                        // watch (the subagent's prompt wait —
                        // `wait_for_settle`), IDENTICALLY to the external
                        // path.
                    changed = loop_settle_rx.changed() => {
                        // `changed()` `Err` = the loop's settle sender was
                        // dropped (the loop task died) — NOT a settle. Skip
                        // the settle processing: the `events_rx.recv() →
                        // None` arm breaks the loop, and the teardown resolves
                        // `pending_turn` with `Cancelled` (a mid-turn death
                        // must not tell `send_prompt` "turn ended normally"
                        // — pre-fix the `_ =` pattern treated the `Err` as a
                        // settle and resolved `pending_turn` `EndTurn` + wrote
                        // a duplicate `(seq, reason)` to the driver watch).
                        if changed.is_err() {
                            break;
                        }
                        // The settle count from the loop's watch (the loop's
                        // `settle_count` atomic — the REAL count: a watch
                        // coalesces two fast settles into ONE wake, so
                        // counting wakes would undercount).
                        let settle_seq = *loop_settle_rx.borrow();
                        // (finding 6) The `pending_turn` take + the settle
                        // watch send happen under ONE `sessions` lock, the
                        // watch send LAST (the `wait_for_settle` race: a
                        // watch send that lands between the waiter's
                        // `pending_turn` snapshot and its `mark_unchanged`
                        // would be marked seen and never resolve the wait;
                        // taking the slot first, under the same lock, orders
                        // the two critical sections). The `cancel_requested`
                        // flag maps the settle to `Cancelled` (else `EndTurn`).
                        let _reason = {
                            let sessions = sessions_arc.lock().await;
                            let reason = if let Some(live) = sessions.get(&info_task.session_id) {
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
                            };
                            if let Some(live) = sessions.get(&info_task.session_id) {
                                if let Some(tx) = live
                                    .pending_turn
                                    .lock()
                                    .unwrap_or_else(|p| p.into_inner())
                                    .take()
                                {
                                    let _ = tx.send(reason);
                                }
                            }
                            let _ = settle_tx.send((settle_seq, reason));
                            reason
                        };
                    }
                    // The close flag (`close_session`).
                    _ = close_rx.changed() => break,
                }
            }

            // The session is over: tear the loop down (the prompt queue +
            // the in-flight turn stop) and clean up (the `drive_session`
            // teardown, minus the bridge listener — a native session has no
            // bridge: it runs in-process).
            backend.cancel.cancel();
            let kind = *kind_for_task.lock().unwrap_or_else(|p| p.into_inner());
            let reason = match kind {
                Some(CloseKind::User) => ClosedReason::User,
                None => ClosedReason::AgentExited,
            };
            // Guard by the generation token (a SUPERSEDED driver must not
            // clobber the replacement's entry). Resolve the pending turn
            // (an in-flight `send_prompt` awaiting its `agent_settled`)
            // with `Cancelled` BEFORE the session is removed (finding 3a —
            // the session is over, so the turn will never settle; without
            // this the `send_prompt`'s unbounded `rx.await` hangs forever
            // — a close / cancel could not unblock the waiter).
            {
                let mut sessions = sessions_arc.lock().await;
                if sessions
                    .get(&info_task.session_id)
                    .is_some_and(|l| l.generation == live_generation)
                {
                    if let Some(live) = sessions.get(&info_task.session_id) {
                        if let Some(tx) = live
                            .pending_turn
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .take()
                        {
                            let _ = tx.send(StopReason::Cancelled);
                        }
                    }
                    sessions.remove(&info_task.session_id);
                }
            }
            // Keys are `"{session_id}/{request_id}"` — match on the
            // trailing-slash prefix so closing "s1" does not cancel the
            // pending prompt of the longer session "s10".
            let prefix = permission::session_key_prefix(&info_task.session_id);
            pending_permissions_arc
                .lock()
                .await
                .retain(|key, _| !key.starts_with(&prefix));
            let bridge_prefix = bridge::session_key_prefix(&info_task.session_id);
            pending_bridge_arc
                .lock()
                .await
                .retain(|key, _| !key.starts_with(&bridge_prefix));
            pending_sudo_arc
                .lock()
                .await
                .retain(|key, _| !key.starts_with(&bridge_prefix));
            sudo_password_arc.lock().await.remove(&info_task.session_id);
            todo_store_arc.remove(&info_task.session_id);
            sink.emit(
                "session-closed",
                json!({
                    "sessionId": info_task.session_id,
                    "reason": reason.as_str(),
                }),
            );
        });

        let live = LiveSession {
            handle: SessionBackend::Native(handle),
            generation: live_generation,
            session_id: info.session_id.clone(),
            cwd,
            agent_id: agent_id.to_string(),
            close_tx: close_tx.clone(),
            close_kind: kind,
            thought_state: Arc::new(StdMutex::new(ThoughtState::default())),
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

/// A factory that builds a `Provider` from a `Model` (the native path's
/// provider seam — `SessionManager::provider_factory`; the production
/// default is `OpenAiCompatibleProvider`, a test sets a mock before
/// `start_session`). `pub` so `subagent.rs`'s `NativeDeps` can carry
/// one (the native dispatch builds the `Provider` through it).
pub type ProviderFactory = Arc<dyn Fn(&Model) -> Box<dyn Provider> + Send + Sync>;

/// Manages all live ACP sessions (the MAIN sessions).
///
/// Owns a [`SessionDriver`] (db: attached via [`Self::attach_db`],
/// `trust_db`: the same db, attached via [`Self::attach_db`],
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
    /// The installed tools-override extension's path (`None` when the
    /// install failed — the spawn then skips the tools args, and the
    /// session runs on the suite's original tools rather than broken).
    tools_path: Option<PathBuf>,
    /// The model catalog (Task 5 — seeded from the user's pi config, ADR
    /// 0012; the native session's model source + the `set_config_option`
    /// re-synthesizer). Best-effort: a missing pi config degrades to an
    /// empty catalog (a logged warning), never a crash.
    catalog: ModelCatalog,
    /// The provider factory seam (reviewer-corrected Major #21): the native
    /// path builds the `Provider` through it (the production default is
    /// `OpenAiCompatibleProvider`; a test sets a mock BEFORE `start_session`),
    /// so `start_session` never constructs the provider inline.
    provider_factory: ProviderFactory,
    /// The per-provider live-discovery cache (the `GET /v1/models` result —
    /// the OpenAI endpoint "supplies everything"; the `pi-provider-litellm`
    /// `fetchModels` pattern). At most one fetch per provider (a failed /
    /// unreachable endpoint is not retried every session); a failure /
    /// absent model degrades to the static `models-store.json` metadata.
    discovery_cache: tokio::sync::Mutex<HashMap<String, ProviderDiscovery>>,
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
        // Install the desktop-provided tools override (idempotent — both
        // managers install the same file). A failure is NON-fatal: the
        // session runs on the suite's original tools rather than broken.
        let tools_path = match crate::agent::tools::install_tools_extension(&config_dir) {
            Ok(path) => Some(path),
            Err(e) => {
                eprintln!(
                    "tools extension install failed: {e} (sessions run on the suite's tools)"
                );
                None
            }
        };
        Ok(Self {
            driver: SessionDriver::new(),
            registry,
            config_dir,
            gate_path,
            tools_path,
            // The model catalog (Task 5 — seeded from the user's pi
            // config; best-effort, ADR 0012). Tests override it via
            // `set_catalog` (a test cannot control the user's real pi
            // config).
            catalog: seed_from_pi_config(),
            // The provider factory seam (reviewer-corrected Major #21 —
            // the production default; a test sets a mock via
            // `set_provider_factory` BEFORE `start_session`).
            provider_factory: Arc::new(|m: &Model| {
                Box::new(OpenAiCompatibleProvider {
                    base_url: m.base_url.clone(),
                    api_key: m.api_key.clone(),
                }) as Box<dyn Provider>
            }),
            discovery_cache: tokio::sync::Mutex::new(HashMap::new()),
        })
    }

    /// Override the model catalog (tests — `new` seeds from the user's pi
    /// config, which a test cannot control). Set BEFORE `start_session`.
    pub fn set_catalog(&mut self, catalog: ModelCatalog) {
        self.catalog = catalog;
    }

    /// Inject the provider factory (reviewer-corrected Major #21 — the
    /// native path builds the `Provider` through it; the production default
    /// is `OpenAiCompatibleProvider`). Set BEFORE `start_session`.
    pub fn set_provider_factory(
        &mut self,
        f: impl Fn(&Model) -> Box<dyn Provider> + Send + Sync + 'static,
    ) {
        self.provider_factory = Arc::new(f);
    }

    /// Best-effort refresh a `Model`'s metadata (context window, thinking
    /// levels) from the provider's live `GET /v1/models` (the OpenAI
    /// endpoint "supplies everything" — the `pi-provider-litellm`
    /// `fetchModels` pattern). Bounded (a `discover_models` timeout) +
    /// cached per-provider (at most one fetch per provider — a failed /
    /// unreachable endpoint is NOT retried every session). A failure, or a
    /// model absent from the response, degrades to the static
    /// (`models-store.json`) metadata (the `Model` is returned unchanged).
    async fn refresh_model_metadata(&self, model: &Model) -> Model {
        if model.base_url.is_empty() {
            return model.clone();
        }
        let mut cache = self.discovery_cache.lock().await;
        let entry = cache
            .entry(model.provider.clone())
            .or_insert_with(ProviderDiscovery::default);
        if !entry.attempted {
            entry.attempted = true;
            // Best-effort: a failure (unreachable endpoint, non-2xx) leaves
            // `models` empty → the static metadata is kept (no retry).
            if let Ok(models) = discover_models(&model.base_url, &model.api_key).await {
                entry.models = models;
            }
        }
        let Some(meta) = entry.models.get(&model.id) else {
            return model.clone();
        };
        // Apply the fresh metadata (only the `Some` fields — an absent
        // field keeps the static `models-store.json` value).
        let mut updated = model.clone();
        if let Some(cw) = meta.context_window {
            updated.context_window = cw;
        }
        if let Some(levels) = meta.thinking_levels.clone() {
            updated.thinking_levels = levels;
        }
        if let Some(st) = meta.supports_thinking {
            updated.supports_thinking = st;
        }
        updated
    }

    /// The effective catalog: the seeded catalog (ADR 0012) + the user's
    /// providers from `settings.json` (fresh read via `load_settings`),
    /// discovered via `discover_models` (best-effort; the existing
    /// per-provider `discovery_cache` — `force_refresh` bypasses the cache
    /// for provider `force_refresh` when `Some`). A provider whose discovery
    /// fails contributes 0 models but still shadows the seeded models for
    /// its id (ADR 0014 — via `merge_catalog`'s `shadowed_provider_ids`).
    pub async fn effective_catalog(&self, force_refresh: Option<&str>) -> ModelCatalog {
        let settings = load_settings(&self.config_dir);
        let mut user_models: Vec<Model> = Vec::new();
        let mut cache = self.discovery_cache.lock().await;
        for provider in &settings.providers {
            let entry = cache
                .entry(provider.id.clone())
                .or_insert_with(ProviderDiscovery::default);
            // `force_refresh` (a provider id) bypasses the cache for that
            // provider (the settings page's refresh affordance); a failed
            // re-fetch clears the stale entry (0 models — the provider row
            // shows `unreachable` + refresh, the stale seeded models stay
            // shadowed).
            let bypass = Some(provider.id.as_str()) == force_refresh;
            if !entry.attempted || bypass {
                entry.attempted = true;
                match discover_models(&provider.base_url, &provider.api_key).await {
                    Ok(models) => entry.models = models,
                    Err(_) => entry.models.clear(),
                }
            }
            // A discovered model becomes a `Model`: the provider's
            // `base_url` / `api_key`; `context_window` falls back to
            // `DEFAULT_CONTEXT_WINDOW` (a user model has no static metadata);
            // the thinking fields map straight from the `DiscoveredMeta`
            // (`None` → `vec![]` / `false` — the `refresh_model_metadata`
            // field-mapping pattern, minus the static-value fallback); v1 is
            // OpenAI-compatible only (ADR 0012).
            for (id, meta) in &entry.models {
                user_models.push(Model {
                    id: id.clone(),
                    provider: provider.id.clone(),
                    base_url: provider.base_url.clone(),
                    api_key: provider.api_key.clone(),
                    context_window: meta.context_window.unwrap_or(DEFAULT_CONTEXT_WINDOW),
                    cost_per_mtok_in: 0.0,
                    cost_per_mtok_out: 0.0,
                    supports_tools: true,
                    supports_thinking: meta.supports_thinking.unwrap_or(false),
                    thinking_levels: meta.thinking_levels.clone().unwrap_or_default(),
                    api: Some("openai-completions".to_string()),
                });
            }
        }
        // EVERY configured provider id shadows (regardless of whether its
        // discovery succeeded — a provider that discovered 0 models still
        // replaces the stale seeded models for its id, ADR 0014).
        let shadowed: Vec<String> = settings.providers.iter().map(|p| p.id.clone()).collect();
        merge_catalog(&self.catalog, &user_models, &shadowed)
    }

    /// Attach the persistence database. Sets BOTH `db` (transcript
    /// persistence) and `trust_db` (the ADR 0010 trust lookup) to the same
    /// db — main-session behavior is unchanged (same db, same lookup).
    /// Persistence is a no-op without it.
    pub fn attach_db(&mut self, db: Arc<Db>) {
        self.driver.db = Some(db.clone());
        self.driver.trust_db = Some(db);
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
    ///
    /// ALSO wires the native-harness deps onto the manager (set-once via
    /// `set_native_deps` — a `&self` `OnceLock`, so it's callable through
    /// the `Arc` received here): the `db` is the SIGNAL that native
    /// wiring is present (skipped when `None` — the manager stays
    /// external-pi-only); it is NOT passed (the throwaway child `Db` is
    /// built fresh in `dispatch_native`). `trust_db` IS threaded (the
    /// SAME db — ADR 0010: a native child in a trusted Space inherits
    /// the parent's trust, matching the external `dispatch`); it is
    /// `Some` whenever `db` is (both are set together by `attach_db`).
    /// In production the native-session path always `attach_db`s, so
    /// this is set.
    pub fn set_subagent_manager(&mut self, m: Arc<crate::agent::subagent::SubagentSessionManager>) {
        self.driver.subagent = Some(m.clone());
        if let Some(_db) = &self.driver.db {
            m.set_native_deps(crate::agent::subagent::NativeDeps {
                provider_factory: self.provider_factory.clone(),
                catalog: self.catalog.clone(),
                todo_store: self.driver.todo_store.clone(),
                sudo: SudoDeps {
                    runner: self.driver.runner.clone(),
                    pending_sudo: self.driver.pending_sudo.clone(),
                    sudo_password: self.driver.sudo_password.clone(),
                },
                settle_timeout: self.driver.settle_timeout,
                trust_db: self.driver.trust_db.clone(),
            });
        }
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
            // (settings) A start/resume updates or creates the space row
            // (and `resume` re-touches `last_opened_at`): a space is
            // born/touched when a conversation starts or resumes in it.
            // A NEW row is born `trusted` per the settings'
            // `default_trust_new_spaces` (the sync `load_settings` —
            // `record_session` is sync); the CONFLICT branch never touches
            // an existing row's flag (no retroactive trust).
            let _ = db.upsert_space(
                &info.cwd.display().to_string(),
                load_settings(&self.config_dir).default_trust_new_spaces,
            );
        }
    }

    /// The NATIVE session (Task 7): resolve the model (the harness's
    /// `default_model` → the `ModelCatalog`), build the `Provider` (the
    /// `provider_factory` seam), the `SessionStore` (the `native_messages`
    /// table), and the `AgentLoop` — `tokio::spawn` it (IN-PROCESS; no
    /// subprocess), drive it (the `drive_native_session` driver task —
    /// `pending_turn` / `settle_tx` / `close_kind` populated IDENTICALLY to
    /// the external path), and record the session (the `capabilities_json`
    /// has NO `piSessionFile` — resume is from the `native_messages`
    /// table, so `loadSession` is `true`).
    ///
    /// The `config_options` are SYNTHESIZED from the `ModelCatalog` (the
    /// existing `synthesize_config_options` shape — the frontend is
    /// unchanged; a new `get_models` command driving a native-only picker
    /// would be a UI change, so there is none).
    async fn start_native_session(
        &self,
        entry: &AgentEntry,
        cwd: PathBuf,
        sink: &Arc<dyn EventSink>,
    ) -> Result<SessionInfo, RpcError> {
        let info = self.build_native_session(entry, cwd, sink, None).await?;
        self.record_session(&info);
        Ok(info)
    }

    /// The NATIVE resume (Task 7, reviewer-corrected Major #18): a native
    /// session's `capabilities_json` has NO `piSessionFile` (the external
    /// path would be `NotResumable`), so `kind: native` routes HERE — a
    /// fresh `AgentLoop` + `SessionStore::load_messages` (resume from the
    /// `native_messages` table). The model comes from the stored
    /// `capabilities.model` (a stale / unknown key falls back to the
    /// harness / catalog default); the thinking level from the stored
    /// `thinkingLevel` (the raw value — the resolution chain (memory →
    /// stored → harness seed) resolves in `build_native_session`, after
    /// the model metadata refresh).
    async fn resume_native_session(
        &self,
        entry: &AgentEntry,
        session_id: &str,
        cwd: PathBuf,
        sink: &Arc<dyn EventSink>,
    ) -> Result<SessionInfo, RpcError> {
        let db = self.driver.db.clone().ok_or_else(|| {
            RpcError::Io("a native session requires an attached database".to_string())
        })?;
        // The stored row must exist (a native session is recorded at start
        // — `record_session`; a missing row is unresumable, like the
        // external path's missing row). KEEP THE WHOLE ROW: the resume
        // carries the desktop's `archived` flag (ADR 0016 — the desktop
        // is the source of truth; a resumed session may be re-archived
        // later and the client's sticky view must agree with the DB).
        let row = db
            .session(session_id)
            .ok()
            .flatten()
            .ok_or_else(|| RpcError::NotResumable {
                id: session_id.to_string(),
            })?;
        let caps_json = row.capabilities_json;
        let caps: Value = serde_json::from_str(&caps_json).unwrap_or(Value::Null);
        let harness = entry.harness.as_ref().ok_or_else(|| RpcError::Command {
            error: "the native entry has no harness config".to_string(),
        })?;
        // The model: the stored `model` (a composed key → the EFFECTIVE
        // catalog — a user-provider model resolves); an absent / stale key
        // falls back to the resolution chain (the harness / the settings /
        // the catalog default — never a hard error; the transcript still
        // loads).
        let catalog = self.effective_catalog(None).await;
        let settings = load_settings(&self.config_dir);
        let model = caps
            .get("model")
            .and_then(Value::as_str)
            .and_then(|key| resolve_composed_model(&catalog, key))
            .or_else(|| {
                resolve_native_model(&catalog, &harness.default_model, &settings.default_model).ok()
            });
        let Some(model) = model else {
            return Err(RpcError::Command {
                error: "no models available for the native session".to_string(),
            });
        };
        // The raw STORED `thinkingLevel` (NO harness fallback — the
        // resolution chain (memory → stored → harness seed) resolves in
        // `build_native_session`, after the model metadata refresh).
        let thinking_level = caps
            .get("thinkingLevel")
            .and_then(Value::as_str)
            .map(str::to_string);

        let mut info = self
            .build_native_session(entry, cwd, sink, Some((session_id, model, thinking_level)))
            .await?;
        // The desktop's `archived` flag (ADR 0016): a native start mints a
        // fresh session (`build_native_session` reports `false`); a resume
        // carries the stored row's flag (the `record_session` re-record
        // below never clears it — the `DO UPDATE` branch never touches
        // `archived`).
        info.archived = row.archived;
        self.record_session(&info);
        Ok(info)
    }

    /// Build + spawn + drive one native session (shared by `start` / `resume`;
    /// `resume` carries the stored `session_id` + the loaded transcript
    /// source — `start` mints a fresh UUID and starts with an empty
    /// transcript).
    async fn build_native_session(
        &self,
        entry: &AgentEntry,
        cwd: PathBuf,
        sink: &Arc<dyn EventSink>,
        resume: Option<(&str, Model, Option<String>)>,
    ) -> Result<SessionInfo, RpcError> {
        let db = self.driver.db.clone().ok_or_else(|| {
            RpcError::Io("a native session requires an attached database".to_string())
        })?;
        let harness = entry.harness.as_ref().ok_or_else(|| RpcError::Command {
            error: "the native entry has no harness config".to_string(),
        })?;
        // (finding 13b) The harness `provider` must be
        // `"openai-compatible"` (v1 is OpenAI-compatible only — ADR
        // 0012): any other value is REJECTED at session start rather
        // than silently accepted (pre-fix it got the OpenAI wire
        // regardless).
        if harness.provider != "openai-compatible" {
            return Err(RpcError::Command {
                error: format!(
                    "unsupported harness provider `{}` (v1 is OpenAI-compatible only, ADR 0012)",
                    harness.provider
                ),
            });
        }
        // The EFFECTIVE catalog (Task 2 — the seeded catalog + the user's
        // providers, a fresh `load_settings` read; the per-provider
        // `discovery_cache` makes the fetch cheap): the model resolution,
        // the `AgentLoop`'s catalog, and the synthesized config options
        // all run against it (a user-provider model is selectable +
        // switchable in-session).
        let catalog = self.effective_catalog(None).await;
        // The model: the resolution chain (the harness's `default_model`
        // → the `Settings.default_model` (a fresh `load_settings` read)
        // → the catalog's `default_model` → the v1-selectable
        // (`openai_compatible`) set — an unresolvable key at any rung
        // falls through to the next rung). A resume overrides it with the
        // stored model (see `resume_native_session`).
        let settings = load_settings(&self.config_dir);
        let is_resume = resume.is_some();
        // The resume carries the raw STORED `thinkingLevel` (NO harness
        // fallback — the resolution chain (memory → stored → harness seed)
        // resolves BELOW, after the model metadata refresh); the start arm
        // has no stored level.
        let (session_id, model, stored_level) = match resume {
            Some((id, model, level)) => (id.to_string(), model.clone(), level),
            None => (
                uuid::Uuid::new_v4().to_string(),
                resolve_native_model(&catalog, &harness.default_model, &settings.default_model)?,
                None,
            ),
        };
        // (live `/v1/models` discovery) Best-effort refresh the model's
        // metadata from the provider's live endpoint (the OpenAI endpoint
        // "supplies everything" — the `pi-provider-litellm` `fetchModels`
        // pattern). Bounded + cached per-provider; a failure degrades to
        // the static (`models-store.json`) metadata.
        let model = self.refresh_model_metadata(&model).await;
        // (ADR 0015) The effective thinking level: the remembered (VALIDATED
        // against the model's live `thinking_levels`) > the stored (LENIENT
        // — non-empty levels must be a member; empty levels apply as-is, the
        // pre-change behavior) > the harness seed > `None` (the model's own
        // default). Validated against the POST-refresh model (the live
        // `thinking_levels` are the freshest).
        let thinking_level = remembered_thinking_level(&settings.default_thinking_levels, &model)
            .or_else(|| {
                stored_level
                    .as_ref()
                    .filter(|l| {
                        model.thinking_levels.is_empty()
                            || model.thinking_levels.iter().any(|t| t == *l)
                    })
                    .cloned()
            })
            .or_else(|| harness.default_thinking_level.clone());

        let store = SessionStore::new(db.clone());
        let (events_tx, events_rx) = mpsc::channel(256);
        let (prompt_tx, prompt_rx) = mpsc::channel(8);
        let cancel = CancellationToken::new();
        // The TURN cancel (finding 8c): SHARED with the handle — the loop
        // arms a fresh token per prompt; a `cancel_session` Stop cancels
        // the CURRENT turn only (the session stays alive, matching the
        // external `abort`), a `close_session` (`handle.close`) tears the
        // loop down (the `cancel` token).
        let turn_cancel: Arc<StdMutex<CancellationToken>> =
            Arc::new(StdMutex::new(CancellationToken::new()));
        // The settle watch (finding 3): the loop writes it on every
        // `agent_settled` (a watch send is NEVER dropped — a full / slow
        // `events` mpsc cannot lose the settle); the driver's settle arm
        // consumes it.
        let (settle_tx, settle_rx) = watch::channel(0u64);
        // The `Provider` (the `provider_factory` seam — reviewer-corrected
        // Major #21: the production default is `OpenAiCompatibleProvider`,
        // a test sets a mock BEFORE `start_session`).
        let provider = (self.provider_factory)(&model);
        let mut loop_ = AgentLoop::new(
            session_id.clone(),
            cwd.clone(),
            model.clone(),
            provider,
            catalog.clone(),
            store.clone(),
            events_tx,
            cancel.clone(),
            turn_cancel.clone(),
            settle_tx,
            prompt_tx.clone(),
            prompt_rx,
            self.driver.pending_permissions.clone(),
            self.driver.pending_bridge.clone(),
            self.driver.trust_db.clone(),
            sink.clone(),
            self.driver.todo_store.clone(),
            self.driver.subagent.clone(),
            SudoDeps {
                runner: self.driver.runner.clone(),
                pending_sudo: self.driver.pending_sudo.clone(),
                sudo_password: self.driver.sudo_password.clone(),
            },
            RetryPolicy::new(),
        );
        // The harness config's `enabled_tools` (`[]` = all — finding
        // 13b: a disabled tool is a tool-result error, NOT executed).
        // The `[]` = all convention maps to `None` (all); a non-empty
        // set is `Some(v)` (exactly `v`).
        let parent_enabled_tools = harness.enabled_tools.clone();
        loop_.set_enabled_tools(if parent_enabled_tools.is_empty() {
            None
        } else {
            Some(parent_enabled_tools)
        });
        // The default thinking level (the harness's; a resume overrides it
        // with the stored `thinkingLevel`).
        if let Some(level) = &thinking_level {
            loop_.set_thinking_level(Some(level.clone()));
        }
        // (MOVED UP) the handle — the `info` block below reads
        // `handle.config_state()`.
        let handle = NativeHandle::new(
            prompt_tx,
            loop_.control_tx.clone(),
            cancel,
            turn_cancel,
            model.clone(),
            thinking_level.clone(),
        );
        // The `SessionInfo` (the `capabilities_json` has NO `piSessionFile` —
        // resume is from the `native_messages` table; the `config_options`
        // are SYNTHESIZED from the `ModelCatalog` in the existing shape —
        // the frontend is unchanged). Block-scoped so the `MutexGuard`
        // (and the `Arc` it borrows through) die BEFORE the `await` below
        // (a `std::sync::MutexGuard` is not `Send` — the Tauri command's
        // future must be `Send`).
        let info = {
            let state_guard = handle.config_state();
            let state = state_guard.lock().unwrap();
            SessionInfo {
                session_id: session_id.clone(),
                agent_id: entry.id.clone(),
                cwd: cwd.clone(),
                capabilities: native_capabilities(&state.model, state.thinking_level.as_deref()),
                config_options: synthesize_catalog_config_options(
                    &catalog,
                    &state.model,
                    state.thinking_level.as_deref(),
                ),
                // A native START mints a fresh session (ADR 0016); a
                // resume overrides `archived` with the stored row's flag
                // (`resume_native_session`).
                archived: false,
            }
        };
        // (NEW) The `sessions` row BEFORE the seq-0 persist (the FK fix):
        // `native_messages.session_id` references `sessions(id)`
        // (`PRAGMA foreign_keys = ON`) — the caller's `record_session`
        // runs AFTER `build_native_session` returns, so the seq-0 persist
        // below would hit an FK violation and be silently dropped without
        // this early record (the caller's `record_session` stays — an
        // idempotent refresh).
        self.record_session(&info);
        // (NEW) The system prompt (ADR 0017) — NEW sessions only: a
        // resume replays the stored transcript verbatim (the
        // `load_transcript` below restores the system message at index 0;
        // NO rebuild — a changed `AGENTS.md` applies from the next new
        // session). Built from the `advertised_specs` (the `<tools>`
        // section matches the `tools[]` API param) + the discovered skills
        // (ADR 0013) + the global context dir `~/.pi/agent` (best-effort:
        // no home dir → no global file).
        if !is_resume {
            let agent_dir = crate::skills::home_dir()
                .map(|h| h.join(".pi/agent"))
                .unwrap_or_default();
            let skills = crate::skills::discover_skills(Some(&cwd));
            let specs = loop_.advertised_specs();
            let prompt = build_main_prompt(&PromptContext {
                cwd: &cwd,
                agent_dir: &agent_dir,
                tools: &specs,
                skills: &skills,
            });
            loop_.prepend_system(prompt);
        }
        // A RESUME: `load_messages` restores the stored provider transcript
        // (the `native_messages` table) BEFORE the first model call (the
        // `Compactor` is re-estimated on the loaded context).
        if is_resume {
            // A corrupt transcript row must NOT silently load as an empty
            // one (store.rs): the next persist would upsert over the
            // stored rows (seq 0 = the system prompt, cascading to seq
            // 1, 2, …), so a load failure fails the session start.
            let messages = store
                .load_messages(&session_id)
                .map_err(|e| RpcError::Io(e.to_string()))?;
            loop_.load_transcript(messages);
        }
        // SPAWN the loop task (in-process — no subprocess; the external
        // path's `PiRpc::spawn` is NEVER reached for a native kind).
        let loop_handle = tokio::spawn(loop_.run());
        // The test-only seam: store the `AbortHandle` so a test can kill the
        // loop task DIRECTLY (no token cancel → no settle → deterministic
        // `changed()` `Err`).
        handle.set_loop_task(loop_handle.abort_handle());
        self.driver
            .drive_native_session(
                handle,
                events_rx,
                settle_rx,
                &entry.id,
                cwd,
                sink,
                info.clone(),
            )
            .await?;
        Ok(info)
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

        // The NATIVE backend (Task 7): a `kind: native` entry carries a
        // harness config, NOT a spawn spec — spawn an in-process `AgentLoop`
        // task (the external path below is UNCHANGED for `kind: external`).
        if entry.kind == AgentKind::Native {
            return self.start_native_session(entry, cwd, sink).await;
        }

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
        // (the extension is inert without the env var). The tools override
        // (Phase 1 + Phase 2): a SECOND `-e <tools.ts>` (inert without the
        // bridge env, which the bridge setup above already set when
        // available) + `--no-builtin-tools` ONLY when the override will
        // actually register the built-ins (a Linux bridge spawn — the
        // override is self-gated on the platform; `tools_path` is `Some`
        // on EVERY platform, so keying the flag on it would strip pi's
        // built-ins off-Linux with nothing to replace them → zero tools).
        let args = crate::agent::tools::spawn_args(
            &entry.args,
            self.gate_path.as_deref(),
            self.tools_path.as_deref(),
            bridge_setup.is_some() && cfg!(target_os = "linux"),
        );
        let mut agent_env = agent_env;
        if self.gate_path.is_some() {
            crate::agent::gate::gate_env(&mut agent_env);
        }
        let rpc = PiRpc::spawn(&entry.command, &args, &agent_env, &cwd)?;
        let handle = rpc.handle();

        let agent_id_owned = agent_id.to_string();
        let cwd_owned = cwd.clone();
        // The `config_dir` is captured into the establisher closure (a
        // `PathBuf` — cloned; the closure is `move`).
        let config_dir = self.config_dir.clone();

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
                    // The default model (settings): a validly-shaped
                    // `default_model` (a `"provider/id"` split — the CATALOG
                    // is NOT consulted: pi's own `get_available_models` is
                    // the real source for an external session, and a model
                    // pi doesn't know about is rejected by pi) → `set_model`
                    // sent BEFORE the first `get_state` (LENIENT — a failure
                    // is logged and the session establishes on pi's own
                    // default; an absent/unset setting sends nothing). The
                    // `get_state` response then reflects the applied model
                    // (`build_capabilities` picks up `state.model`).
                    let settings = load_settings(&config_dir);
                    if let Some(key) = settings.default_model.clone() {
                        if let Some((provider, model_id)) = key.split_once('/') {
                            if let Err(e) = handle
                                .send(json!({
                                    "type": "set_model",
                                    "provider": provider,
                                    "modelId": model_id
                                }))
                                .await
                            {
                                eprintln!(
                                    "settings default model: set_model failed at start: {e} (establishing on pi's default)"
                                );
                            }
                        }
                    }
                    // (ADR 0015) The remembered thinking level for the
                    // starting model: sent LENIENT after the `set_model`
                    // (a failure is logged — the session establishes on pi's
                    // own default level; pi is the authority, so NO
                    // validation). When `defaultModel` is absent the
                    // starting model is unknown → nothing is sent.
                    if let Some(key) = &settings.default_model {
                        if let Some(level) = settings.default_thinking_levels.get(key) {
                            if let Err(e) = handle
                                .send(json!({
                                    "type": "set_thinking_level",
                                    "level": level
                                }))
                                .await
                            {
                                eprintln!(
                                    "remembered thinking level: set_thinking_level failed at start: {e} (establishing on pi's default level)"
                                );
                            }
                        }
                    }
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
                        // A fresh START is never archived (ADR 0016).
                        archived: false,
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

        // The NATIVE branch (reviewer-corrected Major #18) — BEFORE the
        // `piSessionFile` extraction below (a native session's
        // `capabilities_json` has NO `piSessionFile`, so the external path
        // would be `NotResumable`): route `kind: native` to a fresh
        // `AgentLoop` + `SessionStore::load_messages` (resume from the
        // `native_messages` table).
        if entry.kind == AgentKind::Native {
            return self
                .resume_native_session(entry, session_id, cwd, sink)
                .await;
        }

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
        // The tools override (Phase 1 + Phase 2): a second `-e <tools.ts>`
        // + `--no-builtin-tools` ONLY when the override will actually
        // register the built-ins (a Linux bridge spawn — see `start_session`
        // for the zero-tools regression the condition guards against).
        let args = crate::agent::tools::spawn_args(
            &args,
            self.gate_path.as_deref(),
            self.tools_path.as_deref(),
            bridge_setup.is_some() && cfg!(target_os = "linux"),
        );
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
                    // The desktop's `archived` flag (ADR 0016): the desktop
                    // is the source of truth (a resumed session may be
                    // re-archived later; the client's sticky view must
                    // agree with the DB). A missing row fails closed to
                    // `false` — the command's pre-check has already
                    // rejected an unresumable row, so in practice the row
                    // exists.
                    let archived = db
                        .as_ref()
                        .and_then(|db| db.session(&session_id_owned).ok().flatten())
                        .map(|row| row.archived)
                        .unwrap_or(false);
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
                        archived,
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
        // Clone just the (cheap) backend handle, not the whole LiveSession.
        let (backend, pending_turn, cancel_requested) = {
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

        // A NATIVE session is one-turn-at-a-time (the frontend's
        // composer is locked until the turn resolves): a turn ALREADY
        // IN FLIGHT (the `pending_turn` slot is occupied) is REJECTED
        // rather than queued — the slot is a single last-wins resolver,
        // and a queued prompt would be settled by the PREVIOUS turn's
        // `agent_settled` (mis-attribution: the composer unlocks while
        // a turn is still live). The external path keeps its
        // steer/last-wins behavior UNCHANGED (a single steer turn).
        //
        // The check-and-claim is ATOMIC under ONE `pending_turn` lock
        // acquisition (finding 2 — the pre-fix check DROPPED the lock, then
        // `begin_user_turn` + `record_message` (two awaits) ran before the
        // resolver was stored: two concurrent `send_prompt`s both observed
        // an empty slot, both passed, and the second's `*slot = Some(tx)`
        // dropped the first's sender (a phantom `Cancelled`) while the
        // second's resolver was resolved by the FIRST turn's
        // `agent_settled` (the composer unlocked mid-turn). Claiming the
        // resolver BEFORE the user-row write / dispatch closes it: the
        // user-row write order vs. the resolver is not load-bearing — what
        // matters is that the check-and-claim is atomic (a rejected prompt
        // writes nothing and overwrites nothing; an accepted prompt's
        // resolver is claimed before the dispatch, so a settle arriving
        // while the prompt is in flight is never lost on an empty slot).
        let (tx, rx) = oneshot::channel::<StopReason>();
        {
            let mut slot = pending_turn.lock().unwrap_or_else(|p| p.into_inner());
            // Native: rejected if a turn is already in flight (the slot is
            // occupied); external: last-wins (a replaced turn's sender is
            // dropped → its `send_prompt` resolves `Cancelled` below).
            if matches!(&backend, SessionBackend::Native(_)) && slot.is_some() {
                return Err(RpcError::Command {
                    error: "a turn is already in flight".to_string(),
                });
            }
            *slot = Some(tx);
        }
        // Reset the cancel flag (a fresh turn is not a cancel).
        *cancel_requested.lock().unwrap_or_else(|p| p.into_inner()) = false;

        // Record the user's message in the transcript (the client owns
        // history) before the turn begins.
        self.begin_user_turn(session_id).await;
        if let Some(db) = &self.driver.db {
            let payload = user_message_payload(&text, &images);
            let _ = db.record_message(session_id, "user", None, &payload.to_string());
        }

        // Dispatch the prompt (the `SessionBackend` generalization, Task 7):
        // the EXTERNAL path is UNCHANGED (the pi `prompt` command — the text
        // message + the image content + a `get_state` steer check); the
        // NATIVE path queues the text + the image attachments on the loop's
        // prompt queue (a full queue is a best-effort drop, mapped to the
        // same error path as a pi refusal below).
        let send_error = match &backend {
            SessionBackend::Pi(handle) => {
                // Build the prompt command: the text message + the image
                // content (pi's `ImageContent` = `{type: "image", data,
                // mimeType}` — the `ImagePayload` maps onto it verbatim; the
                // `name` / `sizeBytes` are transcript-only, not wire fields).
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
                // A prompt while the session is already streaming is a STEER
                // (the turn continues with the new input — the frontend's
                // composer is locked until the turn resolves, so a
                // concurrent send means the previous turn is still running).
                if let Ok(state) = handle.send(json!({ "type": "get_state" })).await {
                    if state.get("isStreaming").and_then(Value::as_bool) == Some(true) {
                        command["streamingBehavior"] = json!("steer");
                    }
                }
                handle.send(command).await.err()
            }
            SessionBackend::Native(handle) => {
                // The wire `ImagePayload` maps onto the loop's `ImageRef`
                // (the `name` / `sizeBytes` are transcript-only — the
                // model transcript carries the `data` + `mimeType`).
                let image_refs: Vec<ImageRef> = images
                    .iter()
                    .map(|img| ImageRef {
                        data: img.data.clone(),
                        mime_type: img.mime_type.clone(),
                    })
                    .collect();
                if handle.send_prompt(&text, &image_refs) {
                    None
                } else {
                    Some(RpcError::Command {
                        error: "the native session is busy; the prompt was dropped".to_string(),
                    })
                }
            }
        };

        // A `success: false` prompt response is a REFUSAL — emit the error
        // as a chunk (the user sees it) and resolve the turn `Refusal`
        // without waiting for a settle that never comes.
        if let Some(e) = send_error {
            match e {
                RpcError::Command { error } => {
                    let mut slot = pending_turn.lock().unwrap_or_else(|p| p.into_inner());
                    if let Some(tx) = slot.take() {
                        let _ = tx.send(StopReason::Refusal);
                    }
                    return Err(RpcError::Command { error });
                }
                other => return Err(other),
            }
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
    ///
    /// BEHAVIOR (finding 8c, documented per the reviewer's request): the
    /// EXTERNAL path is the pi `abort` — the agent aborts the TURN and the
    /// session STAYS ALIVE (a new prompt reuses it). The NATIVE path
    /// matches it: `handle.cancel()` cancels the loop's current TURN token
    /// (the loop settles the turn `Cancelled` and STAYS ALIVE — a new
    /// prompt reuses the session; only a `close_session` — `handle.close`
    /// — tears the native session down). Pre-fix the native Stop cancelled
    /// the loop's teardown token, which ENDED THE WHOLE SESSION (the
    /// driver tore it down) — a silent asymmetry with the external path.
    pub async fn cancel_session(&self, session_id: &str) -> Result<(), RpcError> {
        let (backend, cancel_requested) = {
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
        // The `SessionBackend` dispatch (Task 7): the EXTERNAL path is the
        // pi `abort` command (UNCHANGED — stop the turn, keep the session
        // alive); the NATIVE path cancels the loop's current TURN token
        // (finding 8c — the native Stop matches the external `abort`:
        // the turn stops and the session STAYS ALIVE — a new prompt
        // reuses it; only a `close_session` tears the native session
        // down). The `cancel_requested` flag (set above) maps the settle
        // to `Cancelled`.
        match &backend {
            SessionBackend::Pi(handle) => {
                handle.send(json!({ "type": "abort" })).await?;
            }
            SessionBackend::Native(handle) => {
                handle.cancel();
            }
        }
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
        let (backend, config_state) = {
            let sessions = self.driver.sessions.lock().await;
            let live = sessions
                .get(session_id)
                .map(|l| {
                    let state = match &l.handle {
                        SessionBackend::Native(handle) => Some(handle.config_state()),
                        SessionBackend::Pi(_) => None,
                    };
                    (l.handle.clone(), state)
                })
                .ok_or_else(|| RpcError::UnknownSession {
                    id: session_id.to_string(),
                })?;
            live
        };

        // The `SessionBackend` dispatch (Task 7): the EXTERNAL path is
        // UNCHANGED (the pi `set_model` / `set_thinking_level` commands +
        // re-synthesize from `get_state` / `get_available_models` /
        // `get_available_thinking_levels`); the NATIVE path routes to
        // `AgentLoop::set_model` / `set_thinking_level` (the loop's control
        // channel — applied when the loop is idle) and RE-SYNTHESIZES FROM
        // THE `ModelCatalog` (a native session has no `get_state`).
        match &backend {
            SessionBackend::Pi(handle) => {
                // The config id → the pi command. `model` values are
                // `"<provider>/<modelId>"` (the synthesizer's option
                // values).
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
                    "thought_level" => {
                        json!({ "type": "set_thinking_level", "level": value })
                    }
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
                // (ADR 0015) An EXPLICIT thinking-level change remembers
                // the level for the session's CURRENT model (the
                // `get_state` response's `model` — absent → skip;
                // best-effort: a write failure is logged and does NOT
                // fail the config change, which pi applied either way).
                if config_id == "thought_level" && !value.is_empty() {
                    if let Some(model) = state.get("model") {
                        let provider = model
                            .get("provider")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        let id = model.get("id").and_then(Value::as_str).unwrap_or_default();
                        let key = format!("{provider}/{id}");
                        let mut settings = load_settings(&self.config_dir);
                        settings
                            .default_thinking_levels
                            .insert(key, value.to_string());
                        if let Err(e) = write_settings(&self.config_dir, &settings) {
                            eprintln!("remembered thinking level: save failed: {e}");
                        }
                    }
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
            SessionBackend::Native(handle) => {
                let Some(state) = config_state else {
                    return Err(RpcError::Command {
                        error: "no config options available".to_string(),
                    });
                };
                // The EFFECTIVE catalog (Task 2 — the seeded catalog +
                // the user's providers): the model lookup + the re-
                // synthesizer run against it (a user-provider model can
                // be switched TO mid-session, not just the seeded ones).
                let catalog = self.effective_catalog(None).await;
                // Apply (the loop's control channel — `AgentLoop::set_model`
                // / `set_thinking_level` on the loop task) + mirror the
                // change on the handle's config state (the re-synthesizer
                // source). Mirror + emit ONLY when `try_send` SUCCEEDS
                // (finding 12): a full / closed queue means the loop never
                // applies the change — claiming success (a mirrored state +
                // a `config_option_update`) would silently diverge from
                // the model / level the loop is actually running.
                match config_id {
                    "model" => {
                        let (provider, model_id) =
                            value
                                .split_once('/')
                                .ok_or_else(|| RpcError::InvalidPrompt {
                                    reason: format!(
                                        "invalid model value: {value} (expected provider/modelId)"
                                    ),
                                })?;
                        let model = catalog
                            .models
                            .iter()
                            .find(|m| m.provider == provider && m.id == model_id)
                            .cloned()
                            .ok_or_else(|| RpcError::Command {
                                error: format!("unknown model: {value}"),
                            })?;
                        let sent = handle
                            .control_tx_clone()
                            .try_send(ControlCmd::SetModel(model.clone()));
                        if sent.is_err() {
                            return Err(RpcError::Command {
                                error: "the session's loop is not running; the config change could not be applied".to_string(),
                            });
                        }
                        let mut state = state.lock().unwrap_or_else(|p| p.into_inner());
                        state.model = model.clone();
                        // (ADR 0015) Minimal-surprise reset: the current
                        // level is KEPT across the switch when valid for
                        // the new model (its `thinking_levels` are
                        // non-empty and contain it — or EMPTY, the status
                        // quo); it is replaced (the new model's
                        // remembered level, or `None`) only when the new
                        // model doesn't support it. The second `try_send`
                        // mirrors ONLY on success (finding 12): a failed
                        // send leaves the mirror untouched (the loop never
                        // applies the reset). The arm does NOT write
                        // memory.
                        if let Some(level) = state.thinking_level.as_deref() {
                            let valid = model.thinking_levels.is_empty()
                                || model.thinking_levels.iter().any(|t| t == level);
                            if !valid {
                                let reset = remembered_thinking_level(
                                    &load_settings(&self.config_dir).default_thinking_levels,
                                    &model,
                                );
                                let sent = handle
                                    .control_tx_clone()
                                    .try_send(ControlCmd::SetThinkingLevel(reset.clone()));
                                if sent.is_ok() {
                                    state.thinking_level = reset;
                                }
                            }
                        }
                    }
                    "thought_level" => {
                        let sent = handle
                            .control_tx_clone()
                            .try_send(ControlCmd::SetThinkingLevel(Some(value.to_string())));
                        if sent.is_err() {
                            return Err(RpcError::Command {
                                error: "the session's loop is not running; the config change could not be applied".to_string(),
                            });
                        }
                        let mut state = state.lock().unwrap_or_else(|p| p.into_inner());
                        state.thinking_level = Some(value.to_string());
                        // (ADR 0015) Remember the level for the session's
                        // CURRENT model (best-effort: a write failure is
                        // logged and does NOT fail the config change — the
                        // level is applied in the loop either way). Only a
                        // non-empty `value` is remembered.
                        if !value.is_empty() {
                            let key = format!("{}/{}", state.model.provider, state.model.id);
                            let mut settings = load_settings(&self.config_dir);
                            settings
                                .default_thinking_levels
                                .insert(key, value.to_string());
                            if let Err(e) = write_settings(&self.config_dir, &settings) {
                                eprintln!("remembered thinking level: save failed: {e}");
                            }
                        }
                    }
                    other => {
                        return Err(RpcError::Command {
                            error: format!("unknown config option: {other}"),
                        })
                    }
                }
                // Re-synthesize FROM THE `ModelCatalog` (a native session
                // has no `get_state`) + emit (the agent does not emit a
                // `config_option_update` itself — the client owns the frame).
                let state = state.lock().unwrap_or_else(|p| p.into_inner());
                let options = synthesize_catalog_config_options(
                    &catalog,
                    &state.model,
                    state.thinking_level.as_deref(),
                )
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
        }
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
            None => {
                // Phase 2: the `pending_bridge` lookup missed — check the
                // `pending_sudo` sub-prompt oneshots (the `sudo_exec`
                // `:confirm` / `:password` keys, `"{sid}/{id}:confirm"` /
                // `"{sid}/{id}:password"`). The legacy `confirm` /
                // `password` flow still resolves via `pending_bridge` (the
                // lookup above is NOT removed).
                let sudo_sender = self.driver.pending_sudo.lock().await.remove(&key);
                match sudo_sender {
                    Some(sender) => {
                        let _ = sender.send(result);
                        Ok(true)
                    }
                    None => Ok(false),
                }
            }
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
        let (close_tx, close_kind, backend, cancel_requested) = {
            let sessions = self.driver.sessions.lock().await;
            let live = sessions
                .get(session_id)
                .map(|l| {
                    (
                        l.close_tx.clone(),
                        l.close_kind.clone(),
                        l.handle.clone(),
                        l.cancel_requested.clone(),
                    )
                })
                .ok_or_else(|| RpcError::UnknownSession {
                    id: session_id.to_string(),
                })?;
            live
        };
        // A close KILLS an in-flight turn: mark it a cancel BEFORE the
        // close (the driver's settle arm may win the race over the
        // teardown — the `cancel_requested` flag maps that settle to
        // `Cancelled`, not `EndTurn`, for a turn that was killed, not
        // ended; the teardown arm sends `Cancelled` too, so the outcome
        // is `Cancelled` either way, not timing-dependent).
        *cancel_requested.lock().unwrap_or_else(|p| p.into_inner()) = true;
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
        // The `SessionBackend` dispatch (Task 7): the EXTERNAL path closes
        // the child's stdin (idempotent — the driver teardown may close it
        // too): a clean pi shutdown. The NATIVE path tears the loop down
        // (the prompt queue + the in-flight turn stop; the driver teardown
        // cancels too — idempotent).
        match &backend {
            SessionBackend::Pi(handle) => {
                handle.close().await;
            }
            SessionBackend::Native(handle) => {
                handle.close();
            }
        }
        Ok(())
    }
}

/// Resolve a composed model key (`"<provider>/<id>"` — the catalog / config
/// option key form) to a `Model` (the first `/` split — model ids themselves
/// contain `/`). `None` when the key is malformed or unknown. `pub(crate)` so
/// the `dispatch_native` driver resolves a `launch.model` override (the
/// `:<level>` suffix is stripped by the caller BEFORE calling this — this
/// function does not split it).
pub(crate) fn resolve_composed_model(catalog: &ModelCatalog, key: &str) -> Option<Model> {
    let (provider, id) = key.split_once('/')?;
    catalog
        .models
        .iter()
        .find(|m| m.provider == provider && m.id == id)
        .cloned()
}

/// The remembered thinking level for a model (ADR 0015): the map entry,
/// `Some` only when the model's `thinking_levels` is non-empty AND
/// contains the entry (a stale entry — the provider changed its levels —
/// is ignored; a model with no advertised levels gets nothing).
fn remembered_thinking_level(levels: &HashMap<String, String>, model: &Model) -> Option<String> {
    if model.thinking_levels.is_empty() {
        return None;
    }
    levels
        .get(&format!("{}/{}", model.provider, model.id))
        .filter(|l| model.thinking_levels.iter().any(|t| t == *l))
        .cloned()
}

/// Resolve a native session's model (the harness's `default_model` composed
/// key → the catalog; `None` (the built-in) → the `Settings.default_model`
/// (the middle rung — a fresh `load_settings` read) → the catalog's
/// `default_model` → the v1-selectable (`openai_compatible`) set). Each rung
/// is tried IN ORDER: an UNRESOLVABLE key at any rung falls through to the
/// NEXT rung (only when all three rungs are absent/unresolvable does it fall
/// to the set) — a stale configured default degrades rather than a hard
/// error.
fn resolve_native_model(
    catalog: &ModelCatalog,
    harness_default: &Option<String>,
    settings_default: &Option<String>,
) -> Result<Model, RpcError> {
    for key in [
        harness_default.as_deref(),
        settings_default.as_deref(),
        catalog.default_model.as_deref(),
    ]
    .iter()
    .flatten()
    {
        if let Some(model) = resolve_composed_model(catalog, key) {
            return Ok(model);
        }
    }
    catalog
        .openai_compatible()
        .first()
        .copied()
        .cloned()
        .ok_or_else(|| RpcError::Command {
            error: "no models available for the native session".to_string(),
        })
}

/// The capability envelope for a NATIVE session (the item-1 shape minus the
/// pi keys — there is NO `piSessionFile`: resume is from the
/// `native_messages` table, so `loadSession` is `true` (the frontend's
/// Resume button); `image` is `true` (the native `Prompt` carries image
/// blocks — the provider's `to_wire` sends them as `image_url` parts;
/// `audio` / `embeddedContext` stay fail-closed `false`).
fn native_capabilities(model: &Model, thinking_level: Option<&str>) -> Value {
    let mut caps = json!({
        "native": true,
        "model": format!("{}/{}", model.provider, model.id),
        "loadSession": true,
        "promptCapabilities": { "image": true, "audio": false, "embeddedContext": false },
    });
    if let Some(level) = thinking_level {
        caps["thinkingLevel"] = Value::String(level.to_string());
    }
    caps
}

/// Synthesize a NATIVE session's config options (the model / thinking-level
/// selectors) from the `ModelCatalog` (the existing `synthesize_config_options`
/// shape — the frontend is unchanged; a new `get_models` command driving a
/// native-only picker would be a UI change, so there is none): model options
/// are the `openai_compatible()` ids (`"<provider>/<id>"`); the thinking level
/// is the current model's `thinking_levels` (absent → no selector).
fn synthesize_catalog_config_options(
    catalog: &ModelCatalog,
    current: &Model,
    thinking_level: Option<&str>,
) -> Option<Vec<Value>> {
    let mut out: Vec<Value> = Vec::new();
    let options: Vec<Value> = catalog
        .openai_compatible()
        .iter()
        .map(|m| {
            json!({
                "value": format!("{}/{}", m.provider, m.id),
                "name": m.id.clone(),
            })
        })
        .collect();
    out.push(json!({
        "id": "model",
        "name": "Model",
        "category": "model",
        "type": "select",
        "currentValue": format!("{}/{}", current.provider, current.id),
        "options": options,
    }));
    if !current.thinking_levels.is_empty() {
        let options: Vec<Value> = current
            .thinking_levels
            .iter()
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
            "currentValue": thinking_level.unwrap_or_default(),
            "options": options,
        }));
    }
    (!out.is_empty()).then_some(out)
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
/// emits / persists the frames it returns. `pub(crate)` so the native
/// `AgentLoop` (Task 6) runs its `RpcEvent`s through the SAME pipeline
/// (the frontend is unchanged — the FROZEN frames are identical).
///
/// No-frame events are bookkeeping (`agent_start` / `agent_end` /
/// `turn_start` / `turn_end` / `queue_update` / `entry_appended` /
/// `bash_execution_update` / `message_end` / `text_end` / `thinking_end` —
/// the delta stream already delivered the content, and the authoritative
/// `message_end` text is NOT re-emitted) or the turn's resolution signal
/// (`agent_settled` — the driver resolves the pending turn, not a frame).
pub(crate) fn normalize(e: &RpcEvent, st: &mut TurnState) -> Vec<Value> {
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
                // ANNOUNCE only when the name is known (a non-empty
                // `toolName` — the OpenAI-compatible streaming delivers
                // `function.name` in a LATER delta, so `toolcall_start`
                // often carries `""`). An empty / missing name (or an
                // empty `id`) defers the announcement to `toolcall_end`
                // (where the name is known) — announcing now would make
                // the frontend display the `toolCallId` (e.g.
                // `chatcmpl-tool-…`) instead of the tool name.
                Some("toolcall_start") => {
                    let Some(id) = ev
                        .get("id")
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                    else {
                        return Vec::new();
                    };
                    let Some(name) = ev
                        .get("toolName")
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                    else {
                        return Vec::new();
                    };
                    st.announced_tool_calls.insert(id.to_string());
                    vec![json!({
                        "sessionUpdate": "tool_call",
                        "toolCallId": id,
                        "title": name,
                        "status": "in_progress",
                        "rawInput": {},
                    })]
                }
                // Accumulate the partial-args JSON fragments. For an
                // ANNOUNCED id a complete object is sent as `rawInput`,
                // an incomplete one as `partialArgs` (the adapter-era
                // behavior, kept for the streaming tool-call frames). For
                // an UNANNOUNCED id the delta is BUFFER ONLY — no frame:
                // a `tool_call_update` for an id the frontend never saw
                // would create a message with `title = toolCallId` (e.g.
                // `chatcmpl-tool-…`); the announcement comes from
                // `toolcall_end` (or `tool_execution_start`).
                Some("toolcall_delta") => {
                    let Some(id) = ev.get("id").and_then(Value::as_str) else {
                        return Vec::new();
                    };
                    let delta = ev.get("delta").and_then(Value::as_str).unwrap_or_default();
                    st.toolcall_args
                        .entry(id.to_string())
                        .or_default()
                        .push_str(delta);
                    if !st.announced_tool_calls.contains(id) {
                        return Vec::new();
                    }
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
                // If the id was ALREADY announced (a `toolcall_start` with a
                // name), this is an update. Otherwise (the NATIVE-HARNESS
                // turn — no `toolcall_start` at all — or a
                // `toolcall_start` with an empty name) THIS is the
                // announcement: a `tool_call` frame with the real `title`
                // (NOT a `tool_call_update`, which has no `title`, so the
                // frontend would fall back to displaying the `toolCallId`,
                // e.g. `chatcmpl-tool-…`). A name that never arrived keeps
                // the legacy update (the degenerate case, no worse than
                // before).
                Some("toolcall_end") => {
                    let Some(tc) = ev.get("toolCall") else {
                        return Vec::new();
                    };
                    let Some(id) = tc
                        .get("id")
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                    else {
                        return Vec::new();
                    };
                    st.toolcall_args.remove(id);
                    let Some(name) = tc
                        .get("name")
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                    else {
                        return vec![json!({
                            "sessionUpdate": "tool_call_update",
                            "toolCallId": id,
                            "rawInput": tc.get("arguments"),
                            "status": "in_progress",
                        })];
                    };
                    if st.announced_tool_calls.contains(id) {
                        vec![json!({
                            "sessionUpdate": "tool_call_update",
                            "toolCallId": id,
                            "rawInput": tc.get("arguments"),
                            "status": "in_progress",
                        })]
                    } else {
                        st.announced_tool_calls.insert(id.to_string());
                        vec![json!({
                            "sessionUpdate": "tool_call",
                            "toolCallId": id,
                            "title": name,
                            "status": "in_progress",
                            "rawInput": tc.get("arguments"),
                        })]
                    }
                }
                _ => Vec::new(),
            }
        }
        // The native `tool_execution_start` CARRIES the tool name + args
        // (the provider already accumulated them). If the id was ALREADY
        // announced (a `toolcall_start` with a name, or `toolcall_end` —
        // the native-harness turn), this is an UPDATE: the frontend applies
        // `title` / `rawInput` / `status` in place (a second `tool_call`
        // frame would APPEND a duplicate row). If it was NOT announced (no
        // `toolcall_*` frames at all — the defensive fallback), THIS is the
        // announcement: a `tool_call` frame with the real `title` (NOT just
        // a `tool_call_update`, which has no `title`, so the frontend would
        // fall back to displaying the `toolCallId`, e.g. `chatcmpl-tool-…`).
        RpcEvent::tool_execution_start {
            tool_call_id,
            tool_name,
            args,
        } => {
            if st.announced_tool_calls.contains(tool_call_id) {
                // A `null` `title` is a no-op for the frontend (`update.title
                // ?? prev.title` keeps the existing title) — never overwrite
                // a good title with an empty one.
                let title = if tool_name.is_empty() {
                    Value::Null
                } else {
                    Value::String(tool_name.clone())
                };
                vec![json!({
                    "sessionUpdate": "tool_call_update",
                    "toolCallId": tool_call_id,
                    "title": title,
                    "rawInput": args,
                    "status": "in_progress",
                })]
            } else {
                st.announced_tool_calls.insert(tool_call_id.clone());
                vec![json!({
                    "sessionUpdate": "tool_call",
                    "toolCallId": tool_call_id,
                    "title": tool_name,
                    "status": "in_progress",
                    "rawInput": args,
                })]
            }
        }
        // `rawOutput` is the tool's result (the RPC `AgentToolResult`): a live
        // partial while the tool runs, final on `tool_execution_end`. It is
        // consumed by the frontend (the `tool-call` `Message` carries it) AND
        // persisted into the tool-call row via `merge_json` (so a resume
        // restores the summary + output).
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
/// gone with the crate types). `pub(crate)` so the native `AgentLoop`
/// (Task 6) persists through the SAME function.
pub(crate) fn persist_update(
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

    /// `tool_execution_*` → `tool_call` (start, with the real `title` +
    /// `rawInput`) / `tool_call_update` (partial `rawOutput` /
    /// completed-or-failed + `rawOutput`).
    #[test]
    fn tool_execution_frames_map_to_updates() {
        let mut st = TurnState::default();
        // `tool_execution_start` CARRIES the tool name + args (the provider
        // already accumulated them), so it maps to a `tool_call` frame (with
        // the real `title` + `rawInput`) — NOT a `tool_call_update` (no
        // `title` → the frontend would display the `toolCallId`, e.g.
        // `chatcmpl-tool-…`, instead of the tool name).
        let start = normalize(
            &ev(
                json!({ "type": "tool_execution_start", "toolCallId": "tc1", "toolName": "bash", "args": { "cmd": "ls" } }),
            ),
            &mut st,
        );
        assert_eq!(
            start[0],
            json!({ "sessionUpdate": "tool_call", "toolCallId": "tc1", "title": "bash", "status": "in_progress", "rawInput": { "cmd": "ls" } })
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

    /// The NATIVE-HARNESS flow (no `toolcall_start` at all — the harness
    /// emits `toolcall_delta` + `toolcall_end` only): the FIRST frame for
    /// an id must be a `tool_call` with the real `title` (a
    /// `tool_call_update` for an id the frontend never saw would create a
    /// message with `title = toolCallId`, e.g. `chatcmpl-tool-…`), and
    /// `tool_execution_start` must NOT re-announce (the frontend's
    /// `tool_call` case always APPENDS — a second `tool_call` frame would
    /// duplicate the row).
    #[test]
    fn native_harness_toolcall_end_announces_single_row() {
        let mut st = TurnState::default();
        // `toolcall_delta` (the harness's first frame for the id):
        // buffer only — NO frame.
        for delta in ["{\"cmd\":", "\"ls\"}"] {
            let frames = normalize(
                &ev(json!({
                    "type": "message_update",
                    "usage": null,
                    "assistantMessageEvent": { "type": "toolcall_delta", "id": "chatcmpl-tool-x", "delta": delta },
                })),
                &mut st,
            );
            assert!(
                frames.is_empty(),
                "an unannounced delta must not emit a frame"
            );
        }

        // `toolcall_end` (the name is known here): the ANNOUNCEMENT — a
        // `tool_call` with the real `title` + the full `rawInput`.
        let end = normalize(
            &ev(json!({
                "type": "message_update",
                "usage": null,
                "assistantMessageEvent": {
                    "type": "toolcall_end",
                    "toolCall": { "id": "chatcmpl-tool-x", "name": "bash", "arguments": { "cmd": "ls" } },
                },
            })),
            &mut st,
        );
        assert_eq!(
            end[0],
            json!({ "sessionUpdate": "tool_call", "toolCallId": "chatcmpl-tool-x", "title": "bash", "status": "in_progress", "rawInput": { "cmd": "ls" } })
        );

        // `tool_execution_start` for the ANNOUNCED id: an UPDATE (the
        // frontend applies `title` / `rawInput` / `status` in place), NOT
        // a re-announcement.
        let start = normalize(
            &ev(
                json!({ "type": "tool_execution_start", "toolCallId": "chatcmpl-tool-x", "toolName": "bash", "args": { "cmd": "ls" } }),
            ),
            &mut st,
        );
        assert_eq!(start[0]["sessionUpdate"], "tool_call_update");
        assert_eq!(start[0]["title"], "bash");
        assert_eq!(start[0]["rawInput"], json!({ "cmd": "ls" }));
        assert_eq!(start[0]["status"], "in_progress");

        // `tool_execution_end` completes the single row.
        let done = normalize(
            &ev(
                json!({ "type": "tool_execution_end", "toolCallId": "chatcmpl-tool-x", "toolName": "bash", "result": "done", "isError": false }),
            ),
            &mut st,
        );
        assert_eq!(done[0]["sessionUpdate"], "tool_call_update");
        assert_eq!(done[0]["status"], "completed");
        assert_eq!(done[0]["rawOutput"], "done");
    }

    /// An external-pi `toolcall_start` with an EMPTY `toolName` (the
    /// OpenAI-compatible streaming delivers `function.name` in a LATER
    /// delta) must NOT announce — the announcement is deferred to
    /// `toolcall_end` (where the name is known). A `tool_call` frame with
    /// the empty title would make the frontend display the `toolCallId`
    /// (e.g. `chatcmpl-tool-…`).
    #[test]
    fn empty_toolname_defers_announcement_to_toolcall_end() {
        let mut st = TurnState::default();
        let start = normalize(
            &ev(json!({
                "type": "message_update",
                "usage": null,
                "assistantMessageEvent": { "type": "toolcall_start", "contentIndex": 0, "id": "tc1", "toolName": "" },
            })),
            &mut st,
        );
        assert!(start.is_empty(), "an empty toolName must not announce");

        let delta = normalize(
            &ev(json!({
                "type": "message_update",
                "usage": null,
                "assistantMessageEvent": { "type": "toolcall_delta", "contentIndex": 0, "id": "tc1", "delta": "{\"cmd\":\"ls\"}" },
            })),
            &mut st,
        );
        assert!(delta.is_empty(), "an unannounced id must buffer only");

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
        assert_eq!(
            end[0],
            json!({ "sessionUpdate": "tool_call", "toolCallId": "tc1", "title": "bash", "status": "in_progress", "rawInput": { "cmd": "ls" } })
        );

        // `tool_execution_start` for the announced id: an update, not a
        // re-announcement (no duplicate row).
        let exec_start = normalize(
            &ev(
                json!({ "type": "tool_execution_start", "toolCallId": "tc1", "toolName": "bash", "args": { "cmd": "ls" } }),
            ),
            &mut st,
        );
        assert_eq!(exec_start[0]["sessionUpdate"], "tool_call_update");
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
    use crate::agent::harness::provider::{
        ChatRole, FinishReason, MessageContent, ModelRequest, ProviderError, ProviderEvent,
    };
    use crate::agent::permission::PermissionOutcome;
    use crate::storage::Db;
    use futures_util::StreamExt;
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
            archived: false,
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
            archived: false,
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

    /// (ADR 0016) The EXTERNAL resume carries the stored `archived` flag:
    /// `set_session_archived` before the resume; the resumed `SessionInfo`
    /// reports the stored flag, and the resume's `record_session` re-record
    /// did NOT clear it (the `DO UPDATE` branch never touches `archived`).
    #[tokio::test]
    async fn resume_carries_the_archived_flag() {
        let dir = temp_config_dir();
        write_agents_json_pi(&dir, &[]);
        let db = open_db(&dir);
        let mut manager = SessionManager::new(dir.clone()).unwrap();
        manager.attach_db(db.clone());
        let (tx, _rx) = mpsc::unbounded_channel();
        let sink: Arc<dyn EventSink> = Arc::new(TestSink { tx });

        // Pre-seed the stored row (a resumption of a session with a file)
        // and archive it BEFORE the resume.
        db.record_session(&SessionInfo {
            session_id: "resume-arch-1".to_string(),
            agent_id: "fake".to_string(),
            cwd: dir.clone(),
            capabilities: serde_json::json!({
                "piSessionId": "resume-arch-1",
                "piSessionFile": "/tmp/resume-arch-1.jsonl",
                "model": "fake/fake-model",
                "thinkingLevel": "off",
                "loadSession": true,
                "promptCapabilities": { "image": true, "audio": false, "embeddedContext": false },
            }),
            config_options: None,
            archived: false,
        })
        .expect("record_session should succeed");
        db.set_session_archived("resume-arch-1", true)
            .expect("set_session_archived should succeed");

        let info = crate::test_support::run_with_retry(|| {
            manager.resume_session("fake", "resume-arch-1", dir.clone(), &sink)
        })
        .await
        .unwrap();
        assert_eq!(info.session_id, "resume-arch-1");
        assert!(
            info.archived,
            "the external resume carries the stored archived flag"
        );
        let row = db
            .session("resume-arch-1")
            .expect("session should work")
            .expect("the row exists");
        assert!(
            row.archived,
            "the resume's re-record did not clear the stored flag"
        );

        let _ = manager.close_session(&info.session_id).await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// (ADR 0016) The NATIVE resume carries the stored `archived` flag:
    /// a fresh native start is `archived: false`; after
    /// `set_session_archived`, the resumed `SessionInfo` reports the stored
    /// flag and the resume's `record_session` re-record did NOT clear it.
    #[tokio::test]
    async fn native_resume_carries_the_archived_flag() {
        let dir = temp_config_dir();
        write_agents_json_native(&dir);
        let db = open_db(&dir);
        let mut manager = SessionManager::new(dir.clone()).unwrap();
        manager.attach_db(db.clone());
        manager.set_catalog(native_test_catalog());
        manager.set_provider_factory(|_m: &Model| Box::new(HangingProvider));
        let (tx, _rx) = mpsc::unbounded_channel();
        let sink: Arc<dyn EventSink> = Arc::new(TestSink { tx });

        let info = crate::test_support::run_with_retry(|| {
            manager.start_session("nativetest", dir.clone(), &sink)
        })
        .await
        .expect("the native session started");
        assert!(!info.archived, "a fresh native start is never archived");
        db.set_session_archived(&info.session_id, true)
            .expect("set_session_archived should succeed");

        let resumed = crate::test_support::run_with_retry(|| {
            manager.resume_session("nativetest", &info.session_id, dir.clone(), &sink)
        })
        .await
        .expect("the native resume succeeded");
        assert_eq!(resumed.session_id, info.session_id);
        assert!(
            resumed.archived,
            "the native resume carries the stored archived flag"
        );
        let row = db
            .session(&info.session_id)
            .expect("session should work")
            .expect("the row exists");
        assert!(
            row.archived,
            "the resume's re-record did not clear the stored flag"
        );

        let _ = manager.close_session(&info.session_id).await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// (ADR 0016) `SessionInfo` round-trips the `archived` flag (a
    /// single-word key — the `camelCase` rename leaves it as `archived`).
    #[test]
    fn session_info_round_trips_the_archived_flag_camel_case() {
        let info = SessionInfo {
            session_id: "s1".to_string(),
            agent_id: "fake".to_string(),
            cwd: PathBuf::from("/tmp/proj"),
            capabilities: serde_json::json!({ "loadSession": true }),
            config_options: None,
            archived: true,
        };
        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("\"archived\":true"), "got: {json}");
        let back: SessionInfo = serde_json::from_str(&json).unwrap();
        assert!(back.archived);
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

    /// Write a `settings.json` with the given `defaultModel` (the
    /// camelCase wire shape — `None` = the key present-but-null, which
    /// parses to `default_model: None`).
    fn write_settings_default_model(dir: &Path, default_model: Option<&str>) {
        let settings = serde_json::json!({ "defaultModel": default_model });
        std::fs::write(
            dir.join("settings.json"),
            serde_json::to_string_pretty(&settings).unwrap(),
        )
        .unwrap();
    }

    /// (settings) an EXTERNAL session started with a validly-shaped
    /// `Settings.default_model` (a `"provider/id"` split) gets a
    /// `set_model` sent BEFORE the first `get_state` (LENIENT — a failure
    /// is logged and the session establishes on pi's own default; an
    /// absent/unset setting sends nothing): the `get_state`-based
    /// `info.capabilities.model` reflects the applied model (`fake_pi`'s
    /// `set_model` handler updates `current_model_id` and its
    /// `get_state` response substitutes it).
    #[tokio::test]
    async fn an_external_session_start_sends_set_model_for_the_settings_default() {
        let dir = temp_config_dir();
        write_agents_json_pi(&dir, &[]);
        write_settings_default_model(&dir, Some("fake/fake-model-2"));
        let manager = SessionManager::new(dir.clone()).unwrap();
        let (tx, _rx) = mpsc::unbounded_channel();
        let sink: Arc<dyn EventSink> = Arc::new(TestSink { tx });
        let info = crate::test_support::run_with_retry(|| {
            manager.start_session("fake", dir.clone(), &sink)
        })
        .await
        .expect("the external session started");
        assert_eq!(
            info.capabilities["model"], "fake/fake-model-2",
            "the settings default model is applied before the first get_state"
        );
        let _ = manager.close_session(&info.session_id).await;

        // The negative: NO `defaultModel` → no `set_model` sent → the
        // session establishes on pi's own default.
        write_settings_default_model(&dir, None);
        let manager = SessionManager::new(dir.clone()).unwrap();
        let info = crate::test_support::run_with_retry(|| {
            manager.start_session("fake", dir.clone(), &sink)
        })
        .await
        .expect("the external session started");
        assert_eq!(
            info.capabilities["model"], "fake/fake-model",
            "an absent settings default sends no set_model (pi's own default)"
        );
        let _ = manager.close_session(&info.session_id).await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── Native-backend tests (the in-process `AgentLoop` — a NATIVE
    // registry entry + a mock `provider_factory` seam; the production
    // default is `OpenAiCompatibleProvider`) ──

    /// Write an `agents.json` with a single NATIVE entry (the `harness`
    /// config points at `fake/m1` — the `native_test_catalog` model).
    fn write_agents_json_native(dir: &Path) {
        let agents = serde_json::json!({
            "agents": [{
                "id": "nativetest",
                "name": "Native Test",
                "kind": "native",
                "harness": { "provider": "openai-compatible", "default_model": "fake/m1" },
            }]
        });
        std::fs::write(dir.join("agents.json"), agents.to_string()).unwrap();
    }

    /// The native test catalog (a single `fake/m1` OpenAI-compatible
    /// model — `set_config_option` + `resolve_native_model` resolve it
    /// from the composed key).
    fn native_test_catalog() -> ModelCatalog {
        ModelCatalog {
            models: vec![Model {
                id: "m1".to_string(),
                provider: "fake".to_string(),
                base_url: "http://fake".to_string(),
                api_key: "k".to_string(),
                context_window: 128000,
                cost_per_mtok_in: 0.0,
                cost_per_mtok_out: 0.0,
                supports_tools: true,
                supports_thinking: false,
                thinking_levels: Vec::new(),
                api: Some("openai-completions".to_string()),
            }],
            default_model: Some("fake/m1".to_string()),
            compaction: crate::agent::harness::catalog::CompactionConfig::default(),
        }
    }

    /// A `Provider` whose `complete` never resolves (the in-flight turn
    /// hangs in the model call — the cancel / close tests).
    struct HangingProvider;
    #[async_trait::async_trait]
    impl Provider for HangingProvider {
        async fn complete(
            &self,
            _req: &ModelRequest,
        ) -> Result<futures_util::stream::BoxStream<'static, ProviderEvent>, ProviderError>
        {
            futures_util::future::pending().await
        }
    }

    /// Start a native session with the given `provider_factory` seam.
    async fn start_native_session_with(
        dir: &Path,
        sink: &Arc<dyn EventSink>,
        factory: impl Fn(&Model) -> Box<dyn Provider> + Send + Sync + 'static,
    ) -> (SessionManager, SessionInfo) {
        let db = open_db(dir);
        let mut manager = SessionManager::new(dir.to_path_buf()).unwrap();
        manager.attach_db(db);
        manager.set_catalog(native_test_catalog());
        manager.set_provider_factory(factory);
        let info = crate::test_support::run_with_retry(|| {
            manager.start_session("nativetest", dir.to_path_buf(), sink)
        })
        .await
        .expect("the native session started");
        (manager, info)
    }

    /// Start a native session (the mock `provider_factory` seam — the
    /// `HangingProvider` hangs the turn in the model call).
    async fn start_native_session(
        dir: &Path,
        sink: &Arc<dyn EventSink>,
    ) -> (SessionManager, SessionInfo) {
        start_native_session_with(dir, sink, |_m: &Model| Box::new(HangingProvider)).await
    }

    /// A full `Model` literal for the `resolve_native_model` chain test
    /// (`base_url` EMPTY so `refresh_model_metadata` is a no-op — no
    /// network; `openai-completions` so the model is v1-selectable).
    fn chain_test_model(provider: &str, id: &str) -> Model {
        Model {
            id: id.to_string(),
            provider: provider.to_string(),
            base_url: String::new(),
            api_key: "k".to_string(),
            context_window: 128000,
            cost_per_mtok_in: 0.0,
            cost_per_mtok_out: 0.0,
            supports_tools: true,
            supports_thinking: false,
            thinking_levels: Vec::new(),
            api: Some("openai-completions".to_string()),
        }
    }

    /// The NATIVE capability envelope advertises `image: true` (the native
    /// `Prompt` carries image blocks — the frontend's `imageCapable` gate
    /// opens the paste / attach path; `false` was the v1 text-only shape).
    #[test]
    fn native_capabilities_advertises_image() {
        let caps = native_capabilities(&chain_test_model("fake", "m1"), None);
        assert_eq!(caps["native"], true);
        assert_eq!(caps["loadSession"], true);
        assert_eq!(
            caps["promptCapabilities"],
            json!({ "image": true, "audio": false, "embeddedContext": false })
        );
    }

    /// Write an `agents.json` with a single NATIVE entry whose harness
    /// `default_model` is `harness_default` (`None` = the built-in).
    fn write_agents_json_native_with_default(dir: &Path, harness_default: Option<&str>) {
        let agents = serde_json::json!({
            "agents": [{
                "id": "nativetest",
                "name": "Native Test",
                "kind": "native",
                "harness": {
                    "provider": "openai-compatible",
                    "default_model": harness_default,
                },
            }]
        });
        std::fs::write(dir.join("agents.json"), agents.to_string()).unwrap();
    }

    /// Write an `agents.json` with a single NATIVE entry whose harness
    /// `default_model` is `harness_default` and `default_thinking_level` is
    /// `harness_level` (both `None`-able).
    fn write_agents_json_native_with_thinking(
        dir: &Path,
        harness_default: Option<&str>,
        harness_level: Option<&str>,
    ) {
        let agents = serde_json::json!({
            "agents": [{
                "id": "nativetest",
                "name": "Native Test",
                "kind": "native",
                "harness": {
                    "provider": "openai-compatible",
                    "default_model": harness_default,
                    "default_thinking_level": harness_level,
                },
            }]
        });
        std::fs::write(dir.join("agents.json"), agents.to_string()).unwrap();
    }

    /// Write a `settings.json` with arbitrary JSON (the camelCase wire
    /// shape — absent keys parse to the defaults).
    fn write_settings_json(dir: &Path, settings: serde_json::Value) {
        std::fs::write(
            dir.join("settings.json"),
            serde_json::to_string_pretty(&settings).unwrap(),
        )
        .unwrap();
    }

    /// A `Model` with the given `thinking_levels` (`base_url` EMPTY so
    /// `refresh_model_metadata` is a no-op — no network).
    fn level_test_model(provider: &str, id: &str, levels: &[&str]) -> Model {
        Model {
            id: id.to_string(),
            provider: provider.to_string(),
            base_url: String::new(),
            api_key: "k".to_string(),
            context_window: 128000,
            cost_per_mtok_in: 0.0,
            cost_per_mtok_out: 0.0,
            supports_tools: true,
            supports_thinking: !levels.is_empty(),
            thinking_levels: levels.iter().map(|s| s.to_string()).collect(),
            api: Some("openai-completions".to_string()),
        }
    }

    /// Start a native session with the given catalog (the `HangingProvider`
    /// seam — the turn hangs in the model call).
    async fn start_native_session_with_catalog(
        dir: &Path,
        sink: &Arc<dyn EventSink>,
        catalog: ModelCatalog,
    ) -> (SessionManager, SessionInfo) {
        let db = open_db(dir);
        let mut manager = SessionManager::new(dir.to_path_buf()).unwrap();
        manager.attach_db(db);
        manager.set_catalog(catalog);
        manager.set_provider_factory(|_m: &Model| Box::new(HangingProvider));
        let info = crate::test_support::run_with_retry(|| {
            manager.start_session("nativetest", dir.to_path_buf(), sink)
        })
        .await
        .expect("the native session started");
        (manager, info)
    }

    /// (ADR 0015) Native start: the remembered level (VALIDATED against the
    /// model's `thinking_levels`) wins over the harness's
    /// `default_thinking_level` seed.
    #[tokio::test]
    async fn a_native_session_starts_with_the_remembered_level_over_the_harness_default() {
        let catalog = ModelCatalog {
            models: vec![level_test_model(
                "tama",
                "m1",
                &["off", "low", "medium", "xhigh"],
            )],
            default_model: Some("tama/m1".to_string()),
            compaction: crate::agent::harness::catalog::CompactionConfig::default(),
        };
        let dir = temp_config_dir();
        let (tx, _rx) = mpsc::unbounded_channel();
        let sink: Arc<dyn EventSink> = Arc::new(TestSink { tx });
        write_agents_json_native_with_thinking(&dir, Some("tama/m1"), Some("high"));
        write_settings_json(
            &dir,
            serde_json::json!({
                "defaultModel": null,
                "defaultThinkingLevels": { "tama/m1": "xhigh" },
            }),
        );
        let (manager, info) = start_native_session_with_catalog(&dir, &sink, catalog).await;
        assert_eq!(
            info.capabilities["thinkingLevel"], "xhigh",
            "the remembered (validated) level beats the harness seed"
        );
        let _ = manager.close_session(&info.session_id).await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// (ADR 0015) A STALE remembered entry (not a member of the model's
    /// current `thinking_levels` — the provider changed its levels) is
    /// IGNORED: the harness seed applies.
    #[tokio::test]
    async fn a_stale_remembered_level_falls_back_to_the_harness_default() {
        let catalog = ModelCatalog {
            models: vec![level_test_model(
                "tama",
                "m1",
                &["off", "low", "medium", "xhigh"],
            )],
            default_model: Some("tama/m1".to_string()),
            compaction: crate::agent::harness::catalog::CompactionConfig::default(),
        };
        let dir = temp_config_dir();
        let (tx, _rx) = mpsc::unbounded_channel();
        let sink: Arc<dyn EventSink> = Arc::new(TestSink { tx });
        write_agents_json_native_with_thinking(&dir, Some("tama/m1"), Some("high"));
        write_settings_json(
            &dir,
            serde_json::json!({
                "defaultModel": null,
                "defaultThinkingLevels": { "tama/m1": "ultra" },
            }),
        );
        let (manager, info) = start_native_session_with_catalog(&dir, &sink, catalog).await;
        assert_eq!(
            info.capabilities["thinkingLevel"], "high",
            "a stale (unvalidated) entry is ignored — the harness seed applies"
        );
        let _ = manager.close_session(&info.session_id).await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// (ADR 0015) A model that advertises NO `thinking_levels` gets NO
    /// remembered level (the memory is only applied to a member of a
    /// NON-EMPTY level set): the `thinkingLevel` key is ABSENT (the model's
    /// own default — `None`).
    #[tokio::test]
    async fn a_remembered_level_is_ignored_for_a_model_without_levels() {
        let catalog = ModelCatalog {
            models: vec![level_test_model("tama", "m1", &[])],
            default_model: Some("tama/m1".to_string()),
            compaction: crate::agent::harness::catalog::CompactionConfig::default(),
        };
        let dir = temp_config_dir();
        let (tx, _rx) = mpsc::unbounded_channel();
        let sink: Arc<dyn EventSink> = Arc::new(TestSink { tx });
        write_agents_json_native_with_thinking(&dir, Some("tama/m1"), None);
        write_settings_json(
            &dir,
            serde_json::json!({ "defaultThinkingLevels": { "tama/m1": "xhigh" } }),
        );
        let (manager, info) = start_native_session_with_catalog(&dir, &sink, catalog).await;
        assert!(
            info.capabilities.get("thinkingLevel").is_none(),
            "a model with no advertised levels gets no remembered level, got {:?}",
            info.capabilities
        );
        let _ = manager.close_session(&info.session_id).await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// (ADR 0015) An EXPLICIT `thought_level` change (native) persists
    /// `memory[<the session's model key>] = level` to `settings.json`
    /// (best-effort — the change itself is applied regardless).
    #[tokio::test]
    async fn a_native_thought_level_change_remembers_the_level() {
        let dir = temp_config_dir();
        write_agents_json_native(&dir);
        write_settings_json(&dir, serde_json::json!({}));
        let (tx, _rx) = mpsc::unbounded_channel();
        let sink: Arc<dyn EventSink> = Arc::new(TestSink { tx });
        let (manager, info) = start_native_session(&dir, &sink).await;
        let _ = manager
            .set_config_option(&info.session_id, "thought_level", "medium", &sink)
            .await
            .expect("the level change applies");
        let settings = load_settings(&dir);
        assert_eq!(
            settings.default_thinking_levels.get("fake/m1"),
            Some(&"medium".to_string()),
            "the explicit change writes the memory for the session's model"
        );
        // The file on disk is updated (a fresh read — not just the in-memory map).
        let raw = std::fs::read_to_string(dir.join("settings.json")).unwrap();
        let on_disk: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(on_disk["defaultThinkingLevels"]["fake/m1"], "medium");
        let _ = manager.close_session(&info.session_id).await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// (ADR 0015) A native model switch is a MINIMAL-SURPRISE reset: the
    /// current level is invalid for the NEW model (its `thinking_levels`
    /// are non-empty and don't contain it) → the level resets to the new
    /// model's remembered level (or `None`).
    #[tokio::test]
    async fn a_native_model_switch_resets_an_invalid_level_to_the_new_model_s_memory() {
        let catalog = ModelCatalog {
            models: vec![
                level_test_model("tama", "a", &["off", "high"]),
                level_test_model("tama", "b", &["off", "low", "medium", "xhigh"]),
            ],
            default_model: Some("tama/a".to_string()),
            compaction: crate::agent::harness::catalog::CompactionConfig::default(),
        };
        let dir = temp_config_dir();
        let (tx, _rx) = mpsc::unbounded_channel();
        let sink: Arc<dyn EventSink> = Arc::new(TestSink { tx });
        write_agents_json_native_with_thinking(&dir, Some("tama/a"), None);
        write_settings_json(
            &dir,
            serde_json::json!({ "defaultThinkingLevels": { "tama/b": "xhigh" } }),
        );
        let (manager, info) = start_native_session_with_catalog(&dir, &sink, catalog).await;
        // The current level (valid for `a` — also writes `a`'s memory).
        let _ = manager
            .set_config_option(&info.session_id, "thought_level", "high", &sink)
            .await
            .expect("the level change applies");
        // The switch: `"high"` is NOT a member of `b`'s levels → the reset
        // to `b`'s remembered level.
        let updated = manager
            .set_config_option(&info.session_id, "model", "tama/b", &sink)
            .await
            .expect("the model switch applies");
        let thought = updated
            .iter()
            .find(|o| o["id"] == "thought_level")
            .expect("a thought_level selector");
        assert_eq!(
            thought["currentValue"], "xhigh",
            "an invalid level resets to the new model's remembered level"
        );
        let _ = manager.close_session(&info.session_id).await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// (ADR 0015) The flip side: a current level that IS a member of the
    /// new model's `thinking_levels` is KEPT across the switch (minimal
    /// surprise — no reset).
    #[tokio::test]
    async fn a_native_model_switch_keeps_a_valid_level() {
        let catalog = ModelCatalog {
            models: vec![
                level_test_model("tama", "a", &["off", "high"]),
                level_test_model("tama", "c", &["off", "high", "medium"]),
            ],
            default_model: Some("tama/a".to_string()),
            compaction: crate::agent::harness::catalog::CompactionConfig::default(),
        };
        let dir = temp_config_dir();
        let (tx, _rx) = mpsc::unbounded_channel();
        let sink: Arc<dyn EventSink> = Arc::new(TestSink { tx });
        write_agents_json_native_with_thinking(&dir, Some("tama/a"), None);
        write_settings_json(&dir, serde_json::json!({}));
        let (manager, info) = start_native_session_with_catalog(&dir, &sink, catalog).await;
        let _ = manager
            .set_config_option(&info.session_id, "thought_level", "high", &sink)
            .await
            .expect("the level change applies");
        let updated = manager
            .set_config_option(&info.session_id, "model", "tama/c", &sink)
            .await
            .expect("the model switch applies");
        let thought = updated
            .iter()
            .find(|o| o["id"] == "thought_level")
            .expect("a thought_level selector");
        assert_eq!(
            thought["currentValue"], "high",
            "a valid level is kept across the switch"
        );
        let _ = manager.close_session(&info.session_id).await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// (ADR 0015) Native resume: the remembered level (VALIDATED) wins over
    /// the STORED `thinkingLevel` (the start-of-session value — stale after
    /// a mid-session change), which wins over the harness seed.
    #[tokio::test]
    async fn a_resume_prefers_the_remembered_level_over_the_stale_stored_value() {
        let catalog = ModelCatalog {
            models: vec![level_test_model(
                "tama",
                "m1",
                &["off", "low", "high", "xhigh"],
            )],
            default_model: Some("tama/m1".to_string()),
            compaction: crate::agent::harness::catalog::CompactionConfig::default(),
        };
        let dir = temp_config_dir();
        let (tx, _rx) = mpsc::unbounded_channel();
        let sink: Arc<dyn EventSink> = Arc::new(TestSink { tx });
        write_agents_json_native_with_thinking(&dir, Some("tama/m1"), Some("low"));
        write_settings_json(
            &dir,
            serde_json::json!({ "defaultThinkingLevels": { "tama/m1": "xhigh" } }),
        );
        let db = open_db(&dir);
        let mut manager = SessionManager::new(dir.clone()).unwrap();
        manager.attach_db(db.clone());
        manager.set_catalog(catalog);
        manager.set_provider_factory(|_m: &Model| Box::new(HangingProvider));
        // The stored row: the start-of-session level (`"high"` — valid for
        // the model; a mid-session change to `xhigh` is only in the memory).
        db.record_session(&SessionInfo {
            session_id: "nat-resume-1".to_string(),
            agent_id: "nativetest".to_string(),
            cwd: dir.clone(),
            capabilities: json!({
                "native": true,
                "model": "tama/m1",
                "thinkingLevel": "high",
                "loadSession": true,
            }),
            config_options: None,
            archived: false,
        })
        .expect("record_session should succeed");
        let info = crate::test_support::run_with_retry(|| {
            manager.resume_session("nativetest", "nat-resume-1", dir.clone(), &sink)
        })
        .await
        .expect("the native resume works");
        assert_eq!(
            info.capabilities["thinkingLevel"], "xhigh",
            "memory wins over the stale stored value"
        );
        let _ = manager.close_session(&info.session_id).await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// (ADR 0015) External (pi) start: a remembered level for
    /// `settings.defaultModel` is sent LENIENT (`set_thinking_level` after
    /// `set_model`, BEFORE the first `get_state` — the `get_state` response
    /// reflects it). `fake_pi`'s `get_state` substitutes `__LEVEL__` with
    /// the level it was sent (default `"off"`).
    #[tokio::test]
    async fn an_external_session_start_sends_the_remembered_thinking_level() {
        let dir = temp_config_dir();
        write_agents_json_pi(&dir, &[]);
        write_settings_json(
            &dir,
            serde_json::json!({
                "defaultModel": "fake/fake-model-2",
                "defaultThinkingLevels": { "fake/fake-model-2": "medium" },
            }),
        );
        let manager = SessionManager::new(dir.clone()).unwrap();
        let (tx, _rx) = mpsc::unbounded_channel();
        let sink: Arc<dyn EventSink> = Arc::new(TestSink { tx });
        let info = crate::test_support::run_with_retry(|| {
            manager.start_session("fake", dir.clone(), &sink)
        })
        .await
        .expect("the external session started");
        assert_eq!(
            info.capabilities["thinkingLevel"], "medium",
            "the remembered level is sent before the first get_state"
        );
        let _ = manager.close_session(&info.session_id).await;

        // The negative: the same `defaultModel` but NO memory entry →
        // nothing sent → pi's own default level.
        write_settings_json(
            &dir,
            serde_json::json!({ "defaultModel": "fake/fake-model-2" }),
        );
        let manager = SessionManager::new(dir.clone()).unwrap();
        let info = crate::test_support::run_with_retry(|| {
            manager.start_session("fake", dir.clone(), &sink)
        })
        .await
        .expect("the external session started");
        assert_eq!(
            info.capabilities["thinkingLevel"], "off",
            "no memory entry → no set_thinking_level → pi's default level"
        );
        let _ = manager.close_session(&info.session_id).await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// (settings chain) the native model resolution chain: per-agent
    /// `HarnessConfig.default_model` > `Settings.default_model` (the new
    /// MIDDLE rung — a fresh `load_settings` read) > the catalog's
    /// `default_model` > the v1-selectable (`openai_compatible`) set. An
    /// UNRESOLVABLE key at any rung falls through to the NEXT rung (the
    /// settings rung is tried BEFORE the catalog default — not skipped).
    #[tokio::test]
    async fn the_native_model_chain_settings_default_sits_between_per_agent_and_catalog() {
        let catalog = ModelCatalog {
            models: vec![
                chain_test_model("s", "m1"),
                chain_test_model("c", "m2"),
                chain_test_model("h", "m3"),
            ],
            default_model: Some("c/m2".to_string()),
            compaction: crate::agent::harness::catalog::CompactionConfig::default(),
        };
        let dir = temp_config_dir();
        let (tx, _rx) = mpsc::unbounded_channel();
        let sink: Arc<dyn EventSink> = Arc::new(TestSink { tx });

        // Phase 1: harness `None` + settings `s/m1` → the settings rung
        // wins (it sits between the per-agent and the catalog default).
        write_agents_json_native_with_default(&dir, None);
        write_settings_default_model(&dir, Some("s/m1"));
        let db = open_db(&dir);
        let mut manager = SessionManager::new(dir.clone()).unwrap();
        manager.attach_db(db);
        manager.set_catalog(catalog.clone());
        manager.set_provider_factory(|_m: &Model| Box::new(HangingProvider));
        let info = crate::test_support::run_with_retry(|| {
            manager.start_session("nativetest", dir.clone(), &sink)
        })
        .await
        .expect("the native session started");
        assert_eq!(
            info.capabilities["model"], "s/m1",
            "the settings default sits between the per-agent and the catalog default"
        );
        let _ = manager.close_session(&info.session_id).await;

        // Phase 2: harness `h/m3` (in the catalog) + settings `s/m1` →
        // the per-agent rung wins.
        write_agents_json_native_with_default(&dir, Some("h/m3"));
        let db = open_db(&dir);
        let mut manager = SessionManager::new(dir.clone()).unwrap();
        manager.attach_db(db);
        manager.set_catalog(catalog.clone());
        manager.set_provider_factory(|_m: &Model| Box::new(HangingProvider));
        let info = crate::test_support::run_with_retry(|| {
            manager.start_session("nativetest", dir.clone(), &sink)
        })
        .await
        .expect("the native session started");
        assert_eq!(
            info.capabilities["model"], "h/m3",
            "the per-agent default wins over the settings default"
        );
        let _ = manager.close_session(&info.session_id).await;

        // Phase 3: harness `None` + settings `gone/m1` (NOT in the
        // catalog) → the unresolvable settings key falls through to the
        // CATALOG DEFAULT rung (not straight to the v1-selectable set).
        write_agents_json_native_with_default(&dir, None);
        write_settings_default_model(&dir, Some("gone/m1"));
        let db = open_db(&dir);
        let mut manager = SessionManager::new(dir.clone()).unwrap();
        manager.attach_db(db);
        manager.set_catalog(catalog.clone());
        manager.set_provider_factory(|_m: &Model| Box::new(HangingProvider));
        let info = crate::test_support::run_with_retry(|| {
            manager.start_session("nativetest", dir.clone(), &sink)
        })
        .await
        .expect("the native session started");
        assert_eq!(
            info.capabilities["model"], "c/m2",
            "an unresolvable settings key degrades to the catalog default"
        );
        let _ = manager.close_session(&info.session_id).await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// (live `/v1/models` discovery) `refresh_model_metadata` applies the
    /// live metadata (context window, thinking levels) from the provider's
    /// `GET /v1/models`, and degrades to the static metadata when the
    /// endpoint is unreachable (best-effort — a failure never blocks the
    /// session).
    #[tokio::test]
    async fn refresh_model_metadata_applies_live_and_degrades_on_failure() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        // A mock server serving `GET /v1/models` with fresh metadata.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    let mut data = Vec::new();
                    while !data.windows(4).any(|w| w == b"\r\n\r\n") {
                        let Ok(n) = stream.read(&mut buf).await else {
                            return;
                        };
                        if n == 0 {
                            return;
                        }
                        data.extend_from_slice(&buf[..n]);
                    }
                    let body = r#"{"data":[{"id":"m1","max_model_len":4242,
                        "reasoningLevels":["low"],
                        "supportsReasoningEffort":true}]}"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes()).await;
                });
            }
        });
        let dir = std::env::temp_dir().join(format!("refresh-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut manager = SessionManager::new(dir.clone()).unwrap();
        manager.set_catalog(ModelCatalog {
            models: vec![Model {
                id: "m1".into(),
                provider: "fake".into(),
                base_url: format!("http://{addr}/v1"),
                api_key: "k".into(),
                context_window: 128000,
                cost_per_mtok_in: 0.0,
                cost_per_mtok_out: 0.0,
                supports_tools: true,
                supports_thinking: false,
                thinking_levels: Vec::new(),
                api: Some("openai-completions".into()),
            }],
            ..Default::default()
        });
        let model = manager.catalog.clone().models[0].clone();
        // First refresh: fetches + applies the live metadata.
        let refreshed = manager.refresh_model_metadata(&model).await;
        assert_eq!(refreshed.context_window, 4242);
        assert_eq!(refreshed.thinking_levels, vec!["low".to_string()]);
        assert!(refreshed.supports_thinking);
        // The `api_key` / `base_url` / `id` are untouched (only the metadata
        // fields are refreshed).
        assert_eq!(refreshed.api_key, "k");
        assert_eq!(refreshed.base_url, model.base_url);
        // A model whose endpoint is UNREACHABLE degrades to the static
        // metadata (the `Model` is returned unchanged — best-effort). A
        // distinct `provider` (a fresh cache entry) so the unreachable
        // endpoint is actually fetched (not the first model's warm cache).
        let unreachable = Model {
            provider: "unreach".to_string(),
            base_url: "http://127.0.0.1:1/v1".to_string(),
            ..model.clone()
        };
        let degraded = manager.refresh_model_metadata(&unreachable).await;
        assert_eq!(degraded.context_window, 128000);
        assert!(!degraded.supports_thinking);
        server.abort();
    }

    /// Write a `settings.json` with a single user provider (the
    /// `effective_catalog` tests — the desktop-owned provider store, ADR
    /// 0014; the camelCase wire shape).
    fn write_settings_provider(dir: &Path, id: &str, base_url: &str) {
        let settings = serde_json::json!({
            "providers": [
                { "id": id, "name": id, "baseUrl": base_url, "apiKey": "k" }
            ]
        });
        std::fs::write(
            dir.join("settings.json"),
            serde_json::to_string_pretty(&settings).unwrap(),
        )
        .unwrap();
    }

    /// A full `Model` literal (the `provider` / `base_url` are
    /// parameterized — `session.rs` has no `Model` helper of its own).
    fn provider_model(id: &str, provider: &str, base_url: &str) -> Model {
        Model {
            id: id.to_string(),
            provider: provider.to_string(),
            base_url: base_url.to_string(),
            api_key: "k".to_string(),
            context_window: DEFAULT_CONTEXT_WINDOW,
            cost_per_mtok_in: 0.0,
            cost_per_mtok_out: 0.0,
            supports_tools: true,
            supports_thinking: false,
            thinking_levels: Vec::new(),
            api: Some("openai-completions".to_string()),
        }
    }

    /// (ADR 0014) `effective_catalog` discovers a user provider's models
    /// via `GET {base_url}/models` (a fresh `settings.json` read) and
    /// maps them onto `Model` rows (the provider's `base_url` / `api_key`;
    /// `context_window` falls back to `DEFAULT_CONTEXT_WINDOW` when the
    /// response lacks `max_model_len`).
    #[tokio::test]
    async fn effective_catalog_discovers_a_user_provider() {
        let dir = temp_config_dir();
        std::fs::write(
            dir.join("agents.json"),
            serde_json::json!({ "agents": [] }).to_string(),
        )
        .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = crate::test_support::raw_json_server(
            listener,
            200,
            r#"{"data":[{"id":"m/1"}]}"#, // no `max_model_len` → the default window
            None,
        )
        .await;
        write_settings_provider(&dir, "tama", &format!("http://{addr}/v1"));
        // A seeded catalog under a DIFFERENT provider id (coexists — no
        // clash).
        let mut manager = SessionManager::new(dir).unwrap();
        manager.set_catalog(ModelCatalog {
            models: vec![provider_model("s/1", "other", "https://other/v1")],
            ..Default::default()
        });
        let effective = manager.effective_catalog(None).await;
        let user: Vec<&Model> = effective
            .models
            .iter()
            .filter(|m| m.provider == "tama")
            .collect();
        assert_eq!(user.len(), 1, "the discovered model is in the catalog");
        assert_eq!(user[0].id, "m/1");
        assert_eq!(user[0].api.as_deref(), Some("openai-completions"));
        assert_eq!(user[0].api_key, "k");
        assert_eq!(user[0].base_url, format!("http://{addr}/v1"));
        // The response lacks `max_model_len` → the default window.
        assert_eq!(user[0].context_window, DEFAULT_CONTEXT_WINDOW);
        // The seeded model coexists (no id clash).
        assert!(effective.models.iter().any(|m| m.provider == "other"));
        server.abort();
    }

    /// (ADR 0014) A provider whose discovery FAILS (unreachable endpoint)
    /// still SHADOWS the seeded models for its id (a transient failure
    /// must not resurrect stale seeded models under the same id).
    #[tokio::test]
    async fn effective_catalog_a_failed_discovery_still_shadows_the_seeded_provider() {
        let dir = temp_config_dir();
        std::fs::write(
            dir.join("agents.json"),
            serde_json::json!({ "agents": [] }).to_string(),
        )
        .unwrap();
        write_settings_provider(&dir, "tama", "http://127.0.0.1:1/v1"); // unreachable
        let mut manager = SessionManager::new(dir).unwrap();
        manager.set_catalog(ModelCatalog {
            models: vec![
                provider_model("stale/1", "tama", "https://stale/v1"),
                provider_model("q/1", "q", "https://q/v1"),
            ],
            default_model: Some("tama/stale/1".to_string()),
            ..Default::default()
        });
        let effective = manager.effective_catalog(None).await;
        assert!(
            !effective.models.iter().any(|m| m.provider == "tama"),
            "a failed discovery must NOT resurrect the stale seeded models"
        );
        // A non-shadowed provider's seeded models survive.
        assert!(effective.models.iter().any(|m| m.provider == "q"));
        // The seeded default belonged to the shadowed provider → `None`.
        assert_eq!(effective.default_model, None);
    }

    /// (ADR 0014) The discovery is CACHED per provider (a second
    /// `effective_catalog` is not re-fetched), and a `force_refresh` for
    /// the provider BYPASSES the cache (a fresh fetch overwrites the
    /// entry — even a re-pointed `base_url` is honored, the `settings.json`
    /// read is fresh).
    #[tokio::test]
    async fn effective_catalog_caches_and_force_refresh_bypasses_the_cache() {
        let dir = temp_config_dir();
        std::fs::write(
            dir.join("agents.json"),
            serde_json::json!({ "agents": [] }).to_string(),
        )
        .unwrap();
        let counter = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = crate::test_support::raw_json_server(
            listener,
            200,
            r#"{"data":[{"id":"v1"}]}"#,
            Some(counter.clone()),
        )
        .await;
        write_settings_provider(&dir, "tama", &format!("http://{addr}/v1"));
        let mut manager = SessionManager::new(dir.clone()).unwrap();
        manager.set_catalog(ModelCatalog::default());
        let first = manager.effective_catalog(None).await;
        assert!(first.models.iter().any(|m| m.id == "v1"));
        // Second call: the cache serves it (the server is NOT hit again).
        let second = manager.effective_catalog(None).await;
        assert_eq!(
            counter.load(std::sync::atomic::Ordering::Relaxed),
            1,
            "the second call must be cache-served"
        );
        assert!(second.models.iter().any(|m| m.id == "v1"));
        // A `force_refresh` for the provider bypasses the cache (a fresh
        // fetch overwrites the entry — a NEW response body is asserted via
        // the counter-driven flow: re-point the provider at a second
        // listener serving `v2`).
        let listener2 = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr2 = listener2.local_addr().unwrap();
        let server2 = crate::test_support::raw_json_server(
            listener2,
            200,
            r#"{"data":[{"id":"v2"}]}"#,
            Some(counter.clone()),
        )
        .await;
        write_settings_provider(&dir, "tama", &format!("http://{addr2}/v1"));
        let refreshed = manager.effective_catalog(Some("tama")).await;
        assert_eq!(
            counter.load(std::sync::atomic::Ordering::Relaxed),
            2,
            "the force refresh must re-fetch"
        );
        assert!(refreshed.models.iter().any(|m| m.id == "v2"));
        assert!(!refreshed.models.iter().any(|m| m.id == "v1"));
        server.abort();
        server2.abort();
    }

    /// A `Provider` whose `complete` blocks until signalled (the stream
    /// then emits a single `Done(Stop)` — the test paces the turn's
    /// settle: a signal settles the turn, no signal hangs it).
    struct PacedProvider {
        settle: Arc<tokio::sync::Notify>,
    }

    #[async_trait::async_trait]
    impl Provider for PacedProvider {
        async fn complete(
            &self,
            _req: &ModelRequest,
        ) -> Result<futures_util::stream::BoxStream<'static, ProviderEvent>, ProviderError>
        {
            let settle = self.settle.clone();
            // The stream BLOCKS on the signal (its first `next()` awaits
            // it), then yields a single `Done(Stop)` and ends — the turn
            // settles when the test signals.
            Ok(futures_util::stream::once(async move {
                settle.notified().await;
                ProviderEvent::Done(FinishReason::Stop)
            })
            .boxed())
        }
    }

    /// (native, finding 3a) `close_session` resolves an in-flight
    /// `send_prompt` `Cancelled` — deterministically, whichever driver
    /// arm wins the race: the turn is KILLED by the close (`handle.close`
    /// cancels the turn token; the loop settles it), so the
    /// `cancel_requested` flag set in `close_session` maps the settle to
    /// `Cancelled` (the settle arm), and the teardown arm sends
    /// `Cancelled` too (pre-fix the settle arm mapped a killed turn to
    /// `EndTurn` — the outcome was timing-dependent). A native turn
    /// blocked on a hanging model call + a `close_session` → the
    /// `send_prompt`'s `pending_turn` resolves `Cancelled` (the driver's
    /// teardown resolves it BEFORE the session is removed — pre-fix the
    /// unbounded `rx.await` hung forever: a close / cancel could not
    /// unblock the waiter).
    #[tokio::test]
    async fn native_close_resolves_the_in_flight_prompt() {
        let dir = temp_config_dir();
        write_agents_json_native(&dir);
        let (tx, _rx) = mpsc::unbounded_channel();
        let sink: Arc<dyn EventSink> = Arc::new(TestSink { tx });
        let (manager, info) = start_native_session(&dir, &sink).await;
        let sid = info.session_id.clone();
        let (reason, close_res) = tokio::join!(
            async { manager.send_prompt(&sid, "hi".to_string()).await },
            async {
                // A head start for the prompt (the turn hangs in the
                // model call before the close arrives).
                tokio::time::sleep(Duration::from_millis(300)).await;
                manager.close_session(&sid).await
            },
        );
        assert_eq!(close_res, Ok(()), "close should succeed");
        assert_eq!(
            reason,
            Ok(StopReason::Cancelled),
            "the close resolved the in-flight prompt `Cancelled` (not `EndTurn` — the turn was killed)"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// (finding 4) A loop task that DIES mid-turn (the `loop_settle_rx`
    /// sender is dropped — `changed()` returns `Err`) must NOT resolve the
    /// in-flight `send_prompt` `EndTurn` (pre-fix the `_ =` pattern treated
    /// the `Err` as a settle and resolved `pending_turn` `EndTurn` + wrote a
    /// duplicate `(seq, reason)` to the driver watch — the `send_prompt`
    /// caller was told "turn ended normally" when the agent actually died
    /// mid-turn). The teardown resolves `pending_turn` with `Cancelled`
    /// instead.
    ///
    /// The loop task is killed DIRECTLY (`handle.close` — NOT `close_session`,
    /// which sets `cancel_requested` and would map the settle to `Cancelled`
    /// even pre-fix): a loop task that dies on its own, not a user close.
    #[tokio::test]
    async fn native_loop_task_death_resolves_the_in_flight_prompt_cancelled() {
        let dir = temp_config_dir();
        write_agents_json_native(&dir);
        let (tx, _rx) = mpsc::unbounded_channel();
        let sink: Arc<dyn EventSink> = Arc::new(TestSink { tx });
        let (manager, info) = start_native_session(&dir, &sink).await;
        let sid = info.session_id.clone();
        // Get the `NativeHandle` (to kill the loop task DIRECTLY — NOT
        // `close_session`, which sets `cancel_requested` and would map the
        // settle to `Cancelled` even pre-fix).
        let handle = {
            let sessions = manager.driver.sessions.lock().await;
            let live = sessions.get(&sid).expect("the session is live");
            match &live.handle {
                SessionBackend::Native(h) => h.clone(),
                _ => panic!("a native session has a native handle"),
            }
        };
        // Start a `send_prompt` (the turn hangs in the `HangingProvider`
        // `complete()`). Poll it (via a `select!` with a sleep arm) until
        // the turn is IN-FLIGHT (the prompt is sent to the loop, the loop
        // starts a turn, and `complete()` hangs) — a never-polled future
        // would leave the loop idle (nothing to resolve).
        let mut prompt = Box::pin(manager.send_prompt(&sid, "hi".to_string()));
        tokio::select! {
            r = &mut prompt => {
                // The turn settled before the abort (unexpected — the
                // `HangingProvider` should hang). Fail the test.
                panic!("the turn settled before the abort: {r:?}");
            }
            _ = tokio::time::sleep(Duration::from_millis(300)) => {
                // The turn is in-flight (hanging in `complete()`).
            }
        }
        // Kill the loop task DIRECTLY (a `JoinHandle::abort` — NO token
        // cancelled, so the loop emits NO final settle; the `settle_tx`
        // sender drops unseen → `changed()` returns `Err` deterministically
        // → the teardown resolves `pending_turn` `Cancelled`, NOT `EndTurn`
        // (which pre-fix the `changed()` `Err` arm produced)).
        handle.abort_loop_task();
        // The `send_prompt` resolves `Cancelled` (the teardown resolves
        // `pending_turn` with `Cancelled` — NOT `EndTurn`, which is what
        // pre-fix the `changed()` `Err` arm produced).
        let reason = tokio::time::timeout(Duration::from_secs(5), &mut prompt)
            .await
            .expect("the prompt resolved (not a hang)");
        assert_eq!(
            reason,
            Ok(StopReason::Cancelled),
            "a loop task that died mid-turn resolves the in-flight prompt `Cancelled` (not `EndTurn` — the agent died, not a normal turn end)"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// (finding 2) Two CONCURRENT native `send_prompt`s: the busy check +
    /// the resolver claim are ATOMIC under one `pending_turn` lock, so
    /// EXACTLY ONE is accepted (it claims the slot) and the other is
    /// rejected "busy". Pre-fix the check dropped the lock, so both
    /// observed an empty slot, both passed, and the second's claim dropped
    /// the first's sender (a phantom `Cancelled`) while the second's
    /// resolver was resolved by the FIRST turn's `agent_settled`.
    ///
    /// The prompts are fired TRULY concurrently (the `tokio::spawn` calls in
    /// the same tick, NO sleep between them): a staggered test (task 2 200 ms
    /// after task 1) saw an occupied slot even pre-fix (the TOCTOU window was
    /// two awaits wide and closed in well under 200 ms), so it could not catch
    /// an atomicity regression. Pre-fix this shape accepts BOTH (one phantom
    /// `Cancelled`), so the exactly-one-`Ok` assertion discriminates.
    #[tokio::test]
    async fn two_concurrent_native_send_prompts_exactly_one_is_accepted() {
        let dir = temp_config_dir();
        write_agents_json_native(&dir);
        let (tx, _rx) = mpsc::unbounded_channel();
        let sink: Arc<dyn EventSink> = Arc::new(TestSink { tx });
        // `HangingProvider`: the turn hangs in the model call, so the
        // claimed slot stays occupied (the second prompt is rejected).
        let (manager, info) = start_native_session(&dir, &sink).await;
        let sid = info.session_id.clone();
        let manager = Arc::new(manager);
        // Fire BOTH `send_prompt`s TRULY concurrently (the `tokio::spawn`
        // calls in the same tick, NO sleep between them — see the doc above).
        let m1 = manager.clone();
        let sid1 = sid.clone();
        let t1 = tokio::spawn(async move { m1.send_prompt(&sid1, "A".to_string()).await });
        let m2 = manager.clone();
        let sid2 = sid.clone();
        let t2 = tokio::spawn(async move { m2.send_prompt(&sid2, "B".to_string()).await });
        // The rejected task resolves immediately (busy); the accepted task
        // hangs in the model call (the `HangingProvider`). Give the rejected
        // task a head start to resolve, then cancel the session (resolve the
        // accepted task `Cancelled` — free the slot).
        tokio::time::sleep(Duration::from_millis(100)).await;
        let _ = manager.cancel_session(&sid).await;
        let r1 = tokio::time::timeout(Duration::from_secs(5), t1)
            .await
            .expect("Task 1 resolved (not a hang)")
            .expect("Task 1 did not panic");
        let r2 = tokio::time::timeout(Duration::from_secs(5), t2)
            .await
            .expect("Task 2 resolved (not a hang)")
            .expect("Task 2 did not panic");
        // EXACTLY ONE is accepted (`Ok` — `Cancelled` after the cancel) + the
        // other is rejected "busy". Pre-fix BOTH were accepted (one phantom
        // `Cancelled`), so this discriminates.
        let outcomes = [r1, r2];
        let accepted = outcomes.iter().filter(|r| r.is_ok()).count();
        let busy = outcomes
            .iter()
            .filter(|r| matches!(r, Err(RpcError::Command { .. })))
            .count();
        assert_eq!(
            accepted, 1,
            "exactly one prompt is accepted, got {outcomes:?}"
        );
        assert_eq!(
            busy, 1,
            "exactly one prompt is rejected busy, got {outcomes:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// (finding 2, multi-round) Five ROUNDS of two CONCURRENT native
    /// `send_prompt`s: each round fires two prompts truly concurrently
    /// (exactly one accepted + one busy-rejected), then cancels the accepted
    /// turn (free the slot for the next round). A staggered single-round test
    /// could not catch an atomicity regression (the pre-fix TOCTOU window
    /// closed in well under a 200 ms stagger), so the multi-round variant
    /// makes the test robust.
    #[tokio::test]
    async fn two_concurrent_native_send_prompts_exactly_one_is_accepted_five_rounds() {
        let dir = temp_config_dir();
        write_agents_json_native(&dir);
        let (tx, _rx) = mpsc::unbounded_channel();
        let sink: Arc<dyn EventSink> = Arc::new(TestSink { tx });
        // `HangingProvider`: the turn hangs in the model call, so the
        // claimed slot stays occupied (the second prompt is rejected).
        let (manager, info) = start_native_session(&dir, &sink).await;
        let sid = info.session_id.clone();
        let manager = Arc::new(manager);
        // 5 rounds: each round fires TWO `send_prompt`s truly concurrently
        // (exactly one accepted + one busy-rejected), then cancels the
        // accepted turn (free the slot for the next round).
        for round in 0..5 {
            let m1 = manager.clone();
            let sid1 = sid.clone();
            let t1 = tokio::spawn(async move { m1.send_prompt(&sid1, format!("A{round}")).await });
            let m2 = manager.clone();
            let sid2 = sid.clone();
            let t2 = tokio::spawn(async move { m2.send_prompt(&sid2, format!("B{round}")).await });
            // The rejected task resolves immediately (busy); the accepted task
            // hangs in the model call (the `HangingProvider`). Give the rejected
            // task a head start to resolve, then cancel the session (resolve the
            // accepted task `Cancelled` — free the slot for the next round).
            tokio::time::sleep(Duration::from_millis(100)).await;
            let _ = manager.cancel_session(&sid).await;
            let r1 = tokio::time::timeout(Duration::from_secs(5), t1)
                .await
                .expect("Task 1 resolved (not a hang)")
                .expect("Task 1 did not panic");
            let r2 = tokio::time::timeout(Duration::from_secs(5), t2)
                .await
                .expect("Task 2 resolved (not a hang)")
                .expect("Task 2 did not panic");
            // EXACTLY ONE is accepted (`Ok`) + the other is rejected "busy".
            let outcomes = [r1, r2];
            let accepted = outcomes.iter().filter(|r| r.is_ok()).count();
            let busy = outcomes
                .iter()
                .filter(|r| matches!(r, Err(RpcError::Command { .. })))
                .count();
            assert_eq!(
                accepted, 1,
                "round {round}: exactly one prompt is accepted, got {outcomes:?}"
            );
            assert_eq!(
                busy, 1,
                "round {round}: exactly one prompt is rejected busy, got {outcomes:?}"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// (settle watch) `wait_for_settle` must not resolve a STALE settle when
    /// a turn is IN FLIGHT: after turn 1 settles, a `wait_for_settle` with a
    /// turn IN FLIGHT (the `pending_turn` slot occupied) must NOT return
    /// turn 1's settle — it is pinned to the current version
    /// (`mark_unchanged`) and resolves only on a NEW settle. With NO turn in
    /// flight (the caller's turn already settled — the subagent dispatches a
    /// raw prompt, which does NOT occupy `pending_turn`, then awaits the
    /// settle: a fast turn settles before the await), the `mark_unchanged` is
    /// SKIPPED: the clone inherits the stored receiver's last-seen version, so
    /// `changed()` resolves immediately with the LATEST settle (the fast-turn
    /// contract — hanging on a new settle that never comes would be the bug).
    /// The `mark_unchanged` + the `pending_turn` snapshot are under ONE
    /// `sessions` lock (finding 6), and the driver's settle arm takes the slot
    /// + sends the watch under the same lock (watch send last), so a settle
    /// landing between the snapshot and the mark is ordered (a settle after the
    /// mark is a NEW version → resolves; one before empties the slot → no mark
    /// → resolves with the latest).
    #[tokio::test]
    async fn wait_for_settle_does_not_return_a_stale_settle() {
        let dir = temp_config_dir();
        write_agents_json_native(&dir);
        let (tx, _rx) = mpsc::unbounded_channel();
        let sink: Arc<dyn EventSink> = Arc::new(TestSink { tx });
        let settle = Arc::new(tokio::sync::Notify::new());
        let provider_settle = settle.clone();
        let (manager, info) = start_native_session_with(&dir, &sink, move |_m: &Model| {
            Box::new(PacedProvider {
                settle: provider_settle.clone(),
            })
        })
        .await;
        let sid = info.session_id.clone();

        // Turn 1 settles (the signal releases the model call).
        let (reason, r) = tokio::join!(
            async { manager.send_prompt(&sid, "one".to_string()).await },
            async {
                // A head start for the prompt (the turn waits in the
                // model call before the signal arrives).
                tokio::time::sleep(Duration::from_millis(500)).await;
                settle.notify_one();
                Ok::<(), ()>(())
            },
        );
        assert_eq!(r, Ok(()), "the signal should have been sent");
        assert_eq!(reason, Ok(StopReason::EndTurn), "turn 1 settles");

        // Turn 2 starts (IN FLIGHT — `send_prompt` occupies `pending_turn`;
        // the turn blocks in the model call until signalled). Poll `prompt`
        // until it has dispatched the turn (it then blocks in the model
        // call — the short timeout elapses, `prompt` stays pending).
        let prompt = manager.send_prompt(&sid, "two".to_string());
        tokio::pin!(prompt);
        let _ = tokio::time::timeout(Duration::from_millis(200), &mut prompt).await;

        // `wait_for_settle` while turn 2 is IN FLIGHT: it must NOT resolve
        // on turn 1's STALE settle (500 ms ≪ the settle timeout) — the
        // `pending_turn` slot is occupied, so the wait is pinned to the
        // channel's current version and resolves only on a NEW settle.
        let stale = tokio::time::timeout(
            Duration::from_millis(500),
            manager.driver.wait_for_settle(&sid),
        );
        assert!(
            stale.await.is_err(),
            "a stale (previous turn's) settle must not resolve the wait while a turn is in flight"
        );

        // Turn 2 settles (the signal releases the model call) → `send_prompt`
        // resolves, and a `wait_for_settle` with NO turn in flight resolves
        // immediately with the turn's (latest) settle — the fast-turn contract
        // (a fast turn settles before the await; hanging on a new settle that
        // never comes would be the bug).
        settle.notify_one();
        assert_eq!(prompt.await, Ok(StopReason::EndTurn), "turn 2 settles");
        let w = manager.driver.wait_for_settle(&sid).await;
        assert_eq!(
            w,
            Ok(StopReason::EndTurn),
            "a settled turn's settle resolves immediately"
        );

        let _ = manager.close_session(&sid).await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// (finding 13b) A native entry with a `provider` that is NOT
    /// `"openai-compatible"` is REJECTED at session start with a clear
    /// error (v1 is OpenAI-compatible only — ADR 0012) rather than
    /// silently accepting it (pre-fix any value got the OpenAI wire).
    #[tokio::test]
    async fn a_non_openai_compatible_harness_provider_is_rejected_at_session_start() {
        let dir = temp_config_dir();
        let agents = serde_json::json!({
            "agents": [{
                "id": "nativetest",
                "name": "Native Test",
                "kind": "native",
                "harness": { "provider": "anthropic", "default_model": "fake/m1" },
            }]
        });
        std::fs::write(dir.join("agents.json"), agents.to_string()).unwrap();
        let db = open_db(&dir);
        let mut manager = SessionManager::new(dir.to_path_buf()).unwrap();
        manager.attach_db(db);
        manager.set_catalog(native_test_catalog());
        let (tx, _rx) = mpsc::unbounded_channel();
        let sink: Arc<dyn EventSink> = Arc::new(TestSink { tx });
        let res = crate::test_support::run_with_retry(|| {
            manager.start_session("nativetest", dir.to_path_buf(), &sink)
        })
        .await;
        let err = res.unwrap_err().to_string();
        assert!(
            err.contains("anthropic"),
            "the error names the unsupported provider, got {err}"
        );
        assert!(
            err.contains("OpenAI-compatible"),
            "the error explains the v1 constraint, got {err}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// (native, finding 8c) A native Stop matches the external `abort`:
    /// the turn stops (the `send_prompt` resolves `Cancelled`) and the
    /// session STAYS ALIVE (a new prompt reuses it — the pre-fix Stop
    /// cancelled the loop's teardown token, which ENDED THE WHOLE
    /// SESSION: a second `send_prompt` would be an `UnknownSession`). A
    /// stale cancel does not settle the new turn (a fresh turn token);
    /// a second Stop settles it.
    #[tokio::test]
    async fn native_cancel_stops_the_turn_and_keeps_the_session() {
        let dir = temp_config_dir();
        write_agents_json_native(&dir);
        let (tx, _rx) = mpsc::unbounded_channel();
        let sink: Arc<dyn EventSink> = Arc::new(TestSink { tx });
        let (manager, info) = start_native_session(&dir, &sink).await;
        let sid = info.session_id.clone();
        // The first Stop: the turn settles `Cancelled` (the session stays
        // alive).
        let (reason, cancel_res) = tokio::join!(
            async { manager.send_prompt(&sid, "hi".to_string()).await },
            async {
                tokio::time::sleep(Duration::from_millis(300)).await;
                manager.cancel_session(&sid).await
            },
        );
        assert_eq!(cancel_res, Ok(()), "cancel should succeed");
        assert_eq!(
            reason,
            Ok(StopReason::Cancelled),
            "the Stop settled the turn"
        );
        // The session is still alive: a new prompt starts (a FRESH turn —
        // the stale cancel does not settle it), and a second Stop
        // settles it.
        let (reason2, cancel_res2) = tokio::join!(
            async { manager.send_prompt(&sid, "again".to_string()).await },
            async {
                tokio::time::sleep(Duration::from_millis(300)).await;
                manager.cancel_session(&sid).await
            },
        );
        assert_eq!(cancel_res2, Ok(()), "the second cancel should succeed");
        assert_eq!(
            reason2,
            Ok(StopReason::Cancelled),
            "the session was reused (a stale cancel did not settle the new turn; the second Stop did)"
        );
        let _ = manager.close_session(&sid).await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// (critical) A native prompt writes EXACTLY ONE `user` row to the
    /// display `messages`: the manager's `record_message` (in
    /// `send_prompt_with_images`) is the SOLE write — the loop's
    /// `handle_turn` must not re-persist the user message (the
    /// `(session_id, kind, message_key)` key with `message_key = NULL`
    /// treats NULLs as DISTINCT in `ON CONFLICT`, so a double write
    /// deterministically duplicates the user bubble in restored
    /// history).
    #[tokio::test]
    async fn a_native_prompt_writes_exactly_one_user_row() {
        let dir = temp_config_dir();
        write_agents_json_native(&dir);
        let (tx, _rx) = mpsc::unbounded_channel();
        let sink: Arc<dyn EventSink> = Arc::new(TestSink { tx });
        let (manager, info) = start_native_session(&dir, &sink).await;
        let sid = info.session_id.clone();
        // The turn hangs in the (hanging) model call: the manager's row
        // is written before the turn begins, and the loop's own write
        // (when it has one) happens when the turn starts. Drive the
        // prompt until it blocks (a 500 ms timeout) so both writes have
        // happened.
        let prompt = manager.send_prompt(&sid, "hi".to_string());
        tokio::pin!(prompt);
        let _ = tokio::time::timeout(Duration::from_millis(500), &mut prompt).await;
        let db = open_db(&dir);
        let rows = db.messages_for(&sid).expect("messages_for");
        let user_rows = rows.iter().filter(|r| r.kind == "user").count();
        assert_eq!(
            user_rows, 1,
            "exactly one `user` row (a double write would show a duplicate user bubble)"
        );
        let _ = manager.cancel_session(&sid).await;
        assert_eq!(prompt.await, Ok(StopReason::Cancelled));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// (concurrent native prompts) The frontend is one-turn-at-a-time:
    /// a `send_prompt` on a native session with a turn ALREADY IN
    /// FLIGHT is REJECTED ("a turn is already in flight") rather than
    /// queued — the `pending_turn` slot is a single last-wins resolver,
    /// and a queued prompt would be settled by the PREVIOUS turn's
    /// `agent_settled` (mis-attribution: the composer unlocks while a
    /// turn is still live). The external path keeps its steer/
    /// last-wins behavior (a single steer turn).
    #[tokio::test]
    async fn a_concurrent_native_prompt_is_rejected_busy() {
        let dir = temp_config_dir();
        write_agents_json_native(&dir);
        let (tx, _rx) = mpsc::unbounded_channel();
        let sink: Arc<dyn EventSink> = Arc::new(TestSink { tx });
        let (manager, info) = start_native_session(&dir, &sink).await;
        let sid = info.session_id.clone();
        // The first prompt's turn hangs in the model call (the
        // `pending_turn` slot stays occupied). Drive the prompt until it
        // blocks (a 300 ms timeout) so its resolver is stored.
        let p1 = manager.send_prompt(&sid, "one".to_string());
        tokio::pin!(p1);
        let _ = tokio::time::timeout(Duration::from_millis(300), &mut p1).await;
        let r2 = manager.send_prompt(&sid, "two".to_string()).await;
        assert!(
            matches!(&r2, Err(RpcError::Command { error }) if error.contains("already in flight")),
            "the concurrent prompt is rejected busy, got {r2:?}"
        );
        // The first resolver was NOT overwritten: a Stop settles the
        // FIRST prompt (not the rejected one).
        let _ = manager.cancel_session(&sid).await;
        assert_eq!(
            p1.await,
            Ok(StopReason::Cancelled),
            "the first turn's resolver was kept"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// (native, finding 12) A `set_config_option` whose `try_send` fails
    /// (the control queue is FULL — the loop is busy in a hanging model
    /// call and never consumes it) returns an error: the state is NOT
    /// mirrored and no `config_option_update` is claimed (pre-fix the
    /// `try_send` failure was ignored — the UI would show the new config
    /// while the loop kept running the old one, a silent divergence).
    #[tokio::test]
    async fn native_set_config_option_fails_when_the_queue_is_full() {
        let dir = temp_config_dir();
        write_agents_json_native(&dir);
        let (tx, _rx) = mpsc::unbounded_channel();
        let sink: Arc<dyn EventSink> = Arc::new(TestSink { tx });
        let (manager, info) = start_native_session(&dir, &sink).await;
        let sid = info.session_id.clone();
        // The loop is BUSY (a hanging model call) — the control queue
        // (8) is not consumed. The first 8 changes queue (Ok); the 9th
        // `try_send` fails (the queue is full) → an error, NOT a silent
        // success.
        for i in 0..8 {
            manager
                .set_config_option(&sid, "thought_level", "low", &sink)
                .await
                .unwrap_or_else(|e| panic!("change {i} should have queued: {e:?}"));
        }
        let result = manager
            .set_config_option(&sid, "thought_level", "low", &sink)
            .await;
        assert!(
            matches!(result, Err(RpcError::Command { .. })),
            "a full control queue is an error (finding 12), got {result:?}"
        );
        let _ = manager.close_session(&sid).await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// (permission) `FAKE_PI_GATE=1`: the `prompt` turn fires the gate
    /// dialog (`extension_ui_request` `confirm` → a `permission-request`
    /// event with the `[Allow, Block, Don't ask again for this Space]`
    /// options — no `db` is attached, so the Space is untrusted and the
    /// third option is offered) → `respond_permission`
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
                // `[Allow, Block, Don't ask again for this Space]`).
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
                                vec!["allow", "reject", "trust-space"],
                                "confirm frames offer [Allow, Block, Don't ask again for this Space]"
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

    /// (teardown) A session close clears the session's Phase 2 shared
    /// state: the cached sudo password (keyed by the BARE session id —
    /// the suite's `credentialCache` is cleared at every session boundary,
    /// so a resumed session under the same id must re-prompt, not silently
    /// reuse the pre-close credential) and the `TodoStore` entry (a
    /// resumed session must not read the previous incarnation's todos).
    #[tokio::test]
    async fn session_close_clears_the_cached_sudo_password_and_the_todos() {
        let dir = temp_config_dir();
        write_agents_json_pi(&dir, &[]);
        let mut manager = SessionManager::new(dir.clone()).unwrap();
        manager.set_establish_timeout(Duration::from_secs(15));
        let (tx, mut rx) = mpsc::unbounded_channel();
        let sink: Arc<dyn EventSink> = Arc::new(TestSink { tx });

        let info = crate::test_support::run_with_retry(|| {
            manager.start_session("fake", dir.clone(), &sink)
        })
        .await
        .unwrap();
        let sid = info.session_id.clone();

        // Simulate mid-session state: a cached credential + todos for this
        // session (the `sudo_password` cache is keyed by the BARE session
        // id — `cache.get(sid)` — and the todo store likewise).
        manager.driver.sudo_password.lock().await.insert(
            sid.clone(),
            CachedPassword {
                password: "pw".to_string(),
                expires_at: std::time::Instant::now() + Duration::from_secs(600),
            },
        );
        manager.driver.todo_store.set(
            &sid,
            vec![crate::agent::todo::TodoItem {
                content: "a".to_string(),
                status: crate::agent::todo::TodoStatus::Pending,
                description: None,
            }],
        );

        let _ = manager.close_session(&sid).await;

        // Wait for the driver's `session-closed` event (it is emitted AFTER
        // the teardown — the assertions below are only meaningful once the
        // teardown has run).
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        let mut closed = false;
        while std::time::Instant::now() < deadline && !closed {
            if let Ok(msg) = tokio::time::timeout(Duration::from_millis(200), rx.recv()).await {
                if msg.unwrap()["event"] == "session-closed" {
                    closed = true;
                }
            }
        }
        assert!(closed, "the session-closed event fired");
        assert!(
            !manager.driver.sudo_password.lock().await.contains_key(&sid),
            "the cached sudo password is cleared at the session boundary"
        );
        assert!(
            manager.driver.todo_store.get(&sid).is_empty(),
            "the todo list is cleared at the session boundary"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── Native system prompt (ADR 0017, Task 3: the prompt is built
    // ONCE at a NEW session's start — persisted at seq 0 — and a
    // RESUME replays the stored transcript verbatim) ──

    /// A `Provider` that RECORDS every `ModelRequest` it receives (pushed
    /// into the shared vec) and then answers with a short canned stream
    /// (`TextDelta` + `Done(Stop)` — the turn settles `EndTurn`).
    struct RecordingProvider {
        requests: Arc<StdMutex<Vec<ModelRequest>>>,
    }

    #[async_trait::async_trait]
    impl Provider for RecordingProvider {
        async fn complete(
            &self,
            req: &ModelRequest,
        ) -> Result<futures_util::stream::BoxStream<'static, ProviderEvent>, ProviderError>
        {
            self.requests.lock().unwrap().push(req.clone());
            Ok(futures_util::stream::iter(vec![
                ProviderEvent::TextDelta("ok".to_string()),
                ProviderEvent::Done(FinishReason::Stop),
            ])
            .boxed())
        }
    }

    /// Build a `SessionManager` (a temp config dir + a native `AgentEntry`
    /// with the given `enabled_tools`) + a Space dir (an EMPTY `.git/` dir
    /// bounds the project-context walk at the Space, per the Task 1
    /// scratch-dir rule; the optional `AGENTS.md` is the controlled project
    /// context) + the `RecordingProvider` seam. Returns the manager, the
    /// Space dir, the shared request vec, the `Db` (the FK assertions), and
    /// the sink.
    async fn native_manager_with_recording(
        enabled_tools: &[&str],
        agents_md: Option<&str>,
    ) -> (
        SessionManager,
        PathBuf,
        Arc<StdMutex<Vec<ModelRequest>>>,
        std::sync::Arc<Db>,
        Arc<dyn EventSink>,
    ) {
        let dir = temp_config_dir();
        let space = dir.join("space");
        std::fs::create_dir_all(space.join(".git")).unwrap();
        if let Some(content) = agents_md {
            std::fs::write(space.join("AGENTS.md"), content).unwrap();
        }
        let agents = serde_json::json!({
            "agents": [{
                "id": "nativetest",
                "name": "Native Test",
                "kind": "native",
                "harness": {
                    "provider": "openai-compatible",
                    "default_model": "fake/m1",
                    "enabled_tools": enabled_tools,
                },
            }]
        });
        std::fs::write(dir.join("agents.json"), agents.to_string()).unwrap();
        let db = open_db(&dir);
        let requests = Arc::new(StdMutex::new(Vec::new()));
        let factory_requests = requests.clone();
        let mut manager = SessionManager::new(dir).unwrap();
        manager.attach_db(db.clone());
        manager.set_catalog(native_test_catalog());
        manager.set_provider_factory(move |_m: &Model| {
            Box::new(RecordingProvider {
                requests: factory_requests.clone(),
            })
        });
        let (tx, _rx) = mpsc::unbounded_channel();
        let sink: Arc<dyn EventSink> = Arc::new(TestSink { tx });
        (manager, space, requests, db, sink)
    }

    /// Start a native session in `space` and drive ONE turn (the
    /// `RecordingProvider` settles it `EndTurn`); returns the `SessionInfo`
    /// + the turn's recorded `ModelRequest`.
    async fn start_and_drive_one_turn(
        manager: &SessionManager,
        space: &Path,
        requests: &Arc<StdMutex<Vec<ModelRequest>>>,
        sink: &Arc<dyn EventSink>,
    ) -> (SessionInfo, ModelRequest) {
        let info = crate::test_support::run_with_retry(|| {
            manager.start_session("nativetest", space.to_path_buf(), sink)
        })
        .await
        .expect("the native session started");
        let sid = info.session_id.clone();
        let reason = tokio::time::timeout(Duration::from_secs(15), {
            manager.send_prompt(&sid, "hello".to_string())
        })
        .await
        .expect("the turn settled (not a hang)")
        .expect("the turn settled");
        assert_eq!(reason, StopReason::EndTurn, "a normal turn settles EndTurn");
        let req = requests
            .lock()
            .unwrap()
            .last()
            .cloned()
            .expect("the model was called");
        (info, req)
    }

    /// The `messages[0]` text (a `System` message with plain text content).
    fn system_text(req: &ModelRequest) -> String {
        let first = &req.messages[0];
        assert!(
            matches!(first.role, ChatRole::System),
            "messages[0] is the system message, got {:?}",
            first.role
        );
        match &first.content {
            MessageContent::Text(t) => t.clone(),
            other => panic!("the system message is plain text, got {other:?}"),
        }
    }

    /// (ADR 0017) A NEW native session's first model request carries the
    /// built system prompt at `messages[0]` (the preamble + a `<tools>`
    /// section + the project context + the `<cwd>` section), AND the seq-0
    /// `native_messages` row exists — the FK fix: the session row is
    /// recorded BEFORE the seq-0 persist inside `build_native_session`
    /// (a persist BEFORE the row would hit an FK violation and be dropped).
    #[tokio::test]
    async fn start_native_session_persists_system_prompt() {
        let (manager, space, requests, db, sink) =
            native_manager_with_recording(&[], Some("project rules")).await;
        let space_canon = std::fs::canonicalize(&space).unwrap();
        let space_str = space_canon.to_string_lossy().into_owned();
        let (info, req) = start_and_drive_one_turn(&manager, &space, &requests, &sink).await;

        // The prompt assertions are `contains`-based: the real `~/.pi/agent`
        // context file + the discovered skills leak in (not injectable at
        // this level) — assert on the controlled content.
        let text = system_text(&req);
        assert!(
            text.contains("You are an expert coding assistant operating inside Archimedes Desktop"),
            "the preamble: {text}"
        );
        assert!(
            text.contains("- read:"),
            "a <tools> section with the read line: {text}"
        );
        assert!(
            text.contains("<project_context>"),
            "a <project_context> section: {text}"
        );
        assert!(
            text.contains("project rules"),
            "the Space's AGENTS.md: {text}"
        );
        assert!(text.contains("<cwd>"), "a <cwd> section: {text}");
        assert!(
            text.contains(&space_str),
            "the <cwd> section carries the Space path: {text}"
        );

        // The FK fix: the session row exists AND a `native_messages` row
        // with the system content exists (the persist did NOT hit an FK
        // violation — the row was recorded BEFORE the prompt block).
        assert!(
            db.session(&info.session_id)
                .expect("the db works")
                .is_some(),
            "the session row exists"
        );
        let rows = db
            .load_native_messages(&info.session_id)
            .expect("the db works");
        assert!(!rows.is_empty(), "a native_messages row exists");
        assert!(
            rows[0].contains("\"role\":\"system\""),
            "the FIRST row (seq 0) is the system message, got: {}",
            &rows[0]
        );

        let _ = manager.close_session(&info.session_id).await;
        let _ = std::fs::remove_dir_all(space.parent().unwrap());
    }

    /// The `<rules>` section body (the text between `<rules>\n` and
    /// `\n</rules>`): built purely from the advertised specs (the two pi
    /// lines + the conditional guidance lines) — env-independent, so
    /// negative assertions are scoped to it (a whole-prompt `!contains`
    /// would be flaky-by-construction: a global `AGENTS.md` / skill
    /// containing the phrase would turn CI red).
    fn rules_body(prompt: &str) -> &str {
        let start = prompt
            .find("<rules>\n")
            .unwrap_or_else(|| panic!("a <rules> section: {prompt}"));
        let end = prompt
            .find("\n</rules>")
            .unwrap_or_else(|| panic!("the </rules> close: {prompt}"));
        &prompt[start + "<rules>\n".len()..end]
    }

    /// (ADR 0017) `enabled_tools` restricts the prompt's `<tools>` section
    /// (the `advertised_specs` — the prompt matches the `tools[]` API param
    /// exactly) AND the conditional `<rules>` guidance lines (`[
    /// "read", "bash"]` → neither the `manage_todo_list` nor the
    /// `subagent` guidance line).
    #[tokio::test]
    async fn start_native_session_prompt_respects_enabled_tools() {
        let (manager, space, requests, _db, sink) =
            native_manager_with_recording(&["read", "bash"], Some("project rules")).await;
        let (info, req) = start_and_drive_one_turn(&manager, &space, &requests, &sink).await;

        let text = system_text(&req);
        // The `<tools>` body: EXACTLY the `read` + `bash` lines (no
        // `subagent` line, no other tool line).
        let start = text.find("<tools>\n").expect("a <tools> section: {text}");
        let end = text.find("\n</tools>").expect("the </tools> close: {text}");
        let body = &text[start + "<tools>\n".len()..end];
        let lines: Vec<&str> = body.split('\n').collect();
        assert_eq!(lines.len(), 2, "exactly the read + bash lines, got: {body}");
        assert!(
            lines.iter().any(|l| l.starts_with("- read:")),
            "the read line: {body}"
        );
        assert!(
            lines.iter().any(|l| l.starts_with("- bash:")),
            "the bash line: {body}"
        );
        assert!(
            !lines.iter().any(|l| l.contains("subagent")),
            "no subagent line: {body}"
        );
        // The conditional guidance: `manage_todo_list` + `subagent` are NOT
        // advertised → both guidance lines absent from the `<rules>`
        // section (scoped to the section — a whole-prompt `!contains` would
        // be flaky-by-construction: a global `AGENTS.md` / skill containing
        // the phrase would turn CI red; the section is built purely from
        // the advertised specs, so it is env-independent).
        let rules = rules_body(&text);
        assert!(
            !rules.contains("manage_todo_list to track"),
            "no TODO guidance in <rules>: {rules}"
        );
        assert!(
            !rules.contains("Delegate independent subtasks"),
            "no subagent guidance in <rules>: {rules}"
        );
        assert!(
            rules.contains("Be concise in your responses")
                && rules.contains("Show file paths clearly when working with files"),
            "the two pi lines in <rules>: {rules}"
        );
        assert!(
            text.contains("- Be concise in your responses")
                && text.contains("- Show file paths clearly when working with files"),
            "the two pi lines: {text}"
        );

        let _ = manager.close_session(&info.session_id).await;
        let _ = std::fs::remove_dir_all(space.parent().unwrap());
    }

    /// (ADR 0017, the static-per-session decision) A RESUME replays the
    /// stored transcript verbatim — the prompt is NOT rebuilt: a changed
    /// `AGENTS.md` applies from the next NEW session, not the resume.
    #[tokio::test]
    async fn resume_native_session_replays_system_prompt_verbatim() {
        let (manager, space, requests, _db, sink) =
            native_manager_with_recording(&[], Some("v1 rules")).await;
        let (info, _req1) = start_and_drive_one_turn(&manager, &space, &requests, &sink).await;

        // The context CHANGES after the start (the Space's `.git` bounds
        // the walk, so the overwrite is the only context change). A FRESH
        // marker makes the negative assertion collision-proof (a whole-prompt
        // `!contains("v2 rules")` would be flaky-by-construction: a global
        // `AGENTS.md` / skill containing the phrase would turn CI red; the
        // marker cannot collide — `v1` lives in the `.git`-bounded temp
        // Space, so its positive `contains` stays unmarked).
        let marker = uuid::Uuid::new_v4().to_string();
        std::fs::write(space.join("AGENTS.md"), format!("v2 rules {marker}")).unwrap();

        let resumed = crate::test_support::run_with_retry(|| {
            manager.resume_session("nativetest", &info.session_id, space.clone(), &sink)
        })
        .await
        .expect("the native resume succeeded");
        assert_eq!(resumed.session_id, info.session_id);
        let sid = resumed.session_id.clone();
        let reason = tokio::time::timeout(Duration::from_secs(15), {
            manager.send_prompt(&sid, "again".to_string())
        })
        .await
        .expect("the turn settled (not a hang)")
        .expect("the turn settled");
        assert_eq!(reason, StopReason::EndTurn);

        // The resumed session's model request replays the STORED prompt
        // verbatim (the seq-0 row from the start — "v1 rules", NOT the
        // overwritten "v2 rules {marker}").
        let req = requests
            .lock()
            .unwrap()
            .last()
            .cloned()
            .expect("the model was called");
        let text = system_text(&req);
        assert!(
            text.contains("v1 rules"),
            "the stored prompt is replayed verbatim: {text}"
        );
        assert!(
            !text.contains(&marker),
            "a changed AGENTS.md does NOT leak into the resumed prompt: {text}"
        );

        let _ = manager.close_session(&info.session_id).await;
        let _ = std::fs::remove_dir_all(space.parent().unwrap());
    }

    /// (review finding 1) A RESUME whose `load_messages` errors (a corrupt
    /// transcript row) must FAIL the session start — the old swallow
    /// proceeded with an empty transcript, and the next persist would
    /// upsert over the stored rows (seq 0 = the system prompt, cascading
    /// to seq 1, 2, …).
    #[tokio::test]
    async fn resume_native_session_fails_on_corrupt_transcript() {
        let (manager, space, _requests, db, sink) =
            native_manager_with_recording(&[], Some("v1 rules")).await;
        let (info, _req) = start_and_drive_one_turn(&manager, &space, &_requests, &sink).await;
        let sid = info.session_id.clone();

        // Corrupt the seq-0 row (invalid JSON → `load_messages` `Json`
        // error — "a corrupt transcript must not silently load as an
        // empty one").
        db.insert_native_message(&sid, 0, "system", "not valid json")
            .expect("the db works");

        // The resume FAILS (the load error is propagated as
        // `RpcError::Io` — NOT silently swallowed into an empty
        // transcript).
        let err = crate::test_support::run_with_retry(|| {
            manager.resume_session("nativetest", &sid, space.clone(), &sink)
        })
        .await
        .expect_err("a corrupt transcript must fail the resume");
        assert!(
            matches!(err, RpcError::Io(_)),
            "the load error is propagated as `RpcError::Io`, got: {err:?}"
        );

        let _ = manager.close_session(&sid).await;
        let _ = std::fs::remove_dir_all(space.parent().unwrap());
    }
}
