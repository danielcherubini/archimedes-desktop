---
status: committed
done-when: A native (Archimedes) session's `subagent` tool call spawns a fully in-process native child session (no pi subprocess, no bridge, no WorkerRuntime) that runs the delegated task with the parent's model (or a per-call `model`/`thinking` override), streams its progress to the UI via the existing subagent panel, returns its final output as the tool result, and cannot itself dispatch subagents — while external (pi) sessions' `subagent` behavior is unchanged.
---

# Native → Native Subagent Dispatch Plan

**Goal:** Make a native (Archimedes) session's `subagent` tool spawn a fully in-process native child session (a new `AgentLoop`), replacing the current external-pi-child behavior.

**Architecture:** Extend `SubagentSessionManager` with a `dispatch_native` method (+ a `NativeDeps` field set via a `&self` setter using `OnceLock`). The native parent's `dispatch_subagent` (loop.rs) calls `dispatch_native` instead of the pi `dispatch`. `dispatch_native` spawns a **driver task in the main runtime** that: builds a child `AgentLoop` (inheriting the parent's model/tools minus `subagent`, `subagent: None`, a `CapturingSink`, a throwaway `Db`), spawns the child loop, drives it (sends the task, races the child's settle vs `settle_timeout` vs the returned `SubagentCancel`, unconditionally tears the child down on every exit), and resolves a `SubagentOutcome`. The child's frames flow to the UI **once** (through the `CapturingSink` → the parent's real sink); the driver is **capture-only** (it does NOT re-normalize/re-emit). External pi parents' `subagent` (the bridge `dispatch`) is unchanged.

**Tech Stack:** Rust (Tauri 2), tokio (multi-thread runtime), the existing `AgentLoop` / `ModelCatalog` / `Provider` / `SessionStore` / `EventSink` / `normalize` machinery.

**Key references (read these before starting):**
- `src-tauri/src/agent/subagent.rs` — `SubagentSessionManager` (struct ~line 120, `dispatch` ~line 226, the worker task ~line 300-520: emits `subagent-session-started`/`subagent-closed`, `captures(&driver, ...)` for `output`/`metrics`, the **unconditional** `task_cancel.cancel()` teardown at ~line 465 — mirror this), `SubagentOutcome` (`Completed { output, metrics: SubagentMetrics }` / `Failed { error }` — no `Cancelled` variant; "cancelled" is `Failed { error: "cancelled" }`), `SubagentCancel::new_external_close()` (`pub(crate)`), `SubagentMetrics` + `metrics_json` (emits `output`/`inputTokens`/`outputTokens`/`cost`/`durationMs`).
- `src-tauri/src/agent/session.rs` — `SessionManager` (struct ~line 1544: `driver`, `catalog`, `provider_factory` — all **private to the `session` module**), `ProviderFactory` type alias (~line 1528, currently private — make `pub`), `set_subagent_manager` (~line 1707, `&mut self` — the wiring site), `build_native_session` (~line 1834; the `AgentLoop::new` call is at ~1893-1922 — the template), `drive_native_session` (~line 1257 — treats `events_rx` as a **liveness signal only**, does NOT normalize/emit), `drive_session` (~line 650; its `text_capture` block ~line 957-985 — the template for `messageId`-keyed text capture from normalized frames), `normalize` (~line 3059, `pub(crate) fn normalize(e: &RpcEvent, st: &mut TurnState) -> Vec<Value>`), `TurnState` (from `harness/loop.rs`), `resolve_composed_model` (~line 2810, currently **private** — make `pub(crate)`).
- `src-tauri/src/agent/harness/loop.rs` — `AgentLoop` (struct ~line 106: `pub model` field, `set_model` ~248, `set_thinking_level` ~255, `set_enabled_tools` ~262, `enabled_tools` **private field** — add a getter, `thinking_level` **private field, no getter** — add one, `pub async fn run(mut self)` ~line 340 — **consumes** the loop + owns both `prompt_tx` and `control_tx`, so `run()` exits ONLY via `self.cancel`), the `emit` helper (~line 1251-1283: normalizes + `sink.emit("session-update", {sessionId, update})` + `persist_update` + `events.try_send`), `dispatch_subagent` (~line 947, signature `async fn dispatch_subagent(&mut self, params: &Value, turn: &CancellationToken) -> ToolResult` — the `turn` param is the current turn token; the `select!` arm on `turn.cancelled()` calls `cancel.cancel()` + maps to `SubagentWait::Cancelled`), `SudoDeps` (~line 88), `tool_specs()` (~line 1338, **private** free fn returning `Vec<ToolSpec>` — the 11 tool names: bash, read, write, edit, find, grep, ls, ask, sudo_exec, manage_todo_list, subagent; `dispatch_tool` also accepts the alias `"dispatch_subagent"` ~line 878).
- `src-tauri/src/agent/harness/catalog.rs` — `ModelCatalog` (`get(id)` ~line 105 matches the **bare** model id; `openai_compatible()` ~line 116).
- `src-tauri/src/agent/rpc.rs` — the `RpcEvent` variants. **The text-delta carrier is `RpcEvent::message_update { assistant_message_event: json!({"type":"text_delta","delta":…}), usage }`** (loop.rs:528-537). There is NO `message_chunk` / `agent_message_chunk` `RpcEvent` variant — `agent_message_chunk` is the **normalized output frame** name. `messageId` is derived from `message_start` / `TurnState.current_message_id` (session.rs:3072-3080), NOT on the delta event.
- `src-tauri/src/agent/storage/db.rs` — `Db::open` (~line 95: `PRAGMA foreign_keys=ON` — `native_messages` FKs to `sessions`).
- `src-tauri/tests/harness_subagent_dispatch.rs` — the existing end-to-end test to update (Task 3; has `FAKE_PI`, `write_pi_agents_json` with `bridge: true`, a file-local `MockProvider` + `RecSink`, and a `contains("Hello")` assertion). `MarkerCounter` lives in `tests/subagent_dispatch.rs` (NOT here).
- `src-tauri/tests/common/` — currently only `proc_scan`; **add `MockProvider` + `RecSink` here** (Task 2) so both `harness_subagent_dispatch.rs` and the new `harness_dispatch_native.rs` can `mod common;` reuse them.

**Conventions:** TDD (failing test → confirm it fails → implement → pass). Validation order per task: `cargo fmt` → `cargo build` → `cargo test --no-fail-fast` → `cargo clippy --all-targets` (0 warnings) — run the `cargo` commands **from `src-tauri/`**. Frontend (from the repo root): `pnpm test` + `pnpm build` — this feature is Rust-only (the frontend is unchanged), so the frontend gate is a no-op (run it to confirm nothing broke). Do NOT change the external `PiRpc` path, the `AgentLoop` `run()` core, `Provider`, `Compactor`, or `RetryPolicy`. The child is EPHEMERAL (its transcript is discarded, not read back).

**Cross-cutting design rules (apply to every task):**
- **One emission**: the child's frames reach the UI exactly ONCE. The child's `emit` helper already normalizes + `sink.emit`s; the driver must NOT re-normalize/re-emit. Use a `CapturingSink` decorator (below) so the child's `emit` → `CapturingSink.emit` → forwards to the parent's real sink (one emission) + captures the `agent_message_chunk` text.
- **Unconditional child teardown**: after the driver's wait resolves (settle OR timeout OR cancel), ALWAYS `child_cancel.cancel()` + `child_turn_cancel.lock().unwrap().cancel()` + `loop_handle.abort()` + `let _ = loop_handle.await;` + drain `events_rx` until `None`. (The child `run()` owns both `prompt_tx` + `control_tx`, so it exits ONLY via `child_cancel` — dropping the `JoinHandle` does NOT stop it.)
- **Throwaway child `Db`**: the child's `SessionStore` uses a throwaway `Db` (`Db::open(Path::new(":memory:"))` if supported — `Db::open` takes `&Path` — else a temp file deleted on teardown) + a `record_session` for the child (so the `native_messages` FK is satisfied). The child's `persist_update`/`persist_transcript_message` write to the throwaway (discarded) — never the parent's real `Db`.

---

### Task 1: `NativeDeps` + `set_native_deps` (`&self`, `OnceLock`) + the wiring

**Context:** `dispatch_native` (Task 2) needs the pieces `build_native_session` uses to build a native `AgentLoop`: a `ProviderFactory`, a `ModelCatalog`, a `TodoStore`, and `SudoDeps`. (A `Db` is ALSO needed — but for the child's THROWAWAY `SessionStore` base + `record_session`, and it is built FRESH inside `dispatch_native` (Task 2 step 4) as `Db::open(Path::new(":memory:"))` — so it is NOT a `NativeDeps` field.) The `SubagentSessionManager` (subagent.rs) is built for external pi dispatch and does NOT have these. The wiring site is `SessionManager::set_subagent_manager(&mut self, m: Arc<SubagentSessionManager>)` (session.rs:1707) — it has `&mut self` on the `SessionManager` (which owns `driver`/`catalog`/`provider_factory`, all private to the `session` module) but receives the manager as an **`Arc`** (so a `&mut self` setter on the manager can't be called through it). Fix: `set_native_deps` takes `&self` + stores into a `OnceLock<NativeDeps>` (set-once at wiring time), so it's callable through the `Arc`. `dispatch_native` reads `self.native_deps.get()`.

**Files:**
- Modify: `src-tauri/src/agent/subagent.rs`
- Modify: `src-tauri/src/agent/session.rs`
- Test: `src-tauri/src/agent/subagent.rs` (the existing `#[cfg(test)] mod tests` — a pure unit test; do NOT use `tests/subagent_dispatch.rs`, which is the external fake_pi/bridge e2e suite)

**What to implement:**
- In `session.rs`, make the `ProviderFactory` alias `pub` (line 1528): `pub type ProviderFactory = Arc<dyn Fn(&Model) -> Box<dyn Provider> + Send + Sync>;`.
- In `subagent.rs`, add a `NativeDeps` struct (public, `Clone`):
  ```rust
  pub struct NativeDeps {
      pub provider_factory: ProviderFactory,   // import from session.rs (now pub)
      pub catalog: ModelCatalog,
      pub todo_store: Arc<TodoStore>,      // NO db field — the throwaway child Db is built fresh in dispatch_native (Task 2 step 4)
      pub sudo: SudoDeps,
      pub settle_timeout: Duration,           // the child's settle bound (default 30 min; Task 4 tests shorten it)
  }
  ```
  (Import `ProviderFactory`, `ModelCatalog`, `TodoStore`, `SudoDeps`, `Duration` from their modules. `NativeDeps` starts with these 5 fields; Task 4 does NOT add a 6th — `settle_timeout` is here from the start.)
- Add a field to `SubagentSessionManager`: `native_deps: std::sync::OnceLock<NativeDeps>` (in `new`, leave it unset — `OnceLock::new()`).
- Add a setter that takes `&self`: `pub fn set_native_deps(&self, deps: NativeDeps) { let _ = self.native_deps.set(deps); }` (set-once; a second call is a no-op).
- Add a getter: `pub fn native_deps(&self) -> Option<&NativeDeps> { self.native_deps.get() }`.
- In `session.rs`, in `set_subagent_manager` (~line 1707), after receiving `m: Arc<SubagentSessionManager>`, wire the deps (the `SessionManager` has `&mut self` + owns all the pieces):
  ```rust
  if let Some(_db) = &self.driver.db {   // the db is the SIGNAL that native wiring is present; it is NOT passed to NativeDeps
      m.set_native_deps(NativeDeps {
          provider_factory: self.provider_factory.clone(),
          catalog: self.catalog.clone(),
          todo_store: self.driver.todo_store.clone(),
          sudo: SudoDeps {
              runner: self.driver.runner.clone(),
              pending_sudo: self.driver.pending_sudo.clone(),
              sudo_password: self.driver.sudo_password.clone(),
          },
          settle_timeout: self.driver.settle_timeout,
      });
  }
  ```
  (If `self.driver.db` is `None`, skip — the manager stays external-pi-only. In production the native-session path always `attach_db`s, so this is set.)
- **Do NOT** change `SubagentSessionManager::new`'s signature or the `dispatch` (pi) method.

**Steps:**
- [ ] Write a failing unit test in `subagent.rs` `#[cfg(test)] mod tests`: a `SubagentSessionManager::new` (temp config dir) has `native_deps().is_none()`; after `set_native_deps(<a NativeDeps built with a mock provider_factory — the 5-field struct, NO db field>)`, `native_deps().is_some()`; a second `set_native_deps` is a no-op (still the first).
- [ ] Run `cargo test --lib agent::subagent` — confirm it fails (no `native_deps` getter yet).
- [ ] Implement `NativeDeps` + the `OnceLock` field + `set_native_deps` (`&self`) + `native_deps()` getter in subagent.rs; make `ProviderFactory` `pub` in session.rs; wire `set_native_deps` in `set_subagent_manager`.
- [ ] Run `cargo test --lib agent::subagent` — confirm it passes.
- [ ] Run `cargo fmt` → `cargo build` → `cargo test --no-fail-fast` → `cargo clippy --all-targets` (0 warnings).
- [ ] Commit: "feat(harness): NativeDeps + set_native_deps (&self, OnceLock) on SubagentSessionManager"

**Acceptance criteria:**
- [ ] `NativeDeps` exists with the 5 fields (incl. `settle_timeout`; NO `db` field); `SubagentSessionManager` has `native_deps: OnceLock<NativeDeps>` + `set_native_deps(&self)` + `native_deps()`.
- [ ] `ProviderFactory` is `pub` (importable from subagent.rs).
- [ ] `set_subagent_manager` wires `set_native_deps` (skipped when `driver.db` is `None`).
- [ ] `SubagentSessionManager::new` signature + the `dispatch` (pi) method are unchanged.

---

### Task 2: `CapturingSink` + `dispatch_native` core

**Context:** This is the heart of the feature. `dispatch_native` spawns a **driver task in the main runtime** that builds a child `AgentLoop` (inheriting the parent's model/tools minus `subagent`, `subagent: None`, a `CapturingSink`, a throwaway `Db`), spawns the child loop, drives it (sends the task, races the child's settle vs `settle_timeout` vs the returned `SubagentCancel`, unconditionally tears the child down on every exit), and resolves a `SubagentOutcome`. The child's frames flow to the UI **once** (through the `CapturingSink` → the parent's real sink); the driver is **capture-only** (it does NOT re-normalize/re-emit — that would double-emit, since the child's `emit` helper already normalizes + `sink.emit`s).

**Files:**
- Modify: `src-tauri/src/agent/subagent.rs`
- Modify: `src-tauri/src/agent/harness/loop.rs` (add `enabled_tools()` + `thinking_level()` getters; make `tool_specs()` `pub(crate)`)
- Modify: `src-tauri/src/agent/session.rs` (make `resolve_composed_model` `pub(crate)`)
- Test: `src-tauri/tests/harness_dispatch_native.rs` (new) + `src-tauri/tests/common/` (add `MockProvider` + `RecSink`)

**What to implement:**
- **`CapturingSink`** (new, in subagent.rs or a small `harness` helper — public): an `EventSink` decorator wrapping `Arc<dyn EventSink>` (the parent's real sink) + a `StdMutex<HashMap<String, String>>` (messageId → accumulated text) + a `last_message_id: StdMutex<Option<String>>` (updated on every non-empty capture — a `HashMap` has no insertion order, so the LAST id is tracked separately, mirroring `captures()` subagent.rs:649-677 + session.rs:974-982). **CRITICAL — the frame is ENVELOPED**: the child's `emit` helper (loop.rs:1257-1262) does `self.sink.emit("session-update", json!({ "sessionId": self.session_id, "update": u }))` — so `EventSink::emit` receives the **envelope** `{ sessionId, update }`, NOT the bare normalized frame. `emit(&self, event, payload)`: if `event == "session-update"` AND `payload["update"]["sessionUpdate"] == "agent_message_chunk"`, let `mid = payload["update"]["messageId"]` + `delta = payload["update"]["content"]["text"]` (`.as_str()`, skip empty); if `mid` + `delta` present, append `delta` to `map[mid]` + set `last_message_id = mid`; ALWAYS `real.emit(event, payload)` (forward — one emission). Add `fn captured_text(&self) -> String` (the `last_message_id`'s accumulated text — mirroring `captures()` so a tool-using turn doesn't concatenate every intermediate message; empty if none) + `fn captured_ids(&self) -> Vec<String>` (arbitrary order).
- **`AgentLoop` getters** (loop.rs): `pub fn enabled_tools(&self) -> &[String]` + `pub fn thinking_level(&self) -> Option<&str>` (both read the private fields). Make `tool_specs()` `pub(crate)` (it's a private free fn ~line 1338) so `dispatch_native` can derive the tool-name set.
- **`resolve_composed_model`** (session.rs ~line 2810): make it `pub(crate)`. NOTE — the REAL signature is `fn resolve_composed_model(catalog: &ModelCatalog, key: &str) -> Option<Model>` (splits `provider/id` + matches `m.provider == provider && m.id == id`; it does NOT split a `:<level>` suffix and returns NO thinking level) — so the `:<level>` suffix is stripped in `dispatch_native` step 1 BEFORE calling it, and a `None` outcome is a `Failed { "unknown model" }` (see Task 2 step 1).
- **`dispatch_native`** (subagent.rs):
  ```rust
  pub fn dispatch_native(
      &self,
      parent_session_id: &str,
      parent_cwd: &Path,
      parent_model: &Model,
      parent_enabled_tools: Vec<String>,
      agent_name: String,
      launch: LaunchConfig,
      task: String,
      sink: &Arc<dyn EventSink>,
  ) -> (oneshot::Receiver<SubagentOutcome>, SubagentCancel)
  ```
  - If `self.native_deps()` is `None`, return an already-resolved oneshot with `SubagentOutcome::Failed { error: "native subagent dispatch is not configured" }` (NO pi fallback — a native parent always has an attached `db` in production; the "no manager" case is handled by `dispatch_subagent`'s existing `subagent: None` check).
  - Otherwise `let (ec, sub_cancel) = SubagentCancel::new_external_close();` (`ec: ExternalClose` carries `ec.rx: watch::Receiver<bool>` — the driver watches it; `sub_cancel: SubagentCancel` is the handle returned to the caller) + `tokio::spawn` a **driver task in the main runtime** (NOT the `WorkerRuntime`) that:
    1. **Resolve the child `Model`**: if `launch.model` is `Some(key)`, FIRST split a trailing `:<level>` suffix off the key (`let (bare, suffix) = key.rsplit_once(':')` — the `suffix` is a candidate thinking level, used in step 2 when `launch.thinking` is `None`); then `let model = match resolve_composed_model(&deps.catalog, bare) { Some(m) => m, None => { return a resolved oneshot with `SubagentOutcome::Failed { error: format!("unknown model: {bare}")` } }` (the REAL `resolve_composed_model(catalog, key) -> Option<Model` (session.rs:2811) splits `provider/id` + matches `m.provider == provider && m.id == id` — it does NOT split a `:<level>` suffix, which is why we strip it first); else `parent_model.clone()`.
    2. **Resolve the child thinking level**: `launch.thinking.clone().or_else(|| suffix.map(str::to_string))` (the `:<level>` suffix from step 1, when `launch.thinking` is `None`); validate `level ∈ model.thinking_levels` (when `model.thinking_levels` is non-empty) — on mismatch, drop it (use `None`) rather than send a bogus `reasoning_effort` upstream.
    3. **Build the child `Provider`**: `(deps.provider_factory)(&model)`.
    4. **Build the throwaway child `Db` + `SessionStore`**: `let child_db = Arc::new(Db::open(Path::new(":memory:")).unwrap_or_else(|_| Db::open(<a temp file>).unwrap()));` (SQLite honors a `":memory:"` filename via `Connection::open`; the `parent()` of `":memory:"` is `""` so `create_dir_all` is a no-op `Ok` — but `Db::open` takes `&Path`, so use `Path::new(":memory:")`, NOT the `&str` literal; `SessionStore::new` takes `Arc<Db>`, hence the `Arc::new` wrap) + `child_db.record_session(&SessionInfo { session_id: <child_uuid>, agent_id: "native".into(), cwd: parent_cwd.to_path_buf(), capabilities: json!({}), config_options: None })` (FK satisfied — `SessionInfo` has no `Default`, fill all fields) + `let store = SessionStore::new(child_db)`. (The child's `persist_update`/`persist_transcript_message` write to the throwaway — discarded on teardown, never the parent's real `Db`. `NativeDeps` does NOT carry a `db` field — the throwaway is built fresh here.)
    5. **Build the `CapturingSink`**: `let capturing = Arc::new(CapturingSink::new(sink.clone()));` (wraps the parent's real sink).
    6. **Build the child `AgentLoop`** via `AgentLoop::new(...)` — **reuse the full `AgentLoop::new` arg list from `build_native_session`** (session.rs:~1893-1922), with these differences:
       - `session_id`: a fresh `uuid::Uuid::new_v4().to_string()`.
       - `provider`: the child provider (step 3).
       - `store`: the throwaway `SessionStore` (step 4).
       - `sink`: the `CapturingSink` (step 5) — the child's `emit` → `CapturingSink.emit` → the parent's real sink (one emission) + text captured.
       - `subagent`: `None` (recursion guard — the child cannot dispatch subagents).
       - `pending_permissions` / `pending_bridge`: the **manager's shared** maps (`self.driver.pending_permissions.clone()` / `self.driver.pending_bridge.clone()`) — so the UI's `respond_permission`/`respond_bridge_request` (which search the manager's shared maps) can resolve the child's prompts (fresh maps would hang the child until `settle_timeout`).
       - `sudo`: `deps.sudo.clone()`; `todo_store`: `deps.todo_store.clone()`; `trust_db`: `None` (the child is ephemeral; threading the parent's trust is a follow-up — document this).
       - `child_cancel` (a fresh `CancellationToken`) / `child_turn_cancel` (a fresh `Arc<StdMutex<CancellationToken>>`) / `settle_tx` / `settle_rx` / `prompt_tx` / `prompt_rx` / `events_tx` / `events_rx`: FRESH channels/tokens for the child (NOT the parent's). **`prompt_tx` is passed by VALUE to `AgentLoop::new`** — so `prompt_tx.clone()` goes into `new` and the driver KEEPS the original clone for step 10 (mirror `build_native_session` session.rs:1905+1926). The teardown (step 12) cancels `child_cancel` + `child_turn_cancel`.
    7. **Configure the child** (ALL of this happens BEFORE `run()` consumes the loop — `run(mut self)` moves it, so nothing may touch `loop_` after the spawn): `loop_.set_enabled_tools(<child tool set>)` where the child tool set = (if `launch.tools` is `Some(t)`, `t` else `parent_enabled_tools`) **minus `"subagent"`** — apply the "empty = all" expansion ONLY to the inherited `parent_enabled_tools` case (where `[]` is the documented harness convention, loop.rs:157-158); a non-empty `launch.tools` that empties out after the minus yields *no* tools (do NOT re-expand); if `parent_enabled_tools` is empty (= all), use `tool_specs()`'s names minus `"subagent"`. If step 2's level is `Some`, `loop_.set_thinking_level(Some(level))`. If `launch.system_prompt` is `Some(sp)`, `loop_.prepend_system(sp)` (a new `pub fn prepend_system(&mut self, text: String)` on `AgentLoop` that pushes a `System` `ChatMessage` to the FRONT of `self.messages` + calls `self.compactor.reestimate(&self.messages)` — mirroring `load_transcript` loop.rs:269-271; it's feasible because `model_request()` (loop.rs:1184-1196) sends `messages: self.messages.clone()`). Do ALL of step 7 BEFORE step 8.
    8. `let loop_handle = tokio::spawn(loop_.run());` (keep the `JoinHandle` — `run(mut self)` consumes the loop + owns `prompt_tx` + `control_tx`, so `run()` exits ONLY via `self.cancel`).
    9. **Emit `subagent-session-started`** on the parent's real `sink` (NOT the `CapturingSink` — the driver emits lifecycle events directly): `json!({ "sessionId": <child_id>, "parentSessionId": parent_session_id, "agentName": agent_name, "task": task, "model": <the COMPOSED "provider/id" form, matching native_capabilities session.rs:2850>, "thinkingLevel": <the step 2 RESOLVED level, driver-local — do NOT call `loop_.thinking_level()` here, since `loop_` is consumed by `run()` in step 8>, "enabledTools": <child tool set> })` (the `model`/`thinkingLevel`/`enabledTools` fields make the child's resolved config **observable** for the tests — Major #8).
    10. **Send the task**: `prompt_tx.send(Prompt { text: task }).await` using the KEPT clone (step 6) (the preflight — a `SendError` means the child died before the turn started → `Failed`). (The `system_prompt` was already seeded in step 7 — BEFORE the task prompt.)
    11. **Race** a `select!` on: the child's `settle_rx` (a fresh `watch` receiver — the child writes it on `agent_settled`; **`changed()` resolving `Err(())` means the child loop task DIED (the `settle_tx` was dropped) — map that to `Failed`, NOT `Completed`** — mirror `drive_native_session` session.rs:1303-1314) vs `tokio::time::sleep(deps.settle_timeout)` vs `ec.rx.changed()` (the `SubagentCancel` handle — a user/caller cancel).
    12. **On ANY exit (settle / timeout / cancel / preflight error) — UNCONDITIONAL teardown**: `child_cancel.cancel()` (the child's session `CancellationToken` — the `cancel` arg from step 6; renamed to avoid colliding with the `sub_cancel` handle) + `child_turn_cancel.lock().unwrap().cancel()` (the child's `turn_cancel` token) + `loop_handle.abort()` + `let _ = loop_handle.await;` + **drain `events_rx` until `None`** (`while events_rx.recv().await.is_some() {}` — after `loop_handle.await` the child task has ended, so its `events_tx` (a struct field) is dropped and `recv()` returns `None`; if it does NOT return `None`, the child is orphaned and this blocks → the oneshot never resolves → the test times out, which is the observable failure) + drop the throwaway `child_db` (temp file deleted if used). (The child `run()` owns both `prompt_tx` + `control_tx`, so it exits ONLY via `self.cancel` — `abort()` is the belt-and-braces for a child hung inside a provider HTTP read that ignores cancellation.)
    13. **Compute the outcome + emit `subagent-closed`** on the parent's real `sink`:
        - *Settle (preflight Ok + settle Ok, NOT the `Err`-died arm)*: `output` = `capturing.captured_text()` (the `last_message_id`'s accumulated text); `metrics` = `SubagentMetrics { output: output.clone(), duration_ms: <elapsed>, ..Default::default() }` (mirror `captures()` — subagent.rs); `sink.emit("subagent-closed", json!({ "sessionId": <child_id>, "status": "completed", "metrics": <metrics_json> }))`; resolve `SubagentOutcome::Completed { output, metrics }`. (Note: a failed child TURN — an exhausted-retry model call — settles "successfully" via a bare `agent_settled` (loop.rs:754-765) and reports `Completed { output: "" }`; this is parity with the pi `dispatch` mapping and a documented follow-up, NOT a bug to fix here.)
        - *Preflight error / timeout / cancel / settle-`Err` (child died)*: `error` = `"cancelled"` (if `ec`'s flag is flipped) / `"timed out"` / the preflight error / `"child process died"` (the settle-`Err` arm); `metrics` = `SubagentMetrics { ..Default::default() }` with `duration_ms`; `sink.emit("subagent-closed", json!({ "sessionId": <child_id>, "status": "failed", "error": <error>, "metrics": <metrics_json> }))`; resolve `SubagentOutcome::Failed { error }`.
  - Return `(dispatch_rx, sub_cancel)` (`sub_cancel` is the `SubagentCancel` the driver watches via `ec.rx`; the caller's `sub_cancel.cancel()` flips the flag the `ec.rx.changed()` arm sees). Add `#[allow(clippy::too_many_arguments)]` to `dispatch_native` (8 params — the existing `dispatch` at subagent.rs:225 does exactly this).

**Steps:**
- [ ] Move `MockProvider` + `MockResponse` + `RecSink` from `harness_subagent_dispatch.rs` into `tests/common/mod.rs` (or `tests/common/mock_provider.rs` + `tests/common/rec_sink.rs`); update `harness_subagent_dispatch.rs` to `mod common;` + `use common::{MockProvider, MockResponse, RecSink};` (it keeps compiling).
- [ ] Write a failing test in `tests/harness_dispatch_native.rs`: build a `SubagentSessionManager` via `new` + `set_native_deps` (a mock `provider_factory` returning a canned `TextDelta`→`Done(Stop)` stream + a temp `Db`), call `dispatch_native`, and assert: the oneshot resolves `SubagentOutcome::Completed { output }` containing the mock's text; the (real) sink received `subagent-session-started` (with `model`/`thinkingLevel`/`enabledTools`) + `subagent-closed { status: "completed" }` + the mock's `agent_message_chunk` frame **exactly once** (assert the sink saw the frame once — NOT twice); and AFTER `Completed`, the child's `events_rx` closes (the loop task ended — the unconditional teardown, Critical #1).
- [ ] Run `cargo test --test harness_dispatch_native` — confirm it fails (`dispatch_native` + `CapturingSink` don't exist yet).
- [ ] Implement `CapturingSink` + the `AgentLoop` getters (`enabled_tools()`, `thinking_level()`) + `prepend_system` + `tool_specs()` `pub(crate)` + `resolve_composed_model` `pub(crate)` + `dispatch_native`.
- [ ] Run the test — confirm it passes (incl. the "frame exactly once" + "events_rx closes after Completed" assertions).
- [ ] Add a recursion-guard assertion: a `subagent-session-started` payload's `enabledTools` excludes `"subagent"`.
- [ ] Add a `model`/`thinking` override assertion: `dispatch_native` with `launch.model = Some(<a second catalog model>)` → `subagent-session-started.model` = that model; `launch.thinking = Some(<a valid level>)` → `subagent-session-started.thinkingLevel` = that level; `launch.model = Some("prov/id:high")` with `thinking: None` → `thinkingLevel` = `"high"` (the `:<level>` suffix). (For the `:<level>` suffix test, the catalog model's `thinking_levels` must include the level — or be empty, which soft-passes the validation in step 2.)
- [ ] Add a `system_prompt` assertion: `launch.system_prompt = Some("You are terse.")` → the child's first model request carries a leading `System` message with that text (assert via the mock `Provider`'s received `ModelRequest.messages[0]`).
- [ ] Run `cargo fmt` → `cargo build` → `cargo test --no-fail-fast` → `cargo clippy --all-targets` (0 warnings).
- [ ] Commit: "feat(harness): CapturingSink + SubagentSessionManager::dispatch_native (in-process native child)"

**Acceptance criteria:**
- [ ] `dispatch_native` with `NativeDeps` set spawns an in-process child `AgentLoop` (no pi process, no `WorkerRuntime`), runs a task, streams frames to the sink **exactly once**, and resolves `Completed { output }`.
- [ ] After `Completed`, the child's `events_rx` closes (the loop task ended — the unconditional teardown; no orphaned child).
- [ ] The child's `enabledTools` excludes `subagent`; the child's `subagent` field is `None`.
- [ ] `model`/`thinking` (incl. the `:<level>` suffix) + `system_prompt` + `tools` `launch` overrides reach the child; absent overrides inherit the parent's.
- [ ] The child's `native_messages` writes go to a throwaway `Db` (never the parent's real `Db`).
- [ ] `AgentLoop` core / `Provider` / `Compactor` / `RetryPolicy` are unchanged (only the 3 small additions: `enabled_tools()`, `thinking_level()`, `prepend_system`).

---

### Task 3: Wire `dispatch_subagent` (loop.rs) to `dispatch_native` + update the end-to-end test

**Context:** Task 2 added `dispatch_native`; this task makes the native parent's `subagent` tool actually USE it (replacing the pi `dispatch`), and updates the existing end-to-end test (`harness_subagent_dispatch.rs`) to assert the child is an in-process `AgentLoop` (not a `fake_pi` process). This is the user-visible behavior change.

**Files:**
- Modify: `src-tauri/src/agent/harness/loop.rs` (`dispatch_subagent`, ~line 947)
- Modify: `src-tauri/tests/harness_subagent_dispatch.rs`

**What to implement:**
- In `dispatch_subagent` (loop.rs), replace the `manager.dispatch(&self.session_id, &self.space_cwd, "pi", agent_name, launch, task, &self.sink)` call with `manager.dispatch_native(&self.session_id, &self.space_cwd, &self.model, self.enabled_tools().to_vec(), agent_name, launch, task, &self.sink)`. (The `turn` param is the current turn token — the existing `select!` arm on `turn.cancelled()` already calls `cancel.cancel()` + maps to `SubagentWait::Cancelled`; keep it. The `SubagentCancel` `cancel` handle is what the driver watches via `ec.rx` — the `turn.cancelled()` arm's `cancel.cancel()` is the propagation path.)
- Keep the existing `select!`-vs-`turn.cancelled()` structure + the `SubagentWait` mapping unchanged (only the dispatch target changes).
- Keep the "no manager" error path (`subagent: None` → "not available").
- Update the stale doc comments (Nits #19): `dispatch_subagent`'s doc (loop.rs:943-946: "a native parent session spawns an EXTERNAL subagent … native subagent dispatch is a follow-up") → "spawns an in-process NATIVE child"; the `AgentLoop.subagent` field doc (loop.rs:151-152) likewise.
- Update `harness_subagent_dispatch.rs`:
  - `native_subagent_tool_spawns_a_real_sub_session_and_captures_the_result` → repurpose to assert the child is an **in-process `AgentLoop`** (NOT a `fake_pi` process): the `SubagentSessionManager` is wired via `set_native_deps` (a mock `provider_factory`, NOT `fake_pi`), the native parent (a mock `Provider` issuing a `subagent` call) dispatches it, and the test asserts the `subagent` tool's `tool_execution_end` carries the mock child's output (NOT a `fake_pi` "Hello", NOT an error) + the `subagent-session-started`/`subagent-closed` events fire. **Keep** a `write_pi_agents_json`-style helper but point the `pi` entry's `command` at a **nonexistent path** (e.g. `/nonexistent/fake_pi`) — `Registry::load` on a missing `agents.json` returns `default_registry()` (a `pi` entry pointing at the REAL `pi` binary), so the pre-fix `dispatch` (which looks up `"pi"`) would otherwise spawn a REAL `pi` process; the nonexistent command makes the pre-fix red step deterministic (the spawn fails) + inert (no real `pi` process). Remove the `FAKE_PI` const + the `contains("Hello")` assertion from this test (the child is in-process — there is no `fake_pi` "Hello"). (Do NOT reference `MarkerCounter` — it lives in `tests/subagent_dispatch.rs`, not here.)
  - Keep `native_subagent_tool_without_a_manager_returns_an_error_result` as-is.

**Steps:**
- [ ] Write the updated failing test in `harness_subagent_dispatch.rs`: the native parent (mock `Provider` → `subagent` tool call) with a `SubagentSessionManager` wired via `set_native_deps` (mock `provider_factory`) → assert the `subagent` tool's `tool_execution_end` carries the mock child's output (NOT a `fake_pi` "Hello", NOT an error) + the `subagent-session-started`/`subagent-closed` events fire + NO `fake_pi` process was spawned.
- [ ] Run `cargo test --test harness_subagent_dispatch` — confirm it fails (the native parent still calls the pi `dispatch`, which has no registry entry for the mock → error).
- [ ] Implement the `dispatch_subagent` change (loop.rs) + the doc-comment updates + the test changes.
- [ ] Run `cargo test --test harness_subagent_dispatch` — confirm it passes (the child is in-process).
- [ ] Run `cargo fmt` → `cargo build` → `cargo test --no-fail-fast` → `cargo clippy --all-targets` (0 warnings).
- [ ] Commit: "feat(harness): native parent's subagent tool spawns an in-process native child"

**Acceptance criteria:**
- [ ] A native parent's `subagent` call spawns an in-process native child (no `fake_pi` process, no `WorkerRuntime`, no bridge).
- [ ] The child's output flows back as the `subagent` tool's `ToolResult`.
- [ ] `native_subagent_tool_without_a_manager_returns_an_error_result` still passes.
- [ ] External pi parents' `subagent` (the bridge `dispatch`) is unchanged.

---

### Task 4: Lifecycle — cancellation + timeout (pin the teardown)

**Context:** Task 2's driver races the child's settle vs `settle_timeout` vs the `SubagentCancel` handle, and unconditionally tears the child down on every exit. This task pins the cancellation + timeout behavior with tests (the implementation landed in Task 2; this is the TDD confirmation + gap-filling). A hung or cancelled child must be torn down (no orphaned child) and surface a clean `Failed` outcome.

**Files:**
- Modify: `src-tauri/src/agent/subagent.rs` (the `dispatch_native` driver, if gaps)
- Modify: `src-tauri/src/agent/harness/loop.rs` (the `dispatch_subagent` cancel propagation, if a gap)
- Test: `src-tauri/tests/harness_dispatch_native.rs`

**What to implement:**
- Confirm the driver's `select!` has all three arms (child `settle_rx`, `settle_timeout` sleep, `ec.rx.changed()`) and that on timeout/cancel it runs the **unconditional teardown** (Task 2 step 13) + emits `subagent-closed { status: "failed" }`.
- Confirm the parent's `dispatch_subagent` `select!`-vs-`turn.cancelled()` arm calls `cancel.cancel()` (the `SubagentCancel` the driver watches) — so a parent turn-cancel propagates to the child.
- `settle_timeout` is already a `NativeDeps` field (Task 1) — the test shortens it (~200ms).

**Steps:**
- [ ] Write a failing test: a parent turn-cancel (cancel the `SubagentCancel` returned by `dispatch_native`, or the `turn` token `dispatch_subagent` races) → the driver tears the child down → the oneshot resolves `Failed { error }` containing `"cancelled"`; assert the child's `events_rx` closes (the loop task ended — the teardown, NOT just a detached `JoinHandle`).
- [ ] Run `cargo test --test harness_dispatch_native` — confirm it fails (if the cancel arm / propagation is missing).
- [ ] Fix the driver (subagent.rs) + the `dispatch_subagent` cancel propagation (loop.rs) if needed.
- [ ] Run the test — confirm it passes.
- [ ] Write a second failing test: a child that never settles (a mock `Provider` that streams a `TextDelta` then hangs — no `Done`) + a SHORT `settle_timeout` (`NativeDeps.settle_timeout = Duration::from_millis(200)`) → the oneshot resolves `Failed { error }` containing `"timed out"` + the child is torn down (`events_rx` closes).
- [ ] Run `cargo test --test harness_dispatch_native` — confirm both tests pass.
- [ ] Run `cargo fmt` → `cargo build` → `cargo test --no-fail-fast` → `cargo clippy --all-targets` (0 warnings).
- [ ] Commit: "test(harness): native subagent lifecycle (cancel + timeout teardown)"

**Acceptance criteria:**
- [ ] Parent turn-cancel → child torn down → `Failed { "cancelled" }`; the child's `events_rx` closes (no orphaned child).
- [ ] `settle_timeout` (hung child) → child torn down → `Failed { "timed out" }`; the child's `events_rx` closes (no orphaned child).
- [ ] `subagent-closed { status: "failed" }` fires on both teardown paths.
- [ ] `settle_timeout` is configurable (via `NativeDeps`) with a 30-minute default.
- The child does NOT call `refresh_model_metadata` (the live `/v1/models` refresh — a documented follow-up; the child uses the seeded/static `Model` metadata).

---

## Out of scope (follow-ups)

- Native subagent **resume** / persistence (the child is ephemeral by design).
- A native child dispatching its OWN subagents (explicitly out — the recursion guard).
- Threading the parent's **trust** into the child (the child's gated tools currently default to prompting; a trusted parent's child should inherit trust — `trust_db: None` for now).
- Per-child `CostAccumulator` (the `subagent-closed` `metrics.cost`/token fields are `Default`/0 until the child's usage is accumulated; `output` + `duration_ms` are real).
- Validating `launch.thinking` against `model.thinking_levels` is a soft-drop (Task 2 step 2); a hard `Failed` on mismatch is a follow-up.
