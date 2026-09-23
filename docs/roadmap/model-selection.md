---
status: committed
done-when: In the desktop, a live pi session's header shows a model selector (populated from pi's model registry) and a thinking-level selector; choosing a value switches the session's model/thinking level mid-session (visible in pi's behavior), with the UI tracking agent-side changes.
---

# Model + Thinking-Level Selection Plan

**Goal:** Let the user select a session's model and thinking level from the UI, over ACP config options.
**Architecture:** The agent (pi-acp) advertises `configOptions` in `newSession`/`loadSession` responses and pushes `config_option_update` notifications; the Client (Rust) carries the options in `SessionInfo`, forwards them to the frontend, and relays `session/set_config_option` requests. The frontend keeps per-session config options in the sessions store and renders two selectors in the session header. Nothing is persisted — the agent is the source of truth (a resume re-fetches from `loadSession`).
**Tech Stack:** Rust (Tauri 2 backend, `agent-client-protocol` 2.x SDK, schema v1), React 19 + TypeScript + Zustand, vitest + @testing-library/react.

**Verified facts (do not re-research):**
- `agent_client_protocol::schema::v1` (crate `agent-client-protocol` 2.1.0) provides: `SessionConfigOption` (fields `id: SessionConfigId`, `name: String`, `description: Option<String>`, `category: Option<SessionConfigOptionCategory>`, `kind: SessionConfigKind`, `meta`; serde camelCase: `currentValue`, `options`, …), `SessionConfigKind::{Select(SessionConfigSelect), Boolean(SessionConfigBoolean)}`, `SessionConfigSelect { current_value: SessionConfigValueId, options: SessionConfigSelectOptions }` — where `SessionConfigSelectOptions` is an `#[serde(untagged)]` ENUM: `Ungrouped(Vec<SessionConfigSelectOption>) | Grouped(Vec<SessionConfigSelectGroup>)` (a flat wire array deserializes to `Ungrouped`; there is NO `.len()` on the enum — match it), `SessionConfigSelectOption { value: SessionConfigValueId, name, description }`, `SessionConfigOptionCategory::{Mode, Model, ModelConfig, ThoughtLevel, Other(String)}` (serde snake_case: `"model"`, `"thought_level"`), `SetSessionConfigOptionRequest::new(session_id, config_id: impl Into<SessionConfigId>, value: impl Into<SessionConfigOptionValue>)` (method `session/set_config_option`; `From<&str>` exists for the VALUE, but `SessionConfigId` has `From` ONLY for `Arc<str>` / `String` / `&'static str` — a borrowed `&str` does NOT convert, so wrap it: `SessionConfigId::new(config_id)` (`new` takes `impl Into<Arc<str>>`)), `SetSessionConfigOptionResponse { config_options: Vec<SessionConfigOption> }`, `NewSessionResponse.config_options: Option<Vec<SessionConfigOption>>`, `LoadSessionResponse.config_options: Option<Vec<SessionConfigOption>>`, `SessionUpdate::ConfigOptionUpdate(ConfigOptionUpdate { config_options: Vec<SessionConfigOption> })`.
- The `load_session(...).block_task().start_session().await` result is `RestoredSession { session, response }` with PRIVATE fields — access the response via the accessor `restored.response() -> &LoadSessionResponse` (or `into_parts()` / `into_session()`); `restored.response` (field access) does NOT compile.
- `AcpError::{UnknownSession { session_id }, Protocol { message }}` exist and are `Serialize` (command return values).
- The Rust `on_receive_notification` handler (in `drive_session`) already serializes EVERY `SessionUpdate` variant to the frontend as a `session-update` event `{ sessionId, update }` — `config_option_update` already flows; no Rust change needed for notifications.
- pi-acp's wire shape (the fixture mirrors it): model option `{ type: "select", id: "model", category: "model", name: "Model", currentValue: "provider/id", options: [{ value: "provider/id", name: "provider/Name" }] }`; thinking option `{ type: "select", id: "thought_level", category: "thought_level", name: "Thinking", currentValue: "medium", options: [{ value: "off", name: "Thinking: off" }, …] }` (levels: off, minimal, low, medium, high, xhigh).
- `fake_agent` (dev binary, `src-tauri/src/bin/fake_agent.rs`): line-based JSON-RPC over stdio; main session id is `fake-session-1` (overridable via `FAKE_SESSION_ID`); mode = first positional arg (`resume` advertises `loadSession: true`); helpers `write_result(w, &id, &value)`, `write_error(w, &id, &error)`, `write_chunk(w, sid, message_id, text)`; the `session/new` and `session/load` responses are currently bare `{"sessionId": sid}`.
- Rust test harness pattern (from `src-tauri/src/acp/subagent.rs` `mod tests`): `TestSink` (mpsc channel implementing `EventSink`), `temp_config_dir()`, `unique_fake_agent(dir)` (copies `target/debug/fake_agent` to a uuid-named path — `cargo test` builds the binary), `write_agents_json_cmd(cmd, dir, mode)` (writes an `agents.json` with one `fake` entry, `bridge: false` by default), `SessionManager::new(config_dir)`.
- Frontend: sessions store (`src/store/sessions.ts`) — `addSession(info)` (start flow), `resumeSession(sessionId)` (calls `resumeSessionCommand` then replaces the session entry), `applySessionUpdate(sessionId, update)` (store method wrapping the standalone `applySessionUpdate(messages, update, at)` reducer), `handleSessionClosed(sessionId, reason)`. `AcpSessionUpdate` in `src/lib/tauri.ts` is a discriminated union on `sessionUpdate`; the Tauri mock in `ChatStream.test.tsx` spreads `vi.importActual` and overrides named functions. Component tests: vitest + @testing-library/react (`render`, `screen`, `fireEvent`, `vi.stubGlobal("matchMedia", …)`).
- The session header (`src/components/ChatStream.tsx`, the `<div className="flex h-12 …">` row): title span → space chip → conversation `Select` (`variant="ghost" size="sm" className="w-24"`) → more-menu `DropdownMenu`. `isLive` = the active session is in `s.sessions`.

