//! The native session layer: the Supervisor-side session coordinator
//! (ADR 0025) — a session's `AgentLoop` runs in its Worker process (a
//! self-exec `archimedes --worker` child); the `SessionManager` is the
//! Supervisor-side coordinator (spawn/reap via the `WorkerManager`, event
//! routing, persistence via the `TranscriptPersister`, the `respond_*`
//! relays, the stalled state). The in-process native driver is DELETED
//! (no dual-mode fallback — ADR 0025).
//!
//! The `WorkerManager`'s `on_event` callback (wired in `lib.rs`) routes
//! each Worker frame: the store frames → the `TranscriptPersister`
//! (persistence only); the `SinkFrame`s → the `TauriSink` VERBATIM (the
//! UI contract — re-emitted with the same event name + payload); the raw
//! `RpcEvent` stream → internal bookkeeping only (the `agent_settled`
//! settle detection + the `pending_turn` resolution — NEVER the UI, to
//! avoid double-delivering the `session-update` frames); the
//! `PermissionRequest` / `InteractiveRequest` frames → re-emitted on the
//! `TauriSink` with the payload UNCHANGED (the frontend contract preserved
//! by construction).
//!
//! Multiple live sessions COEXIST: `start_session` / `resume_session` do
//! NOT close other live sessions; a session is torn down only by an
//! explicit `close_session` (a `detach` + the Worker's clean exit) or a
//! crash (the `on_crash` callback marks the session STALLED — resumable).
//!
//! **The `session-update` contract is FROZEN** (ADR 0011): the `SinkFrame`
//! re-emit delivers the exact JSON envelopes the frontend consumes
//! (`agent_message_chunk` / `agent_thought_chunk` / `tool_call` /
//! `tool_call_update` / `session_info_update` / `config_option_update`), so
//! the frontend needs no changes.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex, OnceLock};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::oneshot;

