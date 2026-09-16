---
status: committed
done-when: Installers for macOS, Windows, and Linux install and launch Archimedes Desktop; the user picks a project directory, starts a pi session, sends a prompt, and sees the streamed response (text, tool calls, file diffs) in the UI with working approve/deny permission prompts; the session history survives an app restart.
---

# Archimedes Desktop Plan

**Goal:** A cross-platform Tauri 2 desktop app that runs coding agents (pi first) over the Agent Client Protocol (ACP).
**Architecture:** A Rust core (Tauri) spawns one agent subprocess per live session and speaks ACP JSON-RPC over its stdio using the official `agent-client-protocol` crate; a React webview renders the conversation and provides the client-role backends (file I/O, PTY terminal, permission prompts). ACP is the only internal protocol (see `docs/decisions/0001-acp-only-internal-protocol.md`).
**Tech Stack:** Rust (Tauri 2, tokio, rusqlite, portable-pty), TypeScript/React (Vite, Tailwind, Zustand, xterm.js, shiki).

**Prerequisites for running the app (not for building):**
- Rust toolchain ≥ 1.88 (the `agent-client-protocol` 2.x crate requires edition 2024 / MSRV 1.88)
- Node ≥ 22.19, `pi` (npm: `@earendil-works/pi-coding-agent`), and the `pi-acp` adapter (npm: `pi-acp`) on PATH
- The app must detect missing prerequisites and show an actionable error, never a crash.

**Wire format note (applies to all ACP work in this plan):** ACP is newline-delimited JSON-RPC 2.0. **Property keys are camelCase** (`sessionUpdate`, `agentCapabilities`, `messageId`); **discriminator values are snake_case** (`"agent_message_chunk"`, `"end_turn"`, `"end_turn"`). Example frame:
```json
{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s1","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"hello"},"messageId":"m1"}}}
```

**Repo layout (after Task 1):**

```
archimedes-desktop/
├── CONTEXT.md                  # project glossary (exists)
├── docs/decisions/0001-*.md    # ADR (exists)
├── docs/roadmap/archimedes-desktop.md   # this plan
├── package.json
├── vite.config.ts
├── index.html
├── src/                        # React frontend
│   ├── main.tsx
│   ├── App.tsx
│   ├── store/                  # Zustand stores
│   ├── components/
│   └── lib/
└── src-tauri/
    ├── Cargo.toml
    ├── tauri.conf.json
    ├── build.rs
    ├── capabilities/default.json
    └── src/
        ├── main.rs
        ├── lib.rs
        ├── bin/fake_agent.rs   # test fixture (Task 2)
        ├── commands/           # Tauri IPC commands
        ├── acp/                # ACP client core
        ├── storage/            # SQLite + settings
        └── config/             # agent registry
```

---

### Task 1: Scaffold the Tauri 2 + React + TypeScript app

**Context:**
The repo `archimedes-desktop/` is greenfield (it contains only `CONTEXT.md` and `docs/`). This task creates the full application skeleton: a Tauri 2 Rust backend, a React + TypeScript frontend, and a working IPC round-trip proving the two halves talk to each other. Everything later builds on this skeleton.