---

### Task 1: Rust — `SessionInfo` carries config options (+ `fake_agent` fixture)

**Context:**
The desktop drops `configOptions` from the `newSession`/`loadSession` responses — `SessionInfo` (the `start_session`/`resume_session` command return value, mirrored by the frontend `SessionInfo` type) has no config-options field. This task makes the options flow to the frontend for every live session: the field is added to `SessionInfo`, populated from both session-establishment responses, and the `fake_agent` dev fixture learns to advertise a model + thinking-level option so downstream tests (and manual dev) can exercise the real wire shape. `list_sessions` (stored sessions) reports `None` — a stored session has no live agent process, and the UI shows no selectors for it. NO database schema change: `record_session` keeps writing the existing columns only (the agent is the source of truth; a resume re-fetches fresh state).

**Files:**
- Modify: `src-tauri/src/acp/session.rs`
- Modify: `src-tauri/src/acp/subagent.rs`
- Modify: `src-tauri/src/commands/history.rs`
- Modify: `src-tauri/src/bin/fake_agent.rs`

**What to implement:**

1. `src-tauri/src/acp/session.rs` — `SessionInfo` (line ~105): add one field after `capabilities`:
   ```rust
   /// The agent's session configuration options (model / thinking level
   /// selectors) from the `newSession` / `loadSession` response; `None`
   /// when the agent does not advertise any (or for stored sessions —
   /// `list_sessions` always reports `None`).
   pub config_options: Option<Vec<SessionConfigOption>>,
   ```
   Import `SessionConfigOption` from `agent_client_protocol::schema::v1` (extend the existing `use` at the top of the file).
2. `start_session` establisher (the `SessionInfo { … }` at line ~866): set `config_options: new_session.config_options.clone()`.
3. `resume_session` establisher (the `SessionInfo { … }` at line ~987): the current code discards the restored session (`let _restored = cx.load_session(…).block_task().start_session().await?;`) — capture it (`let restored = …`) and set `config_options: restored.response().config_options.clone()` (ACCESSOR — the `RestoredSession` fields are private; `restored.response` would not compile). `sid` is moved into `SessionInfo.session_id` after this, so read the response before the `Ok(…)`. 
4. `src-tauri/src/acp/subagent.rs` — the subagent establisher's `SessionInfo` (line ~300, production): set `config_options: new_session.config_options.clone()` (consistent; subagent sessions have no UI selectors, the field is inert there). The two TEST construction sites (lines ~846, ~979, `agent_id: "fake"`): set `config_options: None`.
5. `src-tauri/src/commands/history.rs` — `list_sessions` (line ~23): add `config_options: None` to the `SessionInfo` construction.
6. `src-tauri/src/bin/fake_agent.rs` — the `session/new` and `session/load` response bodies gain `configOptions` (mirror the pi-acp wire shape; keep the existing `sessionId` key):
   ```json
   "configOptions": [
     { "type": "select", "id": "model", "category": "model", "name": "Model",
       "description": "Select the model for this session",
       "currentValue": "acme/alpha",
       "options": [
         { "value": "acme/alpha", "name": "acme/Alpha" },
         { "value": "acme/beta",  "name": "acme/Beta" },
         { "value": "acme/gamma", "name": "acme/Gamma" }
       ] },
     { "type": "select", "id": "thought_level", "category": "thought_level", "name": "Thinking",
       "description": "Set the reasoning effort for this session",
       "currentValue": "medium",
       "options": [
         { "value": "off", "name": "Thinking: off" },
         { "value": "minimal", "name": "Thinking: minimal" },
         { "value": "low", "name": "Thinking: low" },
         { "value": "medium", "name": "Thinking: medium" },
         { "value": "high", "name": "Thinking: high" },
         { "value": "xhigh", "name": "Thinking: xhigh" }
       ] }
   ]
   ```
   Put the payload in a `const` (e.g. `fn fake_config_options() -> serde_json::Value`) shared by both handlers. Do NOT change the `session/new` `hang` behavior (no response in `hang` mode) or any other handler in this task.