use crate::agent::errors::SessionError;
use crate::agent::events::RpcEvent;
use crate::agent::harness::store::DisplayRow;
use crate::agent::harness::{
    build_provider, discover_models, merge_catalog, Model, ModelCatalog, Provider,
    ProviderDiscovery, SessionStore, DEFAULT_CONTEXT_WINDOW,
};
use crate::agent::permission::{self};
use crate::agent::persist::TranscriptPersister;
use crate::agent::tools::ImageRef;
use crate::agent::worker::client::WorkerInboundEvent;
use crate::agent::worker::manager::WorkerManager;
use crate::agent::worker::protocol::{StartEnv, StartMode};
use crate::commands::settings::{load_settings, write_settings};
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
/// `pub(crate)`: the close reason rides the `SessionManager`'s live map
/// across the module boundary (the `close_session` / `respond_*` paths),
/// so it must be visible to the whole crate.
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
fn validate_images(images: &[ImagePayload]) -> Result<(), SessionError> {
    if images.len() > MAX_IMAGE_COUNT {
        return Err(SessionError::InvalidPrompt {
            reason: format!("at most {MAX_IMAGE_COUNT} images per message"),
        });
    }
    for img in images {
        if img.name.len() > MAX_IMAGE_NAME_LEN {
            return Err(SessionError::InvalidPrompt {
                reason: "image name exceeds 255 bytes".to_string(),
            });
        }
        if !SUPPORTED_IMAGE_TYPES.contains(&img.mime_type.as_str()) {
            return Err(SessionError::InvalidPrompt {
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
            return Err(SessionError::InvalidPrompt {
                reason: "malformed base64 image data".to_string(),
            });
        }
        let decoded_bytes = (trimmed.len() as u64) * 3 / 4;
        if decoded_bytes > MAX_IMAGE_BYTES {
            return Err(SessionError::InvalidPrompt {
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

/// Mint a fresh unique session ID with the `arch_` prefix.
pub fn mint_session_id() -> String {
    format!("arch_{}", uuid::Uuid::new_v4())
}

/// A fully established session, ready to accept prompts.
///
/// `Serialize` so it can cross the IPC boundary as a command return value.
///
/// `capabilities` is the pi session's capability envelope (item 1 of the
/// swap plan): `piSessionId` / `piSessionFile`? / `model`? /
/// `thinkingLevel` / `loadSession` / `promptCapabilities` — the `model` key
/// is ABSENT when `get_state` reports no model, and `piSessionFile` is
/// A live session's identity + its current configuration (what the UI shows
/// in the header / the model selector). Serialized camelCase over IPC.
///
/// `capabilities` is the native session's capability envelope (item 1 of the
/// swap plan): `model`? / `thinkingLevel` / `loadSession` / `promptCapabilities`
/// — the `model` key is ABSENT when the session has no model, and the two
/// trailing keys are load-bearing: `loadSession` gates the frontend's Resume
/// button and `promptCapabilities.image` gates image sending (fail-closed).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub session_id: String,
    pub cwd: PathBuf,
    pub capabilities: Value,
    /// The session's configuration options (model / thinking level
    /// selectors) synthesized from the `ModelCatalog` (the native session's
    /// model / thinking-level selectors); `None` when they can't be
    /// synthesized (no catalog / unresolvable stored model).
    pub config_options: Option<Vec<Value>>,
    /// The desktop's archived flag (ADR 0016). `false` for a newly
    /// started or ephemeral session; the resume paths and
    /// `list_sessions` read it from the stored row.
    pub archived: bool,
    /// The session's last known context usage (the `context_usage_update`
    /// frame's values — the provider's `input_tokens` vs the model's
    /// window), persisted on the `sessions` row on every frame so a
    /// CLOSED session's context survives (the frontend's store drops the
    /// entry on close — the row is the source of truth for the stored
    /// session's context bar). `None` until the first frame (a fresh
    /// session with no usage yet).
    pub context_usage: Option<ContextUsage>,
    /// The ephemeral-subagent flag (ADR 0025 §3): `true` for a subagent
    /// session (its transcript persists as a HIDDEN `sessions` row —
    /// `list_sessions` filters `is_subagent = 0`). `false` for a main
    /// session. `#[serde(default)]` so stored rows pre-dating the column
    /// (and the `Value::Null`-ish ephemeral envelope) deserialize `false`.
    #[serde(default)]
    pub is_subagent: bool,
}

/// A session's last known context usage (the `context_usage_update` frame's
/// values — the provider's `input_tokens` vs the model's `context_window`).
/// Persisted on the `sessions` row on every frame (the row's
/// `context_usage_json` column — `{"used": n, "window": n}`) so a CLOSED
/// session's context survives: the frontend's store drops the entry on
/// close, but the row keeps the last known value for the stored session's
/// context bar.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextUsage {
    pub used: u64,
    pub window: u64,
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

/// The Supervisor-side stalled-session bookkeeping (ADR 0025 Task 4 — a
/// Worker crash marks the session STALLED; the frontend reads the state
/// from the `session-stalled` event + the `stalled_info` query). There is
/// no backend `SessionState` enum today (session state lives in the
/// frontend store + ad-hoc flags) — the `stalled` registry is the
/// backend's source of truth for the crash + the crash-log path.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StalledInfo {
    /// Unix milliseconds (the crash time).
    pub at: i64,
    /// The newest `crash-<ts>-worker*.log` in the data dir, globbed and
    /// FROZEN at crash time (best-effort attribution across concurrent
    /// sessions — crash logs are diagnostic, not load-bearing); `None`
    /// when none is found.
    pub crash_log: Option<PathBuf>,
}

/// A live, Worker-backed session (the Supervisor-side coordinator state —
/// the in-process `NativeHandle` is gone for main sessions). The `AgentLoop`
/// runs in the session's Worker; this struct holds the Supervisor's
/// per-session bookkeeping: the `cwd` (the `respond_permission`
/// `trust-space` write + the `session-closed` reason), the close kind
/// (first-set-wins — `close_session` sets `User`), the in-flight turn's
/// resolver (`send_prompt` stores a sender; the router resolves it on
/// `agent_settled` / `Exited` / crash), the `cancel_requested` flag (a
/// late settle maps to `Cancelled`), and the UNANSWERED request ids (the
/// `PermissionRequest` / `InteractiveRequest` frames relayed MINUS the
/// `*Response`s sent — the pending-modal cleanup on session end: a session
/// ending with an open prompt must NOT leave a stuck modal).
struct LiveWorkerSession {
    cwd: PathBuf,
    close_kind: Arc<StdMutex<Option<CloseKind>>>,
    pending_turn: Arc<StdMutex<Option<oneshot::Sender<StopReason>>>>,
    cancel_requested: Arc<StdMutex<bool>>,
    pending_modal_ids: StdMutex<HashSet<String>>,
}

/// The resume context (the `establish_worker_session` `resume` parameter —
/// the stored `session_id` + the resolved `model` + the stored `thinkingLevel`
/// + the desktop's `archived` flag (ADR 0016) + the stored `context_usage`).
struct ResumeCtx {
    session_id: String,
    model: Model,
    stored_level: Option<String>,
    archived: bool,
    context_usage: Option<ContextUsage>,
}

/// A cheap, `'static`-safe handle to the native `AgentLoop` task:
/// `prompt_tx` / `control_tx` clone
/// the loop's channels and `cancel` is the loop's cancellation token. The
/// loop's `RpcEvent` `Receiver` is NOT held here (a `tokio` mpsc `Receiver`
/// is not `Clone`) — `drive_native_session` takes it by move (the driver
/// task consumes it to watch `agent_settled`; the loop has ALREADY run the
/// The subagent-close handle: the subagent cancel path (a subagent session
/// passes it to `drive_native_session`; a main session passes `None`).
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

/// The shared session-driver state. `SessionManager` (main sessions)
/// owns one.
///
/// Holds the persistence + trust-db handles (the ADR 0022/0025 worker
/// runtime — the in-process pending maps / todo / sudo / settle knobs
/// are GONE with the in-process driver: the pending maps live in the
/// session's Worker, the todo store in the Worker, the sudo flow in the
/// Worker, the settle in the `WorkerManager`'s drive tasks).
pub struct SessionDriver {
    /// Transcript persistence (main only; `None` for subagents —
    /// ephemeral, not stored). The loop's persistence goes through the
    /// `Store` seam (the `SessionStore` built from this db in
    /// `build_native_session`); the trust lookup is `trust_db` (below —
    /// main AND subagent).
    pub(crate) db: Option<Arc<Db>>,
    /// The trust lookup source (ADR 0010): the `space_trusted` lookup the
    /// permission gate uses to auto-confirm a TRUSTED Space's `confirm`.
    /// Independent of `db` — `db` gates TRANSCRIPT PERSISTENCE (main only;
    /// `None` for subagents, which are ephemeral), while `trust_db` is the
    /// trust lookup for BOTH main and subagent Sessions (`None` = fail-
    /// closed: the gate prompts, today's flow).
    pub(crate) trust_db: Option<Arc<Db>>,
}

impl SessionDriver {
    /// Create a driver (no persistence / trust db).
    pub fn new() -> Self {
        Self {
            db: None,
            trust_db: None,
        }
    }
}

/// A factory that builds a `Provider` from a `Model` (the native path's
/// provider seam — `SessionManager::provider_factory`; the production
/// default dispatches on `Model.api` (`build_provider`, ADR 0024), a test
/// sets a mock before `start_session`). `pub` so a test can swap the
/// seam (the native session builds the `Provider` through it; the
/// subagent's Worker builds its own `Provider` from the `StartEnv`
/// catalog).
pub type ProviderFactory = Arc<dyn Fn(&Model) -> Box<dyn Provider> + Send + Sync>;

/// The effective-catalog supplier: the BASE catalog + the user's providers
/// from `settings.json` (a fresh read) + live per-provider discovery (`GET
/// /v1/models`, cached per-provider) — merged via `merge_catalog` (ADR
/// 0014). `resolve` is the extracted core of the old
/// `SessionManager::effective_catalog`: the `SessionManager` keeps a thin
/// wrapper over it, and the subagent's `StartEnv` catalog is built from it
/// so a named agent's `model:` frontmatter resolves against the EFFECTIVE
/// catalog at dispatch time (NOT a startup snapshot — the base catalog is
/// empty after the pi-config seeding removal, and a startup snapshot would
/// never resolve a user-provider model).
#[derive(Clone)]
pub struct EffectiveCatalog {
    pub config_dir: PathBuf,
    pub cache: Arc<tokio::sync::Mutex<HashMap<String, ProviderDiscovery>>>,
    pub base: ModelCatalog,
}

impl EffectiveCatalog {
    /// The effective catalog (the `SessionManager::effective_catalog` core):
    /// the base catalog + the user's providers from `settings.json` (fresh
    /// read via `load_settings`), discovered via `discover_models`
    /// (best-effort; the per-provider `cache` — `force_refresh` bypasses
    /// the cache for provider `force_refresh` when `Some`). A provider whose
    /// discovery fails contributes 0 models but still shadows the base
    /// models for its id (ADR 0014 — via `merge_catalog`'s
    /// `shadowed_provider_ids`).
    pub async fn resolve(&self, force_refresh: Option<&str>) -> ModelCatalog {
        let settings = load_settings(&self.config_dir);
        let mut user_models: Vec<Model> = Vec::new();
        let mut cache = self.cache.lock().await;
        for provider in &settings.providers {
            let entry = cache
                .entry(provider.id.clone())
                .or_insert_with(ProviderDiscovery::default);
            // `force_refresh` (a provider id) bypasses the cache for that
            // provider (the settings page's refresh affordance); a failed
            // re-fetch clears the stale entry (0 models — the provider row
            // shows `unreachable` + refresh, the stale base models stay
            // shadowed).
            let bypass = Some(provider.id.as_str()) == force_refresh;
            if !entry.attempted || bypass {
                entry.attempted = true;
                match discover_models(&provider.base_url, &provider.api_key, &provider.api).await {
                    Ok(models) => entry.models = models,
                    Err(_) => entry.models.clear(),
                }
            }
            // A discovered model becomes a `Model`: the provider's
            // `base_url` / `api_key`; `context_window` falls back to
            // `DEFAULT_CONTEXT_WINDOW` (a user model has no static metadata);
            // the thinking fields map straight from the `DiscoveredMeta`
            // (`None` → `vec![]` / `false`); the provider's `api` decides
            // the wire (ADR 0024).
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
                    api: Some(provider.api.clone()),
                });
            }
        }
        // EVERY configured provider id shadows (regardless of whether its
        // discovery succeeded — a provider that discovered 0 models still
        // replaces the stale base models for its id, ADR 0014).
        let shadowed: Vec<String> = settings.providers.iter().map(|p| p.id.clone()).collect();
        merge_catalog(&self.base, &user_models, &shadowed)
    }
}

/// Manages all live native sessions (the MAIN sessions).
///
/// Owns a [`SessionDriver`] (db: attached via [`Self::attach_db`],
/// `trust_db`: the same db, attached via [`Self::attach_db`],
/// captures: `None`, subagent: injected via [`Self::set_subagent_manager`])
/// plus the config dir (the `settings.json` home — the settings-driven
/// native session source) + the base model catalog. DB recording and
/// `record_session` stay here; the driver is delegated to. (The one-live
/// policy is LIFTED — sessions coexist; a session is torn down
/// only by an explicit `close_session` or a subagent cancel.)
///
/// `Sync` — the mutable state is `Arc<Mutex<…>>` internally, so the
/// manager is managed directly (no outer lock); each method locks only
/// its own internal maps, briefly.
pub struct SessionManager {
    driver: SessionDriver,
    config_dir: PathBuf,
    /// The BASE model catalog (the desktop is native-only — the catalog is
    /// the base of the effective catalog; a test overrides it via
    /// `set_catalog`). The effective catalog merges the user's providers
    /// (the `settings.json` `providers` list, ADR 0014) + live discovery
    /// over the base (see `effective_catalog` / `EffectiveCatalog`).
    catalog: ModelCatalog,
    /// The provider factory seam (reviewer-corrected Major #21): the native
    /// path builds the `Provider` through it (the production default
    /// dispatches on `Model.api` (`build_provider`, ADR 0024); a test sets
    /// a mock BEFORE `start_session`), so `start_session` never constructs
    /// the provider inline.
    provider_factory: ProviderFactory,
    /// The per-provider live-discovery cache (the `GET /v1/models` result —
    /// the OpenAI endpoint "supplies everything"; the `pi-provider-litellm`
    /// `fetchModels` pattern). At most one fetch per provider (a failed /
    /// unreachable endpoint is not retried every session); a failure /
    /// absent model degrades to the static metadata (an empty base catalog
    /// contributes nothing — the effective catalog IS the providers list).
    discovery_cache: Arc<tokio::sync::Mutex<HashMap<String, ProviderDiscovery>>>,
    // ── The Worker-based bookkeeping (ADR 0025 Task 4) ──────────────
    // The `AgentLoop` runs in the session's Worker; the `SessionManager`
    // is the Supervisor-side coordinator. The late-wire pattern (the
    // `SessionManager` ↔ `WorkerManager` construction cycle — the
    // `WorkerManager`'s `on_event` / `on_crash` callbacks need the
    // `SessionManager`'s router, and the `SessionManager` needs the
    // `WorkerManager` to send prompts): the `OnceLock`s break the cycle
    // (the `WorkerManager` is built with `OnceLock`-backed callbacks, the
    // router is injected after both are built, then the `WorkerManager`
    // is handed to the `SessionManager`).
    /// The `WorkerManager` (the Worker registry) — `attach` / `detach` /
    /// `send_prompt` / `send_config` / `send_abort` go through it.
    worker_manager: Arc<OnceLock<Arc<WorkerManager>>>,
    /// The `SubagentSessionManager` (the `dispatch_native` home — the
    /// ADR 0025 Task 5 re-plumb: the native dispatch runs in a WORKER
    /// via the `WorkerManager`'s `dispatch_subagent` flow; `None` until
    /// `set_subagent_manager`).
    subagent_manager: Arc<OnceLock<Arc<crate::agent::subagent::SubagentSessionManager>>>,
    /// The UI sink (the `TauriSink` — the router re-emits the `SinkFrame`s
    /// on it verbatim). Late-wired (the `setup_dirs` wiring).
    sink: Arc<OnceLock<Arc<dyn EventSink>>>,
    /// The store-frame applier (the Supervisor's sole-writer persistence).
    /// Late-wired in `attach_db` (it needs the `Db`).
    persister: Arc<OnceLock<Arc<TranscriptPersister>>>,
    /// The per-session config state (the `set_config_option` re-synthesizer
    /// source — the mirror's home moved from the `NativeHandle` here,
    /// field-for-field: `start` / `resume` initialize it, `set_config_option`
    /// updates it when a change is applied).
    config_state: Arc<StdMutex<HashMap<String, NativeConfigState>>>,
    /// The live, Worker-backed sessions (session_id → the coordinator state;
    /// replaces the main-session `LiveSession` / `driver.sessions` ownership).
    live: Arc<StdMutex<HashMap<String, LiveWorkerSession>>>,
    /// The stalled sessions (session_id → `StalledInfo` — a Worker crash
    /// marks the session stalled; the frontend reads it via `stalled_info`).
    stalled: Arc<StdMutex<HashMap<String, StalledInfo>>>,
}

impl SessionManager {
    /// The configured config directory (useful for tests and diagnostics).
    pub fn config_dir(&self) -> &PathBuf {
        &self.config_dir
    }
    /// Create a manager (the config dir is the `settings.json` home — the
    /// settings-driven native session source; there is no agent registry
    /// and no pi config seeding: the desktop is native-only).
    pub fn new(config_dir: PathBuf) -> Self {
        Self {
            driver: SessionDriver::new(),
            config_dir,
            // The base catalog (empty — the desktop is native-only: the
            // effective catalog is the Settings' providers list + live
            // discovery, merged over the base, ADR 0014). Tests override it
            // via `set_catalog`.
            catalog: ModelCatalog::default(),
            // The provider factory seam (reviewer-corrected Major #21 —
            // the production default dispatches on `Model.api`
            // (`build_provider`, ADR 0024); a test sets a mock via
            // `set_provider_factory` BEFORE `start_session`).
            provider_factory: Arc::new(|m: &Model| build_provider(m)),
            discovery_cache: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            // The Worker-based bookkeeping (ADR 0025 Task 4) — the
            // `OnceLock`s are late-wired in `setup_dirs` (the
            // `SessionManager` ↔ `WorkerManager` cycle); the `StdMutex`
            // maps are fresh.
            worker_manager: Arc::new(OnceLock::new()),
            subagent_manager: Arc::new(OnceLock::new()),
            sink: Arc::new(OnceLock::new()),
            persister: Arc::new(OnceLock::new()),
            config_state: Arc::new(StdMutex::new(HashMap::new())),
            live: Arc::new(StdMutex::new(HashMap::new())),
            stalled: Arc::new(StdMutex::new(HashMap::new())),
        }
    }

    /// Inject the `WorkerManager` (the late-wire — the `setup_dirs` wiring
    /// builds the `WorkerManager` with callbacks pointing at THIS manager's
    /// router, then hands it here). `&self` (the `OnceLock` `set` needs no
    /// exclusive access) so the `Arc<SessionManager>` the `WorkerManager`
    /// callbacks capture can already own the manager. Set BEFORE any
    /// `start_session` / `send_prompt`. The ADR 0025 Task 5 re-plumb ALSO
    /// forwards it to the `SubagentSessionManager` (`set_worker_manager` —
    /// the `dispatch_native` delegation) and registers the child-cwd
    /// registrar (the `dispatch_subagent` flow's `ensure_session_row` —
    /// the ephemeral `sessions` row's `cwd` — the `StoreFrame` carries no
    /// `cwd`). The registrar's closure reads the `persister` `OnceLock`
    /// at CALL time (a dispatch runs after `attach_db`, so the persister
    /// is set by then; an unset persister is a no-op).
    pub fn attach_worker_manager(&self, wm: Arc<WorkerManager>) {
        let _ = self.worker_manager.set(wm.clone());
        if let Some(m) = self.subagent_manager.get() {
            m.set_worker_manager(wm.clone());
        }
        let persister = self.persister.clone();
        wm.set_cwd_registrar(Arc::new(move |session_id: &str, cwd: &str| {
            if let Some(p) = persister.get() {
                p.set_cwd(session_id, cwd);
            }
        }));
    }

    /// The `TranscriptPersister` (the `attach_worker_manager`'s
    /// child-cwd registrar's target — the `dispatch_subagent` flow's
    /// `ensure_session_row` creates the ephemeral `sessions` row with
    /// the child's `cwd`). `None` before `attach_db`.
    pub fn persister(&self) -> Option<Arc<TranscriptPersister>> {
        self.persister.get().cloned()
    }

    /// Inject the UI sink (the `TauriSink` — the router re-emits the
    /// `SinkFrame`s on it verbatim). Set BEFORE any `start_session`.
    pub fn set_sink(&mut self, sink: Arc<dyn EventSink>) {
        let _ = self.sink.set(sink);
    }

    /// Override the BASE model catalog (tests — the production base is
    /// empty; the effective catalog is the Settings' providers list + live
    /// discovery, merged over the base). Set BEFORE `start_session`.
    ///
    /// CAVEAT (tests): a session's `StartEnv` catalog is FIXED at
    /// `attach` time — a LATER `set_catalog` desynchronizes it (the
    /// running session's subagent dispatch keeps the old base; the
    /// subagent's model resolution runs against the PARENT's
    /// `StartEnv` catalog). Call `set_catalog` BEFORE `start_session`.
    pub fn set_catalog(&mut self, catalog: ModelCatalog) {
        self.catalog = catalog;
    }

    /// Inject the provider factory (reviewer-corrected Major #21 — the
    /// native path builds the `Provider` through it; the production default
    /// dispatches on `Model.api` (`build_provider`, ADR 0024)).
    /// Set BEFORE `start_session`.
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
            if let Ok(models) = discover_models(
                &model.base_url,
                &model.api_key,
                model.api.as_deref().unwrap_or("openai-completions"),
            )
            .await
            {
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

    /// The effective catalog: the base catalog (the desktop is native-only —
    /// the base is the static catalog a test `set_catalog`s; production is
    /// empty) + the user's providers from `settings.json` (fresh read via
    /// `load_settings`), discovered via `discover_models` (best-effort; the
    /// existing per-provider `discovery_cache` — `force_refresh` bypasses
    /// the cache for provider `force_refresh` when `Some`). A provider whose
    /// discovery fails contributes 0 models but still shadows the base
    /// models for its id (ADR 0014 — via `merge_catalog`'s
    /// `shadowed_provider_ids`). Thin wrapper over
    /// [`EffectiveCatalog::resolve`] (the subagent's resolution uses the
    /// parent's `StartEnv` catalog, built from the same core).
    pub async fn effective_catalog(&self, force_refresh: Option<&str>) -> ModelCatalog {
        EffectiveCatalog {
            config_dir: self.config_dir.clone(),
            cache: self.discovery_cache.clone(),
            base: self.catalog.clone(),
        }
        .resolve(force_refresh)
        .await
    }

    /// Attach the persistence database. Sets BOTH `db` (transcript
    /// persistence) and `trust_db` (the ADR 0010 trust lookup) to the same
    /// db — main-session behavior is unchanged (same db, same lookup).
    /// Persistence is a no-op without it.
    pub fn attach_db(&mut self, db: Arc<Db>) {
        self.driver.db = Some(db.clone());
        self.driver.trust_db = Some(db.clone());
        // The store-frame applier (the Supervisor's sole-writer persistence —
        // it needs the `Db`). Late-wired here (the `Db` is attached after
        // `new`).
        let _ = self.persister.set(Arc::new(TranscriptPersister::new(db)));
    }

    /// Inject the subagent manager (main only — sets the driver's
    /// `subagent` handle so the main session's `task` tool can dispatch
    /// through it). The subagent manager needs nothing from the main
    /// manager; only this field points at it (intra-crate type cycles are
    /// fine in Rust).
    ///
    /// ALSO wires the native-harness deps onto the manager (set-once via
    /// `set_native_deps` — a `&self` `OnceLock`, so it's callable through
    /// the `Arc` received here): the `db` is the SIGNAL that native wiring
    /// is present (skipped when `None`); the `catalog` is the EFFECTIVE
    /// catalog supplier (`EffectiveCatalog` — the base catalog + the
    /// settings' providers + live discovery, resolved at dispatch time —
    /// the desktop is native-only, so a named agent's `model:` frontmatter
    /// resolves against the effective catalog, not a startup snapshot).
    /// `trust_db` IS threaded (the SAME db — ADR 0010: a native child in a
    /// trusted Space inherits the parent's trust); it is `Some` whenever
    /// `db` is (both are set together by `attach_db`).
    pub fn set_subagent_manager(&mut self, m: Arc<crate::agent::subagent::SubagentSessionManager>) {
        let _ = self.subagent_manager.set(m);
    }

    /// Reset the session's open thinking segment at a prompt boundary. The
    /// frontend's `addUserMessage` starts a new thinking block on a user
    /// `persist_update`-era rule: the normalizer never sees user messages
    /// (they are recorded by the `send_prompt` paths, not the event
    /// normalizer) — so the Rust accumulator must be reset here, not in
    /// `compute_display_rows`.
    pub async fn begin_user_turn(&self, session_id: &str) {
        // (ADR 0025) The `thought_state` accumulator now lives in the
        // Worker's `AgentLoop` (the display rows are computed in the
        // Worker's `emit` and shipped as `DisplayUpsert` frames), so the
        // Supervisor's `thought_state` reset is a no-op — the Worker
        // resets its own accumulator at the prompt boundary. The method
        // stays (the `send_prompt` path is unchanged); it is a no-op.
        let _ = self
            .live
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(session_id);
    }

    /// Number of live sessions (the Worker-backed `live` map).
    pub async fn session_count(&self) -> usize {
        self.live.lock().unwrap_or_else(|p| p.into_inner()).len()
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
    /// The native session (the desktop is native-only, ADR 0011): resolve
    /// the model (the `Settings.default_model` → the catalog default —
    /// the resolution chain, never a hard error), build the `Provider`
    /// (the `provider_factory` seam), the `SessionStore` (the
    /// `native_messages` table), and the `AgentLoop` — `tokio::spawn` it
    /// (IN-PROCESS; no subprocess), drive it (the `drive_native_session`
    /// driver task — `pending_turn` / `settle_tx` / `close_kind` populated
    /// identically to a subagent dispatch), and record the session (the
    /// `capabilities_json` has NO `piSessionFile` — resume is from the
    /// `native_messages` table, so `loadSession` is `true`).
    ///
    /// The `config_options` are SYNTHESIZED from the `ModelCatalog` (the
    /// `synthesize_catalog_config_options` shape — the frontend is
    /// unchanged).
    /// A FRESH session (ADR 0025): resolve the model (the `Settings.default_model`
    /// → the catalog default — the resolution chain, never a hard error), build the
    /// `StartEnv` (the `AgentLoop` runs in the Worker — Task 2's `build_loop` builds
    /// it; the Supervisor only supplies the envelope), `attach` the Worker (a spawn
    /// failure → the command returns an error — the visible session error, NO
    /// in-process fallback), and record the session (the `capabilities_json` has NO
    /// `piSessionFile` — resume is from the `native_messages` table, so
    /// `loadSession` is `true`). The `config_options` are synthesized from the
    /// `ModelCatalog` (the existing shape — the frontend is unchanged).
    async fn start_native_session(
        &self,
        cwd: PathBuf,
        sink: &Arc<dyn EventSink>,
    ) -> Result<SessionInfo, SessionError> {
        self.establish_worker_session(cwd, None, sink).await
    }

    /// A RESUME (ADR 0025): the `native_messages` re-read (the
    /// `SessionStore::load_messages` path — moved from harness to Supervisor) +
    /// the `StartEnv { mode: Resume, transcript: Some(…) }` + `attach` (a fresh
    /// Worker + re-hydrate — a `stalled` session's resume is this same path). The
    /// model comes from the stored `capabilities.model` (a stale / unknown key
    /// falls back to the resolution chain); the thinking level from the stored
    /// `thinkingLevel` (the resolution chain (remembered → stored → settings)
    /// resolves in `establish_worker_session`, after the model metadata refresh).
    async fn resume_native_session(
        &self,
        session_id: &str,
        cwd: PathBuf,
        sink: &Arc<dyn EventSink>,
    ) -> Result<SessionInfo, SessionError> {
        let db = self.driver.db.clone().ok_or_else(|| {
            SessionError::Io("a native session requires an attached database".to_string())
        })?;
        // The stored row must exist (a native session is recorded at start —
        // `record_session`; a missing row is unresumable). KEEP THE WHOLE ROW:
        // the resume carries the desktop's `archived` flag (ADR 0016) + the
        // stored `context_usage`.
        let row =
            db.session(session_id)
                .ok()
                .flatten()
                .ok_or_else(|| SessionError::NotResumable {
                    id: session_id.to_string(),
                })?;
        let caps: Value = serde_json::from_str(&row.capabilities_json).unwrap_or(Value::Null);
        let catalog = self.effective_catalog(None).await;
        let settings = load_settings(&self.config_dir);
        let model = caps
            .get("model")
            .and_then(Value::as_str)
            .and_then(|key| resolve_composed_model(&catalog, key))
            .or_else(|| resolve_native_model(&catalog, &settings.default_model).ok())
            .ok_or_else(|| SessionError::Command {
                error: "no models available for the native session".to_string(),
            })?;
        let stored_level = caps
            .get("thinkingLevel")
            .and_then(Value::as_str)
            .map(str::to_string);
        let context_usage = row
            .context_usage_json
            .as_deref()
            .and_then(|s| serde_json::from_str::<ContextUsage>(s).ok());
        self.establish_worker_session(
            cwd,
            Some(ResumeCtx {
                session_id: session_id.to_string(),
                model,
                stored_level,
                archived: row.archived,
                context_usage,
            }),
            sink,
        )
        .await
    }

    /// Establish one Worker-backed session (shared by `start` / `resume`;
    /// `resume` carries the stored `session_id` + the loaded transcript source —
    /// `start` mints a fresh UUID and starts with an empty transcript). Resolves
    /// the model + thinking level (the ADR 0015 chain), builds the `StartEnv`,
    /// `attach`es the Worker (a spawn failure → an error — NO in-process fallback),
    /// records the session, and stores the per-session coordinator state.
    #[allow(clippy::too_many_arguments)]
    async fn establish_worker_session(
        &self,
        cwd: PathBuf,
        resume: Option<ResumeCtx>,
        sink: &Arc<dyn EventSink>,
    ) -> Result<SessionInfo, SessionError> {
        // The `WorkerManager` (the late-wire — the `setup_dirs` wiring attached it
        // before any `start_session` / `send_prompt`).
        let wm = self.worker_manager.get().cloned().ok_or_else(|| {
            SessionError::Io("the session's WorkerManager is not attached".to_string())
        })?;
        let db = self.driver.db.clone().ok_or_else(|| {
            SessionError::Io("a native session requires an attached database".to_string())
        })?;
        // The EFFECTIVE catalog (the base catalog + the user's providers, a fresh
        // `load_settings` read; the per-provider `discovery_cache` makes the fetch
        // cheap): the model resolution + the `StartEnv`'s `catalog`.
        let catalog = self.effective_catalog(None).await;
        let settings = load_settings(&self.config_dir);
        let is_resume = resume.is_some();
        let (session_id, model, stored_level, archived, context_usage) = match resume {
            Some(ctx) => (
                ctx.session_id,
                ctx.model,
                ctx.stored_level,
                ctx.archived,
                ctx.context_usage,
            ),
            None => (
                mint_session_id(),
                resolve_native_model(&catalog, &settings.default_model)?,
                None,
                false,
                None,
            ),
        };
        // (live `/v1/models` discovery) Best-effort refresh the model's metadata
        // (the same as before; a failure degrades to the static metadata).
        let model = self.refresh_model_metadata(&model).await;
        // (ADR 0015) The effective thinking level (the same chain as before —
        // remembered > stored > settings > `None`).
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
            .or_else(|| {
                settings
                    .default_thinking_level
                    .as_ref()
                    .filter(|l| {
                        model.thinking_levels.is_empty()
                            || model.thinking_levels.iter().any(|t| t == *l)
                    })
                    .cloned()
            });
        // The `StartEnv` (the `AgentLoop` runs in the Worker — Task 2's
        // `build_loop` builds it; the Supervisor supplies the envelope). A RESUME
        // carries the `native_messages` re-read (the `SessionStore::load_messages`
        // path — moved from harness to Supervisor); a FRESH session has no
        // transcript (the Worker builds the main system prompt itself, seq 0).
        let transcript = if is_resume {
            let store = Arc::new(SessionStore::new(db.clone()));
            // A corrupt transcript row must NOT silently load as an empty one
            // (store.rs): the next persist would upsert over the stored rows
            // (seq 0 = the system prompt, cascading to seq 1, 2, …), so a load
            // failure fails the session start.
            let messages = store
                .load_messages(&session_id)
                .map_err(|e| SessionError::Io(e.to_string()))?;
            Some(messages)
        } else {
            None
        };
        let env = StartEnv::from_parts(
            session_id.clone(),
            cwd.display().to_string(),
            if is_resume {
                StartMode::Resume
            } else {
                StartMode::Fresh
            },
            transcript,
            model.clone(),
            catalog.clone(),
            thinking_level.clone(),
            db.space_trusted(&cwd).unwrap_or(false),
            if settings.enabled_tools.is_empty() {
                None
            } else {
                Some(settings.enabled_tools.clone())
            },
            self.config_dir.display().to_string(),
            true, // `subagent_enabled` (main session — the Worker's `subagent` tool)
            None, // `system_prompt` (main sessions: the Worker builds the main prompt)
        );
        // SPAWN the Worker (a spawn failure → the command returns an error — the
        // visible session error, NO in-process fallback — ADR 0025).
        wm.attach(&session_id, &env)
            .await
            .map_err(|e| SessionError::Command {
                error: format!("spawn the session's Worker: {e}"),
            })?;
        // The `SessionInfo` (the `capabilities` envelope — the `native_capabilities`
        // shape; the `config_options` synthesized from the EFFECTIVE catalog — the
        // `StartEnv`'s `catalog` is the effective catalog). Block-scoped so the
        // `MutexGuard` (and the `Arc` it borrows through) die BEFORE the `await`
        // below (a `std::sync::MutexGuard` is not `Send`).
        let info = SessionInfo {
            session_id: session_id.clone(),
            cwd: cwd.clone(),
            capabilities: native_capabilities(&env.model, env.thinking.as_deref()),
            config_options: synthesize_catalog_config_options(
                &env.catalog,
                &env.model,
                env.thinking.as_deref(),
            ),
            archived,
            context_usage,
            is_subagent: false,
        };
        // The `sessions` row (the FK source for the Worker's `native_messages`
        // upserts — the `TranscriptPersister`'s `ensure_session_row` is a no-op
        // for a main session; this records it up front) + the `persister`'s
        // `cwd` map (the `ensure_session_row` source).
        self.record_session(&info);
        if let Some(p) = self.persister.get() {
            p.set_cwd(&session_id, &cwd.display().to_string());
        }
        // The per-session coordinator state (the config-state mirror — the
        // `set_config_option` re-synthesizer source; the `live` entry — the
        // `pending_turn` / `cancel_requested` / `close_kind` / `pending_modal_ids`).
        self.config_state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(
                session_id.clone(),
                NativeConfigState {
                    model: env.model.clone(),
                    thinking_level: env.thinking.clone(),
                },
            );
        let live = LiveWorkerSession {
            cwd: cwd.clone(),
            close_kind: Arc::new(StdMutex::new(None)),
            pending_turn: Arc::new(StdMutex::new(None)),
            cancel_requested: Arc::new(StdMutex::new(false)),
            pending_modal_ids: StdMutex::new(HashSet::new()),
        };
        self.live
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(session_id.clone(), live);
        // The `sink` is threaded for the `config_option_update` re-synthesis
        // (the `set_config_option` path); the router uses `self.sink` (the
        // `setup_dirs` wiring) for the `SinkFrame` re-emit.
        let _ = sink;
        Ok(info)
    }

    /// All stored sessions (newest first), as `SessionInfo` (the
    /// `list_sessions` command's shape): the row's `capabilities` (the
    /// `normalize_capabilities` camelCase envelope), `config_options`
    /// synthesized from the stored `model` / `thinkingLevel` (the same
    /// resolution as `resume_native_session` — the stored composed key →
    /// the effective catalog; an absent / unresolvable key falls back to
    /// the resolution chain, never a hard error) so a STORED session's
    /// composer selectors are populated (the frontend renders them
    /// disabled — a stored session can't receive `set_config_option`),
    /// and `context_usage` read from the row's `context_usage_json` (the
    /// last known usage — the frontend's store drops it on close, so the
    /// row is the source of truth for the stored session's context bar).
    pub async fn list_sessions(
        &self,
        include_archived: bool,
    ) -> Result<Vec<SessionInfo>, SessionError> {
        let db = self.driver.db.clone().ok_or_else(|| {
            SessionError::Io("list_sessions requires an attached database".to_string())
        })?;
        let rows = db
            .list_sessions(include_archived)
            .map_err(|e| SessionError::Io(e.to_string()))?;
        let catalog = self.effective_catalog(None).await;
        let settings = load_settings(&self.config_dir);
        Ok(rows
            .into_iter()
            .map(|row| {
                let caps: Value =
                    serde_json::from_str(&row.capabilities_json).unwrap_or(Value::Null);
                // The model: the stored `model` (a composed key → the
                // effective catalog); an absent / unresolvable key falls
                // back to the resolution chain (the settings default →
                // the catalog default → the selectable set — never a
                // hard error). `None` when the chain is empty (no models
                // at all — the options can't be synthesized).
                let model = caps
                    .get("model")
                    .and_then(Value::as_str)
                    .and_then(|key| resolve_composed_model(&catalog, key))
                    .or_else(|| resolve_native_model(&catalog, &settings.default_model).ok());
                let thinking_level = caps
                    .get("thinkingLevel")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                SessionInfo {
                    session_id: row.id,
                    cwd: PathBuf::from(row.cwd),
                    capabilities: normalize_capabilities(&row.capabilities_json),
                    config_options: model.as_ref().and_then(|m| {
                        synthesize_catalog_config_options(&catalog, m, thinking_level.as_deref())
                    }),
                    archived: row.archived,
                    is_subagent: row.is_subagent,
                    context_usage: row
                        .context_usage_json
                        .as_deref()
                        .and_then(|s| serde_json::from_str::<ContextUsage>(s).ok()),
                }
            })
            .collect())
    }

    /// Start a native session: canonicalize the cwd, then delegate to
    /// [`Self::start_native_session`] (the desktop is native-only — there
    /// is no external path and no agent id; the model / thinking level /
    /// tools come from the `Settings`).
    ///
    /// Returns the [`SessionInfo`] once the session is established. The
    /// loop is driven by a background task that lives for the session's
    /// lifetime; it is torn down by [`Self::close_session`].
    pub async fn start_session(
        &self,
        cwd: PathBuf,
        sink: &Arc<dyn EventSink>,
    ) -> Result<SessionInfo, SessionError> {
        // Canonicalize BEFORE the space row is touched: the spaces join
        // key is the canonicalized cwd, so a `~/x` / symlink spelling must
        // not produce a different row.
        let cwd = std::fs::canonicalize(&cwd).map_err(|_| SessionError::FolderMissing {
            path: cwd.display().to_string(),
        })?;
        self.start_native_session(cwd, sink).await
    }

    /// Resume a stored session: canonicalize the cwd, then delegate to
    /// [`Self::resume_native_session`] (a fresh `AgentLoop` +
    /// `SessionStore::load_messages` — resume from the `native_messages`
    /// table; the stored row's `model` / `thinkingLevel` override the
    /// resolution chains).
    ///
    /// Returns [`SessionError::NotResumable`] when the stored row is
    /// missing (the UI shows the history-only banner instead).
    pub async fn resume_session(
        &self,
        session_id: &str,
        cwd: PathBuf,
        sink: &Arc<dyn EventSink>,
    ) -> Result<SessionInfo, SessionError> {
        // Canonicalize (same rationale as `start_session`): everything
        // downstream (the loop cwd, `SessionInfo.cwd`, the space join key)
        // uses the canonical path.
        let cwd = std::fs::canonicalize(&cwd).map_err(|_| SessionError::FolderMissing {
            path: cwd.display().to_string(),
        })?;
        self.resume_native_session(session_id, cwd, sink).await
    }

    /// Send a prompt to a live session and wait for the turn to finish.
    ///
    /// Returns the [`StopReason`] the turn resolved to (the frontend needs
    /// the turn-completion signal).
    pub async fn send_prompt(
        &self,
        session_id: &str,
        text: String,
    ) -> Result<StopReason, SessionError> {
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
    ) -> Result<StopReason, SessionError> {
        // The `WorkerManager` (the late-wire) + the per-session coordinator
        // state (the `LiveWorkerSession` — the `pending_turn` / `cancel_requested`
        // moved from the `NativeHandle` here).
        let wm = self.worker_manager.get().cloned().ok_or_else(|| {
            SessionError::Io("the session's WorkerManager is not attached".to_string())
        })?;
        let (pending_turn, cancel_requested) = {
            let live = self.live.lock().unwrap_or_else(|p| p.into_inner());
            let live = live
                .get(session_id)
                .ok_or_else(|| SessionError::UnknownSession {
                    id: session_id.to_string(),
                })?;
            (live.pending_turn.clone(), live.cancel_requested.clone())
        };

        // Validate the images FIRST: a rejected payload must NOT be written
        // to the transcript and must NOT start a user turn — validating
        // before persisting is what keeps the "cap bounds DB growth"
        // guarantee real.
        validate_images(&images)?;

        // A session is one-turn-at-a-time (the frontend's composer is locked
        // until the turn resolves): a turn ALREADY IN FLIGHT (the `pending_turn`
        // slot is occupied) is REJECTED rather than queued. The check-and-claim
        // is ATOMIC under ONE `pending_turn` lock acquisition (finding 2 — the
        // resolver is claimed BEFORE the user-row write / dispatch, so a settle
        // arriving while the prompt is in flight is never lost on an empty slot).
        let (tx, rx) = oneshot::channel::<StopReason>();
        {
            let mut slot = pending_turn.lock().unwrap_or_else(|p| p.into_inner());
            if slot.is_some() {
                return Err(SessionError::Command {
                    error: "a turn is already in flight".to_string(),
                });
            }
            *slot = Some(tx);
        }
        // Reset the cancel flag (a fresh turn is not a cancel).
        *cancel_requested.lock().unwrap_or_else(|p| p.into_inner()) = false;

        // Record the user's message in the transcript (the client owns history)
        // before the turn begins — the USER DISPLAY ROW write stays Supervisor-side
        // (the Worker persists only the provider-transcript user row via the store
        // frames; the display row + `begin_user_turn` stay in the `send_prompt`
        // path, unchanged).
        self.begin_user_turn(session_id).await;
        if let Some(db) = &self.driver.db {
            let payload = user_message_payload(&text, &images);
            let _ = db.record_message(session_id, "user", None, &payload.to_string());
        }

        // Queue the text + the image attachments on the WORKER's prompt queue
        // (the `Prompt` frame — the `ImagePayload` maps onto the wire `ImageRef`).
        // A `SendError` (the Worker died / the stdin pipe closed) maps to the
        // same error path as a refusal below (resolve the turn `Refusal`).
        let image_refs: Vec<ImageRef> = images
            .iter()
            .map(|img| ImageRef {
                data: img.data.clone(),
                mime_type: img.mime_type.clone(),
            })
            .collect();
        let send_error = match wm.handle_for(session_id) {
            Some(handle) => match handle.send_prompt(&text, &image_refs) {
                Ok(()) => None,
                Err(e) => Some(SessionError::Command {
                    error: format!("send the prompt to the session's Worker: {e}"),
                }),
            },
            None => Some(SessionError::UnknownSession {
                id: session_id.to_string(),
            }),
        };

        // A `success: false` prompt (the Worker died) — resolve the turn
        // `Refusal` without waiting for a settle that never comes.
        if let Some(e) = send_error {
            match e {
                SessionError::Command { error } => {
                    let mut slot = pending_turn.lock().unwrap_or_else(|p| p.into_inner());
                    if let Some(tx) = slot.take() {
                        let _ = tx.send(StopReason::Refusal);
                    }
                    return Err(SessionError::Command { error });
                }
                other => return Err(other),
            }
        }

        // Await the turn (unbounded — the turn can block on a user-paced permission
        // prompt). The Worker's `agent_settled` (the `Event(RpcEvent)` bookkeeping)
        // resolves the `pending_turn` (the router); a dropped resolver (replaced by
        // a new prompt, or the session died — the `Exited` / crash path) maps to
        // `Cancelled` (the composer unlocks — it never stays locked forever).
        match rx.await {
            Ok(reason) => Ok(reason),
            Err(_) => Ok(StopReason::Cancelled),
        }
    }

    /// Cancel the session's in-flight prompt turn (the user pressed Esc).
    /// The `cancel_requested` flag is set BEFORE the cancel is sent so a
    /// fast settle maps to `Cancelled`, not `EndTurn`. Fire-and-forget on
    /// the loop side: a no-op if there is no in-flight turn.
    ///
    /// BEHAVIOR (finding 8c): `handle.cancel()` cancels the loop's current
    /// TURN token — the loop settles the turn `Cancelled` and STAYS ALIVE
    /// (a new prompt reuses the session; only a `close_session` —
    /// `handle.close` — tears the session down). Pre-fix the Stop cancelled
    /// the loop's teardown token, which ENDED THE WHOLE SESSION (the driver
    /// tore it down) — a silent asymmetry with the pi `abort`.
    pub async fn cancel_session(&self, session_id: &str) -> Result<(), SessionError> {
        // The `WorkerManager` (the late-wire) + the per-session coordinator
        // state (the `cancel_requested` moved from the `NativeHandle` here).
        let wm = self.worker_manager.get().cloned().ok_or_else(|| {
            SessionError::Io("the session's WorkerManager is not attached".to_string())
        })?;
        let cancel_requested = {
            let live = self.live.lock().unwrap_or_else(|p| p.into_inner());
            live.get(session_id)
                .map(|l| l.cancel_requested.clone())
                .ok_or_else(|| SessionError::UnknownSession {
                    id: session_id.to_string(),
                })?
        };
        // The `cancel_requested` flag is set BEFORE the `abort` is sent so a fast
        // settle maps to `Cancelled`, not `EndTurn`. Fire-and-forget on the Worker
        // side: a no-op if there is no in-flight turn. BEHAVIOR (finding 8c): the
        // `abort` cancels the Worker's current TURN token — the loop settles the
        // turn `Cancelled` and STAYS ALIVE (a new prompt reuses the session; only a
        // `close_session` tears the session down).
        *cancel_requested.lock().unwrap_or_else(|p| p.into_inner()) = true;
        if let Some(handle) = wm.handle_for(session_id) {
            let _ = handle.send_abort();
        }
        Ok(())
    }
    /// Set a session config option (the model or the thinking level) on a
    /// live session.
    ///
    /// Applies the change through the loop's control channel (`AgentLoop::set_model`
    /// / `set_thinking_level` — applied when the loop is idle), then
    /// re-synthesizes the config options FROM THE `ModelCatalog` (a native
    /// session has no `get_state`) and emits a `config_option_update` (the
    /// agent does not emit one itself — the client owns the frame).
    pub async fn set_config_option(
        &self,
        session_id: &str,
        config_id: &str,
        value: &str,
        sink: &Arc<dyn EventSink>,
    ) -> Result<Vec<Value>, SessionError> {
        // The `WorkerManager` (the late-wire) + the per-session config state
        // (the `NativeConfigState` mirror moved from the `NativeHandle` here —
        // the `set_config_option` re-synthesizer source).
        let wm = self.worker_manager.get().cloned().ok_or_else(|| {
            SessionError::Io("the session's WorkerManager is not attached".to_string())
        })?;
        let state = {
            let config_state = self.config_state.lock().unwrap_or_else(|p| p.into_inner());
            config_state
                .get(session_id)
                .cloned()
                .ok_or_else(|| SessionError::UnknownSession {
                    id: session_id.to_string(),
                })?
        };

        // The `Model` lookup + the re-synthesizer run against the EFFECTIVE
        // catalog (the base catalog + the user's providers — a user-provider
        // model can be switched TO mid-session, not just the base ones).
        let catalog = self.effective_catalog(None).await;
        // Apply (the `WorkerManager`'s `send_config` — the `Config` frame → the
        // Worker's `AgentLoop` `set_model` / `set_thinking_level`, applied when
        // the loop is idle) + mirror the change on the config state (the
        // re-synthesizer source). Mirror + emit ONLY when the `send_config`
        // SUCCEEDS (finding 12): a failed send (the Worker died / the pipe
        // closed) means the loop never applies the change — claiming success
        // (a mirrored state + a `config_option_update`) would silently diverge
        // from the model / level the loop is actually running.
        match config_id {
            "model" => {
                let (provider, model_id) =
                    value
                        .split_once('/')
                        .ok_or_else(|| SessionError::InvalidPrompt {
                            reason: format!(
                                "invalid model value: {value} (expected provider/modelId)"
                            ),
                        })?;
                let model = catalog
                    .models
                    .iter()
                    .find(|m| m.provider == provider && m.id == model_id)
                    .cloned()
                    .ok_or_else(|| SessionError::Command {
                        error: format!("unknown model: {value}"),
                    })?;
                self.send_config_to(&wm, session_id, Some(&model), None, None)?;
                self.update_config_state(session_id, |s| s.model = model.clone());
                // (ADR 0015) Minimal-surprise reset: the current level is KEPT across
                // the switch when valid for the new model (its `thinking_levels` are
                // non-empty and contain it — or EMPTY, the status quo); it is replaced
                // (the new model's remembered level, or `None`) only when the new model
                // doesn't support it. The second `send_config` mirrors ONLY on success
                // (finding 12): a failed send leaves the mirror untouched. The arm does
                // NOT write memory.
                let level = self
                    .config_state
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .get(session_id)
                    .and_then(|s| s.thinking_level.clone());
                if let Some(level) = level {
                    let valid = model.thinking_levels.is_empty()
                        || model.thinking_levels.iter().any(|t| t == &level);
                    if !valid {
                        let reset = remembered_thinking_level(
                            &load_settings(&self.config_dir).default_thinking_levels,
                            &model,
                        );
                        let sent =
                            self.send_config_to(&wm, session_id, None, reset.as_deref(), None);
                        if sent.is_ok() {
                            self.update_config_state(session_id, |s| {
                                s.thinking_level = reset.clone();
                            });
                        }
                    }
                }
            }
            "thought_level" => {
                // (review finding) An empty level is rejected BEFORE the `send_config`
                // is touched (mirroring the model arm's rejection of an unresolvable
                // key — an empty level would flow `reasoning_effort: Some("")` into the
                // provider request body, which some endpoints reject).
                if value.is_empty() {
                    return Err(SessionError::Command {
                        error: "a thinking level must be non-empty".to_string(),
                    });
                }
                self.send_config_to(&wm, session_id, None, Some(value), None)?;
                self.update_config_state(session_id, |s| {
                    s.thinking_level = Some(value.to_string());
                });
                // (ADR 0015) Remember the level for the session's CURRENT model
                // (best-effort: a write failure is logged and does NOT fail the config
                // change — the level is applied in the loop either way; the value is
                // guaranteed non-empty — an empty one is rejected above).
                let key = format!("{}/{}", state.model.provider, state.model.id);
                let mut settings = load_settings(&self.config_dir);
                settings
                    .default_thinking_levels
                    .insert(key, value.to_string());
                if let Err(e) = write_settings(&self.config_dir, &settings) {
                    eprintln!("remembered thinking level: save failed: {e}");
                }
            }
            other => {
                return Err(SessionError::Command {
                    error: format!("unknown config option: {other}"),
                })
            }
        }
        // Re-synthesize FROM THE `ModelCatalog` (a native session has no `get_state`)
        // + emit (the agent does not emit a `config_option_update` itself — the
        // client owns the frame).
        let state = self
            .config_state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(session_id)
            .cloned()
            .ok_or_else(|| SessionError::UnknownSession {
                id: session_id.to_string(),
            })?;
        let options = synthesize_catalog_config_options(
            &catalog,
            &state.model,
            state.thinking_level.as_deref(),
        )
        .ok_or_else(|| SessionError::Command {
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

    /// Send a `Config` frame to the session's Worker (the `send_config` relay —
    /// the `set_config_option` model / thinking changes). A `WorkerError` (the
    /// Worker died / the pipe closed) maps to the "config change could not be
    /// applied" error (finding 12 — a failed send leaves the mirror untouched).
    fn send_config_to(
        &self,
        wm: &WorkerManager,
        session_id: &str,
        model: Option<&Model>,
        thinking: Option<&str>,
        trusted: Option<bool>,
    ) -> Result<(), SessionError> {
        let Some(handle) = wm.handle_for(session_id) else {
            return Err(SessionError::UnknownSession {
                id: session_id.to_string(),
            });
        };
        handle
            .send_config(model, thinking, trusted)
            .map_err(|e| SessionError::Command {
                error: format!("the session's Worker is not running; the config change could not be applied: {e}"),
            })
    }

    /// Update the session's `NativeConfigState` mirror (the `set_config_option`
    /// re-synthesizer source) in place.
    fn update_config_state(&self, session_id: &str, f: impl Fn(&mut NativeConfigState)) {
        let mut config_state = self.config_state.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(state) = config_state.get_mut(session_id) {
            f(state);
        }
    }

    /// Deliver the user's answer to a pending permission request.
    ///
    /// Looks up the oneshot sender by the compound key
    /// `"{session_id}/{request_id}"` and sends the outcome through it. If the
    /// entry is gone (the session closed, or the prompt already resolved), this
    /// is a silent no-op. Returns `true` when an entry was resolved (the caller
    /// can then route a miss to the subagent manager).
    /// Deliver the user's answer to a pending permission request.
    ///
    /// The `pending_permissions` map lives in the WORKER now (the main-session
    /// `SessionManager` ownership is gone — ADR 0025): this relays the `outcome`
    /// to the session's Worker (`send_permission_response` — the Worker's
    /// `PermissionResponse` handler resolves its `pending_permissions` oneshot,
    /// or no-ops if the id is unknown). ADDED (Task 1 moved the `trust-space`
    /// write out of the gate): when the outcome is `Selected { option_id:
    /// "trust-space" }`, the Supervisor applies the `spaces` write (it owns the
    /// `spaces` table — the persistence half; the Worker's `StaticTrustSource`
    /// flip is the live-lookup half). Marks the request id as ANSWERED (the
    /// pending-modal cleanup — the `interactive-request-close` diff).
    pub async fn respond_permission(
        &self,
        session_id: &str,
        request_id: &str,
        outcome: permission::PermissionOutcome,
    ) -> Result<bool, SessionError> {
        let wm = self.worker_manager.get().cloned().ok_or_else(|| {
            SessionError::Io("the session's WorkerManager is not attached".to_string())
        })?;
        // The `cwd` (the `trust-space` write) + the "answered" mark (the
        // pending-modal cleanup — the `interactive-request-close` diff).
        let cwd = {
            let live = self.live.lock().unwrap_or_else(|p| p.into_inner());
            live.get(session_id).map(|l| l.cwd.clone())
        };
        let Some(handle) = wm.handle_for(session_id) else {
            // No live Worker for this session (a main session that was never
            // started, or already closed) — the caller routes a miss to the
            // subagent manager (the subagent's own `pending_*` maps, Task 5).
            return Ok(false);
        };
        match handle.send_permission_response(request_id, &outcome) {
            Ok(()) => {
                // (Task 1) The `trust-space` outcome persists the `spaces` write
                // (the Supervisor owns the `spaces` table — the existing
                // `set_space_trusted` `Db` method). The Worker's `StaticTrustSource`
                // flip (Task 2's `PermissionResponse` handler) is the live-lookup
                // half; this is the persistence half.
                if let permission::PermissionOutcome::Selected { option_id } = &outcome {
                    if option_id == "trust-space" {
                        if let (Some(db), Some(cwd)) = (&self.driver.db, cwd) {
                            let _ = db.set_space_trusted(&cwd.display().to_string(), true);
                        }
                    }
                }
                // Mark the request id as ANSWERED (the pending-modal cleanup — a
                // session ending with an open prompt must NOT leave a stuck modal).
                if let Some(live) = self
                    .live
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .get_mut(session_id)
                {
                    live.pending_modal_ids
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .remove(request_id);
                }
                Ok(true)
            }
            Err(_) => Ok(false),
        }
    }

    /// Deliver the user's answer to a pending interactive request.
    ///
    /// The `pending_bridge` / `pending_sudo` maps live in the WORKER now (the
    /// main-session `SessionManager` ownership is gone — ADR 0025): this relays
    /// the `result` `Value` verbatim to the session's Worker (`send_interactive_response`
    /// — the Worker's `InteractiveResponse` handler resolves its
    /// `pending_bridge` / `pending_sudo` oneshot, or no-ops if the id is unknown).
    /// Marks the request id as ANSWERED (the pending-modal cleanup).
    pub async fn respond_interactive_request(
        &self,
        session_id: &str,
        request_id: &str,
        result: serde_json::Value,
    ) -> Result<bool, SessionError> {
        let wm = self.worker_manager.get().cloned().ok_or_else(|| {
            SessionError::Io("the session's WorkerManager is not attached".to_string())
        })?;
        let Some(handle) = wm.handle_for(session_id) else {
            // No live Worker for this session — the caller routes a miss to the
            // subagent manager (the subagent's own `pending_*` maps, Task 5).
            return Ok(false);
        };
        match handle.send_interactive_response(request_id, &result) {
            Ok(()) => {
                // Mark the request id as ANSWERED (the pending-modal cleanup).
                if let Some(live) = self
                    .live
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .get_mut(session_id)
                {
                    live.pending_modal_ids
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .remove(request_id);
                }
                Ok(true)
            }
            Err(_) => Ok(false),
        }
    }

    /// Close a live session.
    ///
    /// Records the `User` close kind (first-set-wins) + marks the in-flight turn
    /// a cancel, then `detach`es the Worker (the `WorkerManager` removes the
    /// `workers` entry + spawns the background reap — the graceful `close` +
    /// grace + `kill`). The Worker's clean exit (`Exited`) triggers the router's
    /// EXPECTED-exit handling (the `session-closed` emit + the `pending_turn`
    /// `Cancelled` resolution + the pending-modal cleanup + the live-bookkeeping
    /// removal). A `detach` miss (the session is already gone) is a no-op.
    pub async fn close_session(&self, session_id: &str) -> Result<(), SessionError> {
        let wm = self.worker_manager.get().cloned().ok_or_else(|| {
            SessionError::Io("the session's WorkerManager is not attached".to_string())
        })?;
        let (close_kind, cancel_requested) = {
            let live = self.live.lock().unwrap_or_else(|p| p.into_inner());
            let live = live
                .get(session_id)
                .ok_or_else(|| SessionError::UnknownSession {
                    id: session_id.to_string(),
                })?;
            (live.close_kind.clone(), live.cancel_requested.clone())
        };
        // A close KILLS an in-flight turn: mark it a cancel BEFORE the `detach`
        // (the router's `agent_settled` arm may win the race over the `Exited`
        // teardown — the `cancel_requested` flag maps that settle to `Cancelled`,
        // not `EndTurn`, for a turn that was killed, not ended; the `Exited`
        // teardown arm sends `Cancelled` too, so the outcome is `Cancelled` either
        // way, not timing-dependent).
        *cancel_requested.lock().unwrap_or_else(|p| p.into_inner()) = true;
        // Decide the kind BEFORE starting the close. First-set-wins: a kind
        // already present means the close is in progress (another setter won the
        // race) or the reason is already decided.
        if let Ok(mut kind) = close_kind.lock() {
            if kind.is_none() {
                *kind = Some(CloseKind::User);
            }
        }
        // `detach` the Worker (the `WorkerManager` removes the `workers` entry +
        // spawns the background reap — the `Exited` is EXPECTED: NO `on_crash`;
        // the router's EXPECTED-exit handling does the `session-closed` cleanup).
        // A `detach` miss (the session was already torn down) is a no-op — the
        // `UnknownSession` above guards the `live` entry, so a miss here means the
        // Worker was already reaped (idempotent close).
        let _ = wm.detach(session_id);
        Ok(())
    }

    // ── The Worker event router (ADR 0025 Task 4) ──────────────────────────
    // The `WorkerManager`'s `on_event` callback (wired in `lib.rs` with the
    // `OnceLock` late-wire). The reviewer-corrected routing: the UI's contract
    // is the `SinkFrame`s (re-emitted verbatim); the store frames drive
    // persistence only; the raw `RpcEvent` stream drives internal bookkeeping
    // only (settle detection + the `pending_turn` resolution — NEVER the UI, to
    // avoid double-delivering the `session-update` frames); the
    // `PermissionRequest` / `InteractiveRequest` frames are re-emitted with the
    // payload UNCHANGED (the frontend contract preserved by construction).
    pub fn route_event(&self, session_id: &str, is_subagent: bool, evt: WorkerInboundEvent) {
        match evt {
            WorkerInboundEvent::Store(frame) => {
                // Persistence ONLY (not the UI — the `TranscriptPersister` applies
                // the store frame to SQLite, the sole writer).
                if let Some(p) = self.persister.get() {
                    p.apply(is_subagent, &frame);
                }
            }
            WorkerInboundEvent::SinkFrame { event, payload } => {
                // The ENTIRE UI contract — re-emitted VERBATIM (the same event name +
                // payload; the frontend is unchanged).
                if let Some(sink) = self.sink.get() {
                    sink.emit(&event, payload);
                }
            }
            WorkerInboundEvent::Event(e) => {
                // Internal bookkeeping ONLY (NOT the `TauriSink` — forwarding would
                // DOUBLE-DELIVER every `session-update` the loop emits both as an
                // `RpcEvent` and as a `session-update` `SinkFrame`): the
                // `agent_settled` settle detection (the session's "busy" state) + the
                // `pending_turn` resolution.
                if matches!(e, RpcEvent::agent_settled) {
                    self.resolve_turn(session_id);
                }
            }
            WorkerInboundEvent::PermissionRequest { id, payload } => {
                // Re-emit as the `permission-request` Tauri event with the payload
                // UNCHANGED (the frontend contract preserved by construction —
                // `PermissionPrompt.tsx` / `store/permissions.ts` consume the
                // payload's `requestId` / `toolTitle` / `options` verbatim).
                if let Some(sink) = self.sink.get() {
                    sink.emit("permission-request", payload);
                }
                self.track_modal(session_id, &id);
            }
            WorkerInboundEvent::InteractiveRequest { id, payload } => {
                // Re-emit as the `interactive-request` Tauri event with the payload
                // UNCHANGED (same reasoning — `store/interactive.ts` consumes
                // `method` + `params`).
                if let Some(sink) = self.sink.get() {
                    sink.emit("interactive-request", payload);
                }
                self.track_modal(session_id, &id);
            }
            WorkerInboundEvent::Exited(code) => {
                // EXPECTED (a `detach` / `reap_all` was in flight) → the
                // `session-closed` cleanup. UNEXPECTED (a crash — the `WorkerManager`
                // fired `on_crash`) → `is_exit_expected` is `false` → no-op (the
                // `on_crash` path handles the stalled marking + the same cleanup).
                if let Some(wm) = self.worker_manager.get() {
                    if wm.is_exit_expected(session_id) {
                        self.on_session_exited_expected(session_id);
                    }
                }
                let _ = code;
            }
            WorkerInboundEvent::Ready { .. }
            | WorkerInboundEvent::WorkerError { .. }
            | WorkerInboundEvent::SubagentDispatch(_)
            | WorkerInboundEvent::SubagentCancel { .. } => {
                // The `WorkerManager`'s pump handles the `SubagentDispatch` (the
                // Supervisor-side `dispatch_subagent` flow); the `Ready` / `WorkerError`
                // / `SubagentCancel` frames are bookkeeping (the `SubagentCancel` →
                // `cancel_subagent` is Task 5). Ignored here.
            }
        }
    }

    /// The `WorkerManager`'s `on_crash` callback (the UNEXPECTED `Exited` — a Worker
    /// crash): mark the session STALLED (the `stalled` registry + the
    /// `session-stalled` event + the `stalled_info` query), perform the pending-modal
    /// cleanup (the `interactive-request-close` frames — a crashed session's open
    /// permission / interactive modals are dismissed, no stuck modal alongside the
    /// banner), and resolve the in-flight `send_prompt` (the crash outcome — the
    /// composer unlocks; it never stays locked forever).
    pub fn handle_crash(&self, session_id: &str, code: Option<i32>) {
        let at = now_ms();
        let crash_log = find_newest_crash_log();
        // Mark the session STALLED (the `stalled` registry — the frontend reads it via
        // `stalled_info` + the `session-stalled` event).
        self.stalled
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(session_id.to_string(), StalledInfo { at, crash_log });
        // The pending-modal cleanup (the `interactive-request-close` frames — the
        // unanswered-request-id diff: a crashed session's open permission / interactive
        // modals are dismissed, no stuck modal alongside the banner).
        self.cleanup_pending_modals(session_id);
        // Resolve the in-flight `send_prompt` (the crash outcome — the turn is
        // incomplete; `Cancelled` unlocks the composer. The `StopReason` has no error
        // variant — the `session-stalled` event is the frontend's crash signal).
        self.resolve_turn_forced(session_id, StopReason::Cancelled);
        // Emit the `session-stalled` event (the frontend's banner — the `StalledInfo`
        // shape: `at` + `crash_log`).
        if let Some(sink) = self.sink.get() {
            sink.emit(
                "session-stalled",
                json!({
                    "sessionId": session_id,
                    "at": at,
                    "crashLog": find_newest_crash_log(),
                }),
            );
        }
        let _ = code;
    }

    /// The `stalled_info` query (the frontend's banner query — the `SessionManager`
    /// exposes it; the `get_stalled_info` command wraps it).
    pub fn stalled_info(&self, session_id: &str) -> Option<StalledInfo> {
        self.stalled
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(session_id)
            .cloned()
    }

    /// Resolve the session's in-flight turn (the `pending_turn` oneshot) on an
    /// `agent_settled` (the `cancel_requested` flag maps the settle to `Cancelled`,
    /// else `EndTurn` — the existing settle semantics). A no-op when no turn is in
    /// flight (the slot is empty) or the session is gone.
    fn resolve_turn(&self, session_id: &str) {
        let live = self.live.lock().unwrap_or_else(|p| p.into_inner());
        let Some(live) = live.get(session_id) else {
            return;
        };
        let cancelled_guard = live
            .cancel_requested
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let cancelled = *cancelled_guard;
        let reason = if cancelled {
            StopReason::Cancelled
        } else {
            StopReason::EndTurn
        };
        let mut pending_guard = live.pending_turn.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(tx) = pending_guard.take() {
            let _ = tx.send(reason);
        }
    }

    /// Resolve the session's in-flight turn with a FORCED reason (the `Exited`
    /// / crash path — the turn is over regardless of the `cancel_requested` flag;
    /// a mid-turn death / a close must not tell `send_prompt` "turn ended normally").
    fn resolve_turn_forced(&self, session_id: &str, reason: StopReason) {
        let live = self.live.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(live) = live.get(session_id) {
            let mut pending_guard = live.pending_turn.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(tx) = pending_guard.take() {
                let _ = tx.send(reason);
            }
        }
    }

    /// The EXPECTED `Exited` handling (a `detach` / `reap_all` was in flight — the
    /// `close_session` / `reap_all` path): emit `session-closed` (the `ClosedReason`
    /// payload — the `close_kind`: `User` for a `close_session`, `AgentExited` for a
    /// clean self-exit), resolve the in-flight `pending_turn` → `Cancelled`, perform
    /// the pending-modal cleanup (the `interactive-request-close` frames), and remove
    /// the live-session bookkeeping.
    fn on_session_exited_expected(&self, session_id: &str) {
        let (reason, exists) = {
            let live = self.live.lock().unwrap_or_else(|p| p.into_inner());
            let Some(live) = live.get(session_id) else {
                return; // the session is already gone (idempotent).
            };
            let kind_guard = live.close_kind.lock().unwrap_or_else(|p| p.into_inner());
            let kind = *kind_guard;
            // The `ClosedReason` mapping (the current `session.rs:876` shape): a
            // `close_session` (`CloseKind::User`) → `User`; a clean self-exit (`None`
            // — the Worker exited on its own) → `AgentExited`.
            let reason = match kind {
                Some(CloseKind::User) => ClosedReason::User,
                None => ClosedReason::AgentExited,
            };
            (reason, true)
        };
        if !exists {
            return;
        }
        // Resolve the in-flight `pending_turn` → `Cancelled` (a close / a clean self-
        // exit kills the turn — the composer unlocks; it never stays locked forever).
        self.resolve_turn_forced(session_id, StopReason::Cancelled);
        // The pending-modal cleanup (the `interactive-request-close` frames — the
        // unanswered-request-id diff: a session ending with an open prompt must NOT
        // leave a stuck modal).
        self.cleanup_pending_modals(session_id);
        // Remove the live-session bookkeeping (the `live` / `config_state` / `stalled`
        // entries — the session is over).
        {
            let mut live = self.live.lock().unwrap_or_else(|p| p.into_inner());
            live.remove(session_id);
        }
        self.config_state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(session_id);
        // Emit the `session-closed` event (the `ClosedReason` payload — the frontend's
        // close handling).
        if let Some(sink) = self.sink.get() {
            sink.emit(
                "session-closed",
                json!({
                    "sessionId": session_id,
                    "reason": reason.as_str(),
                }),
            );
        }
    }

    /// The pending-modal cleanup (the round-3 fix — the Worker's pending maps die with
    /// it, so the Supervisor tracks the cleanup state itself: per session, the
    /// `PermissionRequest` / `InteractiveRequest` frames it relayed MINUS the
    /// `*Response`s it sent = the UNANSWERED request ids — the Supervisor sees both
    /// halves of every round-trip, so the diff is exact). Emits the same
    /// `interactive-request-close` frames the in-process teardown emits for unanswered
    /// prompts (keyed by the unanswered ids; a session ending with an open
    /// permission / interactive prompt must NOT leave a stuck modal).
    fn cleanup_pending_modals(&self, session_id: &str) {
        let unanswered = {
            let live = self.live.lock().unwrap_or_else(|p| p.into_inner());
            let Some(live) = live.get(session_id) else {
                return;
            };
            let ids_guard = live
                .pending_modal_ids
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            ids_guard.clone()
        };
        if unanswered.is_empty() {
            return;
        }
        let Some(sink) = self.sink.get() else {
            return;
        };
        for request_id in unanswered {
            sink.emit(
                "interactive-request-close",
                json!({
                    "sessionId": session_id,
                    "requestId": request_id,
                }),
            );
        }
    }

    /// Track a relayed `PermissionRequest` / `InteractiveRequest` frame (the
    /// `pending_modal_ids` set — the pending-modal cleanup's source). The `id` is the
    /// request's `requestId` (the payload's key — the `interactive-request-close`
    /// frame's `requestId` field).
    fn track_modal(&self, session_id: &str, id: &str) {
        let mut live = self.live.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(live) = live.get_mut(session_id) {
            let mut ids_guard = live
                .pending_modal_ids
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            ids_guard.insert(id.to_string());
        }
    }
}

/// The newest `crash-<ts>-worker*.log` in the data dir (the `StalledInfo`'s
/// `crash_log` — globbed and FROZEN at crash time; best-effort attribution across
/// concurrent sessions — crash logs are diagnostic, not load-bearing). `None` when
/// the data dir is unresolvable or no crash log is found.
fn find_newest_crash_log() -> Option<PathBuf> {
    let dir = dirs::data_dir()?.join("archimedes");
    let entries = std::fs::read_dir(&dir).ok()?;
    let mut newest: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with("crash-") && name.ends_with("-worker.log") {
            if let Ok(mtime) = entry.metadata().and_then(|m| m.modified()) {
                if newest.as_ref().map(|(t, _)| mtime > *t).unwrap_or(true) {
                    newest = Some((mtime, entry.path()));
                }
            }
        }
    }
    newest.map(|(_, p)| p)
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

/// Resolve a native session's model (the `Settings.default_model` composed
/// key — a fresh `load_settings` read → the catalog; the catalog's
/// `default_model` → the selectable set). Each rung
/// is tried IN ORDER: an UNRESOLVABLE key at any rung falls through to the
/// NEXT rung (only when both rungs are absent/unresolvable does it fall to
/// the set) — a stale configured default degrades rather than a hard error.
fn resolve_native_model(
    catalog: &ModelCatalog,
    settings_default: &Option<String>,
) -> Result<Model, SessionError> {
    for key in [
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
        .selectable()
        .first()
        .copied()
        .cloned()
        .ok_or_else(|| SessionError::Command {
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
/// are the `selectable()` ids (`"<provider>/<id>"`); the thinking level
/// is the current model's `thinking_levels` (absent → no selector).
fn synthesize_catalog_config_options(
    catalog: &ModelCatalog,
    current: &Model,
    thinking_level: Option<&str>,
) -> Option<Vec<Value>> {
    let mut out: Vec<Value> = Vec::new();
    let options: Vec<Value> = catalog
        .selectable()
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

/// Compute the display `messages` rows for one `session-update` (the
/// FROZEN JSON shapes the normalizer emits) — the PURE row-computation
/// half of the ACP-era persistence (the write half is
/// `Store::persist_display` — ADR 0025: the `SqliteStore` writes them via
/// `db.record_message`, the `IpcStore` frames them, the `NoopStore`
/// ignores them).
///
/// The persistence semantics are the ACP-era ones, unchanged: upsert keys
/// (`(session_id, kind, message_key)`), agent-text accumulation per
/// `messageId`, thought segmentation (`{messageId}#{segment}` boundaries),
/// tool-call `merge_json` shallow-merge. (The ACP `content:
/// ToolCallContent[]` diff channel has no RPC input in Phase 1 — pi's tool
/// results carry no diff-structured content — so the `has_diff` branch is
/// gone with the crate types). `pub(crate)` so the native `AgentLoop`
/// (Task 6) computes through the SAME function.
///
/// Returns 0 or 1 rows (the `_` arm — an update kind without a display
/// row — yields none). `created_at` is the current time (the write half's
/// `now_ms` stamp — the `SqliteStore`'s `db.record_message` re-stamps it
/// at write time; the `IpcStore` frames it for the Supervisor's writer).
pub(crate) fn compute_display_rows(
    update: &Value,
    agent_text_acc: &StdMutex<HashMap<String, String>>,
    tool_call_state: &StdMutex<HashMap<String, Value>>,
    thought_state: &StdMutex<ThoughtState>,
) -> Vec<DisplayRow> {
    let kind = update.get("sessionUpdate").and_then(Value::as_str);
    match kind {
        Some("agent_thought_chunk") => {
            let Some(text) = update
                .get("content")
                .and_then(|c| c.get("text"))
                .and_then(Value::as_str)
            else {
                return Vec::new();
            };
            if text.is_empty() {
                return Vec::new();
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
            vec![DisplayRow {
                kind: "agent-thought".to_string(),
                message_key: row_key,
                payload_json: payload,
                created_at: now_ms(),
            }]
        }
        Some("agent_message_chunk") => {
            let Some(text) = update
                .get("content")
                .and_then(|c| c.get("text"))
                .and_then(Value::as_str)
            else {
                return Vec::new();
            };
            if text.is_empty() {
                return Vec::new();
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
            vec![DisplayRow {
                kind: "agent-text".to_string(),
                message_key: key,
                payload_json: payload,
                created_at: now_ms(),
            }]
        }
        Some("tool_call") => {
            let Some(key) = update
                .get("toolCallId")
                .and_then(Value::as_str)
                .map(str::to_string)
            else {
                return Vec::new();
            };
            thought_state
                .lock()
                .expect("thought state poisoned")
                .open_key = None;
            let mut state = tool_call_state.lock().expect("tool-call state poisoned");
            state.insert(key.clone(), update.clone());
            vec![DisplayRow {
                kind: "tool-call".to_string(),
                payload_json: state[&key].clone(),
                message_key: key,
                created_at: now_ms(),
            }]
        }
        Some("tool_call_update") => {
            let Some(key) = update
                .get("toolCallId")
                .and_then(Value::as_str)
                .map(str::to_string)
            else {
                return Vec::new();
            };
            let mut state = tool_call_state.lock().expect("tool-call state poisoned");
            let is_new = !state.contains_key(&key);
            let entry = state
                .entry(key.clone())
                .or_insert_with(|| Value::Object(Default::default()));
            merge_json(entry, update);
            let payload = entry.clone();
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
            vec![DisplayRow {
                kind: "tool-call".to_string(),
                message_key: key,
                payload_json: payload,
                created_at: now_ms(),
            }]
        }
        _ => Vec::new(),
    }
}

/// The current unix time in milliseconds (the `DisplayRow` `created_at`
/// stamp — the `Db::now_ms` mirror; the `SqliteStore`'s `db.record_message`
/// re-stamps it at write time, so the two are the same value in practice).
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod normalize_tests {
    use serde_json::json;

    use super::{normalize, TurnState};
    use crate::agent::events::RpcEvent;

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
}

#[cfg(test)]
mod session_tests {
    use super::*;
    use crate::agent::worker::client::{WorkerError, WorkerHandle};
    use crate::agent::worker::manager::WorkerFactory;
    use crate::storage::Db;
    use std::path::Path;
    use tokio::sync::mpsc;

    #[test]
    fn mint_session_id_format() {
        let sid = mint_session_id();
        assert!(
            sid.starts_with("arch_"),
            "session id must start with arch_, got {sid}"
        );
        let suffix = &sid["arch_".len()..];
        let parsed = uuid::Uuid::parse_str(suffix).expect("suffix must be valid UUID");
        assert_eq!(parsed.get_version_num(), 4, "suffix must be UUID v4");
    }

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

    fn open_db(dir: &Path) -> std::sync::Arc<Db> {
        std::sync::Arc::new(Db::open(&dir.join("archimedes.db")).expect("db should open"))
    }

    /// (ADR 0016) The NATIVE resume carries the stored `archived` flag:
    /// a fresh native start is `archived: false`; after
    /// `set_session_archived`, the resumed `SessionInfo` reports the stored
    /// flag and the resume's `record_session` re-record did NOT clear it.
    #[tokio::test]
    async fn native_resume_carries_the_archived_flag() {
        let dir = temp_config_dir();
        let (tx, _rx) = mpsc::unbounded_channel();
        let sink: Arc<dyn EventSink> = Arc::new(TestSink { tx });
        let (manager, _wm, db) = build_worker_manager(&dir, native_test_catalog(), &sink);

        let info = manager
            .start_session(dir.clone(), &sink)
            .await
            .expect("the native session started");
        assert!(!info.archived, "a fresh native start is never archived");
        db.set_session_archived(&info.session_id, true)
            .expect("set_session_archived should succeed");

        let resumed = manager
            .resume_session(&info.session_id, dir.clone(), &sink)
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
            cwd: PathBuf::from("/tmp/proj"),
            capabilities: serde_json::json!({ "loadSession": true }),
            config_options: None,
            archived: true,
            context_usage: None,
            is_subagent: false,
        };
        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("\"archived\":true"), "got: {json}");
        let back: SessionInfo = serde_json::from_str(&json).unwrap();
        assert!(back.archived);
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

    // ── Native-backend tests (the in-process `AgentLoop` — a NATIVE
    // registry entry + a mock `provider_factory` seam; the production
    // default dispatches on `Model.api` (`build_provider`, ADR 0024)) ──

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

    /// The `fake_worker` fixture factory (the `WorkerFactory` seam —
    /// `cargo test` builds the bin targets; the fixture answers the
    /// `ready` handshake + the `start` / `prompt` frames; the
    /// `AgentLoop` runs in the fixture process, NOT in-process —
    /// ADR 0025).
    struct FixtureFactory;

    impl WorkerFactory for FixtureFactory {
        fn spawn(&self) -> Result<WorkerHandle, WorkerError> {
            WorkerHandle::spawn(&fixture_path())
        }
    }

    /// The `fake_worker` fixture path (the `CARGO_MANIFEST_DIR`/
    /// `target/debug` convention — `cargo test` builds the bin targets).
    fn fixture_path() -> std::path::PathBuf {
        std::path::PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/target/debug/fake_worker"
        ))
    }

    /// Build the Worker-mediated manager (the ADR 0025 late-wire — the
    /// `WorkerManager`'s `on_event` / `on_crash` callbacks point at the
    /// `SessionManager`'s router / crash handler; `attach_worker_manager`
    /// hands the `WorkerManager` to the `SessionManager` after both are
    /// built). Returns the manager + the `WorkerManager` + the `Db`.
    fn build_worker_manager(
        dir: &Path,
        catalog: ModelCatalog,
        sink: &Arc<dyn EventSink>,
    ) -> (Arc<SessionManager>, Arc<WorkerManager>, Arc<Db>) {
        let db = open_db(dir);
        let mut manager = SessionManager::new(dir.to_path_buf());
        manager.attach_db(db.clone());
        manager.set_catalog(catalog);
        manager.set_sink(sink.clone());
        let manager = Arc::new(manager);
        let wm = {
            let m = manager.clone();
            let m2 = manager.clone();
            Arc::new(WorkerManager::new(
                Arc::new(FixtureFactory),
                Arc::new(move |s, c| m.handle_crash(&s, c)),
                Arc::new(move |s, b, e| m2.route_event(&s, b, e)),
                Arc::new(|_s| None),
            ))
        };
        manager.attach_worker_manager(wm.clone());
        (manager, wm, db)
    }

    /// Start a native session with the given catalog (the Worker-mediated
    /// path — the `fake_worker` fixture answers the `ready` handshake;
    /// the Supervisor-side assertions run against the coordinator state).
    async fn start_native_session_with_catalog(
        dir: &Path,
        sink: &Arc<dyn EventSink>,
        catalog: ModelCatalog,
    ) -> (Arc<SessionManager>, SessionInfo) {
        let (manager, _wm, _db) = build_worker_manager(dir, catalog, sink);
        let info = manager
            .start_session(dir.to_path_buf(), sink)
            .await
            .expect("the native session started");
        (manager, info)
    }

    /// Start a native session (the `native_test_catalog` — a single
    /// `fake/m1` model).
    async fn start_native_session(
        dir: &Path,
        sink: &Arc<dyn EventSink>,
    ) -> (Arc<SessionManager>, SessionInfo) {
        start_native_session_with_catalog(dir, sink, native_test_catalog()).await
    }

    /// A full `Model` literal for the `resolve_native_model` chain test
    /// (`base_url` EMPTY so `refresh_model_metadata` is a no-op — no
    /// network; `openai-completions` so the model is selectable).
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

    /// (ADR 0015) Native start: the remembered level (VALIDATED against the
    /// model's `thinking_levels`) wins over the harness's
    /// `default_thinking_level` seed.
    #[tokio::test]
    async fn a_native_session_starts_with_the_remembered_level_over_the_settings_default() {
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
        write_settings_json(
            &dir,
            serde_json::json!({
                "defaultModel": null,
                "defaultThinkingLevel": "high",
                "defaultThinkingLevels": { "tama/m1": "xhigh" },
            }),
        );
        let (manager, info) = start_native_session_with_catalog(&dir, &sink, catalog).await;
        assert_eq!(
            info.capabilities["thinkingLevel"], "xhigh",
            "the remembered (validated) level beats the settings default"
        );
        let _ = manager.close_session(&info.session_id).await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A STALE remembered entry (not a member of the model's
    /// `thinking_levels` — the provider changed its levels) is IGNORED, AND
    /// the `Settings.default_thinking_level` rung is VALIDATED the same way
    /// (the Settings UI offers the UNION of all models' levels — a level one
    /// model advertises may not be a member of THIS model's set): a settings
    /// default that is NOT a member is DROPPED (the session starts with no
    /// thinking level — the model's own default).
    #[tokio::test]
    async fn a_stale_remembered_level_and_an_invalid_settings_default_start_with_no_level() {
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
        write_settings_json(
            &dir,
            serde_json::json!({
                "defaultModel": null,
                "defaultThinkingLevel": "high",
                "defaultThinkingLevels": { "tama/m1": "ultra" },
            }),
        );
        let (manager, info) = start_native_session_with_catalog(&dir, &sink, catalog).await;
        assert!(
            info.capabilities.get("thinkingLevel").is_none(),
            "the stale entry is ignored AND the settings default (not a member of the model's levels) is dropped — no thinking level, got {:?}",
            info.capabilities
        );
        let _ = manager.close_session(&info.session_id).await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The `Settings.default_thinking_level` rung APPLIES when it is a
    /// member of the model's `thinking_levels` (no remembered entry):
    /// the settings default is a validated rung, not a bypass.
    #[tokio::test]
    async fn a_settings_default_level_applies_when_it_is_a_member_of_the_model_levels() {
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
        write_settings_json(
            &dir,
            serde_json::json!({
                "defaultModel": null,
                "defaultThinkingLevel": "xhigh",
            }),
        );
        let (manager, info) = start_native_session_with_catalog(&dir, &sink, catalog).await;
        assert_eq!(
            info.capabilities["thinkingLevel"], "xhigh",
            "a settings default that IS a member of the model's levels applies"
        );
        let _ = manager.close_session(&info.session_id).await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// (the stored rung's lenient rule, mirrored on the settings rung) A
    /// model that advertises NO `thinking_levels` applies the
    /// `Settings.default_thinking_level` AS-IS (no membership check — the
    /// pre-change behavior).
    #[tokio::test]
    async fn a_settings_default_level_applies_as_is_for_a_model_without_levels() {
        let catalog = ModelCatalog {
            models: vec![level_test_model("tama", "m1", &[])],
            default_model: Some("tama/m1".to_string()),
            compaction: crate::agent::harness::catalog::CompactionConfig::default(),
        };
        let dir = temp_config_dir();
        let (tx, _rx) = mpsc::unbounded_channel();
        let sink: Arc<dyn EventSink> = Arc::new(TestSink { tx });
        write_settings_json(
            &dir,
            serde_json::json!({
                "defaultModel": null,
                "defaultThinkingLevel": "high",
            }),
        );
        let (manager, info) = start_native_session_with_catalog(&dir, &sink, catalog).await;
        assert_eq!(
            info.capabilities["thinkingLevel"], "high",
            "a model with no advertised levels applies the settings default as-is"
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

    /// (review finding) An EMPTY `thought_level` value is rejected BEFORE
    /// the control channel is touched (an empty level would flow
    /// `reasoning_effort: Some("")` into the provider request body — some
    /// endpoints reject it): a `Command` error, the mirror is UNCHANGED,
    /// NO `config_option_update` is emitted (mirroring the model arm's
    /// rejection of an unresolvable key).
    #[tokio::test]
    async fn a_native_empty_thought_level_change_is_rejected() {
        let dir = temp_config_dir();
        write_settings_json(&dir, serde_json::json!({}));
        let (tx, mut rx) = mpsc::unbounded_channel();
        let sink: Arc<dyn EventSink> = Arc::new(TestSink { tx });
        let (manager, info) = start_native_session(&dir, &sink).await;
        // Drain any startup events (the assertion below is about the
        // rejected change only).
        while rx.try_recv().is_ok() {}
        let err = manager
            .set_config_option(&info.session_id, "thought_level", "", &sink)
            .await
            .expect_err("an empty level must be rejected");
        assert!(
            matches!(
                err,
                SessionError::Command {
                    ref error
                } if error == "a thinking level must be non-empty"
            ),
            "the empty level is rejected with a Command error, got {err:?}"
        );
        // The mirror is UNCHANGED (a fresh session has no level — an
        // applied change would have mirrored `Some("")`). The mirror's home
        // moved from the `NativeHandle` to the `SessionManager`'s
        // `config_state` map (the `NativeHandle` is gone for main sessions).
        let state = {
            let config_state = manager
                .config_state
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            config_state
                .get(&info.session_id)
                .expect("the session is live")
                .clone()
        };
        assert_eq!(
            state.thinking_level.as_deref(),
            None,
            "the mirror must be untouched by a rejected change"
        );
        // NO `config_option_update` was emitted (a rejected change
        // re-synthesizes nothing).
        assert!(
            rx.try_recv().is_err(),
            "no event may be emitted for a rejected change"
        );
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
        write_settings_json(
            &dir,
            serde_json::json!({ "defaultThinkingLevels": { "tama/m1": "xhigh" } }),
        );
        let (manager, _wm, db) = build_worker_manager(&dir, catalog, &sink);
        // The stored row: the start-of-session level (`"high"` — valid for
        // the model; a mid-session change to `xhigh` is only in the memory).
        db.record_session(&SessionInfo {
            session_id: "nat-resume-1".to_string(),
            cwd: dir.clone(),
            capabilities: json!({
                "native": true,
                "model": "tama/m1",
                "thinkingLevel": "high",
                "loadSession": true,
            }),
            config_options: None,
            archived: false,
            context_usage: None,
            is_subagent: false,
        })
        .expect("record_session should succeed");
        let info = manager
            .resume_session("nat-resume-1", dir.clone(), &sink)
            .await
            .expect("the native resume works");
        assert_eq!(
            info.capabilities["thinkingLevel"], "xhigh",
            "memory wins over the stale stored value"
        );
        let _ = manager.close_session(&info.session_id).await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `list_sessions` carries the stored session's `config_options`
    /// (synthesized from the stored `model` / `thinkingLevel` — the
    /// frontend's `SessionConfigSelect` renders them in the stored
    /// session's composer, disabled) and `context_usage` (persisted via
    /// `record_session_context_usage` — the frontend's store drops it on
    /// close, so the row is the source of truth for the stored session's
    /// context bar).
    #[tokio::test]
    async fn list_sessions_carries_the_stored_config_options_and_context_usage() {
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
        let db = open_db(&dir);
        let mut manager = SessionManager::new(dir.clone());
        manager.attach_db(db.clone());
        manager.set_catalog(catalog);
        // The stored row: the start-of-session `model` / `thinkingLevel`
        // (the `capabilities` envelope) + a persisted context usage.
        db.record_session(&SessionInfo {
            session_id: "nat-list-1".to_string(),
            cwd: dir.clone(),
            capabilities: json!({
                "native": true,
                "model": "tama/m1",
                "thinkingLevel": "high",
                "loadSession": true,
            }),
            config_options: None,
            archived: false,
            context_usage: None,
            is_subagent: false,
        })
        .expect("record_session should succeed");
        db.record_session_context_usage("nat-list-1", 53_760, 128_000)
            .expect("record_session_context_usage should succeed");
        let rows = manager
            .list_sessions(true)
            .await
            .expect("list_sessions should work");
        assert_eq!(rows.len(), 1, "the stored row is listed");
        let row = &rows[0];
        // `config_options`: the model selector (current value = the stored
        // composed key) + the thinking-level selector (current value = the
        // stored level).
        let options = row
            .config_options
            .as_ref()
            .expect("config_options are synthesized for the stored session");
        let model = options
            .iter()
            .find(|o| o["id"] == "model")
            .expect("the model option");
        assert_eq!(model["currentValue"], "tama/m1");
        assert!(
            model["options"]
                .as_array()
                .unwrap()
                .iter()
                .any(|o| o["value"] == "tama/m1"),
            "the stored model is in the options list"
        );
        let thinking = options
            .iter()
            .find(|o| o["id"] == "thought_level")
            .expect("the thinking-level option");
        assert_eq!(thinking["currentValue"], "high");
        // `context_usage`: the persisted last-known usage.
        let usage = row
            .context_usage
            .as_ref()
            .expect("context_usage is carried from the row");
        assert_eq!(usage.used, 53_760);
        assert_eq!(usage.window, 128_000);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A resume carries the stored `context_usage` (the last known before
    /// the close — the `load_transcript` re-estimate's first frame
    /// refreshes it after the resume).
    #[tokio::test]
    async fn a_resume_carries_the_stored_context_usage() {
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
        let (manager, _wm, db) = build_worker_manager(&dir, catalog, &sink);
        db.record_session(&SessionInfo {
            session_id: "nat-resume-cu-1".to_string(),
            cwd: dir.clone(),
            capabilities: json!({
                "native": true,
                "model": "tama/m1",
                "loadSession": true,
            }),
            config_options: None,
            archived: false,
            context_usage: None,
            is_subagent: false,
        })
        .expect("record_session should succeed");
        db.record_session_context_usage("nat-resume-cu-1", 96_000, 128_000)
            .expect("record_session_context_usage should succeed");
        let info = manager
            .resume_session("nat-resume-cu-1", dir.clone(), &sink)
            .await
            .expect("the native resume works");
        let usage = info
            .context_usage
            .expect("the resume carries the stored context usage");
        assert_eq!(usage.used, 96_000);
        assert_eq!(usage.window, 128_000);
        let _ = manager.close_session(&info.session_id).await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// (settings chain) the native model resolution chain: the
    /// `Settings.default_model` (a fresh `load_settings` read) > the
    /// catalog's `default_model` > the selectable set. An UNRESOLVABLE key
    /// at any rung falls through to the NEXT rung.
    #[tokio::test]
    async fn the_native_model_chain_settings_default_wins_over_the_catalog_default() {
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

        // Phase 1: settings `s/m1` (in the catalog) → the settings rung
        // wins over the catalog default.
        write_settings_default_model(&dir, Some("s/m1"));
        let (manager, _wm, _db) = build_worker_manager(&dir, catalog.clone(), &sink);
        let info = manager
            .start_session(dir.clone(), &sink)
            .await
            .expect("the native session started");
        assert_eq!(
            info.capabilities["model"], "s/m1",
            "the settings default wins over the catalog default"
        );
        let _ = manager.close_session(&info.session_id).await;

        // Phase 2: settings `gone/m1` (NOT in the catalog) → the
        // unresolvable settings key falls through to the CATALOG DEFAULT
        // rung (not straight to the selectable set).
        write_settings_default_model(&dir, Some("gone/m1"));
        let (manager, _wm, _db) = build_worker_manager(&dir, catalog.clone(), &sink);
        let info = manager
            .start_session(dir.clone(), &sink)
            .await
            .expect("the native session started");
        assert_eq!(
            info.capabilities["model"], "c/m2",
            "an unresolvable settings key degrades to the catalog default"
        );
        let _ = manager.close_session(&info.session_id).await;

        // Phase 3: NO settings default → the catalog default applies.
        write_settings_default_model(&dir, None);
        let (manager, _wm, _db) = build_worker_manager(&dir, catalog.clone(), &sink);
        let info = manager
            .start_session(dir.clone(), &sink)
            .await
            .expect("the native session started");
        assert_eq!(
            info.capabilities["model"], "c/m2",
            "an absent settings default degrades to the catalog default"
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
        let mut manager = SessionManager::new(dir.clone());
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
    /// 0014; the camelCase wire shape). `api` is the provider's wire API
    /// (ADR 0024 — a pre-feature fixture omits the field, which parses to
    /// `"openai-completions"` via the serde default).
    fn write_settings_provider_api(dir: &Path, id: &str, base_url: &str, api: &str) {
        let settings = serde_json::json!({
            "providers": [
                { "id": id, "name": id, "baseUrl": base_url, "apiKey": "k", "api": api }
            ]
        });
        std::fs::write(
            dir.join("settings.json"),
            serde_json::to_string_pretty(&settings).unwrap(),
        )
        .unwrap();
    }

    /// Write a `settings.json` with a single user provider (the pre-feature
    /// wire shape — NO `api` field; the serde default gives
    /// `"openai-completions"`).
    fn write_settings_provider(dir: &Path, id: &str, base_url: &str) {
        write_settings_provider_api(dir, id, base_url, "openai-completions")
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
        let mut manager = SessionManager::new(dir);
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

    /// (ADR 0024) A provider with `api: "anthropic-messages"` yields an
    /// effective-catalog model with `api: Some("anthropic-messages")` (the
    /// provider's `api` decides the wire — the `SessionManager`'s default
    /// factory dispatches on it; the pre-feature fixtures default to
    /// `openai-completions` via the serde default, asserted in
    /// `effective_catalog_discovers_a_user_provider`).
    #[tokio::test]
    async fn resolve_uses_the_provider_api() {
        let dir = temp_config_dir();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server =
            crate::test_support::raw_json_server(listener, 200, r#"{"data":[{"id":"m/1"}]}"#, None)
                .await;
        write_settings_provider_api(
            &dir,
            "anthropic",
            &format!("http://{addr}/v1"),
            "anthropic-messages",
        );
        let manager = SessionManager::new(dir);
        let effective = manager.effective_catalog(None).await;
        let user: Vec<&Model> = effective
            .models
            .iter()
            .filter(|m| m.provider == "anthropic")
            .collect();
        assert_eq!(user.len(), 1, "the discovered model is in the catalog");
        // The provider's `api` decides the wire (NOT a hard-coded
        // `openai-completions`).
        assert_eq!(user[0].api.as_deref(), Some("anthropic-messages"));
        server.abort();
    }

    /// (ADR 0014) A provider whose discovery FAILS (unreachable endpoint)
    /// still SHADOWS the seeded models for its id (a transient failure
    /// must not resurrect stale seeded models under the same id).
    #[tokio::test]
    async fn effective_catalog_a_failed_discovery_still_shadows_the_seeded_provider() {
        let dir = temp_config_dir();
        write_settings_provider(&dir, "tama", "http://127.0.0.1:1/v1"); // unreachable
        let mut manager = SessionManager::new(dir);
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
        let mut manager = SessionManager::new(dir.clone());
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
}

#[cfg(test)]
mod compute_display_rows_tests {
    use serde_json::json;

    use super::{compute_display_rows, SessionInfo, ThoughtState};
    use crate::agent::harness::store::{SessionStore, Store};
    use crate::storage::Db;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex as StdMutex};

    /// A temp-dir `Db` with a recorded `sessions` row (the `messages`
    /// rows FK to `sessions`).
    fn temp_db() -> Arc<Db> {
        let dir =
            std::env::temp_dir().join(format!("harness-display-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = Arc::new(Db::open(&dir.join("t.db")).expect("db should open"));
        db.record_session(&SessionInfo {
            session_id: "s1".to_string(),
            cwd: std::path::PathBuf::from("/tmp"),
            capabilities: serde_json::json!({}),
            config_options: None,
            archived: false,
            context_usage: None,
            is_subagent: false,
        })
        .expect("record_session");
        db
    }

    /// The `persist_update` split (ADR 0025) GOLDEN test:
    /// `compute_display_rows` on a canned normalized update + canned
    /// accumulators yields the same `Vec<DisplayRow>` the old
    /// `persist_update` wrote (the normalize + accumulator logic moved
    /// verbatim — `SqliteStore::persist_display` applies the rows via
    /// `db.record_message`, so the `messages` rows are identical).
    #[test]
    fn compute_display_rows_matches_the_legacy_persist_update_writes() {
        let db = temp_db();
        let store = SessionStore::new(db.clone());
        let text_acc = StdMutex::new(HashMap::new());
        let tool_state = StdMutex::new(HashMap::new());
        let thought_state = StdMutex::new(ThoughtState::default());

        // The canned normalized update sequence (the FULL
        // `session-update` frames the `normalize` emits):
        let updates = [
            // The assistant message m1: two chunks (the accumulator grows).
            json!({ "sessionUpdate": "agent_message_chunk", "messageId": "m1", "content": { "text": "Hel" } }),
            json!({ "sessionUpdate": "agent_message_chunk", "messageId": "m1", "content": { "text": "lo" } }),
            // A thought segment for m1 (one chunk).
            json!({ "sessionUpdate": "agent_thought_chunk", "messageId": "m1", "content": { "text": "think" } }),
            // A tool-call announcement + update (the `merge_json` shallow-merge).
            json!({ "sessionUpdate": "tool_call", "toolCallId": "t1", "title": "run" }),
            json!({ "sessionUpdate": "tool_call_update", "toolCallId": "t1", "status": { "type": "completed" } }),
            // A second assistant message m2 (segments the thought stream).
            json!({ "sessionUpdate": "agent_message_chunk", "messageId": "m2", "content": { "text": "x" } }),
            // An unknown kind (NO row — the `_` arm).
            json!({ "sessionUpdate": "usage_update" }),
        ];
        let mut all_rows = Vec::new();
        for u in &updates {
            let rows = compute_display_rows(u, &text_acc, &tool_state, &thought_state);
            if !rows.is_empty() {
                assert_eq!(rows.len(), 1, "at most one row per update");
            }
            all_rows.extend(rows);
        }
        // 6 rows (the 7th update — the unknown kind — yields none).
        assert_eq!(all_rows.len(), 6);
        // The per-row golden (what the old `persist_update` wrote):
        let (r0, r1, r2, r3, r4, r5) = (
            &all_rows[0],
            &all_rows[1],
            &all_rows[2],
            &all_rows[3],
            &all_rows[4],
            &all_rows[5],
        );
        assert_eq!(
            (r0.kind.as_str(), r0.message_key.as_str()),
            ("agent-text", "m1")
        );
        assert_eq!(r0.payload_json, json!({ "text": "Hel" }), "the first chunk");
        assert_eq!(
            r1.payload_json,
            json!({ "text": "Hello" }),
            "the accumulator grew"
        );
        assert_eq!(
            (r2.kind.as_str(), r2.message_key.as_str()),
            ("agent-thought", "m1#1"),
            "the thought segment key"
        );
        assert_eq!(r2.payload_json, json!({ "text": "think" }));
        assert_eq!(
            (r3.kind.as_str(), r3.message_key.as_str()),
            ("tool-call", "t1")
        );
        assert_eq!(r3.payload_json["title"], "run", "the announcement row");
        assert_eq!(
            r4.payload_json["title"], "run",
            "the merged row keeps the title"
        );
        assert_eq!(
            r4.payload_json["status"]["type"], "completed",
            "the merge added the status"
        );
        assert_eq!(
            (r5.kind.as_str(), r5.message_key.as_str()),
            ("agent-text", "m2")
        );
        assert_eq!(r5.payload_json, json!({ "text": "x" }));
        // `created_at` is the current time (the write half's `now_ms`).
        assert!(r0.created_at > 0);

        // The WRITE half (`SqliteStore::persist_display` via
        // `db.record_message`): the rows land as `messages` rows — the
        // FINAL golden values (the last row per key wins, the upsert
        // idempotency — the old `persist_update`'s semantics,
        // unchanged).
        store.persist_display("s1", &all_rows).unwrap();
        let db_rows = db.messages_for("s1").unwrap();
        assert_eq!(db_rows.len(), 4, "four distinct rows landed");
        let text = db_rows
            .iter()
            .find(|r| r.kind == "agent-text" && r.message_key.as_deref() == Some("m1"))
            .unwrap();
        assert_eq!(
            text.payload_json, r#"{"text":"Hello"}"#,
            "the m1 row holds the GROWN accumulator (the last upsert wins)"
        );
        let thought = db_rows.iter().find(|r| r.kind == "agent-thought").unwrap();
        assert_eq!(thought.message_key.as_deref(), Some("m1#1"));
        assert_eq!(thought.payload_json, r#"{"text":"think"}"#);
        let tool = db_rows
            .iter()
            .find(|r| r.kind == "tool-call" && r.message_key.as_deref() == Some("t1"))
            .unwrap();
        let tool: serde_json::Value = serde_json::from_str(&tool.payload_json).unwrap();
        assert_eq!(tool["title"], "run");
        assert_eq!(tool["status"]["type"], "completed");
        // A re-send of the SAME rows is an idempotent upsert (no
        // duplicates — the old `persist_update`'s semantics).
        store.persist_display("s1", &all_rows).unwrap();
        let db_rows = db.messages_for("s1").unwrap();
        assert_eq!(db_rows.len(), 4, "the upsert never duplicates");
    }
}
