---
status: committed
done-when: In `pnpm tauri dev`, a `subagent` tool call from a bridge-mode main session spawns a desktop-managed subagent session: the right-rail panel shows its live compact stream (state indicator, tool calls); its `ask` renders in the panel and the answer flows directly to the subagent (no main-agent relay); parallel dispatch shows multiple panels; the tool returns combined results to the main agent; the 2-session concurrency regression test passes on the worker runtime; non-bridge subagent behavior is unchanged; `cargo test` / `pnpm test` / `cargo clippy --all-targets` / `cargo fmt --check` are green.
---

# Subagent Sessions Plan

**Goal:** In bridge mode, move subagent execution from the agent-side suite (the `subagent` tool forking a child pi process) to the desktop (Client): the desktop spawns a full ACP session per delegated task on a dedicated worker runtime, renders it in a right-rail panel through the same ACP pipeline as the main agent, and the subagent's interactive tools (`ask`, `sudo_exec`) talk to the desktop directly.

**Architecture:** A `WorkerRuntime` (a dedicated tokio runtime on a dedicated OS thread, ADR 0004) hosts all subagent sessions; a `SubagentSessionManager` drives them through the same extracted session-driver machinery as `SessionManager` (per-spawn peer-verified bridge listener, launch wrapper per ADR 0005, `PI_ACP_PI_COMMAND` env). The `subagent` tool's new bridge-mode branch sends a `dispatch_subagent` bridge request (method-aware: no 5-min/330 s timeout) that the parent session's bridge waiter runs on the worker runtime and answers with the final output + metrics. Non-bridge modes keep the existing fork path untouched; an unreachable bridge falls back to it. Subagent sessions are ephemeral (not stored, `--no-session`).

**Tech Stack:** Rust (Tauri 2, tokio, rusqlite, `agent-client-protocol` 2.x) + TypeScript/React (Vite, Tailwind, Zustand, Vitest) + the pi-archimedes suite (`packages/subagent`, `packages/core` bridge).

**Repos, paths and commands used in every task below:**
- Desktop repo root: `/home/daniel/Coding/AI/archimedes-desktop` (Rust under `src-tauri/`, frontend under `src/`).
- Suite repo root: `/home/daniel/Coding/Javascript/pi-archimedes` (pnpm workspace; the `subagent` package under `packages/subagent/`, the bridge under `packages/core/src/bridge/`).
- Rust: `cd src-tauri && cargo test --test <name>` (or full `cargo test`), `cargo clippy --all-targets` (must be 0 warnings), `cargo fmt --check`
- Frontend: `pnpm test` (Vitest, repo root), `pnpm build` (tsc + vite)
- Suite: `cd /home/daniel/Coding/Javascript/pi-archimedes && pnpm test`, `pnpm -r exec -- tsc --noEmit`
- Wire case conventions (unchanged): ACP property keys camelCase, discriminator values snake_case; bridge frames carry `v: 1`; bridge event names snake_case (`cost_update`, `todos_update`, `state`); Tauri IPC args camelCase.
- Decisions on record: ADR 0002 (one-live policy — subagent sessions excluded by definition), ADR 0003 (bridge listener, peer verification, cancel semantics), ADR 0004 (worker runtime + per-session-thread fallback), ADR 0005 (per-dispatch `PI_ACP_PI_COMMAND` wrapper). Terminology: `CONTEXT.md` ("Subagent session" entry).

**Wire contract (suite ↔ desktop, `dispatch_subagent`):**