**Steps:**
- [ ] In `src-tauri/src/acp/session.rs` `mod tests`, add the test-harness helpers copied from `src-tauri/src/acp/subagent.rs` `mod tests` (adapt imports): `TestSink` (mpsc channel + `EventSink` impl), `temp_config_dir()`, `unique_fake_agent(dir)`, `write_agents_json_cmd(cmd, dir, mode)`. Then write the failing test:
  ```rust
  /// (start) `start_session` returns the agent's `configOptions` (the
  /// `newSession` response's `configOptions` survive into `SessionInfo`):
  /// a `model` select (3 options, current `acme/alpha`) + a `thought_level`
  /// select (6 options, current `medium`).
  #[tokio::test]
  async fn start_session_returns_config_options_from_new_session_response() { /* … */ }

  /// (resume) `resume_session` returns the `loadSession` response's
  /// `configOptions` (fake agent `resume` mode advertises `loadSession`).
  #[tokio::test]
  async fn resume_session_returns_config_options_from_load_session_response() { /* … */ }
  ```
  Start test body: `let manager = SessionManager::new(temp_config_dir()).unwrap();` (the constructor returns `Result<Self, ConfigError>`) with `write_agents_json_cmd(&unique_fake_agent(…), &dir, None)`; `manager.start_session("fake", <a temp dir that EXISTS — `start_session` canonicalizes it and errors `FolderMissing` otherwise; reuse the created temp dir>, &TestSink)`; assert `info.config_options` is `Some` with 2 entries — locate the model entry by `category == Some(SessionConfigOptionCategory::Model)` (import the type), match `kind` to `SessionConfigKind::Select(select)`, assert `select.current_value == "acme/alpha"` (via `.to_string()`) and — because `select.options` is the `SessionConfigSelectOptions` ENUM, not a `Vec` — `match &select.options { SessionConfigSelectOptions::Ungrouped(opts) => assert_eq!(opts.len(), 3) /* + the three values */, _ => panic!("expected ungrouped options") }` (import `SessionConfigSelectOptions`); same for `SessionConfigOptionCategory::ThoughtLevel` / `medium` / 6 options. Resume test body: same harness with `mode Some("resume")` and `manager.resume_session("fake", "fake-session-1", <existing temp dir>, &sink)` (the fake agent reports the fixed id `fake-session-1` for `session/load`), assert the same two entries.
- [ ] Run `cargo test --lib` from `src-tauri/`
  - Did it fail to compile (missing field / helper)? That is the expected TDD red. (If it fails at runtime with a missing `configOptions` in the fake agent response, that is the expected red too.)
- [ ] Implement the `SessionInfo` field + the three establisher fixes (items 1–5 above) and the `fake_agent` `configOptions` payload (item 6)
- [ ] Run `cargo test` from `src-tauri/`
  - Did all tests pass (including the pre-existing `subagent.rs` / `session.rs` tests, whose `SessionInfo` constructions you updated)? If not, fix and re-run.
- [ ] Run `cargo clippy --all-targets` from `src-tauri/`
  - 0 warnings? If not, fix and re-run.
- [ ] Run `cargo fmt --check` from `src-tauri/`
  - Succeeded? If not, run `cargo fmt` and re-check.
- [ ] Commit with message: "feat: carry ACP config options in SessionInfo (+ fake_agent fixture)"

**Acceptance criteria:**
- [ ] `SessionInfo` has `config_options: Option<Vec<SessionConfigOption>>` (camelCase `configOptions` over IPC)
- [ ] `start_session` / `resume_session` return the agent's `configOptions`; `list_sessions` returns `None`; no DB schema change (`record_session` unchanged)
- [ ] `fake_agent` advertises the model + thinking `configOptions` in `session/new` and `session/load`
- [ ] `cargo test`, `cargo clippy --all-targets` (0 warnings), `cargo fmt --check` all green

---

### Task 2: Rust — `set_session_config_option` command (+ `fake_agent` set handler)

**Context:**
With the options flowing to the frontend (Task 1), the Client still cannot ACT on a selection. This task adds the round-trip: a `set_session_config_option` Tauri command that relays `session/set_config_option` to the live agent and returns the agent's updated `configOptions`. The `fake_agent` learns to handle the method (update its in-memory `currentValue`, emit a `config_option_update` `session/update` notification, then answer the request — mirroring real pi-acp order: notification first, response second) and to reject it in a new `set_config_option_error` mode, so the success, error, and notification paths are all testable end-to-end.

