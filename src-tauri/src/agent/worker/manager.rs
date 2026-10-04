//! The Supervisor-side Worker registry (ADR 0025 Task 3): the
//! `WorkerManager` (the registry of live Workers — session-keyed, the
//! `on_crash` / `on_event` callbacks threaded with the `is_subagent`
//! flag) + the Supervisor-side subagent-dispatch flow (a
//! main-session Worker's `subagent` tool → its `IpcDispatcher` →
//! `Outbound::SubagentDispatch` → here: the model-key resolution
//! (the ADR 0020/0023 resolution moved Supervisor-side), the
//! three-way `enabled_tools` rule, the child `StartEnv`, the
//! `SubagentCapture`, the `settle_timeout`, and the
//! `subagent-session-started` / `subagent-closed` UI lifecycle
//! events).
//!
//! The read-loop plumbing: each attached Worker's `events()` is
//! consumed by a pump task that (a) calls `on_event(session_id,
//! is_subagent, evt)` for EVERY variant (the Task-4 router splits UI
//! vs persistence — the `is_subagent` flag is known per attached
//! Worker: main `false`, subagent `true`), (b) on `SubagentDispatch`
//! runs `dispatch_subagent`, and (c) on `Exited` (unexpected — no
//! `detach` / `reap_all` in flight) calls `on_crash(session_id,
//! code); an EXPECTED `Exited` (a `detach` / `reap_all` was in
//! flight) fires NO `on_crash` (the Task-4 router's
//! `session-closed` handling, Task 4 step 1).
//!
//! The state is `Arc`-backed (`ManagerState`): the pump / drive tasks
//! are `tokio::spawn`ed (`'static`), so they own clones of the `Arc`
//! — the `&self` methods delegate to the `Arc`-backed free functions.

/// The default subagent settle bound (30 min: a hung / slow subagent
/// turn is torn down at the bound).
pub const DEFAULT_SUBAGENT_SETTLE_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(30 * 60);

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::watch;

use crate::agent::debuglog;
use crate::agent::events::RpcEvent;
use crate::agent::harness::prompt::build_child_system_message;
use crate::agent::harness::r#loop::tool_specs;
use crate::agent::session::{mint_session_id, resolve_composed_model, EventSink};
use crate::agent::subagent::{SubagentMetrics, SubagentOutcome};
use crate::agent::worker::client::{WorkerError, WorkerHandle, WorkerInboundEvent};
use crate::agent::worker::protocol::{Inbound, StartEnv, StartMode, SubagentDispatchWire};

/// The Worker spawn seam (the test injection point): production is
/// `WorkerHandle::spawn(std::env::current_exe().as_path())`; tests are
/// the `fake_worker` binary (the manifest-dir path convention).
pub trait WorkerFactory: Send + Sync {
    fn spawn(&self) -> Result<WorkerHandle, WorkerError>;
}

/// The `SubagentOutcome` delivery target (the `dispatch_subagent` flow's
/// two callers): the REQUESTING (main-session) Worker (the
/// `IpcDispatcher` waiter resolves — the `SubagentResult` frame) or the
/// in-process `dispatch_native`'s oneshot (the `SubagentSessionManager`
/// re-plumb — ADR 0025 Task 5; there is no requesting Worker — the
/// outcome resolves the oneshot directly).
enum SubagentOutcomeSink {
    /// The requesting (main-session) Worker (the `SubagentResult` frame
    /// — the `IpcDispatcher` waiter resolves; a dropped waiter is a
    /// no-op).
    Requester,
    /// The in-process `dispatch_native`'s oneshot (an `Arc`'d sender —
    /// `deliver` is a shared-reference method, so the `send`'s move is
    /// of the `Arc`, not the sender; a dropped receiver is a no-op).
    Oneshot(
        std::sync::Arc<std::sync::Mutex<Option<tokio::sync::oneshot::Sender<SubagentOutcome>>>>,
    ),
}

impl SubagentOutcomeSink {
    /// Deliver the outcome (the `Requester` sends the `SubagentResult`
    /// frame to the requesting Worker; the `Oneshot` resolves the
    /// sender). A miss (a dead Worker / a dropped receiver) is a no-op.
    fn deliver(
        &self,
        state: &ManagerState,
        parent_session_id: &str,
        wire: &SubagentDispatchWire,
        outcome: SubagentOutcome,
    ) {
        match self {
            SubagentOutcomeSink::Requester => {
                let _ = send_subagent_result(state, parent_session_id, wire, outcome);
            }
            SubagentOutcomeSink::Oneshot(slot) => {
                if let Some(tx) = slot.lock().unwrap_or_else(|p| p.into_inner()).take() {
                    let _ = tx.send(outcome);
                }
            }
        }
    }
}

/// The `SubagentCapture` (reviewer-corrected: capture from the subagent's
/// `SinkFrame` `session-update` stream — the enveloped `agent_message_chunk`
/// frames — the last-`messageId`-with-non-empty-text rule; NOT from the
/// `message_end` `RpcEvent`s, which are a different shape and carry no text
/// for tool-call messages). The `message_update` `usage` fields feed the
/// metrics' token fields (the `SubagentMetrics` accumulation).
pub struct SubagentCapture {
    /// `messageId` → accumulated text (a `HashMap` has no insertion
    /// order — the LAST id is tracked separately).
    text: StdMutex<HashMap<String, String>>,
    /// The LAST `messageId` with a non-empty capture (updated on every
    /// non-empty capture of a NON-`system` chunk — the final-output
    /// source; the `system` bookkeeping chunks — "Retry failed",
    /// "Compacting context…" — are NEVER captured: a failed child
    /// turn must not settle with a one-liner as its output).
    last_message_id: StdMutex<Option<String>>,
    /// The `message_update` `usage` accumulation (the `inputTokens` /
    /// `outputTokens` fields — the MAX across the frames: a turn's
    /// usage is cumulative per message, the last is the largest).
    usage: StdMutex<(u64, u64)>,
}

impl Default for SubagentCapture {
    fn default() -> Self {
        Self::new()
    }
}

impl SubagentCapture {
    pub fn new() -> Self {
        Self {
            text: StdMutex::new(HashMap::new()),
            last_message_id: StdMutex::new(None),
            usage: StdMutex::new((0, 0)),
        }
    }

    /// Observe one `SinkFrame` (the `session-update`
    /// `agent_message_chunk` frames — the ENVELOPED shape:
    /// `{ sessionId, update }`; the `update`'s `sessionUpdate`
    /// discriminator is `agent_message_chunk`).
    pub fn observe_sink_frame(&self, event: &str, payload: &Value) {
        if event != "session-update" {
            return;
        }
        let update = payload.get("update");
        if update
            .and_then(|u| u.get("sessionUpdate"))
            .and_then(Value::as_str)
            != Some("agent_message_chunk")
        {
            return;
        }
        let mid = update
            .and_then(|u| u.get("messageId"))
            .and_then(Value::as_str);
        let delta = update
            .and_then(|u| u.get("content"))
            .and_then(|c| c.get("text"))
            .and_then(Value::as_str);
        if let (Some(mid), Some(delta)) = (mid, delta) {
            if !delta.is_empty() && mid != "system" {
                let mut text = self.text.lock().unwrap_or_else(|p| p.into_inner());
                text.entry(mid.to_string()).or_default().push_str(delta);
                *self
                    .last_message_id
                    .lock()
                    .unwrap_or_else(|p| p.into_inner()) = Some(mid.to_string());
            }
        }
    }

    /// Observe one `RpcEvent` (the `message_update` `usage` field —
    /// the metrics' token fields; the other events are ignored).
    pub fn observe_event(&self, event: &RpcEvent) {
        if let RpcEvent::message_update { usage, .. } = event {
            let input = usage
                .get("inputTokens")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let output = usage
                .get("outputTokens")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let mut u = self.usage.lock().unwrap_or_else(|p| p.into_inner());
            u.0 = u.0.max(input);
            u.1 = u.1.max(output);
        }
    }

    /// The last-`messageId`-with-non-empty-text ("" when no chunk
    /// arrived — a tool-using turn does NOT concatenate every
    /// intermediate message).
    pub fn captured_text(&self) -> String {
        let last = self
            .last_message_id
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        last.and_then(|id| {
            self.text
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .get(&id)
                .cloned()
        })
        .unwrap_or_default()
    }

