---
status: committed
done-when: The `archimedes` UI process runs no `AgentLoop` — every session (main + subagent) runs in a self-exec `archimedes --worker` child process over the stdio JSONL protocol; a Worker crash degrades to a stalled (resumable) session or a `SubagentOutcome::Failed` tool error, never an app death; a crash writes `~/.local/share/archimedes/crash-<ts>-*.log`; all validations green (cargo test / clippy 0 warnings / fmt / pnpm test / pnpm build)
---

# Sessions in Worker processes — the desktop is a pure Supervisor — Plan

**Goal:** Every session (main + subagent) runs in its own child OS process (a **Worker process** — the desktop's own binary, self-exec `archimedes --worker`) running the existing `AgentLoop` over a stdio JSONL protocol; the Tauri app process becomes the **Supervisor** (UI + worker lifecycle + sole SQLite writer + the response side of the permission/interactive gates); a Worker crash degrades to a *stalled* (resumable) session or a `SubagentOutcome::Failed` tool error — never an app death.

**Architecture:** ADR 0025. The `AgentLoop` (the harness core) gains three trait seams (`Store`, `TrustSource`, `SubagentDispatcher`) so it runs DB-less and IPC-backed inside a Worker; the Worker's `Store` implementation (`IpcStore`) frames the loop's persistence calls over the protocol, and the Supervisor applies those frames to SQLite (sole writer — the transcript rows, including user/system/tool-role messages and compaction rewrites, are exactly what the loop persists today, transported over IPC instead of written directly). A new `agent/worker/` module holds the shared protocol, the Supervisor-side `WorkerHandle`/`WorkerManager`, and the Worker-side `run_worker()`; the `SessionManager`'s native session driver and the `SubagentSessionManager`'s in-process driver are re-plumbed onto the `WorkerManager` and then deleted (no dual-mode fallback).

**Tech Stack:** Rust (Tauri 2, tokio, rusqlite — all existing), React 19 frontend (banner only), stdio JSONL IPC (serde).

**Design reference:** `docs/decisions/0025-sessions-in-worker-processes.md` + the spec sections in the appendix below.

**Global rules for every task:**
- TDD: failing test first, confirm it fails, then make it pass.
- Validation after each task: `cargo test` + `cargo clippy --all-targets` (0 warnings) + `cargo fmt` in `src-tauri/`; tasks touching `src/` also run `pnpm test` + `pnpm build` at the repo root.
- One commit per task (squash-merge convention).
- The `AgentLoop`'s model loop, tool executors, provider clients, retry/compaction logic are NOT to be modified — **except two explicit exceptions**: (1) the `events` channel is changed from bounded (`mpsc::channel(256)`, `emit` at `loop.rs:1729-1739` uses `try_send` with silent drops) to **unbounded** (`UnboundedSender`/`unbounded_channel`). Rationale: in the Worker world the event stream drives the Supervisor's internal bookkeeping (settle detection, the `SubagentCapture`'s `agent_settled`-adjacent signals, the debug log) — a long streaming turn overflowing 256 and silently dropping those frames is unacceptable. `emit`'s `try_send` call is unchanged (on an unbounded sender it never fails — zero drops); the in-process construction sites (`session.rs:1483` and the like) switch to `unbounded_channel`. The `settle` watch and the `cancel`/`turn_cancel` tokens are untouched. (2) `AgentLoop`'s `prompt_tx`/`turn_cancel`/`settle_tx` fields gain `pub` visibility (the Worker's `build_loop` seam needs to keep the receivers/handles — Task 2's `LoopHandles`); the fields' semantics are unchanged.

---

### Task 1: Harness seams — `Store`, `TrustSource`, `SubagentDispatcher` traits + the `events` channel exception

**Context:**
The `AgentLoop` (`src-tauri/src/agent/harness/loop.rs`) must run inside a Worker process with **no DB** and **no in-process subagent dispatch**. Today it holds concrete types: `store: SessionStore` (a struct wrapping `Arc<Db>` — `harness/store.rs`), `trust_db: Option<Arc<Db>>` (used in **three** places — NOT one: (a) the permission gate's `space_trusted` lookup, threaded into `native_permission_gate` at `loop.rs:963-970` and consumed at `permission.rs:109-112`; (b) the context-usage persistence — `emit_context_usage` at `loop.rs:324` calls `db.record_session_context_usage(used, window)`; (c) the gate's **`trust-space` ("Don't ask again") flag write** at `permission.rs:170-186` (`d.set_space_trusted(...)`)), and `subagent: Option<Arc<SubagentSessionManager>>` (the in-process dispatch — `dispatch_subagent` at `loop.rs:1183` calls `manager.dispatch_native(...)` which returns `(oneshot::Receiver<SubagentOutcome>, SubagentCancel)`).

This task introduces the trait seams. **Key design (reviewer-corrected):** the Worker's store is NOT a no-op over the raw event stream — the event stream does not carry user messages, the system prompt (seq 0), or tool-role `ChatMessage`s (they are persisted only by the loop's direct store calls at `loop.rs:402`/`599`/`914`), and compaction rewrites the transcript via `replace_messages` (`loop.rs:1465`), which is not derivable from `compaction_start`/`compaction_end` events. Instead, the **`Store` seam's calls are framed over the protocol**: the Worker's `IpcStore` (Task 2) turns every store call into an outbound frame, and the Supervisor applies the frames to SQLite. The transcript the Supervisor persists is byte-for-byte what the loop persists today — just transported.

**Files:**
- Modify: `src-tauri/src/agent/harness/store.rs` (the `Store` trait + `NoopStore`; keep `SessionStore`)
- Create: `src-tauri/src/agent/harness/trust.rs`
- Create: `src-tauri/src/agent/harness/dispatch.rs`
- Modify: `src-tauri/src/agent/harness/loop.rs` (the 3 fields + call sites + the `events` channel type + the `pub` visibility exception)
- Modify: `src-tauri/src/agent/harness/mod.rs` (re-exports)
- Modify: `src-tauri/src/agent/session.rs` (the `persist_update` refactor — step 3)
- Test: in-file `#[cfg(test)]` modules (the codebase convention)

**What to implement:**

1. **`harness/store.rs`** — add:
   ```rust
   /// One display `messages`-table upsert (the `Db::record_message` shape).
   pub struct DisplayRow {
       pub kind: String,
       pub message_key: String,
       pub payload_json: serde_json::Value,
       pub created_at: i64,
   }

   /// The transcript persistence seam (ADR 0025): `SqliteStore` (the current
   /// `SessionStore`, unchanged behavior) / `NoopStore` (tests) / `IpcStore`
   /// (Task 2 — the Worker: frames every call over the protocol; the
   /// Supervisor applies the frames — the sole writer).
   pub trait Store: Send + Sync {
       fn insert_message(&self, session_id: &str, seq: u64, role: &str, content_json: &str) -> Result<(), DbError>;
       fn load_messages(&self, session_id: &str) -> Result<Vec<ChatMessage>, DbError>;
       fn clear_messages(&self, session_id: &str) -> Result<(), DbError>;
       fn replace_messages(&self, session_id: &str, rows: &[(u64, String, String)]) -> Result<(), DbError>;
       /// The display persistence. Takes PRE-COMPUTED rows (the loop keeps
       /// the `TurnState` accumulators — `text_acc`/`tool_state`/`thought_state`
       /// at `loop.rs:1721-1729`): `SqliteStore` writes them via
       /// `db.record_message` (the write half of today's `persist_update`),
       /// `IpcStore` frames them (`DisplayUpsert`), `NoopStore` ignores them.
       fn persist_display(&self, session_id: &str, rows: &[DisplayRow]) -> Result<(), DbError>;
       /// The context-usage persistence (the `sessions.context_usage_json`
       /// write — today `db.record_session_context_usage(used, window)`,
       /// `storage/db.rs:323-334`, called from `emit_context_usage` at
       /// `loop.rs:324` — the loop has `last_context_tokens: u64` +
       /// `model.context_window: u32`, so the signature matches the `Db`
       /// method's `(used: u64, window: u64)`).
       fn record_context_usage(&self, session_id: &str, used: u64, window: u64) -> Result<(), DbError>;
   }
   ```
   - `impl Store for SessionStore` — the 4 existing methods delegate verbatim; `persist_display` = the `db.record_message` write loop today's `persist_update` does (step 3 moves that write out of `persist_update` and into this method); `record_context_usage` = `self.0.record_session_context_usage(session_id, used, window)` (the existing `Db` method — read `storage/db.rs:323-334` for its exact signature).
   - `pub struct NoopStore;` — all 6 methods return `Ok(())` (never errors, never panics).
   - **Gating note (accepted drift):** today the context-usage write only happens when `trust_db` is `Some` (main sessions; subagents skip it). Unconditional-through-the-store means ephemeral subagent rows now also get `context_usage_json` writes — harmless (the persister's `ensure_session_row` creates the row; the display is per-session).
   - `AgentLoop` field `store: SessionStore` → `store: Arc<dyn Store>`; `new()` parameter likewise.

2. **`harness/trust.rs`** (new) —
   ```rust
   /// The TRUST lookup seam (ADR 0010/0025): `SqliteTrustSource` (the current
   /// `db.space_trusted` lookup — the gate's use (a), `permission.rs:109-112`)
   /// / `StaticTrustSource` (the Worker — the flag arrives in the `start`
   /// envelope, updated by `config` messages AND by the `trust-space`
   /// permission outcome — Task 2's `WorkerCore`).
   pub trait TrustSource: Send + Sync {
       fn is_trusted(&self, cwd: &std::path::Path) -> bool;
   }
   pub struct SqliteTrustSource(Arc<Db>);
   // `is_trusted` = the exact behavior of the current gate lookup (read
   // `permission.rs:109-112` + `storage/db.rs`'s `space_trusted` and move it
   // here verbatim; fail-closed semantics preserved by the `Option` at the
   // call site — `None` = always prompt).
   pub struct StaticTrustSource(std::sync::Arc<std::sync::Mutex<bool>>);
   impl StaticTrustSource {
       pub fn new(trusted: bool) -> Self;
       pub fn set(&self, trusted: bool);  // the `config { trusted }` update + the `trust-space` outcome flip
   }
   ```
   - `AgentLoop` field `trust_db: Option<Arc<Db>>` → `trust: Option<Arc<dyn TrustSource>>`; `new()` parameter renamed + typed likewise.
   - Call site (a) — `loop.rs:963-970`: the gate helper's `db: Option<&Arc<Db>>` parameter becomes `trust: Option<&dyn TrustSource>` (read `native_permission_gate` in `permission.rs` and thread the trait through the lookup).
   - Call site (b) — `loop.rs:324` (`emit_context_usage`'s `if let Some(db) = &self.trust_db { db.record_session_context_usage(used, window) }`): replaced by an UNCONDITIONAL `self.store.record_context_usage(&self.session_id, used, window)` (the store seam carries it — `SqliteStore` writes, `IpcStore` frames, `NoopStore` ignores; the `trust_db`-gating disappears with the field).
   - Call site (c) — the gate's `trust-space` flag write (`permission.rs:170-186`, `d.set_space_trusted(...)`): **removed from the gate** (the Worker has no DB; the write moves to the Supervisor's `respond_permission` relay — Task 4: when the outcome is `Selected { option_id: "trust-space" }`, the Supervisor calls `db.set_space_trusted(cwd, true)` AND the Worker's `StaticTrustSource` is flipped by the `PermissionResponse` handler — Task 2 — so the very next tool call auto-approves, matching today's live-lookup behavior). Read the current code and delete only the DB write — the option itself stays offered to the user.

3. **The `persist_update` refactor** (`session.rs:2607` + the loop call site `loop.rs:1721-1729`): today `persist_update(db, session_id, update, &text_acc, &tool_state, &thought_state)` computes the display rows (normalize + accumulator logic) AND writes them (`db.record_message` inside). Split it:
   - `pub fn compute_display_rows(update: &Value, text_acc: &..., tool_state: &..., thought_state: &...) -> Vec<DisplayRow>` — the pure row-computation half (the current `persist_update` logic minus the `db.record_message` calls — move it verbatim; keep it in `session.rs` or move to `harness/store.rs` — the loop imports it).
   - The loop call site becomes: `let rows = compute_display_rows(&update, &self.text_acc, &self.tool_state, &self.thought_state); if let Err(e) = self.store.persist_display(&self.session_id, &rows) { /* the current error handling */ }`.
   - `SqliteStore::persist_display` = the `db.record_message` write loop (the moved write half).

4. **`harness/dispatch.rs`** (new) —
   ```rust
   /// The subagent dispatch seam (ADR 0025): `InProcessDispatcher` (delegates
   /// to `SubagentSessionManager::dispatch_native` — the current behavior;
   /// after Task 5, `dispatch_native` delegates to the `WorkerManager`, so
   /// this is the test path) / `IpcDispatcher` (Task 2 — the Worker:
   /// `SubagentDispatch` / `SubagentResult` round-trip) / `MockDispatcher`
   /// (test double — the `SubagentWait` select tests).
   pub trait SubagentDispatcher: Send + Sync {
       /// Mirrors `SubagentSessionManager::dispatch_native`'s signature
       /// VERBATIM (same parameters, same `(oneshot::Receiver<SubagentOutcome>, SubagentCancel)` return).
       fn dispatch(
           &self,
           parent_session_id: &str,
           parent_cwd: &std::path::Path,
           parent_model: &Model,
           parent_enabled_tools: Vec<String>,
           agent_name: String,
           launch: LaunchConfig,
           task: String,
           sink: &Arc<dyn EventSink>,
       ) -> (tokio::sync::oneshot::Receiver<SubagentOutcome>, SubagentCancel);
   }
   pub struct InProcessDispatcher(Arc<SubagentSessionManager>);
   // delegates to `dispatch_native` verbatim.
   /// A test double for the `SubagentWait` select tests (the `loop.rs`
   /// in-file tests that exercise the dispatch SELECT — the outcome vs
   /// turn-cancel race — without a real dispatch): a canned
   /// `(oneshot, SubagentCancel)` queue the test resolves.
   pub struct MockDispatcher {
       pub outcomes: std::sync::Mutex<std::collections::VecDeque<(tokio::sync::oneshot::Sender<SubagentOutcome>, SubagentCancel)>>,
   }
   // `dispatch` pops the next queued pair (a `poison`-free default: an empty
   // queue returns a never-resolving oneshot + a no-op `SubagentCancel`).
   ```
   - `AgentLoop` field `subagent: Option<Arc<SubagentSessionManager>>` → `subagent: Option<Arc<dyn SubagentDispatcher>>`; `new()` parameter likewise. Call site `loop.rs:1183` (`dispatch_subagent`): `manager.dispatch_native(...)` → `dispatcher.dispatch(...)` (identical arguments). The `subagent.is_none()` checks at `loop.rs:1590`/`loop.rs:1606` are unchanged.
   - **NOT to change:** the `AgentLoop` model loop, the tool executors, the provider clients, retry/compaction, the `Prompt`/`ControlCmd` types, the `settle`/`cancel`/`turn_cancel` channels.

5. **The `events` channel exception** (the global rule): `AgentLoop` field `events: mpsc::Sender<RpcEvent>` → `events: mpsc::UnboundedSender<RpcEvent>`; `new()` parameter likewise; `emit` (`loop.rs:1729-1739`) unchanged (`try_send` on unbounded never fails — the drop path becomes dead code, delete it or keep it as a defensive no-op — read the current code and keep `emit`'s behavior identical minus the drop). The in-process construction sites (search `mpsc::channel(256)` / the `events` channel builds across `session.rs`/`subagent.rs`/the tests) → `mpsc::unbounded_channel()`.

6. **The `pub` visibility exception** (the global rule): `AgentLoop`'s `prompt_tx`, `turn_cancel`, `settle_tx` fields gain `pub` (the Worker's `build_loop` seam — Task 2's `LoopHandles` — needs to keep the `prompt_rx` receiver, the `turn_cancel` handle, and the `events_rx` receiver; `AgentLoop::new` consumes the receivers, so `build_loop` creates the channels, passes the senders into `new`, and keeps the handles). No behavior change.

7. **`harness/mod.rs`** — re-export `Store`, `DisplayRow`, `NoopStore`, `TrustSource`, `SqliteTrustSource`, `StaticTrustSource`, `SubagentDispatcher`, `InProcessDispatcher`, `MockDispatcher` (the `IpcStore`/`IpcDispatcher` re-exports are added in Task 2).

8. **Update every existing `AgentLoop::new` call site** (search `AgentLoop::new(` across `src-tauri/` — the session driver, the subagent driver, the in-file tests, the `tests/harness_*.rs` integration tests) to the new signature: `store: Arc<dyn Store>` (existing callers pass `Arc::new(SessionStore::new(db))` — the existing `SessionStore` value coerces), `trust: Option<Arc<dyn TrustSource>>` (`Some(Arc::new(SqliteTrustSource(db)))` where they passed `Some(db)`, else `None`), `subagent: Option<Arc<dyn SubagentDispatcher>>` (`Some(Arc::new(InProcessDispatcher(m)))` where they passed `Some(m)`, else `None`), `events: mpsc::UnboundedSender<RpcEvent>` (step 5).

**Steps:**
- [ ] Write failing tests in `harness/store.rs` `#[cfg(test)]`: `NoopStore` — all 6 methods return `Ok(())` (assert each); `SessionStore`-as-`Store` — `insert_message`/`load_messages` round-trip against a temp-dir `Db` (follow the existing `Db` test-fixture pattern in `storage/` tests); `persist_display` — a `Vec<DisplayRow>` lands as `messages` rows via `db.record_message` (assert the upsert idempotency — the existing `record_message` semantics); `record_context_usage` — lands in `sessions.context_usage_json` (mirror the existing `record_session_context_usage` test in `storage/`).
- [ ] Write failing tests in `harness/trust.rs`: `StaticTrustSource` — `new(false).is_trusted(..) == false`, `set(true)` flips it; `SqliteTrustSource` — a temp-dir `Db` with a trusted `spaces` row returns `true` (mirror the existing `space_trusted` test in `storage/`).
- [ ] Write failing tests in `harness/dispatch.rs`: `InProcessDispatcher::dispatch` on a `SubagentSessionManager` resolves the oneshot (reuse the existing `subagent.rs` test fixtures — the `dispatch_native` tests at the bottom of `subagent.rs` are the model; a minimal version suffices: assert the `SubagentCancel` handle is returned and the oneshot receiver is live); `MockDispatcher` — the test resolves the queued oneshot and the `SubagentCancel` flips (the `SubagentWait` select tests use it).
- [ ] Write a failing `persist_update`-refactor test (in `session.rs`): `compute_display_rows` on a canned normalized update + canned accumulators yields the same `Vec<DisplayRow>` the old `persist_update` wrote (a golden test — capture the old function's `db.record_message` calls in a test `Db`, assert the new compute+write path produces identical rows).
- [ ] Run `cargo test -p archimedes --lib agent::harness` — confirm the new tests FAIL (the traits don't exist yet).
- [ ] Implement (the order: `persist_update` split → the traits + `NoopStore` + `SqliteTrustSource` + `StaticTrustSource` + `InProcessDispatcher` + `MockDispatcher` → the 3 `AgentLoop` fields + `new()` parameters → the `events` channel exception → the `pub` visibility exception → the call sites, step 8).
- [ ] Run `cargo test` — ALL tests pass (the existing harness/subagent/session suites + the `tests/harness_*.rs` integration tests — their `AgentLoop::new` call sites updated per step 8; their BEHAVIOR is unchanged).
- [ ] Run `cargo clippy --all-targets` (0 warnings) + `cargo fmt`.
- [ ] Commit: `refactor(harness): Store / TrustSource / SubagentDispatcher seams + unbounded events channel (behavior-identical, ADR 0025 prep)`

**Acceptance criteria:**
- [ ] `AgentLoop` holds `Arc<dyn Store>`, `Option<Arc<dyn TrustSource>>`, `Option<Arc<dyn SubagentDispatcher>>`, `mpsc::UnboundedSender<RpcEvent>` — no `Arc<Db>`-typed store/trust field, no concrete `SubagentSessionManager` field; `prompt_tx`/`turn_cancel`/`settle_tx` are `pub`.
- [ ] The `trust_db`'s three uses are all accounted for: the gate lookup → `TrustSource`; the context-usage write → `Store::record_context_usage` (the `(used, window)` signature); the `trust-space` flag write → removed from the gate (the Supervisor applies it — Task 4; the Worker's `StaticTrustSource` flip — Task 2).
- [ ] The `persist_update` split is behavior-identical (the golden test passes: same rows, same upserts).
- [ ] The entire existing test suite passes (this task is behavior-identical).
- [ ] Clippy 0 warnings; `cargo fmt --check` clean.

---

### Task 2: The IPC protocol + the Worker main (`archimedes --worker`)

**Context:**
The Worker process is the desktop's own binary re-entered with a `--worker` flag: it runs one session's `AgentLoop` (Task 1's seams, Worker-backed — `IpcStore` + `StaticTrustSource` + `IpcEventSink` + `IpcDispatcher`) and speaks JSONL over stdio with the Supervisor. This task builds the protocol (shared wire types — both sides of the boundary), the Worker-side runtime (`run_worker`), the Worker-backed seam implementations, the crash-logging panic hook, and the release-profile `panic = "unwind"`. The Supervisor-side client is Task 3.

**The persistence transport (the architecture's load-bearing part):** the Worker's `IpcStore` frames every store call (`TranscriptInsert`/`TranscriptReplace`/`DisplayUpsert`/`ContextUsage` outbound frames); the Supervisor (Task 4's `TranscriptPersister`) applies them to SQLite. The event stream (`Outbound::Event`) drives the Supervisor's internal bookkeeping (settle detection, `pending_turn` resolution, the debug log) — NOT the UI; the UI's contract is the **sink frames** (`SinkFrame` — the normalized frames the loop's `emit` sends to the `EventSink`, re-emitted by the Supervisor verbatim). Together: the UI gets exactly the frames it gets today, and the transcript rows the Supervisor persists are byte-for-byte what the loop persists today.

**Files:**
- Create: `src-tauri/src/agent/worker/protocol.rs`
- Create: `src-tauri/src/agent/worker/sink.rs` (`IpcEventSink`)
- Create: `src-tauri/src/agent/worker/store.rs` (`IpcStore`)
- Create: `src-tauri/src/agent/worker/dispatch.rs` (`IpcDispatcher`)
- Create: `src-tauri/src/agent/worker/core.rs` (the testable message-handling core)
- Create: `src-tauri/src/agent/worker/mod.rs` (`run_worker` + re-exports)
- Create: `src-tauri/src/agent/crashlog.rs` (the shared panic hook — reused by the Supervisor in Task 6)
- Create: `src-tauri/tests/worker_smoke.rs` (the binary smoke test — INTEGRATION target: `env!("CARGO_BIN_EXE_archimedes")` is only defined for `tests/` targets, NOT for in-file `#[cfg(test)]` modules)
- Modify: `src-tauri/src/main.rs` (the `--worker` branch)
- Modify: `src-tauri/src/lib.rs` (export `run_worker`)
- Modify: `src-tauri/src/agent/events.rs` (add `Serialize` to `RpcEvent`'s derives — it currently derives only `Deserialize`; every variant's fields are `Value`/`String`/`u64`, so the derive is safe)
- Modify: `src-tauri/src/agent/subagent.rs` (add `Serialize`/`Deserialize` derives to `SubagentOutcome` + `SubagentMetrics` — check what's there; `SubagentOutcome` needs `Clone` if it lacks it — the wire + the client-side enum need both)
- Modify: `src-tauri/src/agent/mod.rs` (`pub mod worker; pub mod crashlog;`)
- Modify: `src-tauri/Cargo.toml` (release profile: `panic = "abort"` → `panic = "unwind"`)
- Test: in-file `#[cfg(test)]` modules + `tests/worker_smoke.rs`

**What to implement:**

1. **`protocol.rs`** — the wire vocabulary (serde, `rename_all = "camelCase"` on payload fields; the `type` discriminators snake_case — the same convention as `RpcEvent` in `agent/events.rs`; `Model`, `ModelCatalog`, `PermissionOutcome`, `ChatMessage`, `ImageRef` already derive serde — `catalog.rs:21`/`catalog.rs:79`/`permission.rs:48`/`provider.rs`/`tools/exec.rs:79` — use them verbatim as the wire shapes; `LaunchConfig` gets `Serialize`/`Deserialize` if it lacks them):
   ```rust
   /// Supervisor → Worker.
   #[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
   #[serde(tag = "type")]
   pub enum Inbound {
       /// The session-start envelope (handshake: spawn → the Worker emits
       /// `Ready` → the Supervisor sends `Start`).
       Start {
           session_id: String,
           cwd: String,                        // the Space's folder — `AgentLoop::new`'s `space_cwd` (the tool executors, the MCP project layer, the trust lookup, the main prompt's project context all need it)
           mode: StartMode,                   // Fresh | Resume
           transcript: Option<Vec<ChatMessage>>,  // Resume: the Supervisor-rehydrated provider transcript (the `native_messages` re-read — moved from harness to Supervisor)
           model: Model,                      // the RESOLVED model — `Model` already carries `api`/`base_url`/`api_key` (`catalog.rs:21`) and `build_provider(&Model)` uses exactly those (`provider.rs:2187`) — NO separate provider wire type
           catalog: ModelCatalog,             // the EFFECTIVE catalog (base + providers + discovery — the Supervisor owns it): the Worker's `Compactor` rebuild (`set_model`, `loop.rs:346`), compaction's `keep_recent_tokens` (`loop.rs:1402`), and the `subagent` tool's model-key pre-flight (`resolve_composed_model(&self.catalog, …)` — `loop.rs:1285`/`1296`, the ADR 0020/0023 soft-degradation) all need it
           thinking: Option<String>,
           trusted: bool,                     // the Space's trust flag (the Supervisor owns `spaces` — ADR 0010)
           enabled_tools: Option<Vec<String>>,   // the HARNESS convention on the wire (reviewer-corrected — unambiguous): `None` = all, `Some(v)` = exactly `v`, `Some(vec![])` = NO tools. The SETTINGS `[]` = all convention is mapped at `StartEnv` CONSTRUCTION (Task 4/3: `if settings.enabled_tools.is_empty() { None } else { Some(…) }`); the subagent path passes the three-way rule's `child_tools` VERBATIM as `Some(…)` (an empty `child_tools` = NO tools — NOT re-expanded). `build_loop` applies it directly: `loop_.set_enabled_tools(env.enabled_tools)` (NO mapping in `build_loop` — the round-2 `[]`→`None` mapping is gone; it would have re-expanded a legitimately-empty child tool set into ALL tools, defeating the recursion guard)
           config_dir: String,                // the `settings.json` home — the Worker reads it for MCP servers (ADR 0019) + the `subagentModels` override (ADR 0023); NOT for providers (those ride in `model`)
           subagent_enabled: bool,            // false for a subagent Worker (the recursion guard — `subagent`/`list_agents` are already excluded from `enabled_tools`)
           system_prompt: Option<String>,     // main sessions: `None` (the Worker builds the main prompt itself, step 6); subagent Workers: the `build_child_system_message(launch.system_prompt, has_todo_tool)` output — computed SUPERVISOR-side (it needs the resolved child tool list's `has_todo_tool` — the `launch` arrives in the `SubagentDispatch` frame), `prepend_system`ed by `build_loop`
       },
       Prompt { text: String, images: Vec<ImageRef> },  // the existing `Prompt` type's fields verbatim (`loop.rs:88-92` — `ImageRef` already serde-derives)
       Config { model: Option<Model>, thinking: Option<String>, trusted: Option<bool> },
       Abort,
       Close,
       PermissionResponse { id: String, outcome: PermissionOutcome },  // the REAL outcome (3-option UI: `Selected { option_id }` ∈ allow/reject/trust-space, or `Cancelled` — `permission.rs:48-53`); NOT a bool
       InteractiveResponse { id: String, value: serde_json::Value },
       SubagentResult { id: String, outcome: SubagentOutcome },
   }
   /// Worker → Supervisor.
   #[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
   #[serde(tag = "type")]
   pub enum Outbound {
       Ready { version: String, session_id: Option<String> },  // emitted at startup, BEFORE `Start` (`session_id: None`)
       Event { event: RpcEvent },          // the existing event vocabulary, verbatim — drives the Supervisor's INTERNAL bookkeeping (settle detection, `pending_turn` resolution, the debug log) — NOT the UI
       // The persistence frames (the `Store` seam over IPC — Task 1's `IpcStore` emits these):
       TranscriptInsert { session_id: String, seq: u64, role: String, content_json: String },  // `Store::insert_message` — the provider transcript rows (system prompt seq 0, user messages, assistant + tool-role messages — `loop.rs:402`/`599`/`914`)
       TranscriptReplace { session_id: String, rows: Vec<(u64, String, String)> },  // `Store::replace_messages` — the compaction rewrite (`loop.rs:1465`)
       DisplayUpsert { session_id: String, rows: Vec<DisplayRow> },  // `Store::persist_display` — the display `messages` rows
       ContextUsage { session_id: String, used: u64, window: u64 },  // `Store::record_context_usage` — the `sessions.context_usage_json` write (`Db::record_session_context_usage`'s signature)
       // The control frames — the FULL original event payloads VERBATIM (reviewer-corrected: the gate's `permission-request` payload carries `sessionId`, `requestId`, `request: { toolCall: { title }, options: [allow, reject, trust-space] }` — `permission.rs:135-158` — and the `interactive-request` payloads carry `method` (`ask`/`confirm`/`password`), `source`, `toolCallId`, `params` — `interactive.rs:941-953`/`1021-1033` — the frontend (`PermissionPrompt.tsx`/`store/permissions.ts`/`store/interactive.ts`) consumes exactly those fields; a lossy `{tool, args}` restatement cannot reproduce them):
       PermissionRequest { id: String, payload: serde_json::Value },  // `id` = the payload's `requestId` (the `PendingPermissions` map key — the same key the loop used); `payload` = the gate's `permission-request` sink payload VERBATIM
       InteractiveRequest { id: String, payload: serde_json::Value },  // `id` = the payload's request key; `payload` = the `interactive-request` sink payload VERBATIM
       SubagentDispatch { id: String, parent_session_id: String, parent_cwd: String, parent_enabled_tools: Vec<String>, agent_name: String, launch: LaunchConfig, task: String, model_key: String },  // the RESOLVED model key (the loop's pre-flight resolved it against the Worker's catalog — `loop.rs:1285`/`1296`); the Supervisor resolves the key → provider config (it owns the catalog + settings) → the child's `Start`
       SubagentCancel { id: String },
       SinkFrame { event: String, payload: serde_json::Value },  // the CATCH-ALL for every other `EventSink` emission — the ENTIRE UI contract (the `interactive-event` todo frames, `interactive-request-close` modal cleanup, the `session-update` context-usage display frames — the frontend contract is preserved by the Supervisor re-emitting `SinkFrame` as the same-named Tauri event verbatim)
       WorkerError { code: String, message: String, backtrace: Option<String> },
   }
   /// The `start` envelope as a standalone struct (Tasks 3/4/7 reference it):
   pub struct StartEnv { /* the `Inbound::Start` payload fields, verbatim */ }
   // `impl From<&StartEnv> for Inbound` (the `Start` variant) + `StartEnv::from_parts(...)`
   // for the Task-4/5 construction sites.
   /// The store-frame summary (Task 4's `TranscriptPersister` consumes it):
   pub enum StoreFrame {
       Insert { session_id: String, seq: u64, role: String, content_json: String },
       Replace { session_id: String, rows: Vec<(u64, String, String)> },
       Display { session_id: String, rows: Vec<DisplayRow> },
       ContextUsage { session_id: String, used: u64, window: u64 },
   }
   /// The `SubagentDispatch` payload as a standalone struct (Task 3's
   /// `dispatch_subagent` consumes it):
   pub struct SubagentDispatchWire { /* the `Outbound::SubagentDispatch` payload fields, verbatim */ }
   // Framing: `encode_line(&Inbound|&Outbound) -> String` (one JSON line, `\n`-terminated);
   // `decode_inbound(line) -> Result<Inbound, ProtoError>` / `decode_outbound(line)` —
   // PERMISSIVE of unknown `type` (an `Unknown { raw }` variant, mirroring
   // `RpcEvent::unknown` — never an error: the surface is unversioned).
   ```

2. **`crashlog.rs`** —
   ```rust
   /// Write a crash log to `<dir>/crash-<ts>-<tag>.log` (the `dir` =
   /// `dirs::data_dir().join("archimedes")` — the same resolution as `lib.rs`'s
   /// setup; a `None` base = no-op, never panics). Content: the panic message
   /// + location (the `PanicHookInfo`'s `payload_as_str()` + `location()`) + a
   /// captured `std::backtrace::Backtrace::force_capture()` (NOTE: plain
   /// `Backtrace::capture()` is EMPTY unless `RUST_BACKTRACE` is set —
   /// `force_capture` gets the frames regardless; raw frames survive `strip`;
   /// the message+location always resolve).
   pub fn write_crash_log_in(dir: &std::path::Path, tag: &str) -> Option<std::path::PathBuf>;
   /// The production entry (resolves the dirs home; the tests use `write_crash_log_in`).
   pub fn write_crash_log(tag: &str) -> Option<std::path::PathBuf>;
   /// Install the hook (idempotent — `std::panic::set_hook`): writes the log,
   /// then calls the previous hook.
   pub fn install_panic_hook(tag: &str);
   ```
   The Worker's tag: `"worker"` (the session id is unknown before `Start`; the file name carries the timestamp — the Supervisor's crash attribution uses the exit + the `session-stalled` bookkeeping, not the file name); the Supervisor's (Task 6): `"supervisor"`.

3. **`sink.rs`** — `IpcEventSink`: implements `EventSink` (`fn emit(&self, event: &str, payload: Value)`): maps the Tauri event names the `AgentLoop`/gate/interactive machinery emits to `Outbound` frames — `"permission-request"` → `PermissionRequest { id: <the payload's `requestId` — read the current emit sites in `permission.rs:135-158` to find the key field>, payload: <the payload VERBATIM> }`, `"interactive-request"` → `InteractiveRequest { id: <the payload's request key — read the `interactive.rs` emit sites:941-953/1021-1033>, payload: <the payload VERBATIM> }`, **every other name → `SinkFrame { event, payload }` verbatim** (the `interactive-event` todo frames, `interactive-request-close`, `session-update` — the frontend contract is preserved by the Supervisor re-emitting `SinkFrame` as the same-named Tauri event). Writes via a `try_send` to a `mpsc::UnboundedSender<Outbound>` the `core` drains to stdout (the sink is synchronous — `emit` is not `async` — unbounded `try_send` never fails; nothing is dropped).

4. **`store.rs`** — `IpcStore` (implements Task 1's `Store`): `insert_message` → `try_send(Outbound::TranscriptInsert { … })`; `replace_messages` → `TranscriptReplace`; `persist_display` → `DisplayUpsert`; `record_context_usage` → `ContextUsage { session_id, used, window }`; `load_messages` → `Err(DbError::Io(std::io::Error::other("worker store is write-only — resume is rehydrated via the start envelope")))` (defensive — the Worker's loop never calls it: the Supervisor rehydrates and sends the transcript in `Start`; if it IS called, the error is explicit, not a silent empty); `clear_messages` → `Ok(())` (no-op — the native resume path's `clear` is external-era; nothing in the Worker world calls it — read the call sites to confirm, and if one exists, frame it as a `TranscriptReplace` with an empty row set). All `try_send`s are unbounded (never fail).

5. **`dispatch.rs`** — `IpcDispatcher` (implements Task 1's `SubagentDispatcher`): `dispatch(...)` → mint an `id` (uuid), `try_send` `Outbound::SubagentDispatch { id, …, model_key: <the `parent_model`'s catalog key — read `Model`'s fields: the key is the `provider/id` composition the loop resolved in its pre-flight> }` to the core's outbound channel, return `(oneshot::Receiver<SubagentOutcome>, SubagentCancel)` — the receiver is resolved by the core when `Inbound::SubagentResult { id, outcome }` arrives; the `SubagentCancel` (read its definition in `subagent.rs` — it has a `cancel()` that flips a flag) → `try_send` `Outbound::SubagentCancel { id }`. (Unbounded channel — `try_send` never fails.)

6. **`core.rs`** — the testable core (NO stdio I/O — a pure message handler so tests drive it directly):
   ```rust
   /// The handles `build_loop` returns (the core keeps them — the
   /// `AgentLoop`'s `prompt_tx`/`turn_cancel`/`settle_tx` fields are `pub`
   /// per Task 1's visibility exception, and the events receiver is consumed
   /// by `AgentLoop::new`, so the seam creates the channels, passes the
   /// senders into `new`, and returns the handles):
   pub struct LoopHandles {
       pub task: tokio::task::JoinHandle<()>,        // `tokio::spawn(loop_.run())` — THE crash-detection seam (step 7)
       pub prompt_tx: mpsc::Sender<Prompt>,         // a CLONE of the sender (taken from the `pub` field before `tokio::spawn` — `AgentLoop::new` consumes the original sender AND the receiver; the core's `Prompt` handler `send`s via this clone — `Sender::send` takes `&self`, one clone suffices; the bounded channel's backpressure is the intended `send().await` stall)
       pub events_rx: mpsc::UnboundedReceiver<RpcEvent>,  // the `pump_events` consumer
       pub turn_cancel: Arc<std::sync::Mutex<CancellationToken>>,  // the `Abort` handler swaps this
       pub cancel: CancellationToken,            // the `Close` handler cancels this (AND `turn_cancel` — step 7)
   }
   pub struct WorkerCore {
       pub outbound: tokio::sync::mpsc::UnboundedSender<Outbound>,  // drained to stdout by `run_worker`
       inbound_rx: tokio::sync::mpsc::Receiver<Inbound>,          // fed by `run_worker`'s stdin loop
       pending_permissions: PendingPermissions,                   // the existing types from `permission.rs`/`interactive.rs` — `HashMap<String, oneshot::Sender<PermissionOutcome>>` / `HashMap<String, oneshot::Sender<Value>>`
       pending_bridge: PendingInteractive,
       pending_sudo: PendingSudo,
       subagent_waiters: std::sync::Mutex<std::collections::HashMap<String, oneshot::Sender<SubagentOutcome>>>,
       trust: Option<StaticTrustSource>,
       handles: Option<LoopHandles>,
       session_id: Option<String>,
       closed: bool,
   }
   impl WorkerCore {
       pub fn new(outbound: UnboundedSender<Outbound>) -> (Self, mpsc::Sender<Inbound>);
       /// One inbound message → zero or more outbound frames (the test
       /// observation point). `Start` builds the `AgentLoop` via the
       /// `build_loop` seam — injectable in tests:
       /// `fn build_loop(env: &StartEnv) -> LoopHandles`; the DEFAULT
       /// (the production path) — read the current in-process session
       /// initialization at `session.rs:1572-1632` and move it here:
       /// (1) construct the channels/tokens (the events `unbounded_channel`,
       /// the prompt `mpsc::channel(8)`, the cancel/turn_cancel tokens, the
       /// settle watch), (2) `AgentLoop::new(…)` with the Task-1 seams:
       /// `NoopStore`-replaced-by-`IpcStore::new(outbound)`,
       /// `trust: Some(Arc::new(StaticTrustSource::new(env.trusted)))`,
       /// `sink: Arc::new(IpcEventSink::new(outbound))`,
       /// `subagent: env.subagent_enabled.then(|| Arc::new(IpcDispatcher::new(outbound)))`,
       /// FRESH `PendingPermissions`/`PendingInteractive`/`PendingSudo` maps,
       /// `SudoDeps { runner: Arc::new(RealSudoRunner), …fresh maps… }`,
       /// `RetryPolicy::new()`, `env.config_dir`, (3) the SESSION
       /// INITIALIZATION (the `session.rs:1572-1632` steps, moved verbatim):
       /// `Fresh` → `build_main_prompt` (the cwd, the `~/.pi/agent` roots,
       /// the advertised tool specs, the discovered skills — the existing
       /// function in `harness/prompt.rs`) + `loop_.prepend_system(prompt)`
       /// (this is what creates the seq-0 transcript row the e2e asserts);
       /// `Resume` → `loop_.load_transcript(env.transcript.unwrap())` (the
       /// existing resume hydration — the data now arrives in the envelope
       /// instead of the DB); `env.system_prompt` `Some` (subagent Workers)
       /// → `loop_.prepend_system(env.system_prompt.clone())` (the
       /// Supervisor-computed `build_child_system_message` output — the
       /// child's seq-0 row); ALWAYS →
       /// `loop_.set_enabled_tools(env.enabled_tools)` (the wire field IS the
       /// harness convention — `None` = all, `Some(v)` = exactly `v`,
       /// `Some(vec![])` = NO tools — applied VERBATIM, NO mapping: the
       /// settings `[]`→`None` mapping happens at `StartEnv` construction,
       /// Task 3/4) + `loop_.set_thinking_level` (the
       /// `env.thinking` — the existing `session.rs` step), (4) return
       /// `LoopHandles { task: tokio::spawn(loop_.run()), prompt_tx
       /// (a clone of the `pub` field's sender — taken before the `spawn`;
       /// `AgentLoop::new` consumes the original sender AND the receiver),
       /// events_rx, turn_cancel, cancel }`.
       /// (The `AgentLoop::new` argument list — for completeness, beyond the
       /// seam replacements above: `provider: build_provider(&env.model)`
       /// (`provider.rs:2187` — the envelope's `Model` carries the provider
       /// config), `todo_store: Arc::new(TodoStore::new())` (fresh, per-Worker
       /// — the per-session todo state), the `prompt_queue` receiver (the
       /// channel's receiver half — `new` consumes it), the settle watch, the
       /// `cancel`/`turn_cancel` tokens — read the existing `new` signature
       /// and fill the remaining arguments verbatim.)
       pub async fn handle(&mut self, msg: Inbound) -> Vec<Outbound>;
       /// The `events` pump — the `run_worker` pump task (step 7's restructure:
       /// the receiver is moved out of the core; this is the free-function
       /// form over the moved `events_rx`): forward each `RpcEvent` as
       /// `Outbound::Event` until the channel closes.
       pub async fn pump_events(&mut self) -> bool;  // false when the channel closed
   }
   ```
   `handle` semantics: `Start` (build via the seam; the `Ready` was already emitted by `run_worker` at startup); `Prompt` → `handles.prompt_tx.send(Prompt { … }).await` (the sender clone — backpressure = the bounded channel's natural stall); `Config` → `control_tx.send` (the existing `ControlCmd` variants for model/thinking — read `harness/loop.rs`'s `ControlCmd` enum) + `trust.set` when `trusted` is `Some`; `Abort` → the `turn_cancel` swap (the existing turn-cancel semantics — read the loop's current wiring); `Close` → **cancel BOTH `handles.turn_cancel` AND `handles.cancel`** (the current in-process teardown cancels both — `NativeHandle::close`, `session.rs:465-471` — the session token alone doesn't stop a mid-model-call turn promptly) + set `self.closed = true` (the `run_worker` loop exits 0 when `closed`); `PermissionResponse` → resolve `pending_permissions` (remove the `id` key, `send(outcome)` — the `PermissionOutcome` verbatim, mirroring the existing `respond_permission` resolution in `interactive.rs`/`permission.rs`) **AND when the outcome is `Selected { option_id: "trust-space" }`, `trust.set(true)`** (the mid-session trust flip — the very next tool call auto-approves, matching today's live-lookup behavior; the Supervisor's `db.set_space_trusted` write — Task 4 — is the persistence half); `InteractiveResponse` → resolve `pending_bridge`/`pending_sudo` (mirror the existing `respond_interactive_request`); `SubagentResult` → resolve `subagent_waiters`; unknown → ignored (permissive).

7. **`mod.rs`** — `run_worker()`: install the panic hook (tag `"worker"`), build the `WorkerCore`, emit `Ready` (the binary version — `env!("CARGO_PKG_VERSION")`), then observe three things concurrently — **structure (reviewer-corrected: a single `tokio::select!` over three arms all borrowing `core` won't compile — restructure as follows)**: the `events_rx` and the `JoinHandle` live in `run_worker` (moved out of the `core` — the `WorkerCore::new` + `build_loop` return them; the core keeps only what `handle` needs), and TWO tasks are spawned: a **pump task** (the `events_rx` → `Outbound::Event` → stdout loop — the `pump_events` logic as a free function over the moved receiver) and a **crash-watch task** (awaits the `JoinHandle` — **`Ok(())` → a no-op signal** (the normal loop end — e.g. after `Close` — the `closed`/stdin-EOF conditions decide exit; do NOT exit on `Ok`); **`Err` (panic — the panic hook already wrote the crash log) → best-effort `WorkerError` frame + `std::process::exit(1)`** — REQUIRED: a panic in a `tokio::spawn`'d task is otherwise SWALLOWED by tokio (surfaces only as a `JoinError`), and without this watch a panicked loop would leave the Worker running forever with a dead loop — no exit code, no `on_crash`, the session hangs instead of stalling). The **main loop** owns stdin only (line-by-line via `tokio::io::AsyncBufReadExt` on `stdin`; each line → `decode_inbound` → `core.handle` → drain `core.outbound` to stdout with `flush`), `select!`-ing over the stdin stream + the two tasks' completion signals (the stdout writer is a `tokio::sync::Mutex` shared by the pump task and the main loop — the two producers serialized). Exit 0 when `core.closed` or stdin hits EOF.

8. **`main.rs`** —
   ```rust
   fn main() {
       // The Worker mode (ADR 0025): checked BEFORE Tauri init — the
       // Tauri runtime never starts in a Worker.
       if std::env::args().any(|a| a == "--worker") {
           archimedes_lib::run_worker();
           return;
       }
       archimedes_lib::run()
   }
   ```
   `lib.rs`: `pub fn run_worker() { /* a tokio runtime — `tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap().block_on(worker::run_worker_inner())` */ }`.

9. **`Cargo.toml`** — `[profile.release]`: `panic = "abort"` → `panic = "unwind"` (the ADR 0025 consequence: a Worker panic stays in the Worker; the binary grows with unwind tables — accepted).

**Steps:**
- [ ] Write failing tests in `protocol.rs`: every `Inbound`/`Outbound` variant round-trips through `encode_line` + `decode_*` (including the `PermissionResponse` carrying `PermissionOutcome::Selected { option_id: "trust-space" }`, the `PermissionRequest`/`InteractiveRequest` frames carrying full canned gate payloads verbatim, and the `SubagentResult` carrying each `SubagentOutcome` variant); an unknown `type` line decodes to the `Unknown` variant (never an error); the `RpcEvent`-inside-`Event` nesting round-trips (this test also forces the `Serialize` derive addition on `RpcEvent` — step 1's `events.rs` change).
- [ ] Write failing tests in `store.rs`: `IpcStore` — each of the 5 write methods emits the right frame on the (test-observed) outbound channel (`record_context_usage` → `ContextUsage { used, window }`); `load_messages` returns the defensive `Err` (the `DbError::Io` variant); `clear_messages` returns `Ok(())`.
- [ ] Write failing tests in `sink.rs`: `IpcEventSink` — a `permission-request` emission → a `PermissionRequest` frame with the payload VERBATIM (assert field equality of the `requestId` + the `request.options` list); an `interactive-request` emission (a canned `ask` payload) → an `InteractiveRequest` frame with the payload VERBATIM; an `interactive-event` (todo) emission → a `SinkFrame` with the same event name + payload.
- [ ] Write failing tests in `core.rs`: a `WorkerCore` driven by canned `Inbound`s — `Start` (with the `build_loop` seam injected as a stub returning a `LoopHandles` with a pre-built `AgentLoop` on a test provider — reuse the existing harness test fixtures: the mock `Provider` + the canned SSE streams in `harness/provider.rs` tests) emits the loop's `RpcEvent`s via `pump_events` AND the store frames (`TranscriptInsert` for the system-prompt seq 0 + the user message, `DisplayUpsert` for the display rows); `Prompt` reaches the loop's prompt queue (observe via the emitted `turn_start`/`message_*` events); `PermissionResponse { outcome: Selected { option_id: "allow" } }` resolves a seeded `pending_permissions` oneshot with the right `PermissionOutcome`; a `PermissionResponse { outcome: Selected { option_id: "trust-space" } }` ALSO flips the `StaticTrustSource` (assert `is_trusted` becomes `true`); `SubagentResult` resolves an `IpcDispatcher` waiter; `Close` sets the closed flag AND cancels both tokens (assert the `turn_cancel` + `cancel` states); `Config { trusted: Some(true) }` flips the `StaticTrustSource`.
- [ ] Write a failing test in `crashlog.rs`: `write_crash_log_in` (temp dir) writes a file containing the panic location (trigger a `panic!` in a thread with the hook installed; assert the file appears with the location text; assert the backtrace section is NON-EMPTY — the `force_capture` behavior).
- [ ] Write the failing `tests/worker_smoke.rs` (INTEGRATION target — `env!("CARGO_BIN_EXE_archimedes")`): spawn the built binary with `--worker`; assert a `ready` JSON line on stdout; send a `close` line; assert exit code 0; a second spawn: send no `close`, just EOF (close stdin); assert exit code 0 (the clean-EOF path).
- [ ] Run `cargo test -p archimedes --lib agent::worker` + `cargo test -p archimedes --test worker_smoke` — confirm the new tests FAIL.
- [ ] Implement (the order: the `events.rs`/`subagent.rs` derives → protocol → crashlog → sink → store → dispatch → core → mod → main/lib → the Cargo.toml profile).
- [ ] Run `cargo test` — ALL tests pass.
- [ ] Run `cargo clippy --all-targets` (0 warnings) + `cargo fmt`.
- [ ] Commit: `feat(worker): the Worker process — protocol + run_worker + crash logging (ADR 0025)`

**Acceptance criteria:**
- [ ] `./target/debug/archimedes --worker` starts, emits a `ready` JSON line, and exits 0 on stdin EOF (the Tauri runtime never starts — the `worker_smoke` test proves it).
- [ ] A `start` message builds a real `AgentLoop` (the `build_loop` default — the session initialization steps included: the main system prompt for `Fresh`, the transcript hydration for `Resume`, the `enabled_tools` `[]`→`None` mapping, the thinking level) that runs a full turn against a mock provider and emits the `RpcEvent` stream on stdout, with the store frames (`TranscriptInsert`/`DisplayUpsert`) emitted for the transcript + display rows.
- [ ] `permission-response` (the real `PermissionOutcome`) / `interactive-response` / `subagent-result` resolve the Worker's pending maps; the `trust-space` outcome flips the `StaticTrustSource`.
- [ ] A panic in the loop task → the `JoinHandle` arm fires (`Err` — NOT `Ok`) → `crash-<ts>-worker.log` written (with a non-empty backtrace — `force_capture`) + non-zero exit (the session does NOT hang).
- [ ] The release profile is `panic = "unwind"`.

---

### Task 3: The Supervisor-side `WorkerHandle` + `WorkerManager`

**Context:**
The Supervisor (the Tauri app process) needs a client per Worker: spawn the self binary, speak the protocol, route events, detect crashes. This task builds that — plus the `WorkerManager` (the registry of live Workers, including the Supervisor-side subagent-dispatch flow) and the `fake_worker` test fixture (the repo already ships `fake_mcp_stdio` as a test-fixture binary — `src-tauri/src/bin/fake_mcp_stdio.rs`; follow that pattern).

**Files:**
- Create: `src-tauri/src/agent/worker/client.rs`
- Create: `src-tauri/src/agent/worker/manager.rs`
- Create: `src-tauri/src/bin/fake_worker.rs` (the test fixture — a tiny binary that speaks the protocol via the shared `protocol` types: `ready` on start, a canned event + store-frame stream on `start`+`prompt`, permission round-trip, `close` → exit 0, a `__crash__` prompt → exit 137)
- Modify: `src-tauri/src/agent/worker/mod.rs` (re-exports)
- Test: in-file `#[cfg(test)]` modules

**What to implement:**

1. **`client.rs`** —
   ```rust
   pub struct WorkerError { pub kind: WorkerErrorKind }  // SpawnFailed | ReadyTimeout | Io(String) | Protocol(String)
   pub struct WorkerHandle {
       child: tokio::process::Child,
       stdin_tx: tokio::sync::mpsc::UnboundedSender<String>,  // the stdin writer task (one line per message)
       events: tokio::sync::mpsc::Receiver<WorkerInboundEvent>,  // the read loop's output
       round_trips: std::sync::Mutex<std::collections::HashMap<String, tokio::sync::oneshot::Sender<Value>>>,
       exited: tokio::sync::watch::Sender<Option<i32>>,
   }
   pub enum WorkerInboundEvent {
       Ready { version: String },
       Event(RpcEvent),
       Store(StoreFrame),  // the 4 store-frame variants (the `protocol::StoreFrame` summary — the Supervisor's persister consumes them)
       PermissionRequest { id: String, payload: Value },   // the FULL gate payload verbatim (the `protocol` frame shape)
       InteractiveRequest { id: String, payload: Value },  // the FULL interactive payload verbatim
       SubagentDispatch(SubagentDispatchWire),  // the `protocol` struct
       WorkerError { code: String, message: String },
       Exited(Option<i32>),
   }
   impl WorkerHandle {
       /// Spawn `<exe> --worker` (piped stdio). The `exe` PATH is a
       /// parameter: production callers pass `std::env::current_exe()` (the
       /// self-exec); tests/e2e pass the fixture path. (A no-arg
       /// `current_exe()`-only version would point at the test-harness
       /// binary under `cargo test` — wrong.)
       pub fn spawn(exe: &std::path::Path) -> Result<Self, WorkerError>;
       pub async fn wait_ready(&self, timeout: std::time::Duration) -> Result<String, WorkerError>;  // the version
       pub fn send_start(&self, env: &StartEnv) -> Result<(), WorkerError>;   // encodes `Inbound::Start`
       pub fn send_prompt(&self, text: &str, images: &[ImageRef]) -> Result<(), WorkerError>;
       pub fn send_config(&self, model: Option<&Model>, thinking: Option<&str>, trusted: Option<bool>) -> Result<(), WorkerError>;
       pub fn send_abort(&self) / send_close(&self) / send_permission_response(&self, id: &str, outcome: &PermissionOutcome) / send_interactive_response(&self, id: &str, value: &Value) -> Result<(), WorkerError>;
       pub fn events(&self) -> &Receiver<WorkerInboundEvent>;
       pub async fn exited(&self) -> Option<i32>;  // blocks until the process exits (crash detection)
       pub fn kill(&self) -> Result<(), WorkerError>;  // the process-group kill (the existing `RealSudoRunner` `#[cfg(unix)]` `setpgid`/`kill(-pgid)` pattern — reuse it; Windows: `child.kill()`)
   }
   ```
   Internals: a stdin writer task (drains `stdin_tx` → `ChildStdin` line writes, `flush` per line); a stdout read task (line-by-line → `decode_outbound` — the Worker emits `Outbound` frames — route every variant into the `WorkerInboundEvent` enum above: the 4 store-frame variants → `WorkerInboundEvent::Store(StoreFrame::…)`; on process exit → `Exited` + the `exited` watch). **Test path convention:** the in-file tests (which CANNOT use `env!("CARGO_BIN_EXE_…")` — integration targets only) locate the fixture via `concat!(env!("CARGO_MANIFEST_DIR"), "/target/debug/fake_worker")` — the EXISTING convention for the `fake_mcp_stdio` fixture (read `mcp/stdio.rs:323` and follow it).

2. **`manager.rs`** —
   ```rust
   pub struct WorkerManager {
       workers: std::sync::Mutex<std::collections::HashMap<String, WorkerHandle>>,  // session_id → handle
       factory: Arc<dyn WorkerFactory>,  // the test seam
       on_crash: Arc<dyn Fn(String, Option<i32>) + Send + Sync>,  // (session_id, exit code) — the Task-4 stalled-session callback
       on_event: Arc<dyn Fn(String, bool, WorkerInboundEvent) + Send + Sync>,  // (session_id, is_subagent, evt) — the Task-4 router (UI + persistence)
   }
   pub trait WorkerFactory: Send + Sync {
       fn spawn(&self) -> Result<WorkerHandle, WorkerError>;  // production: `WorkerHandle::spawn(std::env::current_exe().as_path())`; tests: the `fake_worker` binary (the manifest-dir path convention above)
   }
   impl WorkerManager {
       pub fn new(factory: Arc<dyn WorkerFactory>, on_crash: ..., on_event: ...) -> Self;
       pub async fn attach(&self, session_id: &str, env: &StartEnv) -> Result<(), WorkerError>;  // spawn + `wait_ready` (5 s timeout) + `send_start`
       pub fn detach(&self, session_id: &str) -> Option<WorkerHandle>;
       pub fn reap_all(&self)  // `close` + grace (2 s) + `kill` — called on app exit
       /// The Supervisor-side subagent-dispatch flow (a main-session Worker's
       /// `subagent` tool → its `IpcDispatcher` → `Outbound::SubagentDispatch`
       /// → here): resolve `model_key` against the effective catalog + the
       /// settings' `subagentModels` override (the ADR 0020/0023 resolution —
       /// read the current `subagent.rs` driver's model/launch resolution at
       /// lines ~412-450 and move it here, Supervisor-side), build the child
       /// `StartEnv` (the resolved `Model` with its provider config, the
       /// catalog, `cwd` = `parent_cwd`, `trusted` = the parent Space's trust
       /// (the `db.space_trusted` lookup — the Supervisor owns the `spaces`
       /// table), `system_prompt` = `build_child_system_message(launch.system_prompt, has_todo_tool)` (the `harness/prompt` function — `has_todo_tool` from the resolved child tool list), `enabled_tools` = the FULL three-way rule (read `subagent.rs:577-598` and move it verbatim): `launch.tools` `Some` → VERBATIM minus `subagent`/`list_agents` (a non-empty set that empties out yields NO tools — NOT re-expanded); `launch.tools` `None` → the parent's list minus the guard; the `[]` (empty parent list) case → ALL tools minus the guard (the "empty = all" expansion applies ONLY to the inherited-parent case) — the wire field is `Option<Vec<String>>` with the HARNESS convention, Task 2's protocol — applied VERBATIM: `enabled_tools` = `Some(child_tools)` (an empty `child_tools` = NO tools — NOT re-expanded), `subagent_enabled: false`), spawn the subagent Worker (a fresh ephemeral `session_id` — `mint_session_id`), `wait_ready` + `send_start` + the initial `prompt` (the `task`), then the drive task: `agent_settled` → the `SubagentCapture`'s final text → `SubagentOutcome::Completed`; unexpected `Exited` → `SubagentOutcome::Failed { error: "subagent worker exited unexpectedly (code N)" }`; the `settle_timeout` (the current value — read `subagent.rs`) → the current timeout outcome; `SubagentCancel` (from the parent's turn abort) → `send_abort` + reap. Send `Inbound::SubagentResult { id, outcome }` to the REQUESTING (main-session) Worker.
       /// **The subagent UI lifecycle (reviewer-corrected — the frontend's
       /// subagents store depends on these, `tauri.ts:668/676`/
       /// `store/subagents.ts`):** emit `subagent-session-started` (at spawn —
       /// the `subagent.rs:635` payload: `model`/`thinkingLevel`/
       /// `enabledTools`/`agentName`/`task` — to the PARENT session's
       /// `TauriSink`) and `subagent-closed` (on EVERY exit path — the
       /// `subagent.rs:721/743` payload: `status`/`error`/`metrics` — the
       /// `metrics` captured from the subagent's event stream + wall clock,
       /// mirroring the current driver's metric accumulation) on the same
       /// parent-scope `TauriSink`.
       /// The `SubagentCapture` (replaces the deleted `CapturingSink` —
       /// reviewer-corrected: capture from the subagent's `SinkFrame`
       /// `session-update` stream — the SAME enveloped
       /// `agent_message_chunk` frames the current `CapturingSink`
       /// (`subagent.rs:133-165`) consumes — the last-`messageId`-with-
       /// non-empty-text rule; NOT from the `message_end` `RpcEvent`s, which
       /// are a different shape and carry no text for tool-call messages).
   }
   ```
   The read-loop plumbing: each attached Worker's `events()` is consumed by a task that (a) calls `on_event(session_id, is_subagent, evt)` for EVERY variant (the Task-4 router splits UI vs persistence — the `is_subagent` flag is known per attached Worker: main `false`, subagent `true`), (b) on `SubagentDispatch` runs `dispatch_subagent` (above), (c) on `Exited` (unexpected — no `detach` was called) calls `on_crash(session_id, code)`; on `Exited` (EXPECTED — a `detach`/`reap_all` was in flight) → no `on_crash` (the Task-4 router's `session-closed` handling, Task 4 step 1).

3. **`bin/fake_worker.rs`** — a ~150-line binary (the `fake_mcp_stdio` pattern — read it): imports the protocol types from `archimedes_lib::agent::worker::protocol`; reads JSONL on stdin; on startup prints a `Ready` line; on `Start` stores the `session_id` + `enabled_tools`; on `Prompt` prints a canned `Event` stream (`turn_start` → `message_start`/`message_end` with a canned assistant `Value` → `turn_end` → `agent_settled`) PLUS canned store frames (`TranscriptInsert` for a canned user + assistant `ChatMessage`, `DisplayUpsert` for a canned `agent-text` row) PLUS a canned `SinkFrame` `session-update` with an `agent_message_chunk` (the `SubagentCapture` test input); on a `Prompt` whose text is `"__crash__"` exits 137 immediately (no `agent_settled`); on a `Prompt` whose text is `"__permission__"` prints a `PermissionRequest` (id `"p1"`, the FULL canned gate payload — `requestId` + `request: { toolCall: { title }, options }` — mirroring `permission.rs:135-158`'s shape) and, on `PermissionResponse` (any `outcome`), prints the tool-execution events + `agent_settled`; on a `Prompt` whose text is `"__slow__"` sleeps 10 s before settling (the `settle_timeout` tests); on `Close` exits 0.

**Steps:**
- [ ] Write failing tests in `client.rs`: spawn the `fake_worker` (via the `WorkerFactory` seam — the manifest-dir path convention): `wait_ready` returns the version; a `prompt` yields the canned event + store-frame + `SinkFrame` sequence in order; a `"__crash__"` prompt → `Exited(137)`; `close` → `Exited(0)`; a `permission-response` round-trip completes the canned permission flow (the tool-execution events arrive after the response — assert the response's `PermissionOutcome` was delivered verbatim).
- [ ] Write failing tests in `manager.rs`: `attach` + `detach` bookkeeping (the `workers` map — a `test_support`-exposed registry view); `reap_all` kills all attached Workers (assert the processes exited); a crashed Worker fires `on_crash` exactly once with the right code (an expected `detach`-then-exit fires NO `on_crash`); the `dispatch_subagent` flow: a `SubagentDispatch` from a main-session `fake_worker` spawns a subagent `fake_worker` (canned settle) and delivers `SubagentResult` to the requester (two `fake_worker`s); the `SubagentCapture` captures the final text from the subagent's `SinkFrame` `session-update` `agent_message_chunk` frames (the last-`messageId`-with-non-empty-text rule); a `"__slow__"` subagent hits the `settle_timeout` outcome; `subagent-session-started` + `subagent-closed` fire on the parent's `TauriSink` (the test's collector) with the `subagent.rs:635`/`721/743` payload shapes (the `metrics` populated from the subagent's event stream + wall clock); the `enabled_tools` wire convention (reviewer-corrected — the round-3 fix): a `StartEnv` with `enabled_tools: Some(vec!["subagent"])` (a `launch.tools` that empties out after the guard minus) → the child Worker's advertised tool set is EMPTY (the `Some(vec![])` = NO tools semantics preserved — NOT re-expanded to all); `enabled_tools: None` → all tools (the settings convention mapped at construction).
- [ ] Run `cargo test -p archimedes --lib agent::worker` — confirm the new tests FAIL.
- [ ] Implement (`client` → `manager` → `fake_worker`).
- [ ] Run `cargo test` — ALL tests pass.
- [ ] Run `cargo clippy --all-targets` (0 warnings) + `cargo fmt`.
- [ ] Commit: `feat(supervisor): WorkerHandle + WorkerManager + fake_worker fixture (ADR 0025)`

**Acceptance criteria:**
- [ ] A `WorkerHandle` spawns the real binary (the `spawn(exe)` path parameter — production `current_exe()`, tests the fixture), completes the `ready` handshake, streams events + store frames + `SinkFrame`s, and detects crashes (process exit without `close`).
- [ ] `WorkerManager` multiplexes N Workers (session-keyed), fires `on_crash`/`on_event` (the `is_subagent` flag threaded), and `reap_all` on app exit.
- [ ] The Supervisor-side subagent-dispatch flow works end-to-end with `fake_worker`s (dispatch → subagent Worker → `SubagentResult`), including the model-key resolution + the three-way `enabled_tools` rule + the `system_prompt` envelope field + the `SubagentCapture` (the `SinkFrame` `agent_message_chunk` source) + the `settle_timeout` + the `subagent-session-started`/`subagent-closed` UI lifecycle events.
- [ ] `fake_worker` is a committed test fixture (like `fake_mcp_stdio`).

---

### Task 4: The session driver re-plumb — main sessions run in Workers

**Context:**
The `SessionManager` (`src-tauri/src/agent/session.rs`) today drives native sessions in-process: `start_session`/`resume_session` build an `AgentLoop` + a `NativeHandle` (the in-process `SessionDriver` — the `pending_permissions`/`pending_bridge` maps, the `respond_*` resolution, the settle watch); `send_prompt`/`set_session_config_option`/`cancel_session`/`close_session` talk to the `NativeHandle`; the `TauriSink` (`commands/sessions.rs`) emits the per-session Tauri events. This task re-plumbs the `SessionManager` onto the `WorkerManager` (Task 3): a session's `AgentLoop` lives in its Worker; the `SessionManager` becomes the Supervisor-side coordinator (spawn/reap, event routing, persistence, the `respond_*` relays, the stalled state). **The in-process native driver is deleted** (no dual-mode fallback — ADR 0025). The `SubagentSessionManager`'s in-process driver still exists at this point (Task 5 re-plumbs it) — so the shared `SessionDriver` machinery is NOT deleted here, only its main-session users.

**Test strategy (reviewer-corrected):** there is NO `tests/ipc.rs` in the repo today (the stale comments in `lib.rs`/`Cargo.toml` referencing it are dead). The actual headless tests are `tests/session_native.rs` (drives `SessionManager` directly with a mock `Provider` injected via `set_provider_factory`) and the `tests/harness_*.rs` files (Task 5's concern). This task: (a) **creates** `tests/ipc.rs` (the `tauri` dev-dependency with the `test` feature already exists — `tauri::test`'s mock runtime is available; the `setup_dirs` seam in `lib.rs` is extended to take an injectable `WorkerFactory` — the `test_support` pattern), and (b) **rewrites** `tests/session_native.rs` against the re-plumbed `SessionManager` (the `set_provider_factory` mock-provider injection CANNOT survive — the provider lives in the Worker process; the rewrite drives via the `WorkerFactory` seam with the `fake_worker`/wiremock; the start/prompt/resume/`load_messages` coverage it carried is preserved by the new tests).

**Files:**
- Modify: `src-tauri/src/agent/session.rs` (the `SessionManager` — the main re-plumb)
- Modify: `src-tauri/src/commands/sessions.rs` (the Tauri commands — the signatures stay; the bodies re-route through the re-plumbed `SessionManager`; the `TauriSink` stays — it is now fed by the Worker event router instead of the in-process loop)
- Modify: `src-tauri/src/commands/spaces.rs` (the `set_space_trusted` command — gains the running-Workers `config` update, step 1)
- Create: `src-tauri/src/agent/persist.rs` (the `TranscriptPersister` — the Supervisor-side store-frame applier)
- Modify: `src-tauri/src/storage/db.rs` (the `sessions` row shape for ephemeral subagent rows — the `is_subagent` flag; read the current `record_session`/`SessionInfo` and extend minimally — `SessionInfo` gains an `is_subagent: bool` field, default `false`)
- Modify: `src-tauri/src/lib.rs` (the `setup_dirs` wiring: construct the `WorkerManager` with the `SessionManager`'s callbacks — the `OnceLock`/late-wire pattern, step 5; the `RunEvent::Exited` handler — step 6)
- Create: `src-tauri/tests/ipc.rs` (NEW — the headless command-surface test)
- Rewrite: `src-tauri/tests/session_native.rs` (per the test strategy above)
- Test: the rewrites above + in-file tests

**What to implement:**

1. **`session.rs` — the `SessionManager` re-plumb.** Read the current `SessionManager` in full first. The re-plumb:
   - `start_session` (the `commands/sessions.rs` command): resolve the model (the existing `resolve_composed_model` + the settings providers — the Supervisor owns the catalog) → build the `StartEnv` (Task 2's type: `mode: Fresh`, `cwd`, `model` = the resolved `Model` (with its provider config embedded), `catalog` = the effective catalog, `thinking`, `trusted` = `db.space_trusted(cwd)`, `enabled_tools` = `if settings.enabled_tools.is_empty() { None } else { Some(settings.enabled_tools) }` (the SETTINGS `[]` = all convention mapped HERE, at `StartEnv` construction — the `session.rs:1539-1544` mapping; the wire field is the harness convention — Task 2's protocol), `config_dir`, `subagent_enabled: true`, `system_prompt: None`) → `worker_manager.attach(session_id, env)` (a spawn failure → the command returns an error — the visible session error, NO in-process fallback).
   - `resume_session`: the existing `native_messages` re-read (the `SessionStore::load_messages` path — now called by the `SessionManager` against the real `Db`, moved out of the harness) → `StartEnv { mode: Resume, transcript: Some(messages), … }` → `attach`. A `stalled` session's resume is this same path (a fresh Worker + re-hydrate).
   - `send_prompt` / `set_session_config_option` / `cancel_session` (Stop) / `close_session`: → the `WorkerManager`'s `send_prompt`/`send_config`/`send_abort`/`detach` (the `NativeHandle` is gone). **`send_prompt`'s in-flight turn oneshot (the `pending_turn`/`StopReason` resolution — read the current code): on a crash (step 4) AND on a clean `close` (the `Exited` handling below) the in-flight `send_prompt` MUST resolve** (a `StopReason`-equivalent outcome — crash: the error outcome; clean close: `Cancelled`) — otherwise the composer stays locked forever. **The user display row write stays Supervisor-side** (the current `db.record_message(session_id, "user", …)` + `begin_user_turn` at `session.rs:1770-1776` — the Worker persists only the provider-transcript user row via the store frames; the display row + `begin_user_turn` stay in the `send_prompt` path, unchanged).
   - `set_session_config_option` (the `session.rs:1854-1990` body — the model resolution, the ADR-0015 thinking reset, the remembered-level `write_settings`, the `config_option_update` re-synthesis from a per-session `NativeConfigState` mirror): the **`SessionManager` keeps a per-session config-state map (replacing `NativeHandle`'s state — the mirror's home moves here, field-for-field)** for the re-synthesis + the remembered-level writes; the model/thinking changes are relayed via `send_config` (the `ControlCmd` path).
   - `respond_permission` / `respond_interactive_request`: → `send_permission_response`/`send_interactive_response` on the session's Worker (the `pending_*` maps live in the Worker now — the maps the `SessionManager` used to own are gone for main sessions). **`respond_permission` additionally applies the `trust-space` write on the Supervisor side** (Task 1 moved it out of the gate): when the outcome is `Selected { option_id: "trust-space" }`, call `db.set_space_trusted(cwd, true)` (the Supervisor owns the `spaces` table — the existing `set_space_trusted` command's `Db` method, read `commands/spaces.rs` for the exact call). (The Worker's `StaticTrustSource` flip — Task 2's `PermissionResponse` handler — is the live-lookup half; the `db` write is the persistence half.)
   - **The event router** (the `WorkerManager`'s `on_event` callback, wired in `lib.rs`) — reviewer-corrected routing (the frontend's contract is the SINK frames — today's `NativeHandle` consumes the raw `RpcEvent` stream for settle-watching ONLY, `session.rs:344-349`; the UI gets the normalized sink frames):
     - `Store(StoreFrame)` → the `TranscriptPersister` (below) — persistence ONLY, not the UI.
     - `SinkFrame { event, payload }` → the `TauriSink` VERBATIM (the ENTIRE UI contract — the `session-update`/`interactive-event`/`interactive-request-close`/… frames re-emitted with the same event name + payload; the frontend is unchanged).
     - `Event(RpcEvent)` → internal bookkeeping ONLY (NOT the `TauriSink` — forwarding it would DOUBLE-DELIVER every `session-update` the loop emits both as an `RpcEvent` and as a `session-update` `SinkFrame`): the `agent_settled` detection (the session's "busy" state — the existing settle semantics, read the current in-process settle watch and reproduce the state transitions) + the `pending_turn` resolution.
     - `PermissionRequest { id, payload }` → re-emit as the `permission-request` Tauri event with the payload UNCHANGED (the frontend contract preserved by construction — `PermissionPrompt.tsx`/`store/permissions.ts` consume the payload's `requestId`/`toolTitle`/`options` verbatim).
     - `InteractiveRequest { id, payload }` → re-emit as the `interactive-request` Tauri event with the payload UNCHANGED (same reasoning — `store/interactive.ts` consumes `method` + `params`).
     - `Exited(code)` → **EXPECTED** (a `detach`/`reap_all` was in flight): emit `session-closed` (the `ClosedReason` payload — the current `session.rs:876` mapping: `close_session` → `User`, a clean self-exit → `AgentExited` — read the current `CloseKind`/`ClosedReason` mapping and reproduce it), resolve the in-flight `pending_turn` → `Cancelled`, **emit the pending-modal cleanup (reviewer-corrected — the round-3 fix: the Worker's pending maps die with it, so the Supervisor tracks the cleanup state itself: per session, the `PermissionRequest`/`InteractiveRequest` frames it relayed MINUS the `*Response`s it sent = the unanswered request ids — the Supervisor sees both halves of every round-trip, so the diff is exact) — the same `interactive-request-close` frames the in-process teardown emits for unanswered prompts (`interactive.rs:446-455` — read and reproduce, keyed by the unanswered ids; a session ending with an open permission/interactive prompt must NOT leave a stuck modal)**, remove the live-session bookkeeping. **UNEXPECTED** (no `detach` in flight): the `on_crash` path (below) — which ALSO performs the same pending-modal cleanup (the diff computed at crash time).
   - **The crash callback** (`on_crash`): mark the session **stalled** — the mechanism (reviewer-corrected: there is no backend `SessionState` enum today; session state lives in the frontend store + `LiveSession`'s ad-hoc flags): a `WorkerManager`-side stalled registry (`HashMap<String, StalledInfo { at: i64, crash_log: Option<PathBuf> }>` — the `crash_log` = the newest `crash-<ts>-worker*.log` in the data dir, globbed and FROZEN in the `StalledInfo` at crash time — best-effort attribution across concurrent sessions, accepted: crash logs are diagnostic, not load-bearing) + the `SessionManager` exposes `stalled_info(session_id)` (a new method the `commands` surface can query) + emit a `session-stalled` Tauri event `{ session_id, at, crash_log }` + **the pending-modal cleanup (the `Exited`-expected path's cleanup — the unanswered-request-id diff, the `interactive-request-close` frames — a crashed session's open permission/interactive modals are dismissed, no stuck modal alongside the banner)** + **resolve the in-flight `send_prompt` oneshot** (the crash outcome — step 1's bullet). The frontend (Task 6) reads the state from the event + the registry query.
   - **DELETE** the in-process native session driver: the `NativeHandle` type (the config-state mirror moves to the `SessionManager` — step 1), the main-session `SessionDriver` usage, the in-process settle-watch plumbing, the main-session `pending_permissions`/`pending_bridge` ownership. (The `SessionDriver` struct itself may still be referenced by the `SubagentSessionManager` — Task 5 deletes the rest.)

2. **`persist.rs` — the `TranscriptPersister`** (the Supervisor's sole-writer persistence — applies the store frames; the raw event stream does NOT drive persistence — it drives the UI):
   ```rust
   pub struct TranscriptPersister { db: Arc<Db> }
   impl TranscriptPersister {
       /// Apply one store frame (idempotent — a replayed frame must not
       /// duplicate rows; the upsert semantics are the existing `Db` methods'):
       /// - `StoreFrame::Insert { session_id, seq, role, content_json }` →
       ///   `db.insert_native_message(…)` (the `native_messages` upsert — the
       ///   existing idempotent `(session_id, seq)` collapse).
       /// - `StoreFrame::Replace { session_id, rows }` →
       ///   `db.replace_native_messages(…)` (the compaction rewrite — the
       ///   existing ATOMIC single-transaction method, `storage/db.rs`).
       /// - `StoreFrame::Display { session_id, rows }` → the `db.record_message` loop
       ///   over the rows (the `messages` upserts — the existing
       ///   `(session_id, kind, message_key)` upsert keys).
       /// - `StoreFrame::ContextUsage { session_id, used, window }` →
       ///   `db.record_session_context_usage(session_id, used, window)` (the
       ///   `sessions.context_usage_json` write — the existing method,
       ///   `storage/db.rs:323-334`).
       /// Ephemeral subagent rows (the spec's §3 — subagent transcripts persist
       /// as hidden rows): `ensure_session_row(session_id, is_subagent)` — on
       /// the FIRST frame for an unknown session id, `record_session` with
       /// `SessionInfo { …, is_subagent: <the flag the `WorkerManager`
       /// threaded — main sessions `false`, subagent sessions `true` (the
       /// `dispatch_subagent` flow knows)>, archived: false, … }` (read the
       /// current `record_session`/`SessionInfo` and fill the remaining fields
       /// the way the current in-process path does — the `capabilities`
       /// envelope: the existing capability shape, `Value::Null`-ish for
       /// ephemeral subagents as the current throwaway-DB path did).
       pub fn apply(&self, session_id: &str, is_subagent: bool, frame: &StoreFrame);
   }
   ```
   The `WorkerManager`'s `on_event` passes `(session_id, is_subagent, evt)` to the persister (the `is_subagent` flag is known per attached Worker — main `false`, subagent `true`).

3. **`commands/sessions.rs`** — the command signatures + the `TauriSink` are UNCHANGED (the frontend contract). The bodies re-route per step 1. Add ONE new command: `get_stalled_info(session_id) -> Option<StalledInfo>` (the frontend's banner query — or fold the stalled info into an existing session-list command if the frontend can get it there — read `src/store/sessions.ts`'s data flow and pick the minimal surface; prefer the dedicated command).

4. **`storage/db.rs`** — `SessionInfo` + the `sessions` table: add the `is_subagent` column (default `0`; a `rusqlite` migration — the existing migration pattern in `db.rs`; the `record_session` upsert writes it; the `list_sessions` query gains `AND is_subagent = 0`; `load_history`/`load_native_messages` are unchanged — they work for subagent ids). ADB: no data migration (existing rows default `0` — the spec's §6).

5. **`lib.rs` — `setup_dirs` wiring.** The `SessionManager` ↔ `WorkerManager` construction cycle (the `SessionManager` needs the `WorkerManager` to send prompts; the `WorkerManager`'s `on_event`/`on_crash` callbacks need the `SessionManager`'s router/persister): use the EXISTING late-wire pattern — read `session.rs:1173`'s `set_subagent_manager` + the `std::sync::OnceLock` it uses and apply the same shape: construct the `WorkerManager` with `OnceLock`-backed callbacks, inject the `SessionManager`'s router into the locks after both are built, then hand the `WorkerManager` to the `SessionManager`.

6. **`lib.rs` — the exit handler.** The current `lib.rs` ends with a plain `.run(tauri::generate_context!())` — there is NO `RunEvent` handling to "read and hook". Switch to the builder's `run` callback form: `tauri::Builder::…run(|_handle, event| match event { tauri::RunEvent::Exited => { /* the managed `WorkerManager`'s `reap_all` — obtain it from the managed state or a captured `Arc` */ _ => {} })` (read the Tauri 2 builder API in `Cargo.toml`'s `tauri` version — the `run` method takes a `Fn(&AppHandle, RunEvent)`; if the version's API differs, use the equivalent `RunEvent` hook and note it). `reap_all` on exit (the Workers' stdin EOF → clean exit 0 — Task 2's `run_worker` EOF path).

7. **`commands/spaces.rs` — `set_space_trusted` (the Spaces-UI mid-session trust toggle).** The command body gains: after the `db` write, send `Config { trusted: Some(value), … }` to every RUNNING Worker in the affected Space (the `WorkerManager` exposes a `send_config_to_space(cwd, trusted)` helper — read the current command and the `WorkerManager` registry and wire it). (The `trust-space` permission-outcome path — the `respond_permission` `db` write + the Worker's `StaticTrustSource` flip — is Task 1/2/step-1; this is the SEPARATE Spaces-UI toggle path. The mid-session toggle for a session in ANOTHER space is unaffected.)

**Steps:**
- [ ] Write failing tests in `tests/ipc.rs` (NEW file — the `tauri::test` mock runtime + the `setup_dirs` seam extended with the injectable `WorkerFactory` (the `test_support` pattern — read `test_support.rs`'s `ENV_LOCK` + the existing seam conventions)): (a) `start_session` spawns the `fake_worker` (assert via a `test_support` hook that observes the `WorkerManager`'s registry — a new `test_support` accessor), a `send_prompt` yields the canned `SinkFrame`s on the Tauri sink (the test's event collector — the UI contract) AND writes the `messages`/`native_messages` rows (assert via the test's `Db` — the store frames applied by the `TranscriptPersister`) AND the raw `RpcEvent` stream is NOT forwarded to the Tauri sink (no `session-update` duplication — assert the sink saw the `session-update` `SinkFrame` exactly once per loop `emit`); (b) a `"__crash__"` prompt → the `session-stalled` event fires + the in-flight `send_prompt` resolves (the composer-unlock assertion) + the session resumes via `resume_session` (a fresh `fake_worker` attached); (c) `respond_permission` relays to the `fake_worker` (the `__permission__` flow completes — the tool-execution events arrive) AND the `trust-space` outcome writes `spaces.trusted = 1` (assert via the test's `Db`) AND the Worker's `StaticTrustSource` flip is observable (a second `__permission__`-style prompt on the same session is auto-approved — the `fake_worker`'s `__permission__` mode can be extended to skip the prompt when the `Start` `trusted` flag is `true`… the `fake_worker` reads `trusted` from `Start` — assert the second dispatch's `Start`-independent behavior via the `Config` the test sends, OR keep the assertion at the `db` write + the Task-2 unit test that already covers the flip — pick the minimal non-redundant assertion); (d) a `SinkFrame` (`interactive-event` todo payload) re-emits on the Tauri sink with the same event name + payload VERBATIM; (e) a clean `close_session` → `session-closed` (the `ClosedReason` `User` payload) + the in-flight `send_prompt` resolves `Cancelled`; (f) `set_space_trusted` sends a `Config { trusted }` to the space's running `fake_worker` (assert the worker received it — the `fake_worker` logs/echoes `config` frames).
- [ ] Rewrite `tests/session_native.rs` against the re-plumbed `SessionManager` (the `WorkerFactory` seam + `fake_worker`/wiremock — the start/prompt/resume/`load_messages` coverage preserved; the `set_provider_factory`-based tests are DELETED, replaced by their worker-mediated equivalents).
- [ ] Run `cargo test -p archimedes --test ipc` + `cargo test -p archimedes --test session_native` — confirm the new/rewritten tests FAIL (the commands still drive the in-process driver).
- [ ] Implement (the order: `persist` + `db` schema → `session.rs` re-plumb → `commands` re-route → `lib.rs` wiring → `commands/spaces.rs`).
- [ ] Run `cargo test` — ALL tests pass.
- [ ] Run `cargo clippy --all-targets` (0 warnings) + `cargo fmt`.
- [ ] Commit: `feat(supervisor): main sessions run in Worker processes — the in-process native driver is deleted (ADR 0025)`

**Acceptance criteria:**
- [ ] `start_session`/`resume_session` spawn a Worker; the `AgentLoop` runs ONLY in the Worker (no in-process `AgentLoop` construction in the `SessionManager` — a `rg "AgentLoop::new" src-tauri/src/agent/session.rs` returns nothing).
- [ ] A session's events reach the Tauri sink via the `SinkFrame` re-emit (the frontend contract unchanged — verbatim, no duplication — the raw `RpcEvent` stream is bookkeeping-only) AND the store frames are persisted to `messages`/`native_messages`/`sessions.context_usage_json` (the idempotent upserts — a complete transcript: system + user + assistant + tool-role rows + compaction rewrites).
- [ ] A Worker crash → the `session-stalled` event + the in-flight `send_prompt` resolves + the session is resumable (fresh Worker + re-hydrate from `native_messages`); a clean `close` → `session-closed` + `pending_turn` → `Cancelled`.
- [ ] `respond_permission`/`respond_interactive_request` resolve the Worker's pending gates (the `fake_worker` `__permission__` flow) AND the `trust-space` outcome persists `spaces.trusted`; `set_space_trusted` pushes a `Config { trusted }` to running Workers.
- [ ] The in-process native session driver is deleted (no dual-mode path); `tests/session_native.rs` rewritten and green.

---

### Task 5: The subagent driver re-plumb + throwaway-DB deletion

**Context:**
The `SubagentSessionManager` (`src-tauri/src/agent/subagent.rs`) today dispatches a subagent by building an in-process child `AgentLoop` (`dispatch_native` → the driver task at line ~419: `tokio::spawn` the child loop on a throwaway `Db`, a `CapturingSink`, the parent's shared `pending_*` maps; settle vs `settle_timeout` race; `SubagentOutcome` via a oneshot). This task re-plumbs the dispatch onto the `WorkerManager` (Task 3's Supervisor-side `dispatch_subagent` flow — a subagent Worker per dispatch) and deletes the throwaway-DB machinery (the spec's §3: subagent transcripts now persist as hidden ephemeral rows via the Task-4 `TranscriptPersister`). The `IpcDispatcher` (Task 2) is what a main-session WORKER uses to dispatch subagents through the Supervisor; this task is the Supervisor side of that flow. The recursion guard is unchanged: a subagent Worker's `enabled_tools` excludes `subagent`/`list_agents` (the current minus logic moves to the `WorkerManager`'s `dispatch_subagent` — Task 3 built it).

**Test disposition (reviewer-corrected — the round-1 "pass unchanged" claim was WRONG):** the `loop.rs` in-file subagent tests (line 4189+) construct a `SubagentSessionManager` with `NativeDeps { provider_factory: <mock>, … }` (`loop.rs:4299-4307`) and observe the child's model requests via in-process mock `Provider`s (`RequestRecordingProvider`). After this task, `dispatch_native` delegates to `WorkerManager.dispatch_subagent` — spawning real Worker processes — and the in-process child construction/`CapturingSink`/throwaway-`Db` machinery is deleted. **Mock providers CANNOT cross a process boundary** — those tests cannot pass through the new path. Disposition: (a) the tests that exercise a REAL dispatch (the model/launch resolution, the settle/timeout/cancel semantics, the recursion guard) are **rewritten** against the `WorkerFactory` seam + `fake_worker` (a `WorkerManager` with a factory spawning `target/debug/fake_worker` via the manifest-dir path convention — the `mcp/stdio.rs:323` pattern); (b) the tests that exercise the `SubagentWait` SELECT mechanics (the outcome vs turn-cancel race, the `subagent: None` error path, the param validation) **stay** — rewritten to use the `MockDispatcher` test double (Task 1) instead of a real `SubagentSessionManager`; (c) the `tests/harness_dispatch_native.rs` (**67 KB — the largest test file**) + `tests/harness_subagent_dispatch.rs` integration files are **rewritten** against the re-plumbed `dispatch_native` (the `WorkerFactory` seam + `fake_worker`/wiremock; the per-test coverage — the model/launch resolution, the settle/timeout/cancel semantics, the recursion guard — is preserved in the rewrites).

**Files:**
- Modify: `src-tauri/src/agent/subagent.rs` (the `SubagentSessionManager` — `dispatch_native` re-plumbed; the throwaway-DB machinery deleted; the in-file test module rewritten per the disposition)
- Modify: `src-tauri/src/agent/harness/loop.rs` (the in-file subagent test module — line 4189+ — rewritten per the disposition: the real-dispatch tests → the `fake_worker` seam; the `SubagentWait`-mechanics tests → the `MockDispatcher`)
- Modify: `src-tauri/src/agent/worker/manager.rs` (the `dispatch_subagent` flow — Task 3 built it; this task verifies it against the re-plumbed `dispatch_native` and adjusts if the re-plumb changed the seam)
- Modify: `src-tauri/src/commands/sessions.rs` (the `respond_permission`/`respond_interactive_request` routing — today they route to BOTH the main and subagent managers (`commands/sessions.rs:97-143`); after this task the subagent pending maps live in subagent **Workers** — the routing becomes: route by session id through the `WorkerManager` (the main-session `SessionManager` relay OR the subagent Worker's handle — read the current dual-manager routing and replace it with the `WorkerManager`-keyed routing))
- Modify: `src-tauri/src/lib.rs` (the `set_subagent_manager` wiring — the `SubagentSessionManager` no longer needs the `trust_db` parameter: the Supervisor-side `dispatch_subagent` resolves `trusted` itself (Task 3). The constructor simplifies: `SubagentSessionManager::new()` — update the `lib.rs` call site)
- Rewrite: `src-tauri/tests/harness_dispatch_native.rs`, `src-tauri/tests/harness_subagent_dispatch.rs` (per the test disposition above)
- Delete: the `open_throwaway_db` / `TempFileGuard` machinery (wherever it lives — `subagent.rs` + its helpers)

**What to implement:**

1. **`subagent.rs` — `dispatch_native` re-plumbed.** The method signature STAYS (Task 1's `InProcessDispatcher` still wraps it — it is now the Supervisor-side dispatcher, called by the `WorkerManager`'s `dispatch_subagent` flow when a main-session Worker emits `SubagentDispatch`):
   - Keep: the model/launch resolution (the ADR 0020/0023 logic at the top of the driver task, lines ~412-450 — the `resolve_composed_model` + the `launch.model`/`thinking` layering) — **moved to the `WorkerManager`'s `dispatch_subagent`** (Task 3's step 2 — the Supervisor owns the catalog + the `settings.json` `subagentModels` override; the `subagent.rs` method receives the already-resolved `Model` + `thinking` + `launch` — read Task 3's `dispatch_subagent` and align: the resolution lives ONCE, Supervisor-side).
   - Replace: the in-process child construction (the throwaway `Db`, the `CapturingSink`, the child `AgentLoop` + `tokio::spawn`, the driver task) with: delegate to `worker_manager.dispatch_subagent(...)` (the Task-3 flow — spawn the subagent Worker, `wait_ready`, `send_start` with the `task` as the initial `Prompt`, the drive task: `agent_settled` → `SubagentCapture` → `Completed`; crash → `Failed { error: "subagent worker exited unexpectedly (code N)" }`; `settle_timeout` → the current timeout outcome; `SubagentCancel` → `send_abort` + reap) and adapt the return to the existing `(oneshot::Receiver<SubagentOutcome>, SubagentCancel)` contract (the `SubagentCancel` the caller — the loop's `dispatch_subagent` — uses to abort the dispatch maps to the `WorkerManager` flow's cancel).
   - **DELETE:** `open_throwaway_db`, the `TempFileGuard` + `_child_db_guard` plumbing, the child `Db`/`SessionStore`/`record_session` FK dance (lines ~460-530), `dispatch_native_force_temp_file` (its test-only existence — the tests that use it are the rewrites in the Files list), the `CapturingSink` (replaced by the `WorkerManager`'s `SubagentCapture` — Task 3), the `SubagentSessionManager::new(trust_db)` parameter (→ `new()` — step 4).
   - **Ephemeral rows:** the subagent's `session_id` is minted as today (`mint_session_id` — now in the `WorkerManager`'s `dispatch_subagent`); its `sessions` row is recorded by the Task-4 `TranscriptPersister` (the `is_subagent` flag) — the `SubagentSessionManager` no longer records it.

2. **`commands/sessions.rs` — the `respond_*` routing** (step in the Files list): the current dual-manager routing (main `SessionManager` + subagent `SubagentSessionManager` `respond_*` methods, `commands/sessions.rs:97-143`) becomes `WorkerManager`-keyed: the command receives the `session_id`, the `WorkerManager` looks up the handle (main OR subagent session — both are in the `workers` map), and relays the response. The `SubagentSessionManager`'s `respond_*` methods are deleted (the maps live in the Workers now).

3. **`lib.rs`** — the `SubagentSessionManager::new()` call site updated (no `trust_db` argument); the `set_subagent_manager` wiring unchanged otherwise (the `SessionManager`'s `subagent` field still points at the manager — the manager's `dispatch_native` now delegates to the `WorkerManager`; the `SessionManager` gets the `WorkerManager` reference via the Task-4 late-wire).

4. **The test rewrites** (per the disposition): the `loop.rs` in-file subagent tests (line 4189+) — the real-dispatch tests rewritten against a `WorkerManager` with a `WorkerFactory` spawning `target/debug/fake_worker` (the manifest-dir path convention — `mcp/stdio.rs:323`); the `SubagentWait`-mechanics tests rewritten against the `MockDispatcher`. The `tests/harness_dispatch_native.rs`/`tests/harness_subagent_dispatch.rs` rewrites — the same seam. The tests to preserve (the acceptance contract): a successful dispatch resolves `Completed` with the captured final text (`SubagentCapture`); a crash resolves `Failed` with the exit code in the error; `settle_timeout` resolves the timeout outcome; the `SubagentCancel` aborts the dispatch; the model/launch resolution (the ADR 0020/0023 layering — now Supervisor-side, tested via the `WorkerManager`'s `dispatch_subagent` with canned settings/agent-definitions); the recursion guard (a subagent's `enabled_tools` minus `subagent`/`list_agents`); the `SubagentWait` select mechanics (the `MockDispatcher` — the outcome vs turn-cancel race).

**Steps:**
- [ ] Write the rewritten `subagent.rs` in-file tests + the `loop.rs` in-file subagent test rewrites + the `tests/harness_dispatch_native.rs`/`tests/harness_subagent_dispatch.rs` rewrites first (against the re-plumbed `dispatch_native` + the `fake_worker`/`MockDispatcher` seams) — confirm they FAIL (the old in-process driver is still in place).
- [ ] Implement the re-plumb (step 1) — delete the throwaway-DB machinery as you go (the build must stay green: delete only what the re-plumb stops using); the `commands/sessions.rs` routing (step 2); the `lib.rs` call site (step 3).
- [ ] Run `cargo test` — ALL tests pass; `rg "open_throwaway_db|TempFileGuard|CapturingSink" src-tauri/src` returns nothing (deleted); the `loop.rs` in-file `SubagentWait`-mechanics tests pass via the `MockDispatcher` (the rewritten tests).
- [ ] Run `cargo clippy --all-targets` (0 warnings) + `cargo fmt`.
- [ ] Commit: `feat(supervisor): subagents run in Worker processes — throwaway-DB machinery deleted, transcripts persist as ephemeral rows (ADR 0025)`

**Acceptance criteria:**
- [ ] `dispatch_native` (via the `WorkerManager`) spawns a subagent Worker per dispatch; no in-process child `AgentLoop` is built (a `rg "AgentLoop::new" src-tauri/src/agent/subagent.rs` returns nothing).
- [ ] `SubagentOutcome` semantics preserved: `Completed { output }` (the `SubagentCapture`'s final text — the `SinkFrame` `agent_message_chunk` source), `Failed { error }` (crash = the exit code in the message), the `settle_timeout` outcome.
- [ ] The throwaway-DB machinery is deleted (`open_throwaway_db`/`TempFileGuard`/`CapturingSink` gone).
- [ ] Subagent transcripts persist as `is_subagent = 1` rows (hidden from `list_sessions`, `load_history` works, never archived) — including the child's seq-0 system-message row (the `system_prompt` envelope field, Task 3's `build_child_system_message` computation).
- [ ] The subagent's progress still renders in the Client (the Tauri event contract unchanged — the subagent Worker's `SinkFrame`s relayed to the `TauriSink` under the subagent's session id, exactly as the current in-process child's frames did) + the `subagent-session-started`/`subagent-closed` lifecycle events (Task 3).
- [ ] The `respond_*` commands route through the `WorkerManager` (main + subagent sessions both resolve); the `trust-space`/permission semantics for subagent Workers are unchanged.

---

### Task 6: The stalled-session UI + observability (debug log, Supervisor crash hook)

**Context:**
The user-visible half of ADR 0025: a crashed session shows a banner (not a silent death), and the app finally has logs. The frontend is otherwise UNCHANGED (the spec's §3 — same commands, same events; `session-stalled` is the one new event, plus the `get_stalled_info` command from Task 4).

**Files:**
- Modify: `src/lib/tauri.ts` (the `session-stalled` event + `get_stalled_info` command typings — follow the existing event typings' pattern)
- Modify: `src/store/sessions.ts` (the session state — the `stalled` field + the crash-log path)
- Create: `src/components/SessionStalledBanner.tsx` (+ its test)
- Modify: the session view container (read the current layout — the banner slots above the transcript, below the header; the existing component structure decides — `ChatStream.tsx`'s parent)
- Create: `src-tauri/src/agent/debuglog.rs` (the `ARCHIMEDES_DEBUG` protocol log)
- Modify: `src-tauri/src/lib.rs` (the Supervisor panic hook — `crashlog::install_panic_hook("supervisor")` at `run()` start)
- Test: `src/components/SessionStalledBanner.test.tsx` (the existing frontend test convention — vitest + the existing test utilities)

**What to implement:**

1. **The banner** — `SessionStalledBanner`: shown when the session's state is `stalled` (the `sessions.ts` store — read its current shape and extend: a `stalled: { at: number, crash_log: string | null } | null` field set by the `session-stalled` event — the `crash_log` from the event payload (Task 4's `StalledInfo`), cleared by a successful resume). Content (the spec's §4 wording): *"Session stopped unexpectedly at <time> — last turn incomplete."* + a `[Resume]` button (calls `resume_session` — Task 4's path) + a secondary line with the crash-log path (a clickable file path — the existing `tauri-plugin-opener` `openPath` command, the repo already depends on it, when `crash_log` is non-null). The banner is dismissible (a local `useState` — the session stays stalled in the store; the banner reappears on re-open). Style: follow the existing design-system conventions (read `src/index.css` + a sibling component in `src/components/` for the error/warning visual language; the ADR 0006 design system).
2. **`tauri.ts`** — the `session-stalled` event typing + the `get_stalled_info` command typing (Task 4 added the command).
3. **The `ARCHIMEDES_DEBUG` log** (`debuglog.rs`): when `ARCHIMEDES_DEBUG=1` (read once at startup, cached — the test injects the value via the `test_support` `ENV_LOCK` pattern, or the module takes an explicit init in tests), the Supervisor logs to `~/.local/share/archimedes/supervisor.log` (the `dirs::data_dir()` home — the `crashlog.rs` dir-resolution pattern): every Worker lifecycle transition (spawn/ready/exit/crash, with the session id + exit code — the `WorkerManager`/`WorkerHandle` call `debuglog::log(&format!(…))` at the transition sites) + every IPC line (both directions — the `WorkerHandle`'s stdin writer + stdout reader log the raw lines, truncated to 2 KB per line — a streaming token stream would otherwise be unbounded; the truncation marker `"…"`). Rotation: on write, if the file exceeds 5 MB, rename to `supervisor.log.old` (overwrite the old) — a simple size check, no background task. With the var unset, `log` is a no-op (no file created — the cached check makes this free).
4. **The Supervisor panic hook** — `lib.rs` `run()`: `crashlog::install_panic_hook("supervisor")` as the first line (the Task-2 hook; the tag `"supervisor"`). The Supervisor's own crash still kills the app (it IS the app) — but now a `crash-<ts>-supervisor.log` exists, and the Workers exit cleanly on stdin EOF (Task 2's `run_worker` — the `worker_smoke` test's EOF case proves it).

**Steps:**
- [ ] Write the failing `SessionStalledBanner.test.tsx`: the banner renders when the store is stalled (assert the text + the `Resume` button); the `Resume` click calls `resume_session` (the existing test-mock pattern for Tauri commands — read a sibling test); the crash-log line renders + calls `openPath` when present; the banner is hidden when not stalled.
- [ ] Run `pnpm test` — confirm the new test FAILS.
- [ ] Implement the banner + the store field + the `tauri.ts` typings + the container wiring.
- [ ] Run `pnpm test` + `pnpm build` — pass.
- [ ] Write failing Rust tests: `debuglog` — with the debug flag on (the test's init seam), a `log` call writes a line to a temp file; the rotation renames at the size threshold; with the flag off, `log` is a no-op (no file created).
- [ ] Run `cargo test` — pass.
- [ ] Run `cargo clippy --all-targets` (0 warnings) + `cargo fmt` + `pnpm test` + `pnpm build`.
- [ ] Commit: `feat(observability): stalled-session banner + crash logs + ARCHIMEDES_DEBUG protocol log (ADR 0025)`

**Acceptance criteria:**
- [ ] A crashed session shows the banner (text per the spec) with a working `Resume` (the Task-4 path) and the crash-log link when a log exists.
- [ ] `ARCHIMEDES_DEBUG=1` → `supervisor.log` captures the lifecycle + IPC lines (truncated, rotated at 5 MB); unset → no file.
- [ ] A Supervisor panic writes `crash-<ts>-supervisor.log`; its Workers exit cleanly on EOF.

---

### Task 7: End-to-end integration test + full validation

**Context:**
The proof that the architecture works as a whole: a REAL `archimedes` binary (not the `fake_worker`) as a Worker, driven by the REAL Supervisor code (the `WorkerManager` production factory), against a CANNED provider (the existing wiremock dev-dependency — the `harness/provider.rs` tests' pattern: a mock HTTP server with canned SSE streams). This is the only task that exercises the full loop: Supervisor → spawn real Worker → real `AgentLoop` → mock provider SSE → tool execution → events + store frames back → persisted transcript (COMPLETE — the store-frame transport means the user/system/tool-role rows and compaction rewrites are all there, so a resume of the e2e session would work). Plus the final validation sweep (AGENTS.md).

**Files:**
- Create: `src-tauri/tests/worker_e2e.rs`
- Test only — no production code changes (if the e2e exposes a bug, fix it in the offending module, then re-run; a production-code fix in this task gets its own commit before the e2e commit)

**What to implement:**

1. **`tests/worker_e2e.rs`** — four `#[tokio::test]`s (or `#[test]`s driving a blocking runtime):
   - **The full turn.** Resolve the built binary: `env!("CARGO_BIN_EXE_archimedes")` (integration target — set by cargo for the package's binaries). Start a wiremock server (the existing dev-dependency) with a canned OpenAI-compatible SSE stream (read `harness/provider.rs`'s tests for the canned-stream fixtures — a `chat.completions` stream: a few `text_delta`s + a `toolcall` + a `finish_reason: tool_calls` + a second request for the tool-result turn → a final `stop`). Build a `WorkerManager` (the production factory — `WorkerHandle::spawn(env!("CARGO_BIN_EXE_archimedes").as_ref())`; the `WorkerFactory` seam makes this direct) with a `TranscriptPersister` on a temp-dir `Db` + a collecting `on_event` (an `mpsc` channel the test drains). `attach` a session with a `StartEnv` pointing at the wiremock provider (the `Model` with the wiremock `base_url` + a dummy `api_key` + the `openai-completions` wire; a trusted Space; `enabled_tools: Some(vec!["bash"])` — the wire's HARNESS convention: `Some(v)` = exactly `v` (the settings `[]` = all mapping is at `StartEnv` construction — this test builds the `StartEnv` directly, so it passes the harness shape); the effective `ModelCatalog` — the base catalog + the wiremock provider, built via the existing `merge_catalog`/`discover_models` helpers on the wiremock's `GET /models` canned response; `system_prompt: None`). Send a `prompt`. Assert, in order: the `ready` handshake; the `turn_start`/`message_*`/`tool_execution_*`/`agent_settled` `RpcEvent` sequence (the bookkeeping stream); the `SinkFrame` `session-update` frames on the UI side (the frontend contract — verbatim); the `native_messages` rows in the temp `Db` — the COMPLETE transcript: the system prompt (seq 0 — the Worker's `build_main_prompt` + `prepend_system`), the user message, the assistant message with the `tool_calls`, the tool-role message with the result, the final assistant text (the store frames applied by the `TranscriptPersister`); the `messages` display rows (the `agent-text`/`tool-call`/`tool-result` upserts); a clean `close` → exit 0 + the `session-closed` bookkeeping (the `Exited` expected path — the `on_crash` callback did NOT fire).
   - **The permission round-trip.** A non-trusted Space + a canned `bash` tool call (the same wiremock stream) → the `PermissionRequest` outbound (the FULL gate payload verbatim — assert the `request.options` list is present) → the test sends `PermissionResponse { outcome: Selected { option_id: "allow" } }` → the tool-execution events follow (the `fake_worker` does a canned version; this test does it with the REAL worker — the real `AgentLoop` permission gate, the real `StaticTrustSource` fail-closed path).
   - **The crash path.** A `StartEnv` whose provider `base_url` is a dead port (connection-refused on every model call — the harness's retry policy will exhaust; to force a FAST deterministic crash, use a provider config that makes the `Provider` construction panic — read `harness/provider.rs`'s `build_provider` for the exact failure mode: if it returns an `Err` instead of panicking, the crash test instead `SIGKILL`s the spawned Worker from the test — the `WorkerHandle`'s `kill` — and asserts the `on_crash` path). Assert: `on_crash` fires (the `WorkerManager` callback — the `SessionManager`'s stalled marking is Task 4's; here the raw callback) + a `crash-*.log` file exists (the panic path — with a NON-EMPTY backtrace section, the `force_capture` behavior) + the `TranscriptPersister`'s rows are intact (everything before the crash persisted — the pre-crash user message row).
   - **The subagent dispatch (closes the round-2 gap — no prior test covered the subagent path end-to-end).** A main-session Worker (the full-turn setup, `subagent_enabled: true`, the canned main SSE stream containing a `subagent` tool call — `task` + an `agentName` resolving against a canned agent definition — **the fixture: isolate `HOME`/config-dir per the existing `test_support` `ENV_LOCK` pattern and place the canned agent definition in the fixture root (mine the existing `harness_subagent_dispatch.rs`/`subagent.rs` tests for the `agentName`-resolution fixture pattern — the `resolve_launch` pre-flight reads discovered agent definitions from the `~/.pi/agent` / `~/.agents` roots)**) → the main Worker's `IpcDispatcher` emits `SubagentDispatch` (the resolved `model_key` — the pre-flight against the Worker's catalog) → the Supervisor's `dispatch_subagent` resolves the key → spawns a SUBAGENT Worker (a second wiremock — the subagent's canned stream: a short text answer) with the child `StartEnv` (`system_prompt` = the `build_child_system_message` output — assert the child's `native_messages` seq 0 = that system message; `enabled_tools` = `Some(child_tools)` verbatim — the three-way rule applied, the wire's harness convention; `subagent_enabled: false`) → assert: the `subagent-session-started` + `subagent-closed` events on the collector (the `subagent.rs:635`/`721/743` payload shapes, `metrics` populated); the `SubagentResult` (`Completed` with the `SubagentCapture`'s final text) delivered to the main Worker (the main's `subagent` tool resolves); the ephemeral `is_subagent = 1` row (the `TranscriptPersister`'s `ensure_session_row` — hidden from `list_sessions`, `load_history` works); a clean subagent Worker exit (no `on_crash`).
2. **The validation sweep** (AGENTS.md — the branch-ready gate):
   - `cargo test` (in `src-tauri/`) — all green.
   - `cargo clippy --all-targets` — 0 warnings.
   - `cargo fmt --check` — clean.
   - `pnpm test` (repo root) — all green.
   - `pnpm build` (repo root) — green.

**Steps:**
- [ ] Write `tests/worker_e2e.rs` (the four tests above).
- [ ] Run `cargo test -p archimedes --test worker_e2e` — confirm they FAIL (the full loop isn't wired end-to-end yet — or they pass partially if Tasks 2-6 already cover fragments; the e2e's assertions on the COMPLETE transcript + the subagent dispatch + the crash path are what's new).
- [ ] Fix any production bugs the e2e exposes (each fix: its own failing test first, then the fix, then a commit).
- [ ] Run the full validation sweep (all five commands) — all green.
- [ ] Commit: `test: end-to-end Worker integration (real binary + canned provider) — ADR 0025 validation sweep green`

**Acceptance criteria:**
- [ ] The e2e test drives a REAL `archimedes --worker` through a full turn (mock provider SSE → tool call → final answer) with the events + the COMPLETE persisted transcript asserted (system + user + assistant + tool-role rows — a resume of the session would work) + the `SinkFrame` UI contract + the clean-exit bookkeeping.
- [ ] The permission round-trip works with the real Worker (the real `AgentLoop` permission gate, the fail-closed `StaticTrustSource`, the full gate payload verbatim).
- [ ] The crash path degrades cleanly (`on_crash` + the crash log with a non-empty backtrace + the persisted rows intact).
- [ ] The subagent dispatch works end-to-end with real Workers (the `SubagentDispatch` → `dispatch_subagent` → subagent Worker → `SubagentResult` flow, the `system_prompt` envelope field, the `subagent-session-started`/`subagent-closed` lifecycle events, the ephemeral `is_subagent = 1` row).
- [ ] All five validation commands green (AGENTS.md branch-ready).

---

## Appendix: the approved spec (reference for the reviewer)

The approved design (2026-10-04) — ADR 0025. **Mechanism refinement (post-review):** the ADR/spec phrase "the Supervisor persists from the event stream" is refined by this plan: the Supervisor persists from the Worker's **store frames** (the `Store` seam over IPC — `TranscriptInsert`/`TranscriptReplace`/`DisplayUpsert`/`ContextUsage`); the **UI** is driven by the **`SinkFrame`s** (the normalized sink frames, re-emitted verbatim); the raw `RpcEvent` stream drives the Supervisor's **internal bookkeeping only** (settle detection, `pending_turn` resolution, the debug log — never the UI, to avoid double-delivery of the `session-update` frames). The DECISION is unchanged: the Supervisor is the sole SQLite writer; the Worker has no DB. The full spec text as approved: see git history commit `1a47f82` (the spec version in this file, superseded by the plan per the specify convention).