**Files:**
- Modify: `src-tauri/src/acp/session.rs`
- Modify: `src-tauri/src/commands/sessions.rs`
- Modify: `src-tauri/src/lib.rs`
- Modify: `src-tauri/src/bin/fake_agent.rs`

**What to implement:**

1. `src-tauri/src/acp/session.rs` — `SessionManager` gains (next to `send_prompt`, which it mirrors for the connection handling):
   ```rust
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
       let request = SetSessionConfigOptionRequest::new(
           sid,
           SessionConfigId::new(config_id),
           value,
       );
       let response = cx
           .send_request(request)
           .block_task()
           .await
           .map_err(|err| AcpError::Protocol { message: err.message })?;
       Ok(response.config_options)
   }
   ```
   (Import `SessionConfigId` and `SetSessionConfigOptionRequest` from `agent_client_protocol::schema::v1`.)
2. `src-tauri/src/commands/sessions.rs` — the Tauri command (next to `send_prompt`):
   ```rust
   /// Set a session config option (model / thinking level) on a live
   /// session; returns the agent's updated `configOptions`.
   #[tauri::command]
   pub async fn set_session_config_option(
       state: State<'_, Arc<SessionManager>>,
       session_id: String,
       config_id: String,
       value: String,
   ) -> Result<Vec<SessionConfigOption>, AcpError> {
       state
           .set_config_option(&session_id, &config_id, &value)
           .await
   }
   ```
   (Import `SessionConfigOption` from the schema in the command file's `use` block.)
3. `src-tauri/src/lib.rs` — register `commands::sessions::set_session_config_option` in the `generate_handler!` list (after `send_prompt`).
4. `src-tauri/src/bin/fake_agent.rs` — new method arm in the main read loop:
   ```rust
   "session/set_config_option" => { /* … */ }
   ```
   - Parse `params.configId` (string) and `params.value` (string) from the frame.
   - Keep the config options in a `let mut config_options = fake_config_options();` moved into the loop (so the `currentValue` updates persist across requests within one agent run — do NOT re-initialize it per request).
   - Default behavior: find the option whose `id` matches `configId` and set its `currentValue` to `value` (unknown `configId` → `write_error` with `{ "code": -32602, "message": "unknown config option" }` and no notification); then — mutating the loop-scoped `config_options` ONCE — serialize that SAME updated value into both the notification and the response: first a `session/update` notification frame (add a small `write_notification(w, &value)` helper mirroring `write_chunk`'s JSON-RPC 2.0 conventions EXACTLY — `{ "jsonrpc": "2.0", "method": "session/update", "params": { "sessionId": sid, "update": { "sessionUpdate": "config_option_update", "configOptions": <updated> } } }` — NO `v` / `type` / `id` keys; the `{v, type}` envelope is the BRIDGE-socket protocol (ADR 0003), not the ACP stdio channel, which is JSON-RPC 2.0), THEN `write_result` with `{ "configOptions": <updated> }` (notification first, response second — the real pi-acp order).
   - `set_config_option_error` mode: `write_error` with `{ "code": -32602, "message": "fake set config option failure" }` (no notification, no response body).
   - Parse defensively: read `params.get("configId").and_then(serde_json::Value::as_str)` and `params.get("value").and_then(serde_json::Value::as_str)` — a missing or non-string `value` (e.g. a future `boolean`-kind frame, out of scope for the UI but reachable on the wire) takes the unknown-option error path (`-32602`), never a panic.
   - Update the module doc comment with the new mode.

**Steps:**
- [ ] In `src-tauri/src/acp/session.rs` `mod tests`, write the failing tests (reusing the Task 1 harness helpers):
  ```rust
  /// (set) `set_config_option` round-trips: the request reaches the agent,
  /// the response's updated `configOptions` come back (model current value
  /// moved to `acme/beta`, the thinking entry unchanged), AND the agent's
  /// `config_option_update` notification arrives as a `session-update`
  /// event with the same updated options.
  #[tokio::test]
  async fn set_config_option_round_trips_and_notifies() { /* … */ }

  /// (error) `set_config_option_error` mode: the agent's rejection maps to
  /// `AcpError::Protocol`.
  #[tokio::test]
  async fn set_config_option_rejection_maps_to_protocol_error() { /* … */ }

  /// (unknown) `set_config_option` on an unknown session id maps to
  /// `AcpError::UnknownSession`.
  #[tokio::test]
  async fn set_config_option_unknown_session() { /* … */ }
  ```
  Round-trip body: start a session (harness as in Task 1 — `SessionManager::new(dir).unwrap()`, existing temp dir as cwd, `TestSink`), `manager.set_config_option("fake-session-1", "model", "acme/beta")`; assert the returned options' model entry `currentValue == "acme/beta"` (and 3 ungrouped options) and the thinking entry still `medium`; then drain the `TestSink` channel and assert a `session-update` event whose payload `update.sessionUpdate == "config_option_update"` with `configOptions` matching the returned set (the notification and the response carry the SAME updated set). Error body: harness with `mode Some("set_config_option_error")`, start the session, `set_config_option(…)` → `assert!(matches!(err, AcpError::Protocol { .. }))`. Unknown body: no session started; `set_config_option("nope", "model", "x")` → `AcpError::UnknownSession`.
- [ ] Run `cargo test --lib` from `src-tauri/`
  - Did it fail (method missing / fixture doesn't answer the method)? Expected red.
- [ ] Implement the manager method, the command, the handler registration, and the `fake_agent` set handler (items 1–4)
- [ ] Run `cargo test` from `src-tauri/`
  - All green (including pre-existing tests)? If not, fix and re-run.
- [ ] Run `cargo clippy --all-targets` + `cargo fmt --check` from `src-tauri/`
  - 0 warnings / clean? If not, fix and re-run.
- [ ] Commit with message: "feat: set_session_config_option command (ACP session/set_config_option)"

**Acceptance criteria:**
- [ ] `SessionManager::set_config_option` + the `set_session_config_option` Tauri command exist and are registered; return the agent's updated `configOptions`
- [ ] Unknown session → `AcpError::UnknownSession`; agent rejection → `AcpError::Protocol`
- [ ] `fake_agent` handles `session/set_config_option` (updates `currentValue`, emits `config_option_update` before answering) and rejects in `set_config_option_error` mode
- [ ] `cargo test`, `cargo clippy --all-targets` (0 warnings), `cargo fmt --check` all green

---

### Task 3: Frontend — types + store

**Context:**
The Rust side now delivers config options three ways: in the `start_session`/`resume_session` return value, in the `set_session_config_option` response, and in `config_option_update` `session-update` events (already forwarded by the existing notification handler). The frontend has no types for them and no state. This task adds the TypeScript types, the `setSessionConfigOption` invoke wrapper, and per-session config-options state in the sessions store — seeded on start/resume, replaced wholesale by updates (the agent always sends the FULL set — no merge logic), and cleared on close. The UI (Task 4) reads this state.

**Files:**
- Modify: `src/lib/tauri.ts`
- Modify: `src/store/sessions.ts`
- Test: `src/store/sessions.test.ts`

**What to implement:**

1. `src/lib/tauri.ts` — add the types (near `SessionInfo`):
   ```ts
   /** One entry of a config option's `options` list (camelCase). */
   export interface SessionConfigSelectOption {
     value: string;
     name: string;
     description?: string | null;
   }

   /**
    * A session config option the agent advertises (camelCase wire shape).
    * `category` is snake_case per the ACP spec (`"model"`,
    * `"thought_level"`); `type` discriminates the payload shape.
    */
   export interface SessionConfigOption {
     id: string;
     name: string;
     description?: string | null;
     category?: string | null;
     type: "select" | "boolean";
     currentValue: string | boolean;
     options?: SessionConfigSelectOption[]; // select kind only
   }
   ```
   - `SessionInfo` gains: `configOptions?: SessionConfigOption[];`
   - `AcpSessionUpdate` gains a variant:
     ```ts
     | { sessionUpdate: "config_option_update"; configOptions: SessionConfigOption[] }
     ```
   - New invoke (next to `resumeSession`):
     ```ts
     /** Set a session config option (model / thinking level); returns the agent's updated `configOptions`. */
     export async function setSessionConfigOption(
       sessionId: string,
       configId: string,
       value: string,
     ): Promise<SessionConfigOption[]> {
       return invoke<SessionConfigOption[]>("set_session_config_option", {
         sessionId,
         configId,
         value,
       });
     }
     ```
2. `src/store/sessions.ts` — new state + wiring:
   - State field (in `SessionsState` + the initial object): `configOptions: Record<string, SessionConfigOption[]>` (keyed by session id; live sessions only), initial `{}`.
   - New action (in the interface + implementation):
     ```ts
     /** Replace a session's config options (the agent always sends the full set). */
     applyConfigOptions: (sessionId: string, options: SessionConfigOption[]) => void;
     ```
     Implementation: `set((state) => ({ configOptions: { ...state.configOptions, [sessionId]: options } }))`.
   - `addSession`: seed from `info.configOptions` — add to the `set` payload: `configOptions: info.configOptions ? { ...state.configOptions, [info.sessionId]: info.configOptions } : state.configOptions` (i.e. absent → no entry; a re-add of a live session replaces its entry).
   - `resumeSession`: the same seeding on the `info` it stores (add the identical `configOptions` line to its `set` payload).
   - `applySessionUpdate` (the STORE method, line ~589 — NOT the standalone reducer, which keeps handling messages only): extend the `set` payload: when `update.sessionUpdate === "config_option_update"`, set `configOptions: { ...state.configOptions, [sessionId]: update.configOptions }`; otherwise leave `state.configOptions` unchanged.
   - `handleSessionClosed`: clear the entry — in its `set` payload: `const { [sessionId]: _goneConfig, ...restConfig } = state.configOptions; … configOptions: restConfig`.

**Steps:**
- [ ] In `src/store/sessions.test.ts`, write the failing tests. NOTE: the file currently has NO `vi.mock("../lib/tauri", …)` (its existing tests exercise pure helpers + store state that never hits IPC) — the `resumeSession` test requires a module mock, because the store's `resumeSession` calls `resumeSessionCommand` (the `resumeSession` export, imported aliased) AND fire-and-forget `loadHistory` (a naive mock omitting `loadHistory` produces unhandled rejections under jsdom). ALSO: the file's existing import is `import { beforeEach, describe, expect, it } from "vitest";` — it does NOT import `vi`, so extend it to `import { beforeEach, describe, expect, it, vi } from "vitest";` (the mock block below uses `vi.mock` / `vi.importActual` / `vi.fn`). Add:
  ```ts
  vi.mock("../lib/tauri", async () => {
    const actual = await vi.importActual<Record<string, unknown>>("../lib/tauri");
    return {
      ...actual,
      resumeSession: vi.fn().mockResolvedValue({
        sessionId: "s1",
        agentId: "a1",
        cwd: "/x",
        capabilities: {},
        configOptions: [/* one model-shaped entry */],
      }),
      loadHistory: vi.fn().mockResolvedValue([]),
    };
  });
  ```
  (the mock key is the EXPORT name `resumeSession`, not the store's alias `resumeSessionCommand`). Then the tests:
  ```ts
  it("seeds configOptions from a started session's SessionInfo (and not when absent)", …)
  it("seeds configOptions from a resumed session's SessionInfo", …)
  it("replaces a session's configOptions on a config_option_update update", …)
  it("replaces a session's configOptions via applyConfigOptions", …)
  it("clears a session's configOptions on close", …)
  ```
  Seed test: `useSessions.getState().addSession({ sessionId: "s1", agentId: "a1", cwd: "/x", capabilities: {}, configOptions: [/* one model-shaped entry */] })` → `getState().configOptions.s1` deep-equals the entry; a second `addSession` for `s2` WITHOUT `configOptions` → `"s2" in configOptions` is `false`. Resume test: seed a stored session in `historySessions` (`{ sessionId: "s1", agentId: "a1", cwd: "/x", capabilities: {} }`), `await getState().resumeSession("s1")` (uses the mocked `resumeSession` above) → `configOptions.s1` deep-equals the mocked entry. Update test: seed `configOptions.s1 = [A]`, `getState().applySessionUpdate("s1", { sessionUpdate: "config_option_update", configOptions: [B] })` → `configOptions.s1` deep-equals `[B]` (replaced, not merged); a `tool_call_update` update leaves it unchanged. Close test: seed `configOptions.s1`, `handleSessionClosed("s1", "user")` → `"s1" in configOptions` is `false`.
- [ ] Run `pnpm test -- src/store/sessions.test.ts` from the repo root
  - Did the new tests fail (field missing / action missing)? Expected red. Did the PRE-EXISTING tests still pass (you must not break them)?
- [ ] Implement the types + store changes (items 1–2)
- [ ] Run `pnpm test` from the repo root
  - All tests pass? If not, fix and re-run.
- [ ] Run `pnpm build` from the repo root
  - Type-check + build succeeded? If not, fix and re-run.
- [ ] Commit with message: "feat: frontend config-option types + per-session store state"

**Acceptance criteria:**
- [ ] `SessionConfigOption` / `SessionConfigSelectOption` types + `SessionInfo.configOptions?` + `AcpSessionUpdate` `config_option_update` variant + `setSessionConfigOption` invoke exist in `src/lib/tauri.ts`
- [ ] Store: `configOptions` seeded by `addSession`/`resumeSession`, replaced by `config_option_update` and `applyConfigOptions`, cleared by `handleSessionClosed`
- [ ] `pnpm test` + `pnpm build` green

---

### Task 4: Frontend — model + thinking selectors in the session header

**Context:**
The store carries per-session config options (Task 3); nothing renders them yet. This task adds the user-facing selectors: two `Select` components in the session header (the `ChatStream` header row, after the conversation selector and before the more-menu) — **Model** (the option with `category === "model"`, fallback `id === "model"`) and **Thinking** (`category === "thought_level"`, fallback `id === "thought_level"`). Each renders only when its option exists, is a `select` kind, and has non-empty `options`; only for LIVE sessions (a stored session has no agent process to set options through). Selecting a value calls `setSessionConfigOption` and applies the response via `applyConfigOptions` (idempotent — the agent follows with a `config_option_update` notification that the store handles the same way). Out of scope: subagent sessions (they render in `SubagentPanel`), `boolean`-kind options, and any persistence.

**Files:**
- Create: `src/components/SessionConfigSelect.tsx`
- Create: `src/components/SessionConfigSelect.test.tsx`
- Modify: `src/components/ChatStream.tsx`
- Modify: `src/components/ChatStream.test.tsx`

**What to implement:**

1. `src/components/SessionConfigSelect.tsx` — a presentational selector for ONE config option:
   ```tsx
   interface SessionConfigSelectProps {
     option: SessionConfigOption; // always a `select` kind with non-empty `options`
     onSet: (value: string) => Promise<void>;
   }
   ```
   - Mirror the existing conversation selector's composition EXACTLY: `Select` (the Radix ROOT — takes only `value` / `onValueChange`) wraps a `SelectTrigger` (the styling props belong HERE, not on `Select`: `variant="ghost" size="sm" className="max-w-48" disabled={pending}` + `aria-label` = the option's `name`, e.g. `"Model"`, `"Thinking"`), a `SelectValue`, and `SelectContent` with one `SelectItem` per option (`value` = `opt.value`, label = `opt.name`).
   - `value` = `typeof option.currentValue === "string" ? option.currentValue : ""`.
   - Local state: `pending: boolean` (guards double-fires), `error: string | null`.
   - `onValueChange`: if `pending`, return; set `pending` true, `error` null; `try { await onSet(value) } catch (e) { setError(e instanceof Error ? e.message : String(e)) } finally { setPending(false) }`.
   - `disabled={pending}` on the trigger.
   - When `error` is non-null: render a sibling `<span className="text-ui-sm text-destructive" role="alert">{error}</span>` and clear it after 5 s (`setTimeout`, cleared on unmount via `useEffect` return — the component may be unmounted by a session switch while the timer is armed).
2. `src/components/ChatStream.tsx` — the header row (the `<div className="flex h-12 …">`):
   - Derive (near the other header state, after `isLive` is computed):
     ```ts
     const configOptions = useSessions((s) => s.configOptions[s.activeSessionId ?? ""] ?? null);
     const findOption = (category: string, id: string) =>
       configOptions?.find(
         (o) =>
           (o.category === category || o.id === id) &&
           o.type === "select" &&
           (o.options?.length ?? 0) > 0,
       );
     const modelOption = findOption("model", "model");
     const thinkingOption = findOption("thought_level", "thought_level");
     const applyConfigOptions = useSessions((s) => s.applyConfigOptions);
     const setConfigOption = (option: SessionConfigOption) => (value: string) =>
       setSessionConfigOption(activeSessionId!, option.id, value).then(
         (options) => applyConfigOptions(activeSessionId!, options),
       );
     ```
     (Argument order matters: the wrapper is `setSessionConfigOption(sessionId, configId, value)` — `activeSessionId!` is the SESSION id, `option.id` is the CONFIG id. Adjust the rest to the file's actual local names — `activeSessionId` and `isLive` are already in scope; import `setSessionConfigOption` from `../lib/tauri` and the `SessionConfigOption` type.)
   - Render BETWEEN the conversation `Select` and the `DropdownMenu`:
     ```tsx
     {isLive && modelOption && (
       <SessionConfigSelect option={modelOption} onSet={setConfigOption(modelOption)} />
     )}
     {isLive && thinkingOption && (
       <SessionConfigSelect option={thinkingOption} onSet={setConfigOption(thinkingOption)} />
     )}
     ```
   - Do NOT touch the title / space chip / conversation selector / more-menu code.
3. `src/components/SessionConfigSelect.test.tsx` (new; follow the `ChatStream.test.tsx` conventions): jsdom (vite.config.ts `test: { environment: "jsdom" }` — NO `setupFiles`) lacks the DOM APIs Radix Select needs to OPEN and pick an item (`element.scrollIntoView`, `hasPointerCapture` / `setPointerCapture` / `releasePointerCapture`, `PointerEvent`) — no existing test in this repo has ever opened a Radix `Select`/`DropdownMenu`, so the interaction cases will throw `TypeError: … is not a function` UNLESS the test file polyfills them FIRST. Add at the top of the file (before the tests):
   ```ts
   beforeAll(() => {
     Element.prototype.scrollIntoView = vi.fn();
     Element.prototype.hasPointerCapture = vi.fn(() => false);
     Element.prototype.setPointerCapture = vi.fn();
     Element.prototype.releasePointerCapture = vi.fn();
     vi.stubGlobal("matchMedia", (q: string) => ({
       matches: false, media: q,
       addEventListener: vi.fn(), removeEventListener: vi.fn(),
       addListener: vi.fn(), removeListener: vi.fn(), dispatchEvent: vi.fn(),
     }));
   });
   ```
   (The "trigger shows the current value" case does NOT need this — `SelectValue` renders the selected label without opening; only the open-and-pick cases do. If a polyfill still leaves a Radix pointer path failing, the fallback is to drive the selection by calling the `Select` root's `onValueChange` directly instead of a pointer click.)
   Test cases:
   - renders the current option's `name` as the trigger value (e.g. `acme/Alpha`)
   - selecting a different option calls `onSet` with that option's `value` (fire the trigger, pick the item, `await screen.findByText(…)` / `expect(onSet).toHaveBeenCalledWith("acme/beta")`)
   - the trigger is `disabled` while `onSet` is pending (mock `onSet` with a deferred promise; assert `disabled`, resolve, assert enabled)
   - an `onSet` rejection shows the error text (`role="alert"`) and it disappears after the 5 s auto-clear (use `vi.useFakeTimers()` + `vi.advanceTimersByTime(5000)`) — note: `vi.useFakeTimers()` must be scoped to this test (it interferes with the async rendering in the other cases; `vi.useFakeTimers()` at the start of the test, `vi.useRealTimers()` in a `finally`)
4. `src/components/ChatStream.test.tsx` — the new "choosing a model item invokes `setSessionConfigOption`" case opens the Radix `Select` (trigger click → pick item), which in this repo's jsdom environment (NO `setupFiles`) needs the SAME polyfills as item 3 (`Element.prototype.scrollIntoView` / `hasPointerCapture` / `setPointerCapture` / `releasePointerCapture` + `matchMedia`) — add the same `beforeAll` block to this file (the file already stubs `matchMedia` at module scope; add the four `Element.prototype` stubs). ALSO: extend the `vi.mock("../lib/tauri", …)` block with `setSessionConfigOption: vi.fn().mockResolvedValue([])` (the `…actual` spread covers the export, but the override is needed to control it). ALSO: `beforeEach` and the `seedLiveSession` / stored-session seed helpers use `useSessions.setState({...})` (a shallow merge) with objects that omit `configOptions` — after Task 3 adds the field, stale entries leak between tests (a "WITHOUT configOptions" test could see a previous test's entry and fail intermittently): add `configOptions: {}` to the `beforeEach` reset AND to each seed helper. Then add tests (reuse the file's `seedLiveSession` pattern — extend the seeded `SessionInfo` with `configOptions` shaped like the fake agent's payload, and set `useSessions` `configOptions` state accordingly):
   - a live session WITH a model + thinking config option renders BOTH selectors (trigger shows `acme/Alpha` / `Thinking: medium`); choosing a model item invokes `setSessionConfigOption` with `("s1", "model", "acme/beta")`
   - a live session WITHOUT `configOptions` renders neither selector
   - a STORED (non-live) session with `configOptions` renders neither selector (the `seedLiveSession`-style seed with the session in `historySessions` instead of `sessions`)

**Steps:**
- [ ] Write `src/components/SessionConfigSelect.test.tsx` (item 3) and the new `ChatStream.test.tsx` cases (item 4)
- [ ] Run `pnpm test -- src/components/SessionConfigSelect.test.tsx src/components/ChatStream.test.tsx` from the repo root
  - Did the new tests fail (component missing / not rendered)? Expected red. Pre-existing `ChatStream` tests still pass?
- [ ] Implement `SessionConfigSelect` (item 1) and the `ChatStream` header wiring (item 2)
- [ ] Run `pnpm test` from the repo root
  - All pass? If not, fix and re-run.
- [ ] Run `pnpm build` from the repo root
  - Succeeded? If not, fix and re-run.
- [ ] Commit with message: "feat: model + thinking-level selectors in the session header"

**Acceptance criteria:**
- [ ] The session header shows a Model selector (options from the agent's `model` config option) and a Thinking selector (`thought_level` option) for live sessions that advertise them; neither renders otherwise
- [ ] Selecting a value invokes `set_session_config_option` and applies the response; the selector is disabled while in flight; a failure shows a transient inline error (auto-cleared after 5 s)
- [ ] `pnpm test` + `pnpm build` green

---

## End-to-end verification (after all tasks)

- `pnpm test` + `pnpm build` (repo root); `cargo test` + `cargo clippy --all-targets` (0 warnings) + `cargo fmt --check` (`src-tauri/`)
- Manual (dev run, `pi` agent): start a session in a Space → the header shows the Model selector (pi's `provider/id` models) + the Thinking selector; pick a different model → the request round-trips and pi's next turn uses it (pi-acp also pushes a `config_option_update` — the selector stays in sync); pick a thinking level → same. Start a session with no models configured (fresh pi, no auth) → no Model selector (graceful), Thinking still present.