    /// The accumulated usage (the metrics' token fields — the MAX
    /// across the `message_update` frames).
    pub fn usage(&self) -> (u64, u64) {
        *self.usage.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn input_tokens(&self) -> u64 {
        self.usage().0
    }

    pub fn output_tokens(&self) -> u64 {
        self.usage().1
    }
}

/// The in-flight subagent dispatch's bookkeeping (the `drives` map
/// value — the pump reads it on the subagent's `agent_settled` /
/// `Exited`; the drive task removes it on the UNCONDITIONAL teardown):
/// the settle watch (the pump flips it on `agent_settled`), the cancel
/// watch (the `SubagentCancel` flow — `cancel_subagent` flips it), the
/// `SubagentCapture` (the pump feeds it; the drive task reads the
/// final text on settle), the handle (the `cancel_subagent`'s
/// `send_abort`), and the `reaping` flag (the drive task's teardown
/// sets it — the `Exited` is EXPECTED: NO `on_crash`).
struct SubagentDrive {
    /// The PARENT session id (the subagent's UI belongs to the
    /// parent's scope — the pump delivers the subagent's frames
    /// under this id + the `is_subagent` flag; the `on_event`
    /// router uses it for the UI delivery, the flag for the
    /// subagent-specific handling).
    parent_session_id: String,
    settle_tx: watch::Sender<bool>,
    cancel_tx: watch::Sender<bool>,
    capture: Arc<SubagentCapture>,
    handle: Option<WorkerHandle>,
    reaping: bool,
}

/// One live Worker (the `attach`-time `StartEnv` + the `reaping`
/// flag — an `Exited` while reaping is EXPECTED: no `on_crash`).
struct WorkerEntry {
    handle: WorkerHandle,
    env: StartEnv,
    reaping: bool,
}

/// The `WorkerManager`'s `Arc`-backed state (the pump / drive tasks
/// own clones of the `Arc` — the `&self` methods delegate to the
/// `Arc`-backed free functions; the `StdMutex` fields are NOT
/// `Clone`, so the `Arc` wrap is the `'static` move).
struct ManagerState {
    /// session_id → the live Worker (the `attach`-time `StartEnv` —
    /// the `dispatch_subagent` reads the parent's effective `catalog`
    /// / `config_dir` / `trusted` from it — the Supervisor owns them).
    workers: StdMutex<HashMap<String, WorkerEntry>>,
    /// The test seam (production: `current_exe()`; tests: the
    /// `fake_worker` binary).
    factory: Arc<dyn WorkerFactory>,
    /// (session_id, exit code) — the Task-4 stalled-session callback
    /// (an UNEXPECTED exit — no `detach` / `reap_all` in flight).
    on_crash: Arc<dyn Fn(String, Option<i32>) + Send + Sync>,
    /// (session_id, is_subagent, evt) — the Task-4 router (UI +
    /// persistence; the `is_subagent` flag is known per attached
    /// Worker: main `false`, subagent `true`).
    on_event: Arc<dyn Fn(String, bool, WorkerInboundEvent) + Send + Sync>,
    /// The parent-scope sink lookup (session_id → the session's
    /// `TauriSink` — the `subagent-session-started` /
    /// `subagent-closed` UI lifecycle events are delivered on the
    /// PARENT session's sink; `None` = no UI delivery).
    sink_for: SinkLookup,
    /// The in-flight subagent dispatchs (session_id → the drive —
    /// the pump flips the settle watch on `agent_settled`, checks the
    /// `reaping` flag on `Exited`, and the `cancel_subagent` flips
    /// the cancel watch).
    drives: StdMutex<HashMap<String, SubagentDrive>>,
    /// The child-cwd registrar (the `TranscriptPersister`'s `set_cwd` —
    /// the `dispatch_subagent` flow registers the child's `cwd` (the
    /// parent's Space) so the persister's `ensure_session_row` creates
    /// the ephemeral `sessions` row with the right `cwd` — the
    /// `StoreFrame` carries no `cwd`). Set once via `set_cwd_registrar`
    /// (the `lib.rs` late-wire — the persister lives in the
    /// `SessionManager`, which the `WorkerManager` cannot see at
    /// construction).
    cwd_registrar: std::sync::OnceLock<CwdRegistrar>,
}

/// The child-cwd registrar signature (the `TranscriptPersister`'s
/// `set_cwd` — `(session_id, cwd)`; a type alias to keep the
/// `OnceLock<Arc<dyn Fn>>` out of the field's type — the clippy
/// `type_complexity` lint).
type CwdRegistrar = Arc<dyn Fn(&str, &str) + Send + Sync>;

/// The parent-scope sink lookup (session_id → the session's
/// `TauriSink` — the `subagent-session-started` /
/// `subagent-closed` UI lifecycle events are delivered on the
/// PARENT session's sink; `None` = no UI delivery).
type SinkLookup = Arc<dyn Fn(&str) -> Option<Arc<dyn EventSink>> + Send + Sync>;

/// A registry of live Workers (session-keyed) + the Supervisor-side
/// subagent-dispatch flow. The `settle_timeout` is OUTSIDE the `Arc`
/// (the `with_settle_timeout` test variant swaps it without rebuilding
/// the `Arc`-backed state — the `StdMutex` maps are not `Clone`).
pub struct WorkerManager {
    state: Arc<ManagerState>,
    /// The subagent settle bound (default 30 min; tests shorten it via
    /// `with_settle_timeout`).
    settle_timeout: Duration,
}

impl WorkerManager {
    pub fn new(
        factory: Arc<dyn WorkerFactory>,
        on_crash: Arc<dyn Fn(String, Option<i32>) + Send + Sync>,
        on_event: Arc<dyn Fn(String, bool, WorkerInboundEvent) + Send + Sync>,
        sink_for: SinkLookup,
    ) -> Self {
        Self {
            state: Arc::new(ManagerState {
                workers: StdMutex::new(HashMap::new()),
                factory,
                on_crash,
                on_event,
                sink_for,
                drives: StdMutex::new(HashMap::new()),
                cwd_registrar: std::sync::OnceLock::new(),
            }),
            settle_timeout: DEFAULT_SUBAGENT_SETTLE_TIMEOUT,
        }
    }

    /// A test variant with a shortened settle bound (the `__slow__`
    /// settle-timeout tests — the fixture's 10 s settle is longer than
    /// the bound). The `settle_timeout` is outside the `Arc`, so the
    /// swap is a field assignment (the `Arc`-backed state is shared).
    pub fn with_settle_timeout(self, timeout: Duration) -> Self {
        Self {
            state: self.state,
            settle_timeout: timeout,
        }
    }

    /// Spawn a Worker for `session_id` + the `ready` handshake (5 s
    /// bound) + the `start` envelope. The read-loop pump is spawned
    /// (the `on_event` / `on_crash` routing — `is_subagent: false`).
    pub async fn attach(&self, session_id: &str, env: &StartEnv) -> Result<(), WorkerError> {
        let handle = (self.state.factory).spawn()?;
        handle.wait_ready(Duration::from_secs(5)).await?;
        // The lifecycle transition (the `ARCHIMEDES_DEBUG` log — a
        // no-op when the var is unset).
        debuglog::log(&format!("worker ready: {session_id}"));
        // The read-loop pump (the `on_event` / `on_crash` routing —
        // `is_subagent: false`; subscribed BEFORE `send_start` so the
        // `Start` frame — the advertised `enabledTools` — is seen).
        start_pump(
            self.state.clone(),
            self.settle_timeout,
            session_id.to_string(),
            false,
            handle.clone(),
        );
        handle.send_start(env)?;
        self.state
            .workers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(
                session_id.to_string(),
                WorkerEntry {
                    handle: handle.clone(),
                    env: env.clone(),
                    reaping: false,
                },
            );
        Ok(())
    }

    /// Detach a session's Worker (the `workers` map bookkeeping — the
    /// `reaping` flag is set so the `Exited` is EXPECTED: NO
    /// `on_crash`; the graceful `close` + 2 s grace + `kill` runs in
    /// the background). Returns the detached handle.
    /// Detach a session's Worker (the `workers` map bookkeeping — the
    /// entry is REMOVED (the `Exited` is EXPECTED: NO `on_crash`);
    /// the graceful `close` + 2 s grace + `kill` runs in the
    /// background). Returns the detached handle.
    pub fn detach(&self, session_id: &str) -> Option<WorkerHandle> {
        let mut map = self.state.workers.lock().unwrap_or_else(|p| p.into_inner());
        let entry = map.remove(session_id)?;
        // The background reap (the graceful `close` + 2 s grace +
        // `kill` — the `Exited` is EXPECTED: the entry is gone from
        // the `workers` map, so the pump's `unexpected_exit` lookup
        // misses — NO `on_crash`).
        let reap_handle = entry.handle.clone();
        tokio::spawn(async move {
            reap_worker(reap_handle).await;
        });
        Some(entry.handle)
    }

    /// Reap ALL attached Workers (a `close` + grace (2 s) + `kill` —
    /// called on app exit; the `reaping` flags make the `Exited`s
    /// EXPECTED — NO `on_crash`).
    pub async fn reap_all(&self) {
        let handles: Vec<WorkerHandle> = {
            let mut map = self.state.workers.lock().unwrap_or_else(|p| p.into_inner());
            for entry in map.values_mut() {
                entry.reaping = true;
            }
            map.values().map(|e| e.handle.clone()).collect()
        };
        for handle in handles {
            reap_worker(handle).await;
        }
    }

    /// The Supervisor-side subagent-dispatch flow (a main-session
    /// Worker's `subagent` tool → its `IpcDispatcher` →
    /// `Outbound::SubagentDispatch` → here): resolve the `model_key`
    /// against the effective catalog (the ADR 0020/0023 resolution —
    /// the `subagentModels` override is already folded into
    /// `launch.model` by the Worker's `resolve_launch`, soft-degraded),
    /// build the child `StartEnv` (the resolved `Model` with its
    /// provider config, the catalog, `cwd` = `parent_cwd`, `trusted` =
    /// the parent Space's trust — the Supervisor owns the `spaces`
    /// table, `system_prompt` =
    /// `build_child_system_message(launch.system_prompt,
    /// has_todo_tool)`, `enabled_tools` = the FULL three-way rule
    /// VERBATIM, `subagent_enabled: false` — the recursion guard),
    /// spawn the subagent Worker (a fresh ephemeral session —
    /// `mint_session_id`), `wait_ready` + `send_start` + the initial
    /// `prompt` (the `task`), then the drive task (`agent_settled` →
    /// the `SubagentCapture`'s final text → `Completed`; an unexpected
    /// `Exited` → `Failed { error: "subagent worker exited
    /// unexpectedly (code N)" }`; the `settle_timeout` → `Failed {
    /// error: "timed out" }`; a `SubagentCancel` → `send_abort` +
    /// reap). The `Inbound::SubagentResult { id, outcome }` is sent to
    /// the REQUESTING (main-session) Worker. The
    /// `subagent-session-started` / `subagent-closed` UI lifecycle
    /// events fire on the PARENT session's sink (the `subagent.rs:635`
    /// / `721/743` payload shapes — the `metrics` captured from the
    /// subagent's event stream + the wall clock).
    pub fn dispatch_subagent(&self, parent_session_id: &str, wire: SubagentDispatchWire) {
        dispatch_subagent(
            self.state.clone(),
            self.settle_timeout,
            parent_session_id.to_string(),
            wire,
            SubagentOutcomeSink::Requester,
            None,
        );
    }