**Files:**
- Create: `src-tauri/` (whole directory, via scaffolder), `src/` (whole directory, via scaffolder), `package.json`, `vite.config.ts`, `index.html`, `.gitignore` (merge, don't clobber)
- Modify: `src-tauri/src/lib.rs` (add `app_info` command), `src-tauri/Cargo.toml` (add deps), `src-tauri/tauri.conf.json` (productName/identifier), `package.json` (test script)
- Test: `src-tauri/src/commands/mod.rs` (unit test), `src/lib/version.test.ts` (vitest)

**What to implement:**
1. **Scaffold in a temp dir, then move.** `create-tauri-app` does NOT refuse a non-empty directory — it prompts "Current directory is not empty, do you want to overwrite?" and on **yes** recursively deletes everything except `.git`, which would destroy `CONTEXT.md` and `docs/`. **Never confirm that prompt and never pass `--force`.** Instead: run `pnpm create tauri-app@latest archimedes-desktop --template react-ts` in a parent temp dir, then move its contents into the repo root (preserving the existing files).
2. `src-tauri/tauri.conf.json`: set `productName: "Archimedes Desktop"` and a real `identifier` (e.g. `com.archimedes.desktop`) — the scaffolder derives placeholder values when the project name is `.`.
3. `src-tauri/Cargo.toml`: add `tauri = "2"`, `tauri-build = "2"` (dev), `serde`, `serde_json`, `tokio = { version = "1", features = ["process", "io-util", "sync", "macros"] }`, `thiserror = "2"`. Do NOT add `agent-client-protocol` yet (Task 2).
4. `src-tauri/src/commands/mod.rs`:
   ```rust
   pub struct AppInfo { pub version: String, pub platform: String }
   pub fn app_info() -> AppInfo { ... }
   ```
   Register in `lib.rs` via `tauri::generate_handler![commands::app_info]`.
5. Frontend: `src/lib/version.ts` exports `async function getAppInfo(): Promise<AppInfo>` calling `@tauri-apps/api` `invoke("app_info")`. `App.tsx` shows the version on mount.
6. `package.json`: add `"test": "vitest run"` (the template has no test script). Dev deps: `@tauri-apps/api` (v2), `zustand`, `tailwindcss` + `@tailwindcss/vite` (Tailwind v4 via Vite plugin, configured in `vite.config.ts`), `vitest`, `jsdom`, `@testing-library/react`, `react-markdown`, `shiki`, `xterm`, `xterm-addon-fit`.
7. `.gitignore`: keep the scaffolder's entries; ensure `node_modules`, `dist`, `src-tauri/target` are ignored.

**Steps:**
- [ ] Scaffold in temp dir; move into repo root; verify `src-tauri/Cargo.toml` and `src/main.tsx` exist and `CONTEXT.md`/`docs/` are intact
- [ ] Set `productName`/`identifier`; add the Cargo deps and `app_info` command per the spec above
- [ ] Write failing tests: `cargo test` in `src-tauri` for `app_info` returning the crate version; `pnpm test` for `getAppInfo` shape (mock `invoke`)
  - Did it fail with the expected "function not defined"/compile error? If it passed unexpectedly, stop and investigate why.
- [ ] Implement the command + frontend call
- [ ] Run `cargo test` in `src-tauri` — all pass?
- [ ] Run `pnpm test` — all pass?
- [ ] Run `cargo fmt` in `src-tauri` — clean?
- [ ] Run `cargo build` in `src-tauri` — succeeds?
- [ ] Run `pnpm build` (vite build) — succeeds?
- [ ] Run `pnpm tauri dev` once to confirm the window opens and shows the version (manual, on the dev machine); then `git commit -m "chore: scaffold Tauri 2 + React app with IPC round-trip"`

**Acceptance criteria:**
- [ ] `cargo build` and `pnpm build` both succeed from a clean clone
- [ ] `pnpm tauri dev` opens a window titled "Archimedes Desktop" showing the app version
- [ ] The scaffolder's default demo code (counter etc.) is removed — `App.tsx` only contains the version display

---

### Task 2: ACP core — spawn, initialize, session, prompt, streaming

**Context:**
This is the heart of the app: the Rust core that spawns an agent process and speaks ACP to it. Per ADR 0001, ACP is the only protocol; pi connects via the `pi-acp` adapter. The official Rust SDK provides `AcpAgent` (a `ConnectTo` transport that spawns the subprocess and manages its process group — on Unix, dropping the connection terminates the process group, which is our process-lifecycle mechanism), the `Client` role builder with `on_receive_*` handler registration, and `ConnectionTo` for sending requests.

**Critical SDK constraint that shapes this task:** the `connect_with` closure *is* the connection's lifetime — "the connection stays active until main_fn returns, then shuts down." So the closure must stay alive for the whole session, and `close_session` must be what makes it return. This is the hardest part of the plan; read it twice.

**Files:**
- Create: `src-tauri/src/acp/mod.rs`, `src-tauri/src/acp/session.rs`, `src-tauri/src/acp/errors.rs`, `src-tauri/src/config/mod.rs`, `src-tauri/src/config/registry.rs`, `src-tauri/src/commands/sessions.rs`, `src-tauri/src/bin/fake_agent.rs`
- Create: `src-tauri/tests/acp_flow.rs` (integration test)
- Modify: `src-tauri/Cargo.toml` (add `agent-client-protocol = "2"`, `uuid = { version = "1", features = ["v4"] }`), `src-tauri/src/lib.rs` (`.manage(...)` the `SessionManager` state + register the commands)
- Test: `src-tauri/tests/acp_flow.rs`

**What to implement:**

1. **`src-tauri/src/config/registry.rs`** — agent registry:
   ```rust
   #[derive(Debug, Clone, Serialize, Deserialize)]
   pub struct AgentEntry {
       pub id: String,            // e.g. "pi"
       pub name: String,          // display name
       pub command: String,       // e.g. "pi-acp" — parsed by AcpAgent::from_str
       pub args: Vec<String>,     // optional extra args
       pub env: BTreeMap<String, String>,
   }
   pub struct Registry { pub agents: Vec<AgentEntry> }
   impl Registry {
       pub fn load(dir: &Path) -> Result<Self, ConfigError>;   // reads <config_dir>/agents.json; missing file → default registry (one "pi" entry: command "pi-acp")
       pub fn get(&self, id: &str) -> Option<&AgentEntry>;
   }
   ```
   Config dir: `app.path().config_dir()` (Tauri resolve) — in tests, inject a temp dir.

2. **`src-tauri/src/acp/session.rs`** — session manager. The session-lifecycle mechanism:
   ```rust
   pub struct SessionManager {
       sessions: Arc<tokio::sync::Mutex<HashMap<SessionId, LiveSession>>>,  // Arc is cloned into driver tasks
       registry: Registry,
       config_dir: PathBuf,
   }
   pub struct LiveSession {
       cx: agent_client_protocol::ConnectionTo<Agent>,   // cheap clone, shared with the connection task
       session_id: SessionId,                             // SDK newtype over Arc<str>; convert at command boundaries
       cwd: PathBuf,
       agent_id: String,
       close_tx: tokio::sync::watch::Sender<bool>,        // set true → closure returns → connection drops → process group killed
   }
   pub trait EventSink: Send + Sync {
       fn emit(&self, event: &str, payload: serde_json::Value);   // Tauri impl: AppHandle::emit; test impl: mpsc channel
   }
   impl SessionManager {
       pub async fn start_session(&self, agent_id: &str, cwd: PathBuf, sink: &Arc<dyn EventSink>) -> Result<SessionInfo, AcpError>;   // &Arc: the driver task (tokio::spawn, 'static) needs to own a clone
       // 1. look up registry entry (AcpError::UnknownAgent if absent)
       // 2. AcpAgent::from_str / from_args with entry command+args+env; on spawn failure → AcpError::SpawnFailed { hint } (hint mentions installing pi / pi-acp)
       // 3. channels: let (ready_tx, ready_rx) = oneshot; let (session_ready_tx, session_ready_rx) = oneshot;
       //    let (session_id_tx, session_id_rx) = oneshot;   // carries the session id out of the closure to the driver
       //    let (reason_tx, reason_rx) = oneshot; let (close_tx, close_rx) = watch(false)
       //    let builder = Client.builder().name("archimedes-desktop")
       //      .on_receive_notification(forward_session_notifications, on_receive_notification!())
       //      .on_receive_request(forward_permission_request, on_receive_request!())   // Task 2: auto-respond Cancelled; Task 3 replaces with the real bridge
       // 4. SPAWN the connection as a driver task — NEVER await connect_with inline in start_session
       //    (connect_with only resolves when the closure returns, i.e. at session close):
       //    tokio::spawn(async move {
       //        builder.connect_with(agent, |cx: ConnectionTo<Agent>| async move {
       //            let cx2 = cx.clone();
       //            ready_tx.send(cx2).ok();                    // hand the connection to the manager
       //            let init = cx.send_request(InitializeRequest::new(ProtocolVersion::V1)
       //                .client_capabilities(
       //                    ClientCapabilities::default()
       //                        .fs(FileSystemCapabilities::default()
       //                            .read_text_file(true)
       //                            .write_text_file(true))
       //                        .terminal(true))
       //                .block_task().await?;                  // capture init.agent_capabilities (field name: agent_capabilities)
       //            let ns = cx.send_request(NewSessionRequest::new(cwd)).block_task().await?;
       //            session_id_tx.send(ns.session_id.clone()).ok();
       //            session_ready_tx.send(SessionInfo { session_id: ns.session_id.clone(), agent_id, cwd, capabilities: init.agent_capabilities }).ok();
       //            // BLOCK HERE until close_session OR agent death. A clean incoming EOF does NOT
       //            // cancel main_fn (SDK docs are explicit), so select on both signals:
       //            tokio::select! {
       //                _ = close_rx.changed() => { reason_tx.send(ClosedReason::User).ok(); }
       //                _ = cx.incoming_closed() => { reason_tx.send(ClosedReason::AgentExited).ok(); }
       //            }
       //            Ok(())
       //        }).await
       //        // connection returned (closed, agent died, or error): clean up
       //        let reason = reason_rx.await.unwrap_or(ClosedReason::Error);
       //        if let Ok(session_id) = session_id_rx.await {    // None → session never established; start_session's error is the signal
       //            sessions_arc.lock().await.remove(&session_id);   // idempotent remove
       //            pending_permissions_arc.lock().await.retain(|k, _| !k.starts_with(&session_id));  // drop senders → spawned tasks get Canceled
       //            sink.emit("session-closed", { session_id, reason });
       //        }
       //    })
       // 5. await ready_rx → get cx; await session_ready_rx → get SessionInfo (the session id only exists after session/new,
       //    so the store happens AFTER both resolves; clone cwd/agent_id before they move into the closure)
       // 6. store LiveSession { cx, session_id, cwd, agent_id, close_tx } in the shared map keyed by session_id
       //    If either oneshot is dropped before resolving (spawn/init/new-session failure), map it to AcpError::SpawnFailed / InitializeFailed.
       pub async fn send_prompt(&self, session_id: &str, text: String) -> Result<StopReason, AcpError>;
       // look up LiveSession, clone its cx (cheap), then:
       // PromptRequest::new(SessionId::new(session_id), vec![ContentBlock::Text(TextContent::new(text))])
       //   .block_task().await → return prompt_response.stop_reason (the frontend needs the turn-completion signal)
       pub async fn close_session(&self, session_id: &str) -> Result<(), AcpError>;   // async: it must lock the tokio sessions map to find the LiveSession
       // set close_tx → closure returns → connection drops → process group terminated (Unix)
       // the driver task performs the map removal + "session-closed" { session_id, reason: "user" } emit
   }
   ```
   `SessionInfo` and `AcpError` (thiserror: `UnknownAgent`, `SpawnFailed{hint}`, `InitializeFailed{detail}`, `Protocol(String)`) are `Serialize` so they cross IPC. **Name-collision rule:** our `SessionInfo` must never share a module with `agent_client_protocol::schema::v1::SessionInfo` — refer to the SDK type by full path if both are ever needed.
   **Forwarding:** `forward_session_notifications` maps `SessionNotification` (the `session/update` notification) into Tauri event `session-update` with payload `{ session_id, update: <serde_json::Value of the update> }`. Agent-exit detection lives in the closure's `tokio::select!` (step 4 above): when `cx.incoming_closed()` fires, the driver task removes the session from the shared map and emits `session-closed` `{ session_id, reason: "agent-exited" | "error" }`.
   **What NOT to change:** no protocol-v2 imports (`unstable_protocol_v2`) — pin to stable v1 only.

3. **`src-tauri/src/commands/sessions.rs`** — the three Tauri commands (this file is created HERE, extended in Task 3):
   ```rust
   // State type: tauri::State<'_, tokio::sync::Mutex<SessionManager>>
   // — tokio's Mutex, NOT std's: these are async commands and holding a std lock across .await is a deadlock trap.
   #[tauri::command] async fn start_session(app: AppHandle, state: State<'_, tokio::sync::Mutex<SessionManager>>, agent_id: String, cwd: String) -> Result<SessionInfo, AcpError>;
   #[tauri::command] async fn send_prompt(state: State<'_, tokio::sync::Mutex<SessionManager>>, session_id: String, text: String) -> Result<StopReason, AcpError>;
   #[tauri::command] async fn close_session(app: AppHandle, state: State<'_, tokio::sync::Mutex<SessionManager>>, session_id: String) -> Result<(), AcpError>;
   ```
   Each builds the `EventSink` from the `AppHandle` (a small `TauriSink` struct wrapping it) and calls the corresponding `SessionManager` method. Register all three in `lib.rs` and `.manage(Arc::new(Mutex::new(SessionManager::new(...))))` (tokio's Mutex).

4. **`src-tauri/src/bin/fake_agent.rs`** — a minimal fake ACP agent as a **bin target** (this is the only cargo-idiomatic way to spawn a helper binary from integration tests via `env!("CARGO_BIN_EXE_fake_agent")`). Plain `std`, no async: read NDJSON lines from stdin, write NDJSON to stdout. Behavior:
   - `initialize` → respond with `protocolVersion: 1`, `agentInfo`, `agentCapabilities: { loadSession: false }` (note: `terminal` is a *client* capability, not an agent one — do not put it here)
   - `session/new` → respond with a fixed `sessionId`
   - `session/prompt` → emit two `session/update` notifications (`agent_message_chunk`, text "hello" then " world", same `messageId` "m1"), then respond with `stopReason: "end_turn"`
   - (Task 3 will extend this fixture; for now it does nothing else)
   - Remember the wire format from the note above: camelCase keys, snake_case discriminators.
5. **`src-tauri/tests/acp_flow.rs`** — integration test: point the registry at a temp config dir whose agent entry points at `env!("CARGO_BIN_EXE_fake_agent")`; use an `EventSink` test impl backed by an `mpsc` channel. Assert: `SessionInfo.session_id` matches the fake's id; `send_prompt` returns `end_turn`; both chunks arrive in order; `close_session` makes the session go away — assert the `session-closed` event arrives via the mpsc sink, the shared map is empty, and `pgrep -f fake_agent` finds nothing. **Second test (agent death):** start a session, find the fake agent's pid (`pgrep -f fake_agent`), `kill` it, and assert a `session-closed` event with reason `agent-exited` arrives and the session is removed from the map.

**Steps:**
- [ ] Add `agent-client-protocol = "2"` and `uuid` to `src-tauri/Cargo.toml`; run `cargo build` in `src-tauri` — resolves? (If the crate's latest API differs from the signatures above, consult `cargo doc -p agent-client-protocol --open` and the example at `rust-sdk/src/agent-client-protocol/examples/yolo_one_shot_client.rs`; adapt names, keep the architecture.)
- [ ] Write `src/bin/fake_agent.rs` and `tests/acp_flow.rs` with the assertions above
- [ ] Run `cargo test --test acp_flow` in `src-tauri` — fails (SessionManager doesn't exist)?
- [ ] Implement `config/registry.rs`, then `acp/session.rs` (with the closure-lifecycle mechanism from step 2), then `commands/sessions.rs`
- [ ] Run `cargo test` in `src-tauri` — all pass?
- [ ] Run `cargo fmt` — clean?
- [ ] Run `cargo clippy` — no new warnings?
- [ ] `git commit -m "feat: ACP session core — spawn, initialize, prompt, streaming updates"`

**Acceptance criteria:**
- [ ] Integration test drives the fake agent through initialize → session/new → prompt → 2 streamed updates → close, with the connection torn down and the child process reaped
- [ ] Killing the fake agent mid-session produces `session-closed { reason: "agent-exited" }` and removes the session from the map (no leaked entries)
- [ ] `SessionManager` takes an `EventSink` trait (test-injectable); the Tauri wiring implements it with `AppHandle::emit`
- [ ] `send_prompt` returns the stop reason
- [ ] No protocol-v2 code anywhere

---

### Task 3: Client-role backends — file I/O, PTY terminal, permission bridge

**Context:**
ACP agents delegate work to the client: they request file reads/writes (`fs/read_text_file`, `fs/write_text_file`), ask the client to create terminals (`terminal/*`), and ask for permission before tool calls (`session/request_permission`). This task implements all three in the Rust core and exposes the permission flow to the frontend.

**Critical concurrency constraint:** the SDK runs all handler callbacks on a single event-loop task — while a handler is running, no new messages are processed. Therefore **no handler may await anything user-paced**. The permission flow must spawn a task and return immediately.

**Files:**
- Create: `src-tauri/src/acp/fs_backend.rs`, `src-tauri/src/acp/terminal.rs`, `src-tauri/src/acp/permission.rs`
- Modify: `src-tauri/src/acp/session.rs` (wire the real handlers, replacing the Task-2 stub), `src-tauri/src/commands/sessions.rs` (add `respond_permission`), `src-tauri/Cargo.toml` (add `portable-pty = "0.8"`, `async-trait = "0.1"`), `src-tauri/src/bin/fake_agent.rs` (extend fixture)
- Test: `src-tauri/tests/fs_backend.rs`, `src-tauri/tests/terminal_flow.rs`

**What to implement:**

1. **`fs_backend.rs`** — handles `ReadTextFileRequest` / `WriteTextFileRequest`:
   ```rust
   pub struct FsBackend { pub root: PathBuf }   // the session's cwd
   impl FsBackend {
       pub fn read(&self, path: &Path) -> Result<String, AcpError>;
       pub fn write(&self, path: &Path, content: &str) -> Result<(), AcpError>;
   }
   ```
   **Sandboxing (critical):** canonicalize the requested path and reject (with `AcpError::PathEscape`) anything that does not remain under `root`, including escapes via `..` or symlinks. **For writes to a not-yet-existing file, `canonicalize` fails — canonicalize the deepest existing ancestor instead, validate the prefix, then append the remaining components.**
2. **`terminal.rs`** — handles the ACP v1 terminal methods. The real v1 type names are: `CreateTerminalRequest`, `TerminalOutputRequest` (agent pulls output — respond with buffered output), `WaitForTerminalExitRequest` (response carries `exit_code` + optional signal), `KillTerminalRequest`, `ReleaseTerminalRequest`. There is **no** `terminal/write` method in v1 — do not invent one.
   - Spawn a PTY via `portable_pty::native_pty_system()` with a 120×30 default size; shell = `sh -c <cmd>` on Unix / `powershell -Command <cmd>` on Windows.
   - Terminal id = uuid `String`. Output is read on a dedicated task, buffered, and served to `TerminalOutputRequest`; also emit Tauri event `terminal-output` `{ terminal_id, data: base64 }` so the UI can show it live.
   - `WaitForTerminalExitRequest` resolves when the child exits.
3. **`permission.rs`** — the permission bridge (non-blocking by construction):
   - On `RequestPermissionRequest`: (a) emit Tauri event `permission-request` `{ session_id, request_id, request }` where `request_id` = `responder.id()`; (b) create a `oneshot::Sender<PermissionOutcome>` and store it in `SessionManager.pending_permissions: tokio::sync::Mutex<HashMap<String, oneshot::Sender<PermissionOutcome>>>` keyed by `format!("{session_id}/{request_id}")` — the compound key lets the driver-task cleanup (Task 2, step 4) drain all entries for a closing session, which drops the senders so the spawned tasks get `Canceled` promptly instead of waiting out the 300 s timeout; (c) `cx.spawn` a task that owns the `Responder` + oneshot receiver, awaits `tokio::time::timeout(Duration::from_secs(300), rx)`, and calls `responder.respond(...)` — `Selected { option_id }` mapped to `RequestPermissionResponse::new(RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(id)))`, or `Cancelled` otherwise; (d) **return from the handler immediately.** The handler reaches the manager's maps via `Arc` clones of the `Mutex`es captured when the builder was constructed in `start_session`.
   - **Spawned-task discipline (critical):** the SDK shuts down the *entire connection* if a spawned task returns `Err`. The task must therefore return `Ok(())` on ALL paths — map the 300 s timeout, a `Canceled` oneshot (session closed before the user answered), and a failed `respond(...)` all to a `Cancelled` response (send it best-effort, ignore its result) and return `Ok(())`. The task must call `respond` exactly once — a dropped responder leaves the agent's `session/request_permission` hanging forever.
   - New Tauri command `respond_permission(session_id, request_id, outcome: PermissionOutcome)` where `PermissionOutcome` is `Selected { option_id: String } | Cancelled`. The command looks up `pending_permissions` by the compound key `"{session_id}/{request_id}"` and sends the outcome through the oneshot; if the entry is gone, silently drop.
4. Wire all three into the `Client.builder()` chain in `session.rs`, replacing the Task-2 auto-Cancelled permission stub.
5. **Extend `src/bin/fake_agent.rs`:** on `session/prompt`, first send a `session/request_permission` request (one option, id "opt-1") and wait for the response before emitting the message chunks. (The *agent* sends the request; the client responds — keep the roles straight.) For `terminal/create` (added for the terminal tests), remember the wire shape is `command: String` + `args: Vec<String>` — e.g. `{"command": "echo", "args": ["hello"]}` — not a single array.
6. **Tests:**
   - `fs_backend.rs`: read/write round-trip inside root; path-escape attempts (`../etc/passwd`, symlink pointing outside) rejected; write to a new file under root succeeds.
   - `terminal_flow.rs`: fake agent sends `terminal/create` with `{"command": "echo", "args": ["hello"]}` (two fields — `command: String` + `args: Vec<String>`, not a single array); test asserts `terminal-output` events contain "hello" and the `terminal/wait_for_exit` response carries exit code 0.
   - Extend `tests/acp_flow.rs`: fake agent requests permission; test calls `respond_permission` with "opt-1"; assert the fake agent received `Selected("opt-1")` and streaming continued while the prompt was pending (chunks arrive after the response, and the event loop was not blocked — i.e. a `session/update` emitted before the request is still received while the prompt is open).

**Steps:**
- [ ] Write `tests/fs_backend.rs` failing tests (round-trip + 2 escape attempts + new-file write)
- [ ] Run `cargo test --test fs_backend` — fails as expected?
- [ ] Implement `fs_backend.rs`; re-run — pass?
- [ ] Write `tests/terminal_flow.rs` failing test
- [ ] Implement `terminal.rs`; re-run — pass?
- [ ] Implement `permission.rs` + `respond_permission` command; extend the fake agent and `tests/acp_flow.rs` with the permission round-trip
- [ ] Run `cargo test` — all pass?
- [ ] Run `cargo fmt` + `cargo clippy` — clean?
- [ ] `git commit -m "feat: ACP client backends — sandboxed fs, PTY terminal, permission bridge"`

**Acceptance criteria:**
- [ ] Path-escape attempts are rejected with `PathEscape`, including the symlink case; writing a new file under root works
- [ ] Terminal output for `echo hello` reaches the event stream and `wait_for_exit` returns exit code 0
- [ ] A permission request from the fake agent is answerable from a Tauri command, the ACP response matches the chosen option, and streaming is not blocked while the prompt is open

---

### Task 4: Frontend conversation UI

**Context:**
The UI the user actually sees: a three-pane layout (session list / conversation stream / terminal+file tree), with streaming agent output, inline diff blocks, and permission prompts. All data arrives via Tauri events (`session-update`, `terminal-output`, `permission-request`, `session-closed`) and commands from Tasks 2–3.

**Files:**
- Create: `src/store/sessions.ts`, `src/store/permissions.ts`, `src/components/SessionList.tsx`, `src/components/ChatStream.tsx`, `src/components/MessageBubble.tsx`, `src/components/ToolCallCard.tsx`, `src/components/DiffBlock.tsx`, `src/components/PermissionPrompt.tsx`, `src/components/TerminalPane.tsx`, `src/components/NewSessionDialog.tsx`
- Modify: `src/App.tsx` (layout), `src/lib/tauri.ts` (typed wrappers), `package.json` (add `@tauri-apps/plugin-dialog`), `src-tauri/Cargo.toml` (add `tauri-plugin-dialog = "2"`), `src-tauri/src/lib.rs` (register the dialog plugin), `src-tauri/capabilities/default.json` (add `dialog:default`)
- Test: `src/store/sessions.test.ts`, `src/components/PermissionPrompt.test.tsx`

**What to implement:**

1. **Dialog plugin (needed by this task, not Task 6):** add `tauri-plugin-dialog = "2"` to `src-tauri/Cargo.toml`, register it in `lib.rs`, add `dialog:default` to `src-tauri/capabilities/default.json`, and add `@tauri-apps/plugin-dialog` to `package.json`.
2. **`src/lib/tauri.ts`** — typed wrappers over `invoke`/`listen` for every Rust command/event from Tasks 2–3 (including `session-closed`). Event listeners are registered once in `App.tsx` and dispatch into the stores.
3. **`src/store/sessions.ts`** (Zustand): `sessions: SessionInfo[]`, `activeSessionId`, `messages: Record<sessionId, Message[]>` where
   ```ts
   type Message =
     | { kind: "user"; text: string; at: number }
     | { kind: "agent-text"; messageId: string; text: string; at: number }   // keyed by ContentChunk.messageId
     | { kind: "tool-call"; id: string; title: string; status: "pending"|"completed"|"failed"; diff?: { path: string; patch: string }; at: number }
     | { kind: "diff"; path: string; patch: string; at: number };
   ```
   The `session-update` reducer:
   - `agent_message_chunk` → append to the message with the same `messageId`; a **new `messageId` starts a new message** (do NOT just append to "the last agent-text" — real agents emit multiple messages per turn).
   - `tool_call` / `tool_call_update` → create/update the `tool-call` message. **Diffs arrive inside tool calls** as `ToolCallContent::Diff` content — the reducer extracts them into a `diff` message (there is no dedicated file-edit update type).
   - Unknown update types are ignored (forward-compat).
   - On `session-closed`: remove the session, dismiss its permission prompts, mark pending tool calls failed.
   - Turn completion: `sendPrompt` resolves with the stop reason → dispatch a `turnCompleted` action that finalizes the turn (no accumulation state left open).
4. **Components:**
   - `SessionList` — sessions with agent name + cwd; "New session" opens `NewSessionDialog` (directory picker via `@tauri-apps/plugin-dialog` `open({ directory: true })`).
   - `ChatStream` — auto-scrolling list of `MessageBubble`s; agent text via `react-markdown` + `shiki` (github-dark); `ToolCallCard` collapsible; `DiffBlock` renders a unified patch (line-based red/green from the patch string — no diff library needed).
   - `PermissionPrompt` — inline card when a `permission-request` event arrives for the active session: tool title + options; buttons call `respondPermission`.
   - `TerminalPane` — xterm.js fed by `terminal-output` events; fit addon; one per active session.
5. **State rules:** a permission prompt auto-dismisses (as "cancelled") on `session-closed` for its session.

**Steps:**
- [ ] Add the dialog plugin (Rust + npm + capability) — `cargo build` + `pnpm build` still pass
- [ ] Write `sessions.test.ts`: reducer tests for chunk append by messageId, new messageId starting a new message, tool-call create/update, diff extraction from a tool-call update, session-closed cleanup
- [ ] Run `pnpm test` — fails as expected?
- [ ] Implement store + `lib/tauri.ts`
- [ ] Run `pnpm test` — pass?
- [ ] Implement components (layout → ChatStream → DiffBlock → PermissionPrompt → TerminalPane)
- [ ] Write `PermissionPrompt.test.tsx` (renders options; clicking calls `respondPermission` with the right option id — mock the tauri layer); run `pnpm test` — pass? (jsdom is already configured from Task 1)
- [ ] Run `pnpm build` — pass?
- [ ] Manual: `pnpm tauri dev`, start a session against the fake-agent fixture (a dev-only registry entry), send a prompt, verify streaming, diff rendering, and a permission round-trip
- [ ] `git commit -m "feat: conversation UI — streaming chat, diffs, permission prompts, terminal pane"`

**Acceptance criteria:**
- [ ] All store reducer tests pass; `pnpm build` succeeds
- [ ] In `tauri dev` with the fake agent: prompt → text streams token-by-token, a permission prompt appears and is answerable, terminal pane shows `echo hello` output
- [ ] Closing a session kills the agent process (visible in `ps`) and its permission prompt disappears via `session-closed`

---

### Task 5: Persistence — SQLite history, settings, resume semantics

**Context:**
The Client owns history (per the approved design): the app must remember every session and message across restarts, and must distinguish true resume (agent advertised `loadSession`) from history-replay-only.

**Files:**
- Create: `src-tauri/src/storage/mod.rs`, `src-tauri/src/storage/db.rs`, `src-tauri/src/commands/history.rs`
- Modify: `src-tauri/Cargo.toml` (add `rusqlite = { version = "0.37", features = ["bundled"] }`), `src-tauri/src/lib.rs` (migrate on startup, register commands), `src-tauri/src/acp/session.rs` (add `resume_session`), `src-tauri/src/commands/sessions.rs` (add `resume_session`), `src/store/sessions.ts` (load history on boot)
- Test: `src-tauri/tests/storage.rs`

**What to implement:**

1. **`storage/db.rs`** — SQLite in `app.path().app_data_dir()/archimedes.db`:
   ```sql
   CREATE TABLE sessions (
     id TEXT PRIMARY KEY,            -- ACP session id
     agent_id TEXT NOT NULL,
     cwd TEXT NOT NULL,
     created_at INTEGER NOT NULL,    -- unix ms
     title TEXT,
     capabilities_json TEXT NOT NULL
   );
   CREATE TABLE messages (
     id INTEGER PRIMARY KEY,
     session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
     kind TEXT NOT NULL,             -- 'user' | 'agent-text' | 'tool-call' | 'diff'
     message_key TEXT,               -- ContentChunk.messageId for agent-text; NULL otherwise
     payload_json TEXT NOT NULL,
     created_at INTEGER NOT NULL,
     UNIQUE(session_id, kind, message_key)
   );
   CREATE INDEX idx_messages_session ON messages(session_id, id);
   ```
   API: `Db::open(path)`, `Db::record_session(&SessionInfo)`, `Db::record_message(session_id, kind, message_key, payload_json)` (INSERT ... ON CONFLICT(session_id, kind, message_key) DO UPDATE — the upsert), `Db::list_sessions() -> Vec<SessionRow>`, `Db::messages_for(session_id) -> Vec<MessageRow>`.
   Hook into the session-update pipeline: agent-text is stored as the *accumulated* text, upserted by `(session_id, 'agent-text', message_id)` — one row per message, not per chunk.
2. **Settings:** `settings.json` in the config dir — `{ "theme": "dark"|"light", "paneLayout": {...} }` with sane defaults on first run. Tauri commands `get_settings()` / `save_settings(settings)` so the frontend can read and persist it.
3. **History commands:** `list_sessions()`, `load_history(session_id)`, `delete_session(session_id)`.
4. **True resume (Rust side):** add `SessionManager::resume_session(agent_id, session_id, cwd, sink)`: spawn a fresh `AcpAgent` for that agent, `initialize` (same client capabilities), then `cx.load_session(SessionId::new(session_id), cwd.as_path())` — note the **two-argument** signature (session_id + cwd) — and consume the returned `RestoreSessionBuilder` like the session builder: `.block_task().start_session().await?` (yields the `ActiveSession` + `LoadSessionResponse`) instead of `session/new`. **Resume reuses the Task-2 driver-task lifecycle verbatim** (same `LiveSession` storage, `select!` on close/`incoming_closed`, driver cleanup + `session-closed` emit) — only the `NewSessionRequest` is replaced by `load_session`. Add a `resume_session` Tauri command. If the agent's `agent_capabilities.load_session` is false, the command returns `AcpError::NotResumable` and the UI shows the read-only banner instead.
5. **Frontend:** on boot, `list_sessions()` populates `SessionList`; opening an old session loads its history into the chat. If the session is resumable (capabilities say so), a "Resume" button calls `resume_session`; otherwise a read-only banner says "history only — continuing starts a new session."

**Steps:**
- [ ] Write `tests/storage.rs`: open temp DB, record session + 3 messages (two agent-text chunks sharing one message_key, plus one tool-call message), list + fetch back (assert exactly 1 agent-text row with the accumulated content, plus 1 tool-call row), delete cascades
- [ ] Run `cargo test --test storage` — fails as expected?
- [ ] Implement `db.rs` + commands; re-run — pass?
- [ ] Wire persistence into the session-update pipeline (upsert semantics)
- [ ] Implement `resume_session` (manager method + command); extend `tests/acp_flow.rs`: fake agent gains a `session/load` response; test resumes and asserts the session id round-trips
- [ ] Frontend: boot load + resume button / read-only banner; `pnpm test` + `pnpm build` pass?
- [ ] Manual: `tauri dev`, run a session, quit, relaunch — history present; with the fake agent (no loadSession) the banner says history-only
- [ ] `git commit -m "feat: SQLite history, settings, and honest resume semantics"`

**Acceptance criteria:**
- [ ] Kill the app mid-session, relaunch, and the full conversation (text, tool cards, diffs) is restored with no duplicate rows
- [ ] Deleting a session removes its messages (cascade verified in test)
- [ ] The resume UI distinguishes true-resume vs history-only based on negotiated `agent_capabilities.load_session`

---

### Task 6: Packaging, auto-update, CI

**Context:**
Ship installers for all three platforms with a signed-update path. Per the approved design: macOS gets Developer ID + notarization from day one; Windows ships unsigned for v1 (SmartScreen warnings are the category norm); Linux ships AppImage + deb. Auto-update uses `tauri-plugin-updater` with mandatory minisign verification against a static `latest.json` on GitHub Releases.

**Files:**
- Create: `.github/workflows/release.yml`, `.github/workflows/ci.yml`, `README.md` (setup, prerequisites, install, Linux NVIDIA note)
- Modify: `src-tauri/tauri.conf.json` (bundle targets, updater config), `src-tauri/Cargo.toml` (add `tauri-plugin-updater = "2"`), `src-tauri/src/lib.rs` (register updater plugin), `src-tauri/capabilities/default.json` (add `updater:default`), `package.json` (add `@tauri-apps/plugin-updater`), `src/lib/updater.ts`
- Test: `src/lib/updater.test.ts` (mock the updater API)

**What to implement:**

1. **Generate the minisign keypair first** (the updater pubkey is required but nothing ships one): run `pnpm tauri signer generate`, put the **public key** into `plugins.updater.pubkey`, and store the **private key** as the `TAURI_SIGNING_PRIVATE_KEY` CI secret (optionally with `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`).
2. **`tauri.conf.json`:**
   - `bundle.targets`: `"nsis"` (Windows), `"app"` + `"dmg"` (macOS), `"appimage"` + `"deb"` (Linux)
   - `bundle.createUpdaterArtifacts: true` (without this, no `.app.tar.gz`/`.sig` updater artifacts are produced at all)
   - `plugins.updater.pubkey`: the minisign **public key content** (string, not a path) — required, cannot be disabled
   - `plugins.updater.endpoints`: `["https://github.com/<owner>/archimedes-desktop/releases/latest/download/latest.json"]` — a static file, not a templated URL (supported template variables are only `{{current_version}}`, `{{target}}`, `{{arch}}`; do not invent others)
2. **Updater:** register `tauri_plugin_updater::Builder::new().build()` in `lib.rs`; add `updater:default` to `capabilities/default.json`; add `@tauri-apps/plugin-updater` to `package.json`. `src/lib/updater.ts`: `checkForUpdate()` / `installUpdate()` wrappers; a "Check for updates" menu item.
3. **CI:**
   - `ci.yml` (push/PR): `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`, `pnpm test`, `pnpm build`.
   - `release.yml` (on tag `v*.*.*`): three jobs —
     - **macos-latest:** env `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID` (secrets); `pnpm install --frozen-lockfile`, `pnpm tauri build` with `TAURI_SIGNING_PRIVATE_KEY` (+ optional `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`) as build env — `tauri build` signs the artifacts (`.sig` files) at build time.
     - **windows-latest:** no signing for v1; same build steps.
     - **ubuntu-latest:** `apt-get install -y libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf xdg-utils` before building (Tauri's Linux prerequisites); same build steps + signing env.
   - Upload artifacts to the release via `tauri-apps/tauri-action` with `uploadUpdaterJson: true` (it generates `latest.json` for the updater) — replace the `<owner>` placeholder at first release.
   - Validate workflow YAML with the `rhysd/actionlint` GitHub Action (there is no `npx actionlint` package).
4. **README.md:** what the app is, prerequisites (Rust ≥ 1.88 for building; Node ≥ 22.19, pi, `pi-acp` for running), install per platform, the `WEBKIT_DISABLE_DMABUF_RENDERER=1` workaround for Linux NVIDIA/DMABUF users, and a link to `CONTEXT.md`.

**Steps:**
- [ ] Configure `tauri.conf.json` (targets, `createUpdaterArtifacts`, `pubkey`, static endpoint); add the updater plugin + npm package + capability
- [ ] Write `updater.test.ts` (mocks `@tauri-apps/plugin-updater`); `pnpm test` — pass?
- [ ] Write both workflows; verify with `rhysd/actionlint`
- [ ] Run `pnpm tauri build` on the dev machine — produces the native artifact for this OS?
- [ ] `git commit -m "chore: packaging — bundle targets, updater, CI matrix"`

**Acceptance criteria:**
- [ ] `pnpm tauri build` produces a working installer for the dev OS that launches the app, plus `.app.tar.gz` + `.sig` updater artifacts
- [ ] `ci.yml` passes on a clean checkout (fmt, clippy, tests, builds)
- [ ] README accurately states prerequisites and the Linux workaround

---

## Out of scope (later phases)

- Other ACP agents (Claude Code, Codex, Gemini CLI, OpenCode) — registry entries only; no core changes expected
- pi-archimedes TUI-extension features (todo board, subagent side-by-side) — blocked on ACP exposure
- Windows MSI, Windows EV code signing, ACP protocol v2 support
- PTY terminal (ACP terminal/* methods + TerminalPane + xterm)