Request params (suite → desktop, one frame per task, `source: "main"`, `toolCallId` = the subagent tool-call id):
```json
{ "agentName": "reviewer", "task": "…", "systemPrompt": "…" | null,
  "model": "provider/id" | null, "thinking": "medium" | null,
  "tools": ["read", "bash"] | null }
```
`tools` present → pi `--tools` allowlist (the subagent tool is excluded by construction); `tools` null → pi `--exclude-tools subagent` (config-less). `cwd` is NOT sent — the subagent session's cwd is the parent session's Space folder (the tool's `cwd` param is ignored in bridge mode).

Response (desktop → suite, one frame per request):
```json
// success:
{ "v": 1, "type": "response", "id": "…", "result": { "output": "…",
  "metrics": { "inputTokens": 0, "outputTokens": 0, "cost": 0.0, "durationMs": 0 } } }
// failure:
{ "v": 1, "type": "response", "id": "…", "error": "cancelled" }
{ "v": 1, "type": "response", "id": "…", "error": "<message>" }
```

**Metrics note (v1 limitation):** the suite does NOT emit a `cost_update` for a process's OWN usage (the only bus `COST_UPDATE` emitter is the subagent tool reporting forked children's deltas, source `subagent:<name>`). So in v1 the desktop's captured metrics for a subagent session are `{0, 0, 0, <real durationMs>}` — `durationMs` is real (wall clock); the token/cost fields are 0. The desktop's capture mechanism is in place for a future suite-side self-usage push (e.g. on `agent_settled`) — that push is a tracked follow-up, NOT part of this plan (adding it would also change the main session's `cost_update` semantics, which is out of scope). The tool's `usage`/`progressSummary.tokens` report zeros in bridge mode — documented, not silent.

New desktop → frontend events (alongside the existing `session-update` / `bridge-event` / `bridge-request` / `permission-request` / `session-closed`, which all flow for subagent sessions keyed by the subagent's session id):
```json
{ "sessionId": "<subagent ACP id (known AFTER establishment — see Task 3 step 4)>", "parentSessionId": "<parent ACP id from the listener's session-id state (set after the parent's establishment)>", "agentName": "reviewer", "task": "…" }   // "subagent-session-started"
{ "sessionId": "<subagent ACP id>", "status": "completed" | "failed", "error": "…", "metrics": { "inputTokens": 0, "outputTokens": 0, "cost": 0.0, "durationMs": 0 } }  // "subagent-closed" (metrics = the captured `cost_update` payload + duration, same shape as the dispatch response; `error` present only for `failed`)
```

---

### Task 1: Worker runtime + 2-session concurrency regression test (the build gate)

**Context:** ADR 0004: subagent sessions run on a dedicated tokio runtime because two concurrent ACP sessions on ONE runtime hung `send_prompt` 60+ seconds (2026-09-15 repro, root cause never confirmed — suspected SDK/async-io level: two long-lived per-connection transport tasks sharing one global async-io reactor). This task builds the `WorkerRuntime` and the regression test that de-risks the whole feature BEFORE anything is built on top. If the test fails on the worker runtime (i.e. the reactor is process-global), STOP and switch to the ADR 0004 fallback (per-session OS thread + single-threaded runtime) before proceeding to Task 3. **Drop-safety constraint (reviewed):** tokio PANICS with "Cannot drop a runtime in a context where blocking is not allowed" when a `Runtime` is dropped from an async context — so the `Runtime` must be owned by a dedicated OS thread that blocks on a shutdown signal and drops the runtime on THAT thread (never on the calling/async thread). This also matches ADR 0004's "dedicated OS thread" wording.

**Files:**
- Create: `src-tauri/src/acp/worker_runtime.rs`
- Modify: `src-tauri/src/acp/mod.rs` (add `pub mod worker_runtime;`)
- Test: `src-tauri/tests/subagent_concurrency.rs`

**What to implement:**

`WorkerRuntime` — owns a dedicated tokio runtime on a dedicated OS thread (the thread owns the `Runtime` and blocks on a shutdown signal, so the `Runtime` is dropped on the dedicated thread — never from an async context); channel-based handoff only (NEVER `block_on` across runtimes):

```rust
pub struct WorkerRuntime {
    /// Handle captured before the `Runtime` moves into the dedicated thread.
    handle: tokio::runtime::Handle,
    shutdown_tx: std::sync::mpsc::Sender<()>,
    thread: std::thread::JoinHandle<()>,
}

impl WorkerRuntime {
    /// Build the worker runtime on a dedicated OS thread (a multi-thread
    /// runtime with worker_threads = 2 — its own thread pool, separate from
    /// the app runtime's). Built at app setup (`setup_dirs`); two idle
    /// threads are negligible. The dedicated thread owns the `Runtime`,
    /// hands out a `Handle`, then blocks on `shutdown_rx` — dropping the
    /// `Runtime` happens on this thread (tokio panics if a runtime is
    /// dropped from an async context).
    pub fn new() -> Self {
        let (handle_tx, handle_rx) = std::sync::mpsc::channel::<tokio::runtime::Handle>();
        let (shutdown_tx, shutdown_rx) = std::sync::mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("archimedes-subagents".into())
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .thread_name("archimedes-subagents-worker")
                    .enable_all()
                    .build()
                    .expect("worker runtime build");
                let handle = rt.handle().clone();
                handle_tx.send(handle).expect("handle handoff");
                let _ = shutdown_rx.recv(); // block until shutdown
                drop(rt); // dropped on this dedicated (non-async) thread
            })
            .expect("worker thread spawn");
        let handle = handle_rx.recv().expect("handle handoff");
        Self { handle, shutdown_tx, thread }
    }

    /// Run `f` on the worker runtime; the result is delivered through the
    /// returned oneshot. The caller never blocks on the worker runtime.
    pub fn spawn_task<F>(&self, f: F) -> oneshot::Receiver<F::Output>
    where F: Future + Send + 'static, F::Output: Send + 'static
    {
        let (tx, rx) = oneshot::channel();
        self.handle.spawn(async move {
            let out = f.await;
            let _ = tx.send(out);
        });
        rx
    }

    /// Send the shutdown signal and join the dedicated thread (the thread
    /// drops the `Runtime` on itself and exits). Use from tests to await
    /// teardown; `Drop` (signal only, no join — you cannot join self) covers
    /// the app-exit path.
    pub fn shutdown_and_join(self) {
        let _ = self.shutdown_tx.send(());
        let _ = self.thread.join();
    }
}

impl Drop for WorkerRuntime {
    fn drop(&mut self) {
        // `mpsc::Sender::send` is non-blocking — safe from any context.
        // The dedicated thread does the actual `Runtime` drop.
        let _ = self.shutdown_tx.send(());
    }
}
```

The regression test (`src-tauri/tests/subagent_concurrency.rs`) — reproduces the 2026-09-15 shape with the SDK transport stack (the `fake_agent` binary, `CARGO_BIN_EXE_fake_agent`, speaks ACP over stdio — the same transport the real `pi-acp` uses):
1. Build a `SessionManager` (temp config dir, a registry entry pointing at `fake_agent`, `bridge: false` so no bridge env) — the MAIN-runtime session.
2. Build a `WorkerRuntime`; on it (via `spawn_task`), build a second `SessionManager` (same shape) and `start_session` — the WORKER-runtime session. The worker task returns `(Arc<SessionManager>, String /* session_id */)` (the manager is `Send + Sync` — `Arc<Mutex<…>>` internals — so it outlives the first worker task and the test drives it from the main runtime). Note: the worker `SessionManager`'s own driver tasks keep running on the worker runtime (the manager is just a handle) — that is the point: the ACP transport tasks live on the worker runtime.
3. Send `send_prompt` to BOTH sessions concurrently, wrapped in a HARD timeout: `tokio::time::timeout(Duration::from_secs(10), tokio::join!(…))` — an `Err(_)` (the timeout fired) IS the repro: assert on the elapsed-time branch (a regression is a clean red test, NOT a stalled one — the 2026-09-15 hang may be an indefinite stall, and an unbounded `join!` would hang `cargo test` with no output, defeating the STOP gate). On `Ok`, assert both return `StopReason::EndTurn` and the total wall clock is < 10 s (the hang was 60+ s; the fake agent answers immediately, so any stall is the transport).
4. `close_session` on both; assert the `session-closed` events fire for both (teardown on both runtimes works), then `worker.shutdown_and_join()` (asserts the dedicated-thread drop path is panic-free).

**Steps:**
- [ ] Write `src-tauri/tests/subagent_concurrency.rs` with the test above (it will not compile yet — `WorkerRuntime` is missing).
- [ ] Run `cd src-tauri && cargo test --test subagent_concurrency` — confirm it FAILS to compile (missing `worker_runtime` module).
- [ ] Implement `src-tauri/src/acp/worker_runtime.rs` (the struct above) + the `mod.rs` line.
- [ ] Run `cd src-tauri && cargo test --test subagent_concurrency` — did it PASS (both prompts < 10 s; `shutdown_and_join` returns without a panic)? If it fails with a stall ≥ 60 s, STOP: the reactor is process-global — switch to the ADR 0004 fallback (per-session OS thread + single-threaded runtime: replace `WorkerRuntime::new`'s multi-thread runtime with a per-dispatch `std::thread` + `tokio::runtime::Builder::new_current_thread()` — keeping the dedicated-thread-ownership pattern — and re-run before continuing).
- [ ] Run `cd src-tauri && cargo clippy --all-targets` — 0 warnings? Fix if not.
- [ ] Run `cd src-tauri && cargo fmt --check` — clean? Fix if not.
- [ ] Commit with message: "feat: worker runtime + 2-session concurrency regression test (ADR 0004 gate)"

**Acceptance criteria:**
- [ ] `cargo test --test subagent_concurrency` passes: one main-runtime + one worker-runtime ACP session, concurrent prompts, both `end_turn`, total < 10 s; `shutdown_and_join` is panic-free.
- [ ] The `Runtime` is owned by the dedicated OS thread (dropped there, never from an async context); the public surface is `new()` + `spawn_task()` + `shutdown_and_join()` (no `block_on` anywhere in the module).
- [ ] Clippy 0 warnings, fmt clean.

---

### Task 2: Launch wrapper (per-dispatch `PI_ACP_PI_COMMAND` script, ADR 0005)

**Context:** A subagent session must start its pi with a per-dispatch configuration (system prompt, model, tools, thinking, `--no-session`). `pi-acp` has no CLI surface for any of that: it spawns `pi --mode rpc --no-themes` and only honors `PI_ACP_PI_COMMAND` (verified in the installed pi-acp: `PiRpcProcess.spawn` uses `getPiCommand(params.piCommand)`, passes `env: process.env`, and its best-effort `pi --version` probe uses the literal `pi` on PATH, so the wrapper is unaffected). This task builds the wrapper: a pure flag-construction function + a script writer. The wrapper is written into the per-spawn 0700 bridge dir (the `archimedes-bridge-<uid>` dir `bridge_socket_path()` already creates) and unlinked at session teardown.

**Files:**
- Create: `src-tauri/src/acp/launch_wrapper.rs`
- Modify: `src-tauri/src/acp/mod.rs` (add `pub mod launch_wrapper;`)

**What to implement:**

```rust
pub struct LaunchConfig {
    /// The agent file body (named agents); `None` for config-less dispatch.
    pub system_prompt: Option<String>,
    /// Resolved model ("provider/id" or "provider/id:<thinking>"); `None` = pi default.
    pub model: Option<String>,
    /// Explicit thinking level from the agent file; `None` = pi's own resolution.
    pub thinking: Option<String>,
    /// Tool allowlist (named agents with `tools`); `None` → `--exclude-tools subagent`.
    pub tools: Option<Vec<String>>,
}

/// Pure: build the wrapper script text. `pi_command` is the resolved pi
/// binary (usually "pi"). The script execs pi with the config flags and
/// forwards pi-acp's own args (`--mode rpc --no-themes`) via "$@".
pub fn build_wrapper_script(config: &LaunchConfig, pi_command: &str) -> String

/// Write the wrapper into `dir` (the per-spawn 0700 bridge dir) as
/// `wrapper-<uuid>.sh` (0755) and return its path. `dir` must exist.
pub fn write_wrapper(dir: &Path, config: &LaunchConfig, pi_command: &str) -> std::io::Result<PathBuf>
```

Script shape (Linux — the platform where the bridge is fully implemented; macOS/Windows fall back to the suite's fork path because their bridge listeners are no-ops, so the wrapper is effectively Linux-only in v1 — the Windows variant is a `.cmd` file (`@echo off` + `pi <args> %*`) for compilation completeness, with NO quoting engine):
```sh
#!/bin/sh
exec pi --system-prompt '<escaped>' --model '<escaped>' --thinking '<escaped>' --tools a,b --no-session "$@"
```
Escaping: every flag value is single-quoted with embedded single quotes escaped as `'\''` (a pure `fn shell_quote(s: &str) -> String` — unit-tested). Flags are emitted only when present: `--system-prompt` (named agents only), `--model` + `--thinking` (when set), `--tools <comma-joined>` (when `Some`) OR `--exclude-tools subagent` (when `None`), and always `--no-session` (ephemeral — no pi session file). Windows variant: a `.cmd` file (`@echo off` + `pi <args> %*`) written when `cfg!(windows)` — simple, no quoting engine (the fallback covers Windows in v1).

**Steps:**
- [ ] Write failing unit tests in `launch_wrapper.rs` (`#[cfg(test)]`): (1) named agent with tools → script contains `--system-prompt`, `--tools read,bash`, `--no-session`, `"$@"`; (2) named agent without tools → `--exclude-tools subagent`, no `--tools`; (3) config-less (all `None`) → only `--exclude-tools subagent --no-session "$@"`; (4) `shell_quote` escapes embedded single quotes (`a'b` → `'a'\''b'`); (5) `write_wrapper` writes an executable file (0755) and the path exists (temp dir).
- [ ] Run `cd src-tauri && cargo test launch_wrapper` — confirm the tests FAIL (module missing).
- [ ] Implement `launch_wrapper.rs`.
- [ ] Run `cd src-tauri && cargo test launch_wrapper` — did all tests pass?
- [ ] Run `cd src-tauri && cargo clippy --all-targets` + `cargo fmt --check` — clean?
- [ ] Commit with message: "feat: per-dispatch launch wrapper (PI_ACP_PI_COMMAND, ADR 0005)"

**Acceptance criteria:**
- [ ] `build_wrapper_script` is pure and deterministic (no fs, no uuid) — all flag variants + escaping covered by unit tests.
- [ ] `write_wrapper` produces a 0755 executable script in the given dir.
- [ ] Clippy 0 warnings, fmt clean.

---

### Task 3: SubagentSessionManager + shared session driver + `dispatch_subagent` bridge method

**Context:** The desktop needs a second session manager that runs on the worker runtime: `SubagentSessionManager`. It must reuse the exact session-driver machinery `SessionManager` uses (spawn → client backends → establisher → block-until-close → cleanup/teardown) — so this task first EXTRACTS that machinery into a shared `SessionDriver` (behavior-preserving: every existing test must pass unchanged), then builds the subagent manager on top. The subagent's establisher is `initialize` + `session/new` (bounded by the 30 s establish timeout); the task prompt is sent AFTER establishment (it is unbounded — a subagent task may run minutes; cancellation is the close watch). The `dispatch_subagent` bridge method (parent session's listener) has NO timeout (method-aware — the 330 s cap and the agent's 5-min timeout do not apply; cancellation is driven by parent close / agent EOF / app exit, all with the existing explicit terminal frame).

**Files:**
- Create: `src-tauri/src/acp/subagent.rs`
- Modify: `src-tauri/src/acp/session.rs` (extract the driver; `SessionManager` delegates to its `SessionDriver`)
- Modify: `src-tauri/src/acp/bridge.rs` (`dispatch_subagent` method handling; `ConnCtx` gains optional capture fields)
- Modify: `src-tauri/src/acp/mod.rs` (add `pub mod subagent;`)
- Modify: `src-tauri/src/commands/sessions.rs` (`respond_bridge_request` + `respond_permission` route to the subagent manager when the main manager has no entry)
- Modify: `src-tauri/src/lib.rs` (create + manage `SubagentSessionManager`)
- Test: `src-tauri/tests/subagent_dispatch.rs` (the full fake-agent E2E — see Task 6; written here because the manager is the unit under test, and Task 6 extends `fake_agent` for it)

**What to implement:**

**1. `SessionDriver` extraction (behavior-preserving).** From `SessionManager`, move into a new struct (keep it in `session.rs` or a new `driver.rs` — the plan says `session.rs`):

```rust
/// The shared session-driver state. `SessionManager` (main sessions) and
/// `SubagentSessionManager` (worker runtime) each own one.
pub struct SessionDriver {
    pub sessions: Arc<Mutex<HashMap<SessionId, LiveSession>>>,
    pub pending_permissions: PendingPermissions,
    pub pending_bridge: PendingBridge,
    pub establish_timeout: Duration,
    /// Persistence (main only; `None` for subagents — ephemeral, not stored).
    pub db: Option<Arc<Db>>,
    /// In-memory per-`messageId` agent-text accumulator for the FINAL
    /// OUTPUT (subagents only; `None` for main — main persists to the DB).
    pub text_capture: Option<Arc<StdMutex<HashMap<String, String>>>>,
    /// The last-seen `messageId` (updated for EVERY `agent_message_chunk`,
    /// alongside `text_capture`): a plain `HashMap` has no insertion order,
    /// so the "last message" is tracked separately, not derived from
    /// iteration order.
    pub last_message_id: Option<Arc<StdMutex<Option<String>>>>,
    /// Last `cost_update` push payload (subagents only; `None` for main).
    pub cost_capture: Option<Arc<StdMutex<Option<Value>>>>,
    /// The subagent dispatch handle (main manager only — `Some`); `None`
    /// for the subagent manager itself (subagents cannot dispatch
    /// subagents — the tool is excluded from their spawn).
    pub subagent: Option<Arc<SubagentSessionManager>>,
}
```

`drive_session` becomes a method on `SessionDriver` (`pub(crate)`, same signature shape, `self: &SessionDriver`); `LiveSession` (and its fields `cx`, `close_tx`, `close_kind`), `CloseKind`, the cleanup/teardown code, `persist_update`, `map_establish_error`, `spawn_hint` all stay — with these visibility changes: `LiveSession` + its fields become `pub(crate)` (subagent.rs reads `driver.sessions` entries); `bridge_socket_path`/`bridge_spawn_setup` become `pub(crate)` free functions in `session.rs` (subagent.rs reuses them). Three additions inside the driver:
- (a) the `on_receive_notification` handler ALSO updates `text_capture` (the chunk's `messageId` → accumulated text) AND `last_message_id` (the chunk's `messageId`, overwriting) when `Some` (alongside the DB upsert — main behavior unchanged, both `None` there);
- (b) the push branch of `handle_connection` (bridge.rs) stores the `cost_update` payload into `cost_capture` when the listener was started with one (see 3);
- (c) an **external close** parameter: `external_close: Option<ExternalClose>` — the subagent cancel path (main sessions pass `None`): the dispatch worker task owns the sender + kind; the driver task SELECTS on the receiver in BOTH the establish phase (around the `tokio::timeout(establish_timeout, establish(cx))` — so a cancel during the 30 s establish window is honored, not deferred) and the existing block-until-close phase; when `Some`, the driver uses `external_close.kind` INSTEAD of its own internal kind (one kind, first-set-wins across the whole session). `ExternalClose { tx: watch::Sender<bool>, rx: watch::Receiver<bool>, kind: Arc<StdMutex<Option<CloseKind>>> }` — `SubagentCancel` (below) keeps the `tx` + `kind`; the driver keeps the `rx`.

`SessionManager` holds a `SessionDriver` (db: `Some`, captures: `None`, subagent: injected via a setter — see 4) and its `start_session`/`resume_session`/`close_session`/`supersede_live_sessions`/`connection`/`respond_*` delegate to it — EXACTLY the same behavior (the one-live policy, DB recording, `record_session` — all stay in `SessionManager`). `SubagentSessionManager` holds its OWN `SessionDriver` (db: `None`, captures: `Some`, subagent: `None` — subagents cannot dispatch subagents).

**2. `SubagentSessionManager` (`subagent.rs`):**

```rust
pub struct SubagentSessionManager {
    driver: SessionDriver,            // db: None, captures: Some
    worker: WorkerRuntime,            // Task 1
    registry: Registry,
    config_dir: PathBuf,
}

pub struct SubagentMetrics {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost: f64,
    pub duration_ms: u64,
}

pub enum SubagentOutcome {
    Completed { output: String, metrics: SubagentMetrics },
    Failed { error: String },         // includes "cancelled"
}
```

`dispatch` — the core (runs the lifecycle on the worker runtime via `WorkerRuntime::spawn_task`):

```rust
/// Spawn + establish + prompt one subagent session. `parent_session_id`
/// is the parent's ACP id (from the listener's session-id state — after
/// the parent's `set_session_id`; a `dispatch_subagent` frame arrives
/// mid-turn, so it is always the ACP id); `parent_cwd` is the parent's
/// Space folder (the subagent's cwd + fs sandbox root); `parent_agent_id`
/// is the parent's registry agent id (the subagent spawns the SAME
/// registry entry as the parent — the built-in `pi` entry in production;
/// it rides the `SubagentSpawn` handle from `ConnCtx`). The `subagent-
/// session-started` event carries the ACP id — see step 4.
/// Returns the result oneshot + a cancel handle (flips the external close
/// — the driver task tears the session down, the agent's process group
/// dies on Unix).
pub fn dispatch(
    &self,
    parent_session_id: &str,
    parent_cwd: &Path,
    parent_agent_id: &str,
    launch: LaunchConfig,
    task: String,
    sink: &Arc<dyn EventSink>,
) -> (oneshot::Receiver<SubagentOutcome>, SubagentCancel)
```

Lifecycle inside the worker task (the whole lifecycle runs on the worker runtime via `WorkerRuntime::spawn_task`):
1. `session_id = uuid::Uuid::new_v4().to_string()` (BEFORE spawn — this is the **bridge-listener placeholder only**, exactly like `start_session`'s `client_session_id`; the ACP `session_id` is agent-generated and is the identity for everything else — see step 4).
2. Bridge setup: `bridge_spawn_setup(&registry entry for parent_agent_id, &session_id)` (reused — 4 env vars, per-spawn socket) + write the wrapper (`launch_wrapper::write_wrapper` into the bridge dir) + `env.insert("PI_ACP_PI_COMMAND", wrapper_path)`. (On platforms where `bridge::available()` is false this whole path is unreachable — the suite falls back to the fork; the code still compiles.)
3. `driver.drive_session(...)` with the subagent establisher: `initialize` (same client capabilities as main: fs read/write true, terminal false) + `session/new` (cwd = `parent_cwd`) — bounded by the 30 s establish timeout, with the `ExternalClose` from step 3a wired in. On establish failure → resolve `Failed { error }` and return (NO `subagent-*` events — the session never materialized; the main agent's tool result carries the error; the wrapper + socket are unlinked).
   - 3a. Before `drive_session`: build the `ExternalClose` (the `SubagentCancel` handle the caller receives); the driver task selects on its `rx` (see the driver's addition (c)).
4. **After establishment, the ACP `session_id` is known** (the establisher's `new_session.session_id` — the fake/real agent's id, NOT the placeholder). NOW emit `subagent-session-started` `{ sessionId: <ACP id>, parentSessionId: <parent ACP id — from the `ConnCtx` session-id state, which is the parent's ACP id after its `set_session_id`>, agentName, task }`. **This ordering is the fix for the session-id identity bug:** every downstream artifact the panel cross-references (`session-update`, `bridge-request`/`bridge-event`, `permission-request`, `session-closed`, the `pending_*` keys) is keyed by the ACP id, so the panel's lookups only work if the started event carries the ACP id — never the placeholder.
5. Look up the `LiveSession` in `driver.sessions` (by the ACP id), clone the (cheap) `cx`; send the task as the first `session/prompt` (UNBOUNDED await — no timeout; cancellation is the `ExternalClose`).
6. On `end_turn`: `output` = the accumulated text of the `last_message_id` (the last-seen `messageId` from the driver's `last_message_id` — NOT `HashMap` iteration order; a subagent with no text → empty string); `metrics` from `cost_capture` (the last `cost_update` payload: `inputTokens`/`outputTokens`/`cost`, defaulting to 0 — see the wire contract's v1 metrics note) + `duration_ms` (wall clock since step 1).
7. Close the session (set the `ExternalClose` kind `User` + flip its flag — the driver task tears down: process group, bridge listener, socket unlink, `session-closed` emit); the worker task unlinks the wrapper file (it owns the path; the driver teardown does not).
8. Emit `subagent-closed` `{ sessionId: <ACP id>, status: "completed", metrics }`; resolve the oneshot with `Completed`.
- Cancellation (the `SubagentCancel` handle fires, or the prompt fails because the session closed): UNLINK THE WRAPPER FILE (the worker task is still alive — its prompt await failed — so it unlinks before resolving; one leaked `wrapper-<uuid>.sh` per cancelled dispatch otherwise), emit `subagent-closed` `{ status: "failed", error: "cancelled", metrics }` (or the prompt's error message), resolve with `Failed`.

`SubagentCancel`: keeps the `ExternalClose`'s `tx` + `kind`; `cancel()` sets the kind `User` (first-set-wins, mirroring `LiveSession`'s close plumbing) then flips the flag — the driver task's select arm (establish + block phases) tears the session down. `cancel()` is idempotent.

`respond` (direct map access — callable from ANY runtime; the maps are `tokio::sync::Mutex`, locked briefly):
```rust
/// Resolve a pending request of a SUBAGENT session (its own bridge
/// listener's map). `false` when the entry is gone.
pub async fn respond_bridge_request(&self, session_id: &str, request_id: &str, result: Value) -> bool
pub async fn respond_permission(&self, session_id: &str, request_id: &str, outcome: PermissionOutcome) -> bool
```
Note: the `session_id` here is the subagent's **ACP** id (the listener's `set_session_id` ran on the ACP id after establishment) — the command-side routing (section 4) passes the incoming `session_id` through unchanged; a miss on both managers is the existing silent no-op.

**3. `bridge.rs` — the `dispatch_subagent` method.** In `handle_connection`, the request branch currently registers a `pending_bridge` oneshot + emits `bridge-request` + waits (330 s cap). For `method == "dispatch_subagent"` the behavior differs:
- NO `pending_bridge` entry (the desktop answers it itself — not the user); NO `bridge-request` UI event (the panel is fed by `subagent-session-started`, not a user prompt).
- The waiter spawns the subagent dispatch and `select!`s on: `dispatch_rx` (→ write the response frame: `Completed` → `{ "result": { "output", "metrics" } }`, `Failed` → `{ "error": <msg> }`) / `close_rx` (parent close) / `drain_until_eof` (agent EOF) → `cancel.cancel()` + write the terminal `error: "cancelled"` frame. NO timeout arm for this method (the `timeout` field is ignored when `method == "dispatch_subagent"`; all other methods keep the existing 330 s behavior — the existing tests prove it).
- `ConnCtx` gains `subagent: Option<SubagentSpawn>` — a cheap, `'static`-safe handle: `SubagentSpawn { manager: Arc<SubagentSessionManager>, parent_cwd: PathBuf, parent_agent_id: String }` (an `Arc`, NOT a `&` — `ConnCtx` is moved into `tokio::spawn` and must be `'static`). The waiter builds the dispatch call from it: `spawn_task` on `subagent.manager` with `(parent_session_id = the `ConnCtx` session-id state — the parent's ACP id after `set_session_id`, `parent_cwd`, `parent_agent_id`, `launch` (built from the frame's `params`: `agentName`/`task`/`systemPrompt`/`model`/`thinking`/`tools`), `task`, the sink)`. `None` when the listener was started without a subagent manager (tests, non-bridge setups) — a `dispatch_subagent` frame on such a listener gets the existing unknown-method `error` response.
- `ConnCtx` also gains `cost_capture: Option<Arc<StdMutex<Option<Value>>>>` — the push branch stores the `cost_update` payload there when `Some` (the subagent's listener sets it; main listeners pass `None`).
- `start_listener` gains the two new parameters (`subagent: Option<SubagentSpawn>`, `cost_capture: Option<…>`; both `None`-defaulted in practice) — **the existing call sites must be updated mechanically** (Rust has no default arguments): `bridge_integration.rs`'s direct `start_listener` call and the in-file `spawn_server` test helper (which builds `ConnCtx` as a struct literal) gain the `None` fields. The `SessionDriver` passes its own `subagent` handle + the session's `cwd` + `agent_id` into `start_listener` (all three are already in `drive_session`'s scope), so the main session's listener can service `dispatch_subagent` frames. Two subagent-specific call-site details: (a) the subagent's `start_listener` call receives the `ExternalClose` channel (NOT the driver's internal `close_tx`) — so a cancel also cancels the subagent's in-flight `ask` waiters via their `close_rx` arm (the driver-task map drain is the backstop); (b) the wrapper file is written into `socket_path.parent()` (the `archimedes-bridge-<uid>` dir that `bridge_socket_path()` places the socket in — `bridge_socket_path` returns the SOCKET path, not the dir).

**4. Routing + wiring.**
- `commands/sessions.rs`: `respond_bridge_request` and `respond_permission` — after the main manager lookup misses (no entry), try the `SubagentSessionManager` (managed state) with the same args; return success if EITHER resolved (the existing "silent no-op when gone" semantics stay).
- `lib.rs` (`setup_dirs`): (1) create the `SubagentSessionManager` (it owns its `WorkerRuntime` — built in `new()` at app setup; two idle threads, negligible); (2) inject it into the main `SessionManager` via a new setter `SessionManager::set_subagent_manager(&mut self, m: Arc<SubagentSessionManager>)` (sets `self.driver.subagent = Some(m)`); (3) manage BOTH `Arc<SessionManager>` and `Arc<SubagentSessionManager>`. The injection order breaks the apparent cycle: the subagent manager needs nothing from the main manager; only the main manager's `SessionDriver.subagent` field points at it (intra-crate type cycles are fine in Rust).
- App exit: dropping the `SubagentSessionManager` drops the `WorkerRuntime` — `Drop` sends the shutdown signal; the dedicated thread drops the `Runtime` on itself (no async-context drop panic, Task 1), the worker tasks are aborted, the driver tasks' `cx` drops close the connections, and on Unix the ACP SDK kills the agent's process group (verified in Task 6's app-exit test).

**Steps:**
- [ ] Write the extraction first, TDD-style against the EXISTING tests: refactor `session.rs` into `SessionDriver` + delegating `SessionManager` (db: `Some`, captures: `None`, subagent: `None`), including the visibility changes (`LiveSession` + fields `pub(crate)`, `drive_session` `pub(crate)`, `bridge_socket_path`/`bridge_spawn_setup` `pub(crate) free functions`) and the three driver additions ((a) `text_capture` + `last_message_id` in the notification handler, (b) `cost_capture` in the push branch, (c) the `external_close` select arm + kind swap).
- [ ] Run `cd src-tauri && cargo test` — did ALL existing tests pass unchanged (the extraction is behavior-preserving)? If any failed, fix before continuing.
- [ ] Write failing unit tests in `subagent.rs`: (1) `SubagentCancel` — `cancel()` sets the kind `User` (first-set-wins) + flips the flag the driver selects on; idempotent; (2) `respond_bridge_request` resolves an entry in the subagent's `pending_bridge` map and returns `false` for a missing key (same for `respond_permission`); (3) the `text_capture`/`last_message_id` pair: drive a `fake_agent` main-style session through a `SessionDriver` with both `Some`, with the fake agent emitting TWO distinct `messageId`s (e.g. `m1` then `m2`) — assert the per-`messageId` texts AND that `last_message_id` is `m2` (the final-output source, NOT `HashMap` iteration order); (4) the `external_close` arm: a driver task with `external_close: Some` tears down when the external flag flips DURING the establish phase (the fake agent's `hang` mode holds `session/new` open; flip the flag; assert the `session-closed` reason is `user` — the external kind won).
- [ ] Run `cd src-tauri && cargo test subagent` — confirm the new tests FAIL (manager missing), the existing ones still pass.
- [ ] Implement `subagent.rs` (the manager) + the `bridge.rs` `dispatch_subagent` branch + `ConnCtx` `subagent`/`cost_capture` fields + the `start_listener` parameters — and mechanically update the two existing call sites (`bridge_integration.rs`'s direct call, the in-file `spawn_server` helper) with the new `None` fields.
- [ ] Run `cd src-tauri && cargo test` — did all tests pass (existing + new unit)? (The two call-site updates are expected to be the only changes to existing test code; every existing ASSERTION stays verbatim.)
- [ ] Run `cd src-tauri && cargo clippy --all-targets` — 0 warnings? `cargo fmt --check` — clean?
- [ ] Commit with message: "feat: SubagentSessionManager (worker runtime) + dispatch_subagent bridge method"

**Acceptance criteria:**
- [ ] The `SessionDriver` extraction is behavior-preserving: the FULL existing `cargo test` suite passes with verbatim assertions (only the two call sites gain `None` fields).
- [ ] `SubagentSessionManager::dispatch` runs the whole lifecycle on the worker runtime (no `block_on`; the `spawn_task` oneshot is the only handoff); the subagent spawns the parent's registry entry (`parent_agent_id`).
- [ ] `subagent-session-started` carries the ACP `session_id` (emitted AFTER establishment) and the parent's ACP id; `subagent-closed` carries the ACP id + status + metrics; on establish failure no `subagent-*` events fire.
- [ ] `dispatch_subagent` has no timeout arm; every other bridge method keeps the 330 s behavior (existing `bridge.rs` tests pass unchanged).
- [ ] `respond_bridge_request` / `respond_permission` route to the subagent manager on a main-manager miss.
- [ ] Cancellation is honored in the establish phase (external close arm) and the block phase; the kind is first-set-wins `User`.
- [ ] Clippy 0 warnings, fmt clean.

---

### Task 4: Frontend — the subagent side panel

**Context:** The right-rail panel (Section 5 of the spec): one collapsible entry per subagent session, each a compact view of the SAME ACP stream (the `session-update` events for the subagent's session id already flow through the existing `useSessions.applySessionUpdate` — the messages accumulate in `useSessions.messages[sessionId]` with zero new plumbing), plus the state indicator (the `state` push, already stored in `useBridge.agentState[sessionId]`), the cost (the `cost_update` push, already stored in `useBridge.cost[sessionId]`), pending permission prompts (`usePermissions.prompts[sessionId]` — badge + auto-expand), and ask cards (`useBridge.requests[sessionId]`). New plumbing: the `subagent-session-started` / `subagent-closed` events (a small store) and the panel component itself.

**Files:**
- Create: `src/store/subagents.ts`
- Create: `src/components/SubagentPanel.tsx`
- Create: `src/components/SubagentPanel.test.tsx` (Vitest + React Testing Library — follow the `AskQuestionCard.test.tsx` pattern)
- Create: `src/store/subagents.test.ts`
- Modify: `src/App.tsx` (wire the two new listeners into the store; both inside the existing `useEffect` unlisten-cleanup pattern)
- Modify: `src/lib/tauri.ts` (add `listenSubagentSessionStarted` + `listenSubagentClosed` — follow the existing `listenBridgeEvent` pattern)
- Modify: `src/components/ChatStream.tsx` (add `<SubagentPanel />` as a flex child in BOTH return paths — the empty-state path and the main path, alongside `TodoBoardPanel`; the subagent panel is a sibling, not nested)

**What to implement:**

`src/store/subagents.ts`:
```ts
export interface SubagentMetrics {
  inputTokens: number;
  outputTokens: number;
  cost: number;
  durationMs: number;
}

export interface SubagentEntry {
  sessionId: string;
  parentSessionId: string;
  agentName: string;
  task: string;
  status: "running" | "completed" | "failed";
  error?: string;
  /** Snapshot from the `subagent-closed` payload (see the data-lifecycle note below). */
  metrics?: SubagentMetrics;
}

interface SubagentState {
  /** Keyed by the subagent session id. */
  entries: Record<string, SubagentEntry>;
  addSession: (e: SubagentEntry) => void;          // "subagent-session-started"
  markClosed: (sessionId: string, status: "completed" | "failed", error?: string, metrics?: SubagentMetrics) => void; // "subagent-closed"
  dismiss: (sessionId: string) => void;           // user dismiss (panel header)
}
```

**Data-lifecycle note (reviewed — do not read `useBridge.cost` at close time):** the existing `App.tsx` `listenSessionClosed` handler calls `useBridge.dismissSession(sessionId)`, which DELETES `cost`/`agentState`/`requests`/`todos` for that id — and the `session-closed` (driver teardown) and `subagent-closed` (worker task) events are CONCURRENT (the worker task flips the close flag and emits `subagent-closed` without awaiting the driver task's teardown, so the ordering is racy — do not "fix" it, the snapshot makes it moot). So the metrics line MUST read `useSubagents.entries[sessionId].metrics` (the snapshot carried in the `subagent-closed` payload — v1: `{0, 0, 0, <real durationMs>}`, see the wire contract's metrics note) — `useSubagents` is the only store `dismissSession` never touches — never `useBridge.cost[sessionId]`. Also correct the expectation that `handleSessionClosed` for an unknown id is a no-op: it is mostly harmless but it writes `closeReasons` and finalizes `messages` entries — the subagent store is the authority for subagent STATUS; the bridge-store deletion is exactly why the snapshot exists.

`src/components/SubagentPanel.tsx` — collapsible right-rail panel (same chrome family as `TodoBoardPanel`: a fixed-width rail, collapsible to a slim bar with a count badge):
- No entries → render nothing (the rail collapses; `TodoBoardPanel` keeps its own visibility rule).
- One section per entry (arrival order): a header row — `agentName` (the label; the subagent's own `ask` cards render in its stream and the header provides the name, since their `source` is `"main"` from the subagent's own session id), a state chip from `useBridge.agentState[sessionId]` (`working`/`idle`/`blocked` — the `state` push finally rendered), the status (`running`/`completed`/`failed` from `useSubagents`), a dismiss button (calls `dismiss`).
- A pending permission prompt for this session (`usePermissions.prompts[sessionId]` non-empty) → a badge on the header + the panel auto-expands (a `useEffect` that sets the expanded state when the badge count goes 0 → >0). The prompt itself renders in the section's stream (reuse the existing `PermissionPrompt` component — it is session-id-keyed and already generic).
- The compact stream: `useSessions.messages[sessionId]` rendered in a condensed container (max-height + scroll; `MessageBubble` for `agent-text`; `ToolCallCard` for `tool-call` — it is collapsed by default already; `diff` entries via the existing `DiffBlock` path). Condensed = smaller font/spacing (Tailwind classes), NOT a new message renderer.
- Ask cards: `useBridge.requests[sessionId]` filtered to `method === "ask"` → `AskQuestionCard` (the existing component).
- **`confirm`/`password` modals (reviewed — the "no change" assumption is WRONG):** `SudoConfirmModal`/`SudoPasswordModal` are rendered by `ChatStream` ONLY for the ACTIVE session's requests. A subagent session's `sudo_exec` confirm/password requests are keyed by the subagent's session id and would render nowhere — the request would hang until the 330 s bridge timeout. **`SubagentPanel` renders `SudoConfirmModal`/`SudoPasswordModal` for its entries' pending `confirm`/`password` requests** (`useBridge.requests[sessionId]` filtered to those methods; they are `fixed` overlays, so rendering them from the panel is fine). The answer path is unchanged: the modals call `respondBridgeRequest(sessionId, requestId, …)`, which routes to the subagent manager (Task 3).
- Metrics line on close: `useSubagents.entries[sessionId].metrics` (the `subagent-closed` snapshot — see the data-lifecycle note above) + the `status`/`error` from `useSubagents`.
- Auto-expand on dispatch: a `useEffect` on `Object.keys(entries).length` going 0 → >0 sets the expanded state.

`src/App.tsx`: inside the existing `useEffect` (the one with the `unlistenPromises` cleanup), add:
```ts
unlistenPromises.push(
  listenSubagentSessionStarted((p) =>
    useSubagents.getState().addSession({
      sessionId: p.sessionId, parentSessionId: p.parentSessionId,
      agentName: p.agentName, task: p.task, status: "running",
    }),
  ),
);
unlistenPromises.push(
  listenSubagentClosed((p) =>
    useSubagents.getState().markClosed(p.sessionId, p.status, p.error, p.metrics),
  ),
);
```
(The existing `listenSessionClosed` handler stays as-is — see the data-lifecycle note above for why the metrics snapshot exists. The `error`/`metrics` payload fields are optional; type the listener payload accordingly.)

**Steps:**
- [ ] Write failing tests: `src/store/subagents.test.ts` (addSession/markClosed/dismiss transitions; markClosed stores the metrics snapshot) + `src/components/SubagentPanel.test.tsx` (no entries → nothing rendered; one running entry → header with agentName + state chip; a pending permission prompt → badge + auto-expand; a closed entry → metrics line + status, with the metrics coming from `useSubagents` (NOT `useBridge.cost` — assert the metrics still render after a `useBridge.dismissSession` for the same id); an `ask` request for the subagent session id → `AskQuestionCard` rendered in the section; a `confirm` request for the subagent session id → `SudoConfirmModal` rendered by the panel (the `ChatStream` active-session modals do NOT render it).
- [ ] Run `pnpm test` — confirm the new tests FAIL (modules missing).
- [ ] Implement `src/store/subagents.ts`, `src/lib/tauri.ts` listeners, `src/components/SubagentPanel.tsx`, the `App.tsx` wiring, the `ChatStream.tsx` flex child (BOTH return paths).
- [ ] Run `pnpm test` — did all tests pass?
- [ ] Run `pnpm build` — did tsc + vite succeed?
- [ ] Commit with message: "feat: subagent side panel (compact same-pipeline view, state indicator, metrics)"

**Acceptance criteria:**
- [ ] The panel renders one section per subagent session, fed by the EXISTING session/bridge/permission stores keyed by the subagent session id (no new IPC beyond the two new event listeners).
- [ ] Auto-expand on dispatch (0 → >0 entries) and on a pending permission prompt (badge 0 → >0).
- [ ] `SubagentPanel` renders `SudoConfirmModal`/`SudoPasswordModal` for its entries' pending `confirm`/`password` requests (reusing the existing components — `ChatStream` renders them only for the active session); the answer path routes to the subagent manager.
- [ ] `pnpm test` + `pnpm build` green.

---

### Task 5: pi-archimedes — the bridge-mode dispatch branch (suite side)

**Context:** The suite's `subagent` tool currently forks a child pi process (`spawnSubagent`). In bridge mode (the suite's `getBridge().active` — env-gated, root-only, evaluated at `session_start`), it must instead send a `dispatch_subagent` bridge request (the wire contract at the top of this plan) and map the response to a `SubagentResult`. A UNREACHABLE bridge (connect fails — e.g. macOS, where the desktop's listener is not started, or Windows, where it is a no-op stub) falls back to the existing fork path, so subagents keep working everywhere. The channel's `request()` has a hard 5-minute timeout; `dispatch_subagent` must opt out of it (the desktop's lifecycle — parent close / EOF / app exit — is the cancel path; the tool's `AbortSignal` wiring mirrors `ask`).

**Files:**
- Modify: `packages/core/src/bridge/channel.ts` (`request()` gains an optional timeout override + the `BridgeTransportError` class)
- Modify: `packages/core/src/bridge/index.ts` (add `dispatch` to the bridge API)
- Create: `packages/subagent/src/dispatch.ts`
- Modify: `packages/subagent/src/execute.ts` (the bridge branch; `ExecuteOptions` gains `toolCallId?: string`)
- Modify: `packages/subagent/src/spawn.ts` (extract `resolveModel` — the model resolution `options.agent?.model ?? options.model ?? options.activeModel` moves into a shared exported helper used by BOTH the fork and the bridge paths)
- Modify: `packages/subagent/src/index.ts` (the tool's `execute` passes its `_id` argument as `toolCallId` into the single-mode `executeSubagent` call AND into `executeParallel`'s task defs — `ExecuteOptions`/the parallel task type gain the optional `toolCallId` field; `undefined` → the frame's `toolCallId` is absent, which the frame format allows)
- Test: `packages/subagent/src/dispatch.test.ts`
- Modify: `packages/core/src/bridge/index.test.ts` (the no-timeout `request` case + the `BridgeTransportError` case)

**What to implement:**

`channel.ts`: `request<T>(method, params, opts?: { toolCallId?: string; source?: string; timeoutMs?: number | null })` — `timeoutMs: null` = NO timeout (skip the timer entirely); `timeoutMs: undefined` = the existing 5-min default. PLUS a new exported class:
```ts
/** A transport-level failure: the connection never delivered a response
 *  frame (unreachable socket / ECONNREFUSED, a mid-connection error, or a
 *  clean close before any response — the desktop is effectively gone).
 *  Distinct from a response-carrying error (the desktop answered with an
 *  `error` — it is alive and the outcome is authoritative). */
export class BridgeTransportError extends Error { /* … */ }
```
The error/close-before-response handlers reject with `BridgeTransportError`; the `"bridge channel unreachable"` (no socket target) case rejects with it too; a response frame carrying `error` STILL rejects with a plain `Error(msg.error)` — the distinction is the fallback trigger (below). The `cancel()` surface is unchanged (socket destroy → the promise rejects with a `BridgeTransportError`).

`index.ts`:
```ts
export interface DispatchParams {
  agentName: string;
  task: string;
  systemPrompt: string | null;
  model: string | null;
  thinking: string | null;
  tools: string[] | null;
}
export interface DispatchResult {
  output: string;
  metrics: { inputTokens: number; outputTokens: number; cost: number; durationMs: number };
}
export function dispatch(params: DispatchParams, toolCallId: string, signal?: AbortSignal): Promise<DispatchResult>
```
`dispatch` = `channel.request<DispatchResult>("dispatch_subagent", params, { toolCallId, source: "main", timeoutMs: null })` + the SAME abort wiring as `ask` (an already-aborted signal cancels immediately; the listener is removed on settle via the `.then(onFulfilled, onRejected)` form — copy the pattern from `ask`). Throws `BridgeInactiveError` when inactive (same as `ask` — the subagent package catches the UNREACHABLE case, not inactivity: inactivity means non-bridge mode → the fork path is the normal path, so the branch in `execute.ts` gates on `getBridge().active` first).

`dispatch.ts` (new):
```ts
export type DispatchOutcome = SubagentResult | { fallback: true };

export function dispatchViaBridge(
  options: ExecuteOptions,
  signal: AbortSignal | undefined,
  toolCallId: string,
): Promise<DispatchOutcome>
```
**Field names (reviewed — the actual `ExecuteOptions` shape):** `options.agent` is the agent NAME string (`string | undefined`); the `AgentConfig` lives in `options.agentConfig` (`AgentConfig | undefined`). Build `DispatchParams` from `options` as:
- `agentName = options.agent ?? "subagent"`
- `systemPrompt = options.agentConfig?.systemPrompt?.trim() || null`
- `model = resolveModel({ model: options.model, activeModel: options.activeModel, agent: options.agentConfig }) ?? null` (the extracted helper — `agent.model > call model > activeModel`; `?? null` bridges `string | undefined` → `string | null`)
- `thinking = options.agentConfig?.thinking ?? null`
- `tools = options.agentConfig?.tools?.length ? options.agentConfig.tools : null`
`cwd` is intentionally NOT sent (the subagent session's cwd is the parent's Space folder — documented in the spec).
- `dispatch(...)` resolves → map to a `SubagentResult`: `exitCode: 0`, `finalOutput: output`, `usage: { input: metrics.inputTokens, output: metrics.outputTokens, cacheRead: 0, cacheWrite: 0, cost: metrics.cost, turns: 0 }`, `model: <the same resolved model, un-nullified>`, `progress` = a synthesized `SubagentProgress` (`status: "completed"`, `durationMs: metrics.durationMs`, zeros elsewhere — the same defensive synthesis `executeSubagent` already does), `progressSummary: { toolCount: 0, tokens: metrics.inputTokens + metrics.outputTokens, durationMs: metrics.durationMs }`.
- Rejects with a plain `Error` (a response-carrying error — the desktop answered): `error === "cancelled"` (the terminal frame) → a FAILED `SubagentResult` (`exitCode: 1`, `error: "cancelled"`, the same failed-progress synthesis as `executeSubagent`'s catch block); any other response error → a FAILED `SubagentResult` with the error message. (The desktop is alive; its outcome is authoritative — NO fork fallback.)
- Rejects with a `BridgeTransportError` (NO response frame — unreachable socket / dead desktop) → `{ fallback: true }` (the fork path keeps the subagent working; the main agent is still alive — its desktop connection died, not the agent).

`execute.ts`: at the top of `executeSubagent` (after the `agentName`/`startTime` setup):
```ts
if (getBridge().active) {
  const outcome = await dispatchViaBridge(options, options.signal, /* toolCallId */);
  if (outcome !== /* fallback */) return outcome; // SubagentResult — done
  // { fallback: true } — unreachable bridge: fall through to the fork path.
}
// …existing fork path (unchanged)
```
In bridge mode the fork path is NOT reached (except the fallback), so: skip `emitCostUpdate` when the bridge was used (double-counting the main agent's cost would be wrong; see the wire contract's v1 metrics note — the suite does not push the subagent's own usage). The `onUpdate` progress callback: in bridge mode the TUI has no surface (RPC mode) and the desktop panel is the progress surface — call `options.onUpdate` ONCE with a minimal "dispatched" placeholder (`status: "running"`, the task, zeros, `model`) so the pi-acp `tool_call_update` frames stay well-formed, then no further updates. The `toolCallId`: `ExecuteOptions` gains an optional `toolCallId?: string`; the tool's `execute` (index.ts) passes its `_id` argument for single mode, and `executeParallel`'s task defs gain the same optional field (all N tasks share the single subagent tool-call's `_id` — the frame carries it for correlation).

`spawn.ts`: extract
```ts
export function resolveModel(options: Pick<SpawnOptions, "model" | "activeModel"> & { agent?: AgentConfig | undefined }): string | undefined {
  return options.agent?.model ?? options.model ?? options.activeModel;
}
```
and use it in `spawnSubagent` (replacing the inline ternary — behavior identical) and in `dispatch.ts`.

**Steps:**
- [ ] Write failing tests: `packages/core/src/bridge/index.test.ts` — (a) a `request` with `timeoutMs: null` does NOT reject at 5 minutes (use `vi.useFakeTimers()` + advance 6 minutes + assert the promise is pending; the existing 5-min case stays green); (b) a mid-connection close (the server accepts then destroys the socket, no response) → the promise rejects with a `BridgeTransportError`; a response-carrying `error` frame → a plain `Error` (NOT `BridgeTransportError`). `packages/subagent/src/dispatch.test.ts` — a fake bridge server (a `net` server on a temp socket, the pattern from `index.test.ts`'s end-to-end section): (1) success mapping (the server replies `{ result: { output: "done", metrics: { inputTokens: 1, outputTokens: 2, cost: 0.01, durationMs: 5 } } }` → `SubagentResult` with `exitCode 0`, `finalOutput "done"`, the usage/metrics mapped, the synthesized progress); (2) the terminal `error: "cancelled"` frame → a FAILED `SubagentResult` with `error "cancelled"` (NOT a fallback — the desktop answered); (3) an unreachable socket (no server → ECONNREFUSED → `BridgeTransportError`) → `{ fallback: true }`; (4) `resolveModel` precedence (agent.model > call model > activeModel; all undefined → undefined).
- [ ] Run `cd /home/daniel/Coding/Javascript/pi-archimedes && pnpm test` — confirm the new tests FAIL.
- [ ] Implement: the `channel.ts` timeout option + `BridgeTransportError`, the `index.ts` `dispatch`, `dispatch.ts`, the `execute.ts` branch (+ `toolCallId` field), the `index.ts` `_id` threading (single + parallel), the `spawn.ts` `resolveModel` extraction.
- [ ] Run `pnpm test` — did all tests pass (new + existing — the fork path's existing tests must stay green)?
- [ ] Run `pnpm -r exec -- tsc --noEmit` — clean?
- [ ] Commit with message: "feat(subagent): bridge-mode dispatch branch (dispatch_subagent + fork fallback)"

**Acceptance criteria:**
- [ ] Bridge mode: the tool dispatches over the bridge (one request per task; `executeParallel` needs no change — N `executeSubagent` calls → N dispatches, each sharing the tool-call `_id`).
- [ ] Fallback rule: `BridgeTransportError` (no response frame) → the existing fork path runs (subagents work on macOS/Windows as today); a response-carrying error (incl. `cancelled`) → a FAILED `SubagentResult`, never a fallback.
- [ ] The 5-min timeout does NOT apply to `dispatch_subagent` (fake-timer test); the tool's `AbortSignal` cancels the request (the socket close → the desktop's EOF → the subagent session cancels).
- [ ] `pnpm test` + `pnpm -r exec -- tsc --noEmit` green (the `agent`/`agentConfig` field shapes type-check).

---

### Task 6: End-to-end integration test (fake agents, full Rust path) + full validation

**Context:** The E2E proves the whole desktop path with fake agents (no real inference needed in CI): a fake MAIN agent (a `fake_agent` mode that opens a bridge connection and sends a `dispatch_subagent` frame) + a fake SUBAGENT agent (a `fake_agent` mode that answers `session/prompt` with a chunk + `end_turn`). The test asserts the response frame carries the subagent's final output, the `subagent-session-started` / `subagent-closed` events fire, the subagent session is torn down (process reaped), and the cancellation path (the main agent closes its bridge connection mid-dispatch) kills the subagent session. Peer verification works in-test: the fake agents are spawned by the test process (the "desktop"), so their parent chain reaches the anchor (1 hop).

**Files:**
- Modify: `src-tauri/src/bin/fake_agent.rs` (two new modes: `dispatch` and `subagent`; the `subagent` mode also gets a `hang` variant via the existing `FAKE_SESSION_ID`-style env — see below)
- Create: `src-tauri/tests/subagent_dispatch.rs`
- Modify: `src-tauri/tests/subagent_concurrency.rs` (add the app-exit teardown assertion — see Step 4)

**What to implement:**

`fake_agent.rs` — two new modes (the existing mode dispatch is by the first positional arg), plus the mode-override rule:
- **Mode rule (reviewed):** when `env PI_ACP_PI_COMMAND` is SET (the desktop sets it ONLY for subagent spawns, via the wrapper), the fake agent behaves as the subagent agent REGARDLESS of the positional arg (the registry `args: ["dispatch"]` apply to both spawns — the desktop uses the same registry entry for the subagent). Unset → the positional-arg mode. The `subagent-hang` variant: `FAKE_SUBAGENT_MODE=hang` in the registry `env` (default `subagent`).
- `subagent`: `session/new` → `{ sessionId }` with a DISTINCT id — `fake-subagent-1` by default in subagent mode (overridable via `FAKE_SUBAGENT_SESSION_ID` env; the main agent's default `fake-session-1` must NOT be reused — both sessions would otherwise emit `session-update`/`session-closed` under the same key and the assertions would be indistinguishable; in production pi ids are unique). `session/prompt` → one `agent_message_chunk` (messageId `m1`, text `subagent-done`) + `stopReason: "end_turn"`. (The subagent's bridge env vars are set by the desktop; the fake agent ignores them — it never connects — EXCEPT test 3, where it connects to its OWN `PI_ARCHIMEDES_BRIDGE_SOCKET`.)
- `subagent-hang`: `session/new` → `{ sessionId }` (the same distinct id); `session/prompt` → answered with ONE chunk then never responds (the prompt request stays open forever — the cancellation target).
- `dispatch`: reads env `PI_ARCHIMEDES_BRIDGE_SOCKET` (the socket path — the desktop's `bridge_spawn_setup` sets it per spawn to THIS session's own listener; it is the main agent's bridge, exactly the production topology. Do NOT invent a `FAKE_BRIDGE_SOCKET` registry env — the per-spawn socket path is generated inside `start_session` and unknowable from a static `agents.json`) and `FAKE_DISPATCH_TASK` (the task text the frame carries — a static value, fine in the registry `env`). On `session/prompt`: connect to `PI_ARCHIMEDES_BRIDGE_SOCKET` (a bare Unix socket — plain `std::net`), write the frame `{ "v": 1, "type": "request", "id": "fake-dispatch-1", "method": "dispatch_subagent", "source": "main", "params": { "agentName": "fake", "task": <FAKE_DISPATCH_TASK>, "systemPrompt": null, "model": null, "thinking": null, "tools": null } }` + `\n`, read the response line (blocking, the connection stays open until the first data — the desktop's contract), then echo `dispatch:<result-or-error>` as a chunk + `end_turn`. The `dispatch-cancel` variant: write the frame, then close the connection WITHOUT reading (simulates the main agent's abort).

`subagent_dispatch.rs` (the test — the registry entry points at `fake_agent`, `bridge: true`, `env` carrying ONLY `FAKE_DISPATCH_TASK` + `FAKE_SUBAGENT_MODE`; the subagent's spawn uses the SAME registry entry — the desktop spawns the subagent with the parent's registry entry (Task 3), and the `PI_ACP_PI_COMMAND` env (set only for subagent spawns) selects the subagent behavior per the mode rule above). **Test wiring (mirrors production):** build the `SubagentSessionManager` (same config dir), `main_manager.set_subagent_manager(subagent_manager)` (the Task 3 setter), then `start_session` the main. **Reaping assertions (reviewed):** main and subagent fake agents share the same binary AND the same `args` (the registry entry applies to both spawns), so `pgrep -f` cannot tell them apart — assert the process COUNT of matching processes goes 2 → 1 (the `find_fake_agent_pid` pattern from `acp_flow.rs` extended to a count), NOT "absent".

Tests:
1. **success**: `SessionManager` (main, the `dispatch` fake agent, `bridge: true`) + `SubagentSessionManager` (same registry; the subagent spawn uses the entry → the wrapper env → `subagent` mode) + `set_subagent_manager` wiring. `start_session` the main; `send_prompt` (the fake agent's `dispatch` mode fires the bridge frame → the desktop dispatches the subagent on the worker runtime → `subagent-done`). Assert: the main prompt resolves `end_turn` and its stream contains `dispatch:` + the `subagent-done` result text (the fake agent echoes the response frame's `result.output`); `subagent-session-started` emitted with the RIGHT `agentName`/`task` AND the subagent's ACP session id (`fake-subagent-1` — distinct from the main's `fake-session-1`); `subagent-closed` with `status: "completed"` + the metrics snapshot; the subagent's `session-closed` emitted (keyed `fake-subagent-1`); the fake-agent process COUNT goes 2 → 1 (the subagent reaped).
2. **cancellation**: the main fake agent in `dispatch-cancel` mode (sends the frame, closes without reading); the subagent in `subagent-hang` mode (the prompt never settles). Assert: the subagent session is torn down — `subagent-closed` with `status: "failed"` + `error "cancelled"`, the fake-agent process COUNT goes 2 → 1, and the main session stays live (the main prompt still resolves — the fake agent's `dispatch-cancel` mode answers `end_turn` after closing the bridge connection).
3. **the subagent's own bridge round-trip** (the ask path, no relay): the `subagent` fake agent mode, after `session/new`, connects to its OWN `PI_ARCHIMEDES_BRIDGE_SOCKET` (read from its env — the desktop set it for the subagent's spawn, so it is the subagent's own listener, NOT the main's) and sends a `request` frame (`method: "ask"`, `source: "main"`, an `id`); the test answers it via `SubagentSessionManager::respond_bridge_request` (the subagent manager's respond path — the command-LAYER main-miss→subagent-hit routing is a pass-through over this method, covered by the Task 3 unit test on the manager + the command's type-check; a plain tokio test cannot reach the Tauri command's managed state); the fake agent reads the response line and echoes it as a chunk before `end_turn`. Assert: the subagent session's stream (keyed `fake-subagent-1` — distinct from the main's `fake-session-1`) contains the echoed answer.
4. **app-exit teardown** (added to `subagent_concurrency.rs`): start a worker-runtime session, `worker.shutdown_and_join()` (the Task 1 method — signal + join; the `Runtime` is dropped on the dedicated thread, so NO tokio "cannot drop a runtime in an async context" panic), assert the fake-agent child is reaped (the `cx` drop closes the stdio; the ACP SDK kills the process group on Unix) within a few seconds.

**Steps:**
- [ ] Add the `subagent` / `subagent-hang` / `dispatch` / `dispatch-cancel` modes to `fake_agent.rs` (the `PI_ACP_PI_COMMAND`-set → subagent-behavior rule + `FAKE_SUBAGENT_MODE` for the hang variant + the DISTINCT `fake-subagent-1` session id in subagent mode; the `dispatch` mode's blocking bridge read of `PI_ARCHIMEDES_BRIDGE_SOCKET`).
- [ ] Write the four tests in `subagent_dispatch.rs` (+ the app-exit assertion in `subagent_concurrency.rs`).
- [ ] Run `cd src-tauri && cargo test --test subagent_dispatch` — confirm they FAIL (the fake modes / wiring missing or the path incomplete).
- [ ] Fix until green: `cargo test --test subagent_dispatch` AND `cargo test --test subagent_concurrency` — did all pass?
- [ ] Run the FULL validation gauntlet: `cd src-tauri && cargo test` (all), `cargo clippy --all-targets` (0 warnings), `cargo fmt --check`; `pnpm test` + `pnpm build` (repo root); `cd /home/daniel/Coding/Javascript/pi-archimedes && pnpm test` + `pnpm -r exec -- tsc --noEmit`.
- [ ] Commit with message: "test: subagent dispatch E2E (success, cancellation, subagent bridge round-trip, app-exit teardown)"

**Acceptance criteria:**
- [ ] All four E2E tests pass (success / cancellation / subagent's own ask round-trip / app-exit teardown).
- [ ] The full gauntlet is green in BOTH repos (desktop: `cargo test` + clippy 0 + fmt + `pnpm test` + `pnpm build`; suite: `pnpm test` + `tsc --noEmit`).
- [ ] Real-machine verification (this box, `pnpm tauri dev` — per the done-when): a real `subagent` tool call from a bridge-mode pi session spawns a desktop-managed subagent session; the panel shows its live stream; its `ask` renders in the panel and the answer flows directly to the subagent; parallel dispatch shows multiple panels; the tool returns combined results; non-bridge (terminal `pi`) subagent behavior is unchanged.

---

## Cross-cutting notes (for the executing agent)

- **Do NOT touch the one-live policy.** Subagent sessions never call `supersede_live_sessions` and never pass through `start_session`/`resume_session`. The policy code in `SessionManager` stays byte-identical in behavior (Task 3's full existing-test pass proves it).
- **Do NOT store subagent sessions.** No `record_session`, no DB rows, no `persist_update` DB writes for subagents (`SessionDriver.db` is `None` for the subagent manager; `text_capture` is the in-memory replacement).
- **Cancellation semantics are the existing ones.** Connection close = immediate cancel; an explicit terminal `error: "cancelled"` frame is written before close; the `pending_bridge` drain on session close is unchanged (dispatch requests are NOT in `pending_bridge` — the waiter owns them).
- **The suite's fork path is untouched** except the `resolveModel` extraction (Task 5) — TUI/headless/RPC-without-env behavior must stay byte-identical (the suite's existing test suite proves it).
- **Windows/macOS:** the bridge listeners are no-ops there (ADR 0003) → the suite's dispatch branch sees a `BridgeTransportError` (no response frame — the connect fails) and falls back to the fork (Task 5's fallback rule). The wrapper's `.cmd` variant exists for compilation completeness; it is not exercised in v1.
- **Every Rust commit** must pass `cargo clippy --all-targets` (0 warnings) + `cargo fmt --check`; **every frontend commit** `pnpm test` + `pnpm build`; **every suite commit** `pnpm test` + `pnpm -r exec -- tsc --noEmit`.