    /// The in-process `dispatch_native` variant (ADR 0025 Task 5 — the
    /// `SubagentSessionManager` re-plumb): the outcome RESOLVES the
    /// oneshot (NOT delivered to a requesting Worker — there is none),
    /// and the child session id is PRE-MINTED (the caller's
    /// `SubagentCancel` targets it via `cancel_subagent`).
    pub fn dispatch_subagent_to(
        &self,
        parent_session_id: &str,
        wire: SubagentDispatchWire,
        child_session_id: String,
        outcome: tokio::sync::oneshot::Sender<SubagentOutcome>,
    ) {
        dispatch_subagent(
            self.state.clone(),
            self.settle_timeout,
            parent_session_id.to_string(),
            wire,
            SubagentOutcomeSink::Oneshot(std::sync::Arc::new(std::sync::Mutex::new(Some(outcome)))),
            Some(child_session_id),
        );
    }

    /// Set the child-cwd registrar (set-once — a second call is a
    /// NO-OP; the `lib.rs` late-wire — the persister lives in the
    /// `SessionManager`, which the `WorkerManager` cannot see at
    /// construction).
    pub fn set_cwd_registrar(&self, registrar: CwdRegistrar) {
        let _ = self.state.cwd_registrar.set(registrar);
    }

    /// Cancel an in-flight subagent dispatch (a parent turn abort —
    /// the `SubagentCancel` flow: `send_abort` + the drive task's
    /// `cancel` flag → the `Cancelled` outcome + the reap).
    pub fn cancel_subagent(&self, session_id: &str) {
        let (cancel_tx, handle) = {
            let drives = self.state.drives.lock().unwrap_or_else(|p| p.into_inner());
            let Some(drive) = drives.get(session_id) else {
                return; // the dispatch is gone.
            };
            (drive.cancel_tx.clone(), drive.handle.clone())
        };
        // The `send_abort` (the child's turn token cancels — the
        // session stays alive until the reap).
        if let Some(handle) = handle {
            let _ = handle.send_abort();
        }
        // The drive task's `cancel` flag (the `Cancelled` outcome —
        // the UNCONDITIONAL teardown reaps the child).
        let _ = cancel_tx.send(true);
    }

    // ── Test support (the registry view) ────────────────────────────

    /// Whether a session's `Exited` is EXPECTED (a `detach` / `reap_all` was in
    /// flight — the `SessionManager`'s router `Exited` handling uses this to decide
    /// the `session-closed` cleanup vs. the `on_crash` path): `true` when the
    /// session is NOT in the `workers` map (already `detach`ed) OR is reaping
    /// (`reap_all` set the flag); `false` when it is in the map and NOT reaping
    /// (a crash — the `on_crash` fires).
    pub fn is_exit_expected(&self, session_id: &str) -> bool {
        let map = self.state.workers.lock().unwrap_or_else(|p| p.into_inner());
        match map.get(session_id) {
            Some(e) => e.reaping,
            None => true,
        }
    }

    /// The registry view (the `test_support`-exposed accessor — the
    /// session count). NOT `#[cfg(test)]`-gated: the headless IPC test
    /// (an integration target — the lib's `cfg(test)` is not compiled
    /// there) asserts through the `test_support` accessor.
    pub fn test_session_count(&self) -> usize {
        self.state
            .workers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .len()
    }

    /// The registry view (is the session attached?). NOT `#[cfg(test)]`-gated
    /// (the headless IPC test asserts through it — see `test_session_count`).
    pub fn test_is_attached(&self, session_id: &str) -> bool {
        self.state
            .workers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .contains_key(session_id)
    }

    /// The registry view (are ALL sessions reaping?). NOT `#[cfg(test)]`-gated
    /// (the headless IPC test asserts through it — see `test_session_count`).
    pub fn test_all_reaping(&self) -> bool {
        let map = self.state.workers.lock().unwrap_or_else(|p| p.into_inner());
        map.values().all(|e| e.reaping)
    }

    /// Send a `Config { trusted }` frame to every RUNNING Worker in the
    /// `cwd` Space (the `set_space_trusted` mid-session trust toggle —
    /// the Worker's `StaticTrustSource` flip + the `ControlCmd` queue;
    /// the `trust-space` permission-outcome path is the separate
    /// `respond_permission` `db` write + flip). Returns the number of
    /// Workers the frame was DELIVERED to (a failed send — the Worker
    /// died / the pipe closed — is not counted; a mid-session toggle
    /// for a dead Worker is a no-op, not an error).
    pub fn send_config_to_space(&self, cwd: &str, trusted: bool) -> usize {
        let map = self.state.workers.lock().unwrap_or_else(|p| p.into_inner());
        let mut delivered = 0;
        for entry in map.values() {
            if entry.env.cwd == cwd && entry.handle.send_config(None, None, Some(trusted)).is_ok() {
                delivered += 1;
            }
        }
        delivered
    }

    /// The session's `WorkerHandle` (a clone) — `None` when the session is not
    /// attached (or was already reaped). The `SessionManager` uses this to relay
    /// frames (prompt / config / abort / responses) to the session's Worker.
    /// The `drives` fallback (ADR 0025 Task 5 — the `dispatch_native` re-plumb):
    /// a SUBAGENT session's handle is in the `drives` map (the in-flight
    /// dispatchs — the subagent's pending maps live in its Worker; the
    /// `respond_*` commands route through this lookup — main AND subagent
    /// sessions both resolve).
    pub fn handle_for(&self, session_id: &str) -> Option<WorkerHandle> {
        let state = &self.state;
        {
            let map = state.workers.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(e) = map.get(session_id) {
                return Some(e.handle.clone());
            }
        }
        state
            .drives
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(session_id)
            .and_then(|d| d.handle.clone())
    }
}

// ── The `Arc`-backed free functions (the `&self`-free logic) ────────

/// The read-loop pump (the `events()` consumer — the `on_event` /
/// `on_crash` routing; the `is_subagent` flag threaded; the
/// `settle_timeout` for the `SubagentDispatch` it runs).
fn start_pump(
    state: Arc<ManagerState>,
    settle_timeout: Duration,
    session_id: String,
    is_subagent: bool,
    handle: WorkerHandle,
) {
    // The pump owns the sole ACTIVE receiver (a clone — the handle's
    // stored receiver is idle, so every frame routes to the polled
    // clone).
    let mut rx = handle.events();
    tokio::spawn(async move {
        loop {
            let result = rx.recv().await;
            let Ok(evt) = result else {
                // `Lagged` — the ring rotated past the cursor (a
                // stalled consumer; the frames are lost — the loop
                // continues); `Closed` — the read task is gone (the
                // process is reaped — the loop ends).
                match result {
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    // The `Ok` variant is excluded by the `let-else`
                    // above (unreachable — the pattern is exhaustive
                    // for the linter).
                    Ok(_) => unreachable!("the `let-else` excluded `Ok`"),
                }
            };
            // (a) The router sees EVERY variant (including `Exited`).
            // A subagent's frames are delivered under the PARENT
            // session's id (the subagent's UI belongs to the parent's
            // scope — the `SubagentDrive` entry holds the parent id
            // until the teardown; a missing entry — the frame is
            // after the teardown — falls back to the child id).
            let delivery_id = if is_subagent {
                state
                    .drives
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .get(&session_id)
                    .map(|d| d.parent_session_id.clone())
                    .unwrap_or_else(|| session_id.clone())
            } else {
                session_id.clone()
            };
            (state.on_event)(delivery_id, is_subagent, evt.clone());
            match evt {
                // The `SubagentCapture` feed (the subagent's
                // `SinkFrame` `session-update` stream — the final-text
                // source; the `message_update` `usage` — the metrics'
                // token fields) + the settle signal (the drive task
                // races it against the `settle_timeout` / the process
                // exit / the cancel).
                WorkerInboundEvent::SinkFrame { event, payload } if is_subagent => {
                    feed_capture(&state, &session_id, |c| {
                        c.observe_sink_frame(&event, &payload);
                        false
                    });
                }
                WorkerInboundEvent::Event(e) if is_subagent => {
                    feed_capture(&state, &session_id, |c| {
                        c.observe_event(&e);
                        matches!(e, RpcEvent::agent_settled)
                    });
                }
                // (b) The Supervisor-side subagent-dispatch flow (the
                // `on_event` above already delivered the frame to the
                // router).
                WorkerInboundEvent::SubagentDispatch(wire) => {
                    dispatch_subagent(
                        state.clone(),
                        settle_timeout,
                        session_id.clone(),
                        wire,
                        SubagentOutcomeSink::Requester,
                        None,
                    );
                }
                // (c) Crash detection: an UNEXPECTED `Exited` (no
                // `detach` / `reap_all` / drive-task teardown in
                // flight) fires `on_crash` exactly once (the
                // `Exited` frame is terminal — the pump loop ends);
                // an EXPECTED `Exited` (a `detach` / `reap_all` was in
                // flight) fires NO `on_crash`.
                WorkerInboundEvent::Exited(code) => {
                    // The lifecycle transition (the `ARCHIMEDES_DEBUG` log —
                    // a CRASH when the `Exited` is unexpected, a clean
                    // `exit` otherwise; the code + the session id).
                    let crashed = unexpected_exit(&state, &session_id);
                    debuglog::log(&format!(
                        "worker {}: {session_id} (code {code:?})",
                        if crashed { "crash" } else { "exit" }
                    ));
                    if crashed {
                        (state.on_crash)(session_id.clone(), code);
                    }
                    break;
                }
                _ => {}
            }
        }
    });
}

/// Feed the subagent's `SubagentCapture` (the `drives` map entry —
/// `None` when the dispatch is gone); the closure returns `true` when
/// the settle watch should be flipped (the `agent_settled` event).
fn feed_capture(state: &ManagerState, session_id: &str, f: impl Fn(&SubagentCapture) -> bool) {
    let mut drives = state.drives.lock().unwrap_or_else(|p| p.into_inner());
    let Some(drive) = drives.get_mut(session_id) else {
        return;
    };
    if f(&drive.capture) {
        let _ = drive.settle_tx.send(true);
    }
}

/// An UNEXPECTED `Exited` (the `on_crash` fires): the session is still
/// registered (the `workers` map for main sessions, the `drives` map
/// for subagents) AND not reaping. An already-detached / torn-down
/// session (absent from both) is EXPECTED — no `on_crash`.
fn unexpected_exit(state: &ManagerState, session_id: &str) -> bool {
    {
        let workers = state.workers.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(entry) = workers.get(session_id) {
            return !entry.reaping;
        }
    }
    let drives = state.drives.lock().unwrap_or_else(|p| p.into_inner());
    match drives.get(session_id) {
        Some(drive) => !drive.reaping,
        None => false,
    }
}

/// The `dispatch_subagent` flow (the `Arc`-backed form — the `&self`
/// method delegates here; the pump task calls it with its `Arc`
/// clone). `sink` is the `SubagentOutcome` delivery target (the
/// `Requester` Worker / the in-process `dispatch_native`'s oneshot);
/// `pre_minted` is the PRE-MINTED child session id (the in-process
/// `dispatch_native`'s `SubagentCancel` targets it — `None` → the flow
/// mints its own, the `Requester` path).
#[allow(clippy::too_many_arguments)]
fn dispatch_subagent(
    state: Arc<ManagerState>,
    settle_timeout: Duration,
    parent_session_id: String,
    wire: SubagentDispatchWire,
    sink: SubagentOutcomeSink,
    pre_minted: Option<String>,
) {
    // The parent's `attach`-time start envelope (the effective
    // `catalog` / `config_dir` / `trusted` — the Supervisor owns
    // them). The parent is gone (detached / reaped) → the
    // dispatch is dropped (the requester's `IpcDispatcher` waiter
    // is never resolved — its `SubagentWait` select sees the
    // dropped oneshot as `Cancelled` — parity with the current
    // driver's "worker task vanished" behavior).
    let Some(parent_env) = state
        .workers
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(&parent_session_id)
        .map(|e| e.env.clone())
    else {
        return;
    };
    // 1. The child `Model` (the `launch.model` override — a
    // trailing `:<level>` suffix is stripped FIRST — the REAL
    // `resolve_composed_model` splits `provider/id` only; the
    // suffix is a candidate thinking level (step 2, when
    // `launch.thinking` is `None`)); `None` → the `model_key`
    // (the loop's pre-flight resolved it against the Worker's
    // catalog). An unresolvable key is `Failed` (the current
    // driver's "unknown model" — NO fallback).
    let (model, level_suffix) = match &wire.launch.model {
        Some(key) => {
            let (bare, suffix) = key
                .rsplit_once(':')
                .map(|(b, s)| (b.to_string(), Some(s.to_string())))
                .unwrap_or_else(|| (key.clone(), None));
            match resolve_composed_model(&parent_env.catalog, &bare) {
                Some(m) => (m, suffix),
                None => {
                    sink.deliver(
                        &state,
                        &parent_session_id,
                        &wire,
                        SubagentOutcome::Failed {
                            error: format!("unknown model: {bare}"),
                        },
                    );
                    return;
                }
            }
        }
        None => match resolve_composed_model(&parent_env.catalog, &wire.model_key) {
            Some(m) => (m, None),
            None => {
                sink.deliver(
                    &state,
                    &parent_session_id,
                    &wire,
                    SubagentOutcome::Failed {
                        error: format!("unknown model: {}", wire.model_key),
                    },
                );
                return;
            }
        },
    };
    // 2. The child thinking level: explicit > the `:<level>` suffix
    // of the resolved model key > the frontmatter's `thinking` (the
    // doc-correct order — the frontmatter's `thinking` is the LAST
    // rung). Validate against `model.thinking_levels` (when
    // non-empty — an empty set soft-passes) — a mismatch is
    // DROPPED (never sent upstream as a bogus `reasoning_effort`).
    let mut thinking = wire
        .launch
        .thinking
        .clone()
        .or(level_suffix)
        .or(wire.launch.frontmatter_thinking.clone());
    if let Some(level) = &thinking {
        if !model.thinking_levels.is_empty() && !model.thinking_levels.iter().any(|l| l == level) {
            thinking = None;
        }
    }
    // 3. The child tool set (the FULL three-way rule — the current
    // driver's step 7, moved verbatim): `launch.tools` `Some` →
    // VERBATIM minus `subagent`/`list_agents` (a non-empty set that
    // empties out yields NO tools — NOT re-expanded);
    // `launch.tools` `None` → the parent's list minus the guard;
    // the `[]` (empty parent list) case → ALL tools minus the
    // guard (the "empty = all" expansion applies ONLY to the
    // inherited-parent case — the `parent_enabled_tools` `[]` is
    // the documented harness convention).
    let child_tools: Vec<String> = match &wire.launch.tools {
        Some(tools) => tools
            .iter()
            .filter(|t| !matches!(t.as_str(), "subagent" | "list_agents"))
            .cloned()
            .collect(),
        None => {
            if wire.parent_enabled_tools.is_empty() {
                tool_specs()
                    .into_iter()
                    .map(|t| t.name)
                    .filter(|t| !matches!(t.as_str(), "subagent" | "list_agents"))
                    .collect()
            } else {
                wire.parent_enabled_tools
                    .iter()
                    .filter(|t| !matches!(t.as_str(), "subagent" | "list_agents"))
                    .cloned()
                    .collect()
            }
        }
    };
    // 4. The child system message (the `harness/prompt` function —
    // `has_todo_tool` from the resolved child tool list; `None` →
    // no system message — the child's seq-0 row is absent).
    let has_todo_tool = child_tools.iter().any(|t| t == "manage_todo_list");
    let system_prompt =
        build_child_system_message(wire.launch.system_prompt.as_deref(), has_todo_tool);
    // 5. The child `StartEnv` (a FRESH ephemeral session —
    // `mint_session_id` (or the PRE-MINTED id — the in-process
    // `dispatch_native`'s `SubagentCancel` targets it); `enabled_tools`
    // = `Some(child_tools)` VERBATIM — an empty `child_tools` = NO tools,
    // NOT re-expanded; `subagent_enabled: false` — the recursion
    // guard).
    let child_id = pre_minted.unwrap_or_else(mint_session_id);
    // The child's `cwd` (the persister's `ensure_session_row` — the
    // ephemeral `sessions` row; the `StoreFrame` carries no `cwd`).
    if let Some(reg) = &state.cwd_registrar.get() {
        reg(&child_id, &wire.parent_cwd);
    }
    let env = StartEnv::from_parts(
        child_id.clone(),
        wire.parent_cwd.clone(),
        StartMode::Fresh,
        None,
        model.clone(),
        parent_env.catalog.clone(),
        thinking.clone(),
        parent_env.trusted,
        Some(child_tools.clone()),
        parent_env.config_dir.clone(),
        false,
        system_prompt,
    );
    // 6. The `subagent-session-started` UI lifecycle event (the
    // current driver's step 9 — the PARENT session's sink: the
    // `model` in the COMPOSED `provider/id` form, the
    // `thinkingLevel` the step-2 RESOLVED value, the
    // `enabledTools` the child's resolved set).
    if let Some(sink) = (state.sink_for)(&parent_session_id) {
        sink.emit(
            "subagent-session-started",
            json!({
                "sessionId": child_id,
                "parentSessionId": parent_session_id,
                "agentName": wire.agent_name,
                "task": wire.task,
                "model": format!("{}/{}", model.provider, model.id),
                "thinkingLevel": thinking.as_deref(),
                "enabledTools": child_tools,
            }),
        );
    }
    // 7. The in-flight dispatch bookkeeping (the `settle` watch —
    // the pump flips it on `agent_settled`; the `cancel` watch —
    // the `SubagentCancel` flow; the `SubagentCapture` — the pump
    // feeds it, the drive task reads the final text on settle).
    let (settle_tx, settle_rx) = watch::channel(false);
    let (cancel_tx, cancel_rx) = watch::channel(false);
    let capture = Arc::new(SubagentCapture::new());
    {
        let mut drives = state.drives.lock().unwrap_or_else(|p| p.into_inner());
        drives.insert(
            child_id.clone(),
            SubagentDrive {
                parent_session_id: parent_session_id.clone(),
                settle_tx: settle_tx.clone(),
                cancel_tx: cancel_tx.clone(),
                capture: capture.clone(),
                handle: None,
                reaping: false,
            },
        );
    }
    // 8. The drive task (the spawn + `wait_ready` + `send_start` +
    // the initial `prompt` (the `task`) + the settle vs
    // `settle_timeout` vs the process exit vs the cancel race +
    // the UNCONDITIONAL teardown + the `SubagentResult` delivery).
    tokio::spawn(async move {
        let start = std::time::Instant::now();
        let outcome = match (state.factory).spawn() {
            Ok(handle) => {
                // The `cancel_subagent`'s `send_abort` target (the
                // drive owns the handle for the race; the `cancel`
                // reads it from the `drives` map — set it NOW).
                {
                    let mut drives = state.drives.lock().unwrap_or_else(|p| p.into_inner());
                    if let Some(d) = drives.get_mut(&child_id) {
                        d.handle = Some(handle.clone());
                    }
                }
                // The subagent's read-loop pump (the `is_subagent`
                // flag threaded — `true`; subscribed BEFORE
                // `send_start` so the `Start` frame — the advertised
                // `enabledTools` — is seen; the `agent_settled`
                // flips the settle watch; the `SubagentCapture` is
                // fed from the `SinkFrame` / `message_update`
                // frames).
                start_pump(
                    state.clone(),
                    settle_timeout,
                    child_id.clone(),
                    true,
                    handle.clone(),
                );
                drive_subagent(
                    &state,
                    handle,
                    env,
                    &parent_session_id,
                    &wire,
                    start,
                    settle_rx,
                    cancel_rx,
                    &capture,
                    settle_timeout,
                    &sink,
                )
                .await
            }
            Err(e) => {
                sink.deliver(
                    &state,
                    &parent_session_id,
                    &wire,
                    SubagentOutcome::Failed {
                        error: format!("spawn the subagent worker: {e}"),
                    },
                );
                None
            }
        };
        // The UNCONDITIONAL teardown (settle / timeout / cancel /
        // crash / spawn failure): the `reaping` flag is set FIRST
        // (the `Exited` is EXPECTED: NO `on_crash`), the `drives`
        // entry is removed (the pump's `Exited` lookup misses — no
        // `on_crash`), the `SubagentResult` is delivered (the
        // `drive_subagent` already sent it on the early-failure
        // paths — this is the settle / timeout / cancel / crash
        // delivery).
        if let Some(outcome) = outcome {
            finish_subagent(&state, &child_id, &parent_session_id, &wire, outcome, &sink).await;
        } else {
            // The early-failure path (the `SubagentResult` was
            // already sent — the teardown is the `drives` removal).
            let mut drives = state.drives.lock().unwrap_or_else(|p| p.into_inner());
            drives.remove(&child_id);
        }
    });
}

/// The drive task (the settle vs `settle_timeout` vs the process exit
/// vs the cancel race — the `drive_subagent` core): the `wait_ready`,
/// `send_start`, and the initial `prompt` (the `task`) preflight, then
/// the race. Returns the `SubagentOutcome` (`None` on a preflight
/// failure — the `SubagentResult` was already sent).
#[allow(clippy::too_many_arguments)]
async fn drive_subagent(
    state: &ManagerState,
    handle: WorkerHandle,
    env: StartEnv,
    parent_session_id: &str,
    wire: &SubagentDispatchWire,
    start: std::time::Instant,
    mut settle_rx: watch::Receiver<bool>,
    mut cancel_rx: watch::Receiver<bool>,
    capture: &Arc<SubagentCapture>,
    settle_timeout: Duration,
    sink: &SubagentOutcomeSink,
) -> Option<SubagentOutcome> {
    // The preflight (a `wait_ready` failure / a `send_start` error
    // means the child is gone before the turn started → `Failed` —
    // the `SubagentResult` is sent here, the teardown is the
    // `drives` removal).
    if handle.wait_ready(Duration::from_secs(5)).await.is_err() || handle.send_start(&env).is_err()
    {
        sink.deliver(
            state,
            parent_session_id,
            wire,
            SubagentOutcome::Failed {
                error: "the subagent worker failed to start".to_string(),
            },
        );
        return None;
    }
    // The initial `prompt` (the `task` — a `SendError` means the
    // child died before the turn started → `Failed`).
    if handle.send_prompt(&wire.task, &[]).is_err() {
        sink.deliver(
            state,
            parent_session_id,
            wire,
            SubagentOutcome::Failed {
                error: "send the subagent task".to_string(),
            },
        );
        return None;
    }
    // The race: the child's settle (the pump's `agent_settled` flips
    // it) vs the `settle_timeout` vs the process exit (an UNEXPECTED
    // exit — a crash; a settle-then-exit is the teardown below) vs
    // the `SubagentCancel` (a parent turn abort — `cancel_subagent`
    // flips it).
    let race = tokio::select! {
        _ = settle_rx.wait_for(|v| *v) => Race::Settled,
        code = handle.exited() => Race::Exited(code),
        _ = tokio::time::sleep(settle_timeout) => Race::TimedOut,
        _ = cancel_rx.changed() => Race::Cancelled,
    };
    // The outcome (the `SubagentCapture`'s final text — the
    // `SinkFrame` `agent_message_chunk` source; the `usage` from the
    // `message_update` frames; the wall clock).
    let duration_ms = start.elapsed().as_millis() as u64;
    let outcome = match race {
        Race::Settled => {
            let (input_tokens, output_tokens) = capture.usage();
            let output = capture.captured_text();
            let metrics = SubagentMetrics {
                output: output.clone(),
                input_tokens,
                output_tokens,
                cost: 0.0,
                duration_ms,
            };
            SubagentOutcome::Completed { output, metrics }
        }
        Race::TimedOut => SubagentOutcome::Failed {
            error: "timed out".to_string(),
        },
        Race::Cancelled => SubagentOutcome::Failed {
            error: "cancelled".to_string(),
        },
        Race::Exited(code) => {
            let code_str = code
                .map(|c| c.to_string())
                .unwrap_or_else(|| "signal".to_string());
            SubagentOutcome::Failed {
                error: format!("subagent worker exited unexpectedly (code {code_str})"),
            }
        }
    };
    Some(outcome)
}

/// The UNCONDITIONAL teardown (settle / timeout / cancel / crash): the
/// `subagent-closed` UI lifecycle event (EVERY exit path — the
/// `subagent.rs:721/743` payload shapes: `status` / `error` /
/// `metrics`), the `reaping` flag (the `Exited` is EXPECTED: NO
/// `on_crash`), the graceful `close` + 2 s grace + `kill`, the
/// `drives` entry removal, and the `SubagentResult` delivery (the
/// REQUESTING (main-session) Worker).
async fn finish_subagent(
    state: &ManagerState,
    child_id: &str,
    parent_session_id: &str,
    wire: &SubagentDispatchWire,
    outcome: SubagentOutcome,
    sink: &SubagentOutcomeSink,
) {
    // The `subagent-closed` UI lifecycle event (the PARENT session's
    // sink — the `subagent.rs:721/743` payload shapes).
    if let Some(sink) = (state.sink_for)(parent_session_id) {
        match &outcome {
            SubagentOutcome::Completed { metrics, .. } => {
                sink.emit(
                    "subagent-closed",
                    json!({
                        "sessionId": child_id,
                        "status": "completed",
                        "metrics": metrics_json(metrics),
                    }),
                );
            }
            SubagentOutcome::Failed { error } => {
                // The `metrics` (the failed-path shape — the `usage`
                // is 0 — the turn never settled; the `duration_ms` is
                // the wall clock to the failure).
                let metrics = SubagentMetrics::default();
                sink.emit(
                    "subagent-closed",
                    json!({
                        "sessionId": child_id,
                        "status": "failed",
                        "error": error,
                        "metrics": metrics_json(&metrics),
                    }),
                );
            }
        }
    }
    // The `reaping` flag is set FIRST (the `Exited` is EXPECTED: NO
    // `on_crash`).
    {
        let mut drives = state.drives.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(d) = drives.get_mut(child_id) {
            d.reaping = true;
        }
    }
    // The graceful `close` + 2 s grace + `kill` (the `drives` entry's
    // handle — the drive task's handle clone is the same process).
    {
        let handle = state
            .drives
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(child_id)
            .and_then(|d| d.handle.clone());
        if let Some(handle) = handle {
            reap_worker(handle).await;
        }
    }
    // The `drives` entry removal (the pump's `Exited` lookup misses —
    // no `on_crash`).
    {
        let mut drives = state.drives.lock().unwrap_or_else(|p| p.into_inner());
        drives.remove(child_id);
    }
    // The `SubagentResult` delivery (the REQUESTING (main-session)
    // Worker — or the in-process `dispatch_native`'s oneshot, the
    // `sink` target).
    sink.deliver(state, parent_session_id, wire, outcome);
}

/// Send `Inbound::SubagentResult { id, outcome }` to the REQUESTING
/// (main-session) Worker (the `IpcDispatcher` waiter resolves — the
/// loop's `SubagentWait` select sees it). The parent is gone → the
/// delivery is a no-op (the waiter is dropped — `Cancelled`).
fn send_subagent_result(
    state: &ManagerState,
    parent_session_id: &str,
    wire: &SubagentDispatchWire,
    outcome: SubagentOutcome,
) -> bool {
    let handle = state
        .workers
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(parent_session_id)
        .map(|e| e.handle.clone());
    let Some(handle) = handle else {
        return false;
    };
    handle
        .send_inbound_frame(Inbound::SubagentResult {
            id: wire.id.clone(),
            outcome,
        })
        .is_ok()
}

/// The `metrics` object shape (the wire contract: `output` /
/// `inputTokens` / `outputTokens` / `cost` / `durationMs` — the
/// `subagent.rs` `metrics_json` moved verbatim).
fn metrics_json(m: &SubagentMetrics) -> Value {
    json!({
        "output": m.output,
        "inputTokens": m.input_tokens,
        "outputTokens": m.output_tokens,
        "cost": m.cost,
        "durationMs": m.duration_ms,
    })
}

/// The graceful reap (the `close` + grace (2 s) + `kill` — the
/// `reaping` flag is set by the caller BEFORE this runs, so the
/// `Exited` is EXPECTED: NO `on_crash`).
async fn reap_worker(handle: WorkerHandle) {
    let _ = handle.send_close();
    let _ = tokio::time::timeout(Duration::from_secs(2), handle.exited()).await;
    let _ = handle.kill().await;
}

/// The subagent drive's outcome (the settle vs `settle_timeout` vs the
/// process exit vs the `SubagentCancel`).
enum Race {
    /// The child settled (the pump's `agent_settled` flipped the
    /// settle watch).
    Settled,
    /// The process exited (an UNEXPECTED exit — a crash; a
    /// settle-then-exit is the teardown below).
    Exited(Option<i32>),
    /// The `settle_timeout` won (a hung / slow turn — the child is
    /// torn down unconditionally).
    TimedOut,
    /// The `SubagentCancel` handle fired (a user / caller cancel —
    /// `cancel_subagent` flipped it).
    Cancelled,
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex as StdMutex};
    use std::time::Duration;

    use serde_json::Value;

    use crate::agent::harness::catalog::{Model, ModelCatalog};
    use crate::agent::session::EventSink;
    use crate::agent::subagent::SubagentOutcome;
    use crate::agent::worker::client::{WorkerError, WorkerHandle, WorkerInboundEvent};
    use crate::agent::worker::manager::{SubagentCapture, WorkerFactory, WorkerManager};
    use crate::agent::worker::protocol::{StartEnv, StartMode};

    /// The test logging type aliases (the `clippy::type_complexity`
    /// bound — the `Arc<StdMutex<...>>` shapes are repeated across
    /// the test helpers).
    type CrashLog = Arc<StdMutex<Vec<(String, Option<i32>)>>>;
    type EventLog = Arc<StdMutex<Vec<(String, bool, WorkerInboundEvent)>>>;
    type SinkMap = Arc<StdMutex<HashMap<String, Arc<CollectorSink>>>>;
    type SinkEvents = Arc<StdMutex<Vec<(String, Value)>>>;

    /// The `fake_worker` fixture (the `fake_mcp_stdio` convention — the
    /// `CARGO_MANIFEST_DIR`/`target/debug` path; `cargo test` builds the
    /// bin targets).
    fn fixture_path() -> std::path::PathBuf {
        std::path::PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/target/debug/fake_worker"
        ))
    }

    /// The test factory (the `WorkerFactory` seam — the `fake_worker`
    /// binary; production is `WorkerHandle::spawn(std::env::
    /// current_exe().as_path())`).
    struct FakeWorkerFactory;

    impl WorkerFactory for FakeWorkerFactory {
        fn spawn(&self) -> Result<WorkerHandle, WorkerError> {
            WorkerHandle::spawn(&fixture_path())
        }
    }

    /// A UI sink collector (the parent-scope `TauriSink` test double).
    #[derive(Clone)]
    struct CollectorSink {
        events: Arc<StdMutex<Vec<(String, Value)>>>,
    }

    impl CollectorSink {
        fn new() -> (Self, SinkEvents) {
            let events = Arc::new(StdMutex::new(Vec::new()));
            (
                Self {
                    events: events.clone(),
                },
                events,
            )
        }
    }

    impl EventSink for CollectorSink {
        fn emit(&self, event: &str, payload: Value) {
            self.events
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push((event.to_string(), payload));
        }
    }

    /// A test `StartEnv` (a fake model the fixture's `SubagentDispatch`
    /// `model_key` — `fake/m1` — resolves against).
    fn test_env(session_id: &str, enabled_tools: Option<Vec<String>>) -> StartEnv {
        fn model(id: &str) -> Model {
            Model {
                id: id.to_string(),
                provider: "fake".to_string(),
                base_url: "http://fake/v1".to_string(),
                api_key: "k".to_string(),
                context_window: 100_000,
                cost_per_mtok_in: 0.0,
                cost_per_mtok_out: 0.0,
                supports_tools: true,
                supports_thinking: false,
                thinking_levels: Vec::new(),
                api: Some("openai-completions".to_string()),
            }
        }
        StartEnv::from_parts(
            session_id.to_string(),
            "/tmp/space".to_string(),
            StartMode::Fresh,
            None,
            model("m1"),
            ModelCatalog {
                models: vec![model("m1")],
                ..Default::default()
            },
            None,
            true,
            enabled_tools,
            "/tmp/config".to_string(),
            true,
            None,
        )
    }

    /// A `WorkerManager` on the `fake_worker` factory + the collectors
    /// (`on_crash` / `on_event` / the parent-scope sink lookup).
    fn make_manager(
        settle_timeout: Option<Duration>,
    ) -> (WorkerManager, CrashLog, EventLog, SinkMap) {
        let crashes = Arc::new(StdMutex::new(Vec::new()));
        let events = Arc::new(StdMutex::new(Vec::new()));
        let sinks: SinkMap = Arc::new(StdMutex::new(HashMap::new()));
        let factory: Arc<dyn WorkerFactory> = Arc::new(FakeWorkerFactory);
        let crashes_c = crashes.clone();
        let events_c = events.clone();
        let sinks_c = sinks.clone();
        let manager = WorkerManager::new(
            factory,
            Arc::new(move |id: String, code: Option<i32>| {
                crashes_c
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .push((id, code));
            }),
            Arc::new(
                move |id: String, is_subagent: bool, evt: WorkerInboundEvent| {
                    events_c
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .push((id, is_subagent, evt));
                },
            ),
            Arc::new(move |id: &str| {
                sinks_c
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .get(id)
                    .cloned()
                    .map(|s| s as Arc<dyn EventSink>)
            }),
        );
        let manager = match settle_timeout {
            Some(t) => manager.with_settle_timeout(t),
            None => manager,
        };
        (manager, crashes, events, sinks)
    }

    /// A parent-scope sink (registered under `session_id`).
    fn register_sink(sinks: &SinkMap, session_id: &str) -> SinkEvents {
        let (sink, events) = CollectorSink::new();
        sinks
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(session_id.to_string(), Arc::new(sink));
        events
    }

    /// Collect the `on_event` deliveries for `session_id` (no waiting —
    /// the callers bounded-wait first).
    fn collect_now(events: &EventLog, session_id: &str) -> Vec<(bool, WorkerInboundEvent)> {
        events
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .filter(|(id, _, _)| id == session_id)
            .map(|(_, is_sub, evt)| (*is_sub, evt.clone()))
            .collect()
    }

    /// Bounded-wait until the `on_event` deliveries for `session_id`
    /// contain a frame matching `pred` (a deadline, so a missing frame
    /// is a test failure, not a hang).
    async fn wait_for_event(
        events: &EventLog,
        session_id: &str,
        pred: impl Fn(&WorkerInboundEvent) -> bool,
    ) -> Vec<(bool, WorkerInboundEvent)> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        loop {
            let got = collect_now(events, session_id);
            if got.iter().any(|(_, e)| pred(e)) || tokio::time::Instant::now() >= deadline {
                return got;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// The `SubagentResult` the supervisor sent to the requester (the
    /// fixture's `subagent-result` INBOUND frame → the fixture's
    /// `subagent-result-ack` `SinkFrame` — the round-trip observation
    /// point; the `outcome` rides verbatim).
    fn subagent_result_outcome(events: &EventLog, session_id: &str) -> Option<SubagentOutcome> {
        events
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .filter(|(id, _, _)| id == session_id)
            .find_map(|(_, _, e)| match e {
                WorkerInboundEvent::SinkFrame { event, payload }
                    if event == "subagent-result-ack" =>
                {
                    serde_json::from_value::<SubagentOutcome>(payload["outcome"].clone()).ok()
                }
                _ => None,
            })
    }

    /// `attach` + `detach` bookkeeping (the `workers` map — the
    /// `test_support`-exposed registry view): `attach` inserts the
    /// session (the `ready` handshake + the `start` envelope); `detach`
    /// removes it + the process exits (a `close` → exit 0 — the
    /// EXPECTED exit fires NO `on_crash`).
    #[tokio::test]
    async fn attach_and_detach_bookkeeping() {
        let (manager, crashes, _events, _sinks) = make_manager(None);
        manager
            .attach("s1", &test_env("s1", Some(vec!["bash".to_string()])))
            .await
            .expect("the attach completes (the ready handshake)");
        assert!(
            manager.test_is_attached("s1"),
            "the attach inserted the session"
        );
        assert_eq!(manager.test_session_count(), 1);
        let handle = manager.detach("s1").expect("the detach returns the handle");
        assert!(
            !manager.test_is_attached("s1"),
            "the detach removed the session"
        );
        // The `close` → the Worker self-exits 0 (the EXPECTED exit —
        // NO `on_crash`).
        let code = handle.exited().await;
        assert_eq!(code, Some(0), "the detached worker exited 0");
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(
            crashes.lock().unwrap_or_else(|p| p.into_inner()).is_empty(),
            "an expected detach-then-exit fires NO `on_crash`"
        );
    }

    /// `reap_all` kills all attached Workers (the `close` + grace +
    /// `kill` — the app-exit path; the processes exited, the registry
    /// is marked reaping, NO `on_crash`).
    #[tokio::test]
    async fn reap_all_kills_all_attached_workers() {
        let (manager, crashes, _events, _sinks) = make_manager(None);
        let _ = manager.attach("b1", &test_env("b1", None)).await;
        let _ = manager.attach("b2", &test_env("b2", None)).await;
        assert_eq!(manager.test_session_count(), 2);
        manager.reap_all().await;
        // The processes exited (the `close` → the fixture self-exits
        // 0; the `kill` is the belt-and-braces).
        assert!(
            manager.test_all_reaping(),
            "the reap marked every session reaping"
        );
        assert!(
            crashes.lock().unwrap_or_else(|p| p.into_inner()).is_empty(),
            "a `reap_all` is an EXPECTED exit — NO `on_crash`"
        );
    }

    /// A crashed Worker fires `on_crash` exactly ONCE with the right
    /// code (an unexpected `Exited` — no `detach` was called; the
    /// `Exited` frame is terminal, so the pump fires it exactly once).
    #[tokio::test]
    async fn a_crashed_worker_fires_on_crash_exactly_once() {
        let (manager, crashes, events, _sinks) = make_manager(None);
        manager
            .attach("c1", &test_env("c1", None))
            .await
            .expect("the attach completes");
        manager
            .handle_for("c1")
            .expect("the handle is reachable")
            .send_prompt("__crash__", &[])
            .expect("the prompt encodes");
        // The `Exited` frame reaches the router (the `on_event` — the
        // Task-4 router's `session-closed` handling).
        let got = wait_for_event(&events, "c1", |e| {
            matches!(e, WorkerInboundEvent::Exited(_))
        })
        .await;
        assert!(
            got.iter()
                .any(|(_, e)| { matches!(e, WorkerInboundEvent::Exited(Some(137))) }),
            "the `Exited` frame reached the router: {got:?}"
        );
        // The `on_crash` fires (the crash detection — the process exit
        // 137).
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            let c = crashes.lock().unwrap_or_else(|p| p.into_inner()).clone();
            if c.iter().any(|(id, code)| id == "c1" && *code == Some(137))
                || tokio::time::Instant::now() >= deadline
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        // A settle (the `on_crash` is fired exactly ONCE — the
        // `Exited` frame is terminal, the pump loop ends).
        tokio::time::sleep(Duration::from_millis(500)).await;
        let c = crashes.lock().unwrap_or_else(|p| p.into_inner()).clone();
        let hits: Vec<_> = c
            .iter()
            .filter(|(id, code)| id == "c1" && *code == Some(137))
            .collect();
        assert_eq!(
            hits.len(),
            1,
            "the crash fired `on_crash` exactly once: {c:?}"
        );
        // A LATER `detach` (the cleanup) is an expected exit — no
        // second `on_crash`.
        manager.detach("c1");
        tokio::time::sleep(Duration::from_millis(500)).await;
        let c = crashes.lock().unwrap_or_else(|p| p.into_inner()).clone();
        assert_eq!(
            c.iter().filter(|(id, _)| id == "c1").count(),
            1,
            "the `on_crash` is not re-fired by the cleanup detach: {c:?}"
        );
    }

    /// The `dispatch_subagent` flow: a `SubagentDispatch` from a
    /// main-session `fake_worker` (the `__subagent__` prompt) spawns a
    /// subagent `fake_worker` (a fresh ephemeral session — the canned
    /// settle) and delivers `SubagentResult` to the REQUESTING Worker
    /// (two `fake_worker`s — the outcome rides the wire back; the
    /// fixture's `subagent-result-ack` is the round-trip observation
    /// point).
    #[tokio::test]
    async fn dispatch_subagent_spawns_a_subagent_and_delivers_the_result() {
        let (manager, _crashes, events, _sinks) = make_manager(Some(Duration::from_secs(30)));
        manager
            .attach("p1", &test_env("p1", Some(vec!["bash".to_string()])))
            .await
            .expect("the parent attach completes");
        manager
            .handle_for("p1")
            .expect("the parent handle is reachable")
            .send_prompt("__subagent__", &[])
            .expect("the prompt encodes");
        // The `SubagentResult` frame arrives at the REQUESTING Worker
        // (the supervisor sent `Inbound::SubagentResult` to it — the
        // fixture's `subagent-result-ack` is the round-trip
        // observation point).
        let got = wait_for_event(&events, "p1", |e| {
            matches!(e, WorkerInboundEvent::SinkFrame { event, .. } if event == "subagent-result-ack")
        })
        .await;
        assert!(
            got.iter().any(|(_, e)| {
                matches!(e, WorkerInboundEvent::SinkFrame { event, .. } if event == "subagent-result-ack")
            }),
            "the `SubagentResult` frame reached the requester: {got:?}"
        );
        // The outcome is `Completed` (the `SubagentCapture`'s final
        // text — the fixture's canned `agent_message_chunk` stream).
        let outcome =
            subagent_result_outcome(&events, "p1").expect("the outcome rides the ack verbatim");
        match outcome {
            SubagentOutcome::Completed { output, metrics } => {
                assert_eq!(
                    output, "canned answer",
                    "the `SubagentCapture`'s final text"
                );
                assert!(metrics.duration_ms >= 1, "the wall clock (ms)");
            }
            other => panic!("expected `Completed`, got {other:?}"),
        }
        // The subagent Worker ran (a fresh ephemeral session — its
        // `agent_settled` reached the router with `is_subagent: true`).
        // The `cloned` (OWNED) — the `Vec` must not borrow the guard
        // (the guard is dropped at the block's end).
        let sub_events: Vec<(String, bool, WorkerInboundEvent)> = {
            let guard = events.lock().unwrap_or_else(|p| p.into_inner());
            guard
                .iter()
                .filter(|(_, is_sub, _)| *is_sub)
                .cloned()
                .collect()
        };
        assert!(
            !sub_events.is_empty(),
            "a subagent Worker's events reached the router with the flag"
        );
        assert!(
            sub_events
                .iter()
                .any(|(_, is_sub, e)| { *is_sub && e.is_agent_settled() }),
            "the subagent settled (the canned settle): {sub_events:?}"
        );
        // The subagent session is gone (the drive task's UNCONDITIONAL
        // teardown — the `drives` entry is removed).
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_eq!(
            manager.test_session_count(),
            1,
            "only the parent remains (the subagent was reaped)"
        );
    }

    /// The `SubagentCapture`: capture from the subagent's `SinkFrame`
    /// `session-update` stream — the enveloped `agent_message_chunk`
    /// frames — the last-`messageId`-with-non-empty-text rule (a tool-using
    /// turn does NOT concatenate every intermediate message; the
    /// `messageId: "system"` bookkeeping chunks are NEVER captured).
    #[test]
    fn the_subagent_capture_captures_the_final_text_from_the_sink_frames() {
        let capture = SubagentCapture::new();
        // A first message (a tool-using turn — an intermediate
        // message).
        capture.observe_sink_frame(
            "session-update",
            &serde_json::json!({
                "sessionId": "s1",
                "update": {
                    "sessionUpdate": "agent_message_chunk",
                    "messageId": "m1",
                    "content": { "type": "text", "text": "intermediate " }
                }
            }),
        );
        capture.observe_sink_frame(
            "session-update",
            &serde_json::json!({
                "sessionId": "s1",
                "update": {
                    "sessionUpdate": "agent_message_chunk",
                    "messageId": "m1",
                    "content": { "type": "text", "text": "text" }
                }
            }),
        );
        // The `system` bookkeeping chunk (NEVER captured — a failed
        // child turn must not settle with a one-liner as its output).
        capture.observe_sink_frame(
            "session-update",
            &serde_json::json!({
                "sessionId": "s1",
                "update": {
                    "sessionUpdate": "agent_message_chunk",
                    "messageId": "system",
                    "content": { "type": "text", "text": "Retry failed" }
                }
            }),
        );
        // The LAST message (the final output — a tool-using turn does
        // NOT concatenate every intermediate message).
        capture.observe_sink_frame(
            "session-update",
            &serde_json::json!({
                "sessionId": "s1",
                "update": {
                    "sessionUpdate": "agent_message_chunk",
                    "messageId": "m2",
                    "content": { "type": "text", "text": "final " }
                }
            }),
        );
        capture.observe_sink_frame(
            "session-update",
            &serde_json::json!({
                "sessionId": "s1",
                "update": {
                    "sessionUpdate": "agent_message_chunk",
                    "messageId": "m2",
                    "content": { "type": "text", "text": "answer" }
                }
            }),
        );
        assert_eq!(
            capture.captured_text(),
            "final answer",
            "the last-`messageId`-with-non-empty-text rule"
        );
        // The usage accumulation (the `message_update` `usage` field —
        // the metrics' token fields).
        capture.observe_event(&crate::agent::events::RpcEvent::message_update {
            usage: serde_json::json!({ "inputTokens": 10, "outputTokens": 5 }),
            assistant_message_event: serde_json::json!({}),
        });
        assert_eq!(capture.input_tokens(), 10);
        assert_eq!(capture.output_tokens(), 5);
    }

    /// A `"__slow__"` subagent (a 10 s settle) hits the `settle_timeout`
    /// outcome (the current timeout outcome — `Failed { error: "timed
    /// out" }`; the subagent is torn down unconditionally).
    #[tokio::test]
    async fn a_slow_subagent_hits_the_settle_timeout_outcome() {
        // The settle bound is SHORTER than the fixture's 10 s sleep.
        let (manager, _crashes, events, _sinks) = make_manager(Some(Duration::from_secs(1)));
        manager
            .attach("t1", &test_env("t1", None))
            .await
            .expect("the parent attach completes");
        manager
            .handle_for("t1")
            .expect("the parent handle is reachable")
            .send_prompt("__slow_subagent__", &[])
            .expect("the prompt encodes");
        // The `SubagentResult` frame (the `settle_timeout` won — the
        // fixture's 10 s settle never arrives in time; the
        // `subagent-result-ack` is the round-trip observation point).
        let got = wait_for_event(&events, "t1", |e| {
            matches!(e, WorkerInboundEvent::SinkFrame { event, .. } if event == "subagent-result-ack")
        })
        .await;
        assert!(
            got.iter().any(|(_, e)| {
                matches!(e, WorkerInboundEvent::SinkFrame { event, .. } if event == "subagent-result-ack")
            }),
            "the `SubagentResult` frame reached the requester: {got:?}"
        );
        let outcome =
            subagent_result_outcome(&events, "t1").expect("the outcome rides the ack verbatim");
        match outcome {
            SubagentOutcome::Failed { error } => {
                assert_eq!(error, "timed out", "the current timeout outcome");
            }
            other => panic!("expected `Failed {{ timed out }}`, got {other:?}"),
        }
        // The subagent is torn down (the UNCONDITIONAL teardown — the
        // `drives` entry is removed).
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_eq!(manager.test_session_count(), 1);
    }

    /// The `subagent-session-started` + `subagent-closed` UI lifecycle
    /// events fire on the PARENT session's sink (the `subagent.rs:635`
    /// / `721/743` payload shapes — the `metrics` populated from the
    /// subagent's event stream + the wall clock; `subagent-closed` on
    /// EVERY exit path).
    #[tokio::test]
    async fn the_subagent_ui_lifecycle_events_fire_on_the_parent_sink() {
        let (manager, _crashes, _events, sinks) = make_manager(Some(Duration::from_secs(30)));
        let sink_events = register_sink(&sinks, "u1");
        manager
            .attach("u1", &test_env("u1", Some(vec!["bash".to_string()])))
            .await
            .expect("the parent attach completes");
        manager
            .handle_for("u1")
            .expect("the parent handle is reachable")
            .send_prompt("__subagent__", &[])
            .expect("the prompt encodes");
        // Wait for the `subagent-closed` (every exit path emits it).
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        loop {
            let got: Vec<(String, Value)> = sink_events
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone();
            if got.iter().any(|(e, _)| e == "subagent-closed")
                || tokio::time::Instant::now() >= deadline
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let got: Vec<(String, Value)> = sink_events
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        let started = got
            .iter()
            .find(|(e, _)| e == "subagent-session-started")
            .map(|(_, p)| p.clone())
            .expect("the `subagent-session-started` fired on the parent's sink");
        // The `subagent.rs:635` payload shape (`model` / `thinkingLevel`
        // / `enabledTools` / `agentName` / `task` + the session ids).
        assert_eq!(started["parentSessionId"], "u1");
        assert!(started["sessionId"].is_string(), "the child session id");
        assert_eq!(started["agentName"], "tester");
        assert_eq!(started["task"], "the task");
        assert_eq!(
            started["model"], "fake/m1",
            "the COMPOSED `provider/id` form"
        );
        assert!(
            started["enabledTools"].is_array(),
            "the child's resolved `enabledTools`"
        );
        let closed = got
            .iter()
            .find(|(e, _)| e == "subagent-closed")
            .map(|(_, p)| p.clone())
            .expect("the `subagent-closed` fired on the parent's sink");
        // The `subagent.rs:721/743` payload shape (`status` / `error`?
        // / `metrics`).
        assert_eq!(closed["status"], "completed");
        assert_eq!(closed["sessionId"], started["sessionId"], "the same child");
        let metrics = &closed["metrics"];
        assert!(metrics["durationMs"].is_u64(), "the wall clock (ms)");
        assert!(
            metrics["durationMs"].as_u64() >= Some(1),
            "the duration is a real elapsed time: {metrics:?}"
        );
        assert!(metrics["output"].is_string(), "the captured final text");
        assert!(metrics["inputTokens"].is_u64());
        assert!(metrics["outputTokens"].is_u64());
        assert!(metrics["cost"].is_number());
    }

    /// The `enabled_tools` wire convention (reviewer-corrected — the
    /// round-3 fix): a `StartEnv` with `enabled_tools:
    /// Some(vec!["subagent"])` (a `launch.tools` that empties out after
    /// the guard minus) → the child Worker's advertised tool set is
    /// EMPTY (the `Some(vec![])` = NO tools semantics preserved — NOT
    /// re-expanded to all); `enabled_tools: None` → all tools (the
    /// settings convention mapped at construction — the `[]` (empty
    /// parent list) case expands to ALL tools minus the guard).
    #[tokio::test]
    async fn the_enabled_tools_wire_convention_is_applied_verbatim() {
        // Case 1: `Some(vec!["subagent"])` — a `launch.tools` that
        // empties out after the guard minus → the child advertises NO
        // tools (NOT re-expanded). The fixture's `__subagent_tools__`
        // prompt emits a `SubagentDispatch` with `launch.tools:
        // Some(["subagent"])` (the `Some`-case trigger).
        let (manager, _crashes, events, _sinks) = make_manager(Some(Duration::from_secs(30)));
        manager
            .attach("w1", &test_env("w1", Some(vec!["subagent".to_string()])))
            .await
            .expect("the parent attach completes");
        manager
            .handle_for("w1")
            .expect("the parent handle is reachable")
            .send_prompt("__subagent_tools__", &[])
            .expect("the prompt encodes");
        // The child's advertised tool set (the `Start` → the fixture's
        // `session-update` `enabledTools` frame — the `is_subagent`
        // events).
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        let mut child_enabled: Option<Value>;
        loop {
            let got: Vec<(String, bool, WorkerInboundEvent)> =
                events.lock().unwrap_or_else(|p| p.into_inner()).clone();
            child_enabled =
                got.iter()
                    .filter(|(id, _, _)| id == "w1")
                    .find_map(|(_, is_sub, e)| {
                        if !is_sub {
                            return None;
                        }
                        match e {
                            WorkerInboundEvent::SinkFrame { event, payload }
                                if event == "session-update" =>
                            {
                                payload
                                    .get("update")
                                    .and_then(|u| u.get("enabledTools"))
                                    .cloned()
                            }
                            _ => None,
                        }
                    });
            if child_enabled.is_some() || tokio::time::Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let child_enabled = child_enabled.expect("the child advertised its tool set");
        assert_eq!(
            child_enabled,
            Value::Array(Vec::new()),
            "a `launch.tools` that empties out yields NO tools — NOT re-expanded: {child_enabled:?}"
        );
        // Case 2: `launch.tools: None` (the `__subagent__` trigger —
        // the inherited-parent branch) → the child advertises the
        // parent's list minus the guard (`subagent` / `list_agents`
        // excluded; the fixture's `parent_enabled_tools` is
        // `["bash", "read"]` — no guard names, so VERBATIM).
        let (manager, _crashes, events, _sinks) = make_manager(Some(Duration::from_secs(30)));
        manager
            .attach("w2", &test_env("w2", None))
            .await
            .expect("the parent attach completes");
        manager
            .handle_for("w2")
            .expect("the parent handle is reachable")
            .send_prompt("__subagent__", &[])
            .expect("the prompt encodes");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        let mut child_enabled: Option<Value>;
        loop {
            let got: Vec<(String, bool, WorkerInboundEvent)> =
                events.lock().unwrap_or_else(|p| p.into_inner()).clone();
            child_enabled =
                got.iter()
                    .filter(|(id, _, _)| id == "w2")
                    .find_map(|(_, is_sub, e)| {
                        if !is_sub {
                            return None;
                        }
                        match e {
                            WorkerInboundEvent::SinkFrame { event, payload }
                                if event == "session-update" =>
                            {
                                payload
                                    .get("update")
                                    .and_then(|u| u.get("enabledTools"))
                                    .cloned()
                            }
                            _ => None,
                        }
                    });
            if child_enabled.is_some() || tokio::time::Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let child_enabled = child_enabled.expect("the child advertised its tool set");
        let tools = child_enabled
            .as_array()
            .expect("the advertised tool set is an array");
        assert!(
            !tools.is_empty(),
            "the inherited parent list is advertised (minus the guard): {child_enabled:?}"
        );
        assert!(
            !tools.iter().any(|t| t == "subagent"),
            "the guard excludes `subagent`: {child_enabled:?}"
        );
        assert!(
            !tools.iter().any(|t| t == "list_agents"),
            "the guard excludes `list_agents`: {child_enabled:?}"
        );
    }
}
