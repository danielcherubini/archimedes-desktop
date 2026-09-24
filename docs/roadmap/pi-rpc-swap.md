---
status: committed
done-when: The desktop drives pi natively over `pi --mode rpc` — a session starts, streams (text/thinking/tool calls), gates tool calls through the bundled gate extension (existing PermissionPrompt UI), resumes from pi's session files, spawns subagents as direct `pi --mode rpc` children, `agent-client-protocol` is gone from Cargo.toml, and `cargo test` + `cargo clippy --all-targets` (0 warnings) + `cargo fmt --check` (in `src-tauri/`) + `pnpm test` + `pnpm build` (repo root) are all green.
---

# Pi RPC Replaces ACP (Phase 1) Plan

**Goal:** Replace the ACP protocol layer in the Rust core with a native client for pi's RPC mode (`pi --mode rpc`), keeping the Tauri command surface, the bridge channel, the worker runtime, the SQLite persistence, and the frontend event contract unchanged.
**Architecture:** `SessionDriver` is already structured as "generic session machinery + protocol-specific establisher closure + protocol-specific event normalization". The swap replaces the protocol layer: a new `PiRpc` child-process wrapper (JSONL over stdio) replaces `AcpAgent`/`ConnectionTo<Agent>`; the establisher becomes `get_state` (new) / `get_messages` (resume); the event normalizer maps pi events onto the *existing* ACP-shaped `session-update` JSON the frontend already consumes; a small bundled pi extension (the "gate") restores the generic permission prompt via the extension-UI subprotocol; subagents spawn `pi --mode rpc` directly (the launch-wrapper script dies). ACP artifacts are deleted. The bridge (`bridge.rs`), `worker_runtime.rs`, the `SubagentSessionManager` shape, `storage/`, and the frontend are preserved.
**Tech Stack:** Rust (Tauri 2, tokio, serde), the existing fake-binary test pattern; pi v0.87.1 RPC protocol. Protocol reference: `docs/research/pi-rpc-replaces-acp.md` §F1 and the `.d.ts` files below.

**Background reading (required before executing any task):** `docs/research/pi-rpc-replaces-acp.md` (evidence + RPC surface catalog — **its §F1 table has the correct camelCase wire field names; use it as the source of truth for field names**), `docs/decisions/0001`, `0002`, `0003`, `0004`, `0005`. Reference protocol shapes: `/home/daniel/.local/lib/node_modules/@earendil-works/pi-coding-agent/dist/modes/rpc/rpc-types.d.ts` (commands; the extension-UI unions are at **lines 385-453**; the `RpcResponse` region is at :193-248 and carries a **required `command: string`** field), `dist/modes/json-event.d.ts:9-27` (the `assistantMessageEvent` wire shapes), `dist/core/agent-session.d.ts:41-103` (`AgentSessionEvent`), `@earendil-works/pi-ai/dist/types.d.ts` (`AgentMessage` — **has NO `id` field**; `ImageContent = {type:"image", data, mimeType}`), `dist/core/extensions/types.d.ts` (extension API).

**Global rules:**
- TDD: every task writes the failing test first, confirms it fails, then implements.
- The frontend event contract is FROZEN: `session-update` payloads keep their exact current JSON shape (Task 3's mapping table), `permission-request`/`bridge-request`/`bridge-event`/`subagent-*` shapes unchanged. Tauri **command names** are unchanged; command **bodies and return types** are rewritten in Task 3 (the ACP crate types they use die). Permitted frontend changes: none, except a type *widening* if a build breaks (the TS `capabilities` type is already `Record<string, unknown>` at `src/lib/tauri.ts:57` — **build success proves nothing about the semantic keys; see Task 3 item 1**).
- `src-tauri/src/agent/bridge.rs`: receives **only** the mechanical `crate::acp::` → `crate::agent::` path renames in Task 1, the `LaunchConfig` import retarget in Task 5, and the stale `pi-acp` doc-comment updates in Task 6 (docs-only). **No behavioral changes in any task.**
- Do NOT modify `src-tauri/src/agent/worker_runtime.rs` in any task.
- Do NOT modify the pi-archimedes suite (`/home/daniel/Coding/Javascript/pi-archimedes/`) — it is protocol-agnostic and unchanged in Phase 1.
- Commit each task separately (one commit per task, message given in the task).
- Validation gate before each commit: `cargo test` (in `src-tauri/`) + `cargo clippy --all-targets` (0 warnings) + `cargo fmt` (apply, then `cargo fmt --check` passes).

---

### Task 1: Rename `acp/` → `agent/` (mechanical, zero behavior change)

**Context:** The protocol layer is being replaced, so the module name `acp` is misleading. A clean mechanical rename first means all later tasks land in a stable location. This task changes NO behavior — every test must pass exactly as before.

**Files:**
- Rename: `src-tauri/src/acp/` → `src-tauri/src/agent/` (`git mv src-tauri/src/acp src-tauri/src/agent`)
- Modify: `src-tauri/src/lib.rs` (`pub mod acp;` → `pub mod agent;`; `use crate::acp::{…}` → `use crate::agent::{…}`)
- Modify: `src-tauri/src/agent/bridge.rs` — the mechanical `crate::acp::` → `crate::agent::` path renames inside it (allowed exception to the no-touch rule; path renames only, no logic)
- Modify: **all 8 integration test files** `src-tauri/tests/{fs_backend,acp_flow,bridge_integration,storage,subagent_concurrency,subagent_dispatch,acp_concurrency_real,ipc}.rs` — they import `archimedes_desktop_lib::acp::…` (NOT `crate::acp`): rename to `archimedes_desktop_lib::agent::…`
- Modify: every other file referencing the module (grep for `acp::` with ANY prefix — `crate::acp`, `archimedes_desktop_lib::acp`, and `acp::` inside rustdoc `use`/links — across `src-tauri/src/` and `src-tauri/tests/`): includes `commands/*.rs`, `test_support.rs`, `bin/fake_agent.rs` doc comments
- Leave the file *names* in `tests/` as-is in this task (`acp_flow.rs` etc. are renamed/adapted in Tasks 3 and 6); leave the "ACP session core" doc line in `agent/mod.rs` for Task 3

**What to implement:**
- Pure rename: `git mv`, then replace every `acp::` occurrence (any prefix) with `agent::`, and `pub mod acp` → `pub mod agent`. No logic changes, no reordering, no doc rewrites beyond the path itself.
- Do NOT change any file's logic in this task.

**Steps:**
- [ ] Run `cargo test` in `src-tauri/` — record the current green baseline (all tests pass, including the 8 integration test files).
- [ ] `git mv src-tauri/src/acp src-tauri/src/agent`
- [ ] Replace all `acp::` → `agent::` (any prefix) and the `lib.rs` module declaration.
- [ ] Run `cargo test` in `src-tauri/` — all tests pass identically (lib + 8 integration files)? If any test file fails to compile, fix its import path before continuing.
- [ ] Run `cargo clippy --all-targets` — 0 warnings.
- [ ] Run `cargo fmt` then `cargo fmt --check` — clean.
- [ ] Commit with message: "refactor: rename acp module to agent (preparation for the RPC swap)"

**Acceptance criteria:**
- [ ] `src-tauri/src/acp/` no longer exists; `src-tauri/src/agent/` contains the same files.
- [ ] `grep -rn "acp::" src-tauri/src src-tauri/tests` returns nothing (and `grep -rn "pub mod acp" src-tauri/` returns nothing).
- [ ] `cargo test` + clippy + fmt all green with zero test-result changes.

---

### Task 2: The RPC wire client (`PiRpc` + `PiRpcHandle`) + `fake_pi` test double

**Context:** The core of the swap is a Rust client for pi's RPC mode: strict JSONL over stdio, one JSON object per line, bidirectional (commands + `extension_ui_response` on stdin; `response` + session events on stdout), async correlation by an optional `id` echoed in the matching `response`. This task builds that client behind a small, testable API, plus a deterministic `fake_pi` binary so later tasks can test session flows without a real LLM. Protocol reference: `docs/research/pi-rpc-replaces-acp.md` §F1 (verified method/event catalog — **the source of truth for field names**) and the `.d.ts` files above.

**Files:**
- Create: `src-tauri/src/agent/rpc.rs`
- Modify: `src-tauri/src/agent/errors.rs` (this task ADDS `RpcError` to the existing file — `AcpError` stays until Task 3)
- Create: `src-tauri/src/bin/fake_pi.rs` (a test-only binary, mirroring the existing `bin/fake_agent.rs` pattern)
- Modify: `src-tauri/src/agent/mod.rs` (`pub mod rpc;` + `pub use rpc::{PiRpc, PiRpcHandle, RpcEvent, ExtensionUiRequest, ExtensionUiResponse};`)
- Modify: `src-tauri/Cargo.toml` only if a new dependency is genuinely needed (prefer std/tokio/serde/serde_json/uuid — all already present; **do NOT add `futures`** — the establisher uses a generic `Future` bound, see Task 3)

**What to implement:**

`src-tauri/src/agent/errors.rs` — ADD (keep `AcpError` untouched in this task; follow `AcpError`'s existing derive pattern — it derives `Serialize` "so it can cross the Tauri IPC boundary as a command error", `PartialEq`, `Debug`, and has `Display`/`Error` impls; `RpcError` gets the SAME set so it can replace `AcpError` in command signatures in Task 3):
```rust
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RpcError {
    Spawn(String),                    // child failed to start
    Io(String),                      // stdin/stdout read-write failure
    ProcessExited(Option<i32>),      // child died while a command was in flight
    EstablishTimeout { detail: String }, // the kept establish-timeout mechanism (session.rs:543-564 marker, :665-676 no-delivery fallback) — replaces AcpError::InitializeFailed
    Command { error: String },       // response arrived with success: false
    Parse(String),                   // malformed JSON line on stdout
    UnknownAgent { id: String },     // (used by commands, Task 3)
    UnknownSession { id: String },   // (used by commands, Task 3)
    NotResumable { id: String },     // (used by commands, Task 3)
    FolderMissing { path: String },  // (used by commands, Task 3)
    InvalidPrompt { reason: String },// (used by commands, Task 3)
}
```
(Implement `Display` + `std::error::Error` for `RpcError` following `AcpError`'s pattern in the same file.)

`src-tauri/src/agent/rpc.rs` — two public types:
```rust
/// Owns the child process. Created at the spawn site; `close()` is called
/// by `close_session`.
pub struct PiRpc { /* tokio Child + the stdin writer task */ }
impl PiRpc {
    /// Spawn `program` with `args`/`env`/`cwd`. `env` is ADDITIVE (the child
    /// inherits the desktop's environment, matching how the ACP agent was
    /// spawned) and is typed `&BTreeMap<String, String>` to match the existing
    /// `AgentEntry.env` / `bridge_spawn_setup` flow.
    pub fn spawn(program: &str, args: &[String], env: &BTreeMap<String, String>, cwd: &std::path::Path) -> Result<Self, RpcError>;

    /// A cheap-clonable handle (an `Arc` over the shared inner state: the
    /// stdin sender, the pending-response map, the event channels, the exit
    /// watch, the close hook). The driver task, the `LiveSession` entry, and
    /// command paths all hold clones — mirroring how `cx: ConnectionTo<Agent>`
    /// was a cheap clone passed around today. `PiRpc` itself is dropped when
    /// the spawn site returns; ALL lifecycle operations go through the handle.
    pub fn handle(&self) -> PiRpcHandle;
}

#[derive(Clone)]
pub struct PiRpcHandle { /* Arc<RpcInner> */ }
impl PiRpcHandle {
    /// Send one command (a JSON object; `PiRpc` assigns a unique `id` when
    /// absent) and await the matching `{"type":"response","command":…,"id":…,"success":…}`
    /// line. `success: false` → `RpcError::Command { error }`. Child death
    /// while in flight → `RpcError::ProcessExited`. NO timeout here —
    /// callers apply it. Returns `response.data` (or `Value::Null` when absent).
    pub async fn send(&self, command: &Value) -> Result<Value, RpcError>;

    /// Stream of session events (everything on stdout that is not a `response`
    /// and not an `extension_ui_request`), in wire order.
    pub fn events(&self) -> tokio::sync::mpsc::Receiver<RpcEvent>;

    /// Stream of `extension_ui_request` frames (the extension-UI subprotocol).
    pub fn extension_ui(&self) -> tokio::sync::mpsc::Receiver<ExtensionUiRequest>;

    /// Send an `extension_ui_response` frame (one of the three shapes below).
    pub fn respond_extension_ui(&self, resp: &ExtensionUiResponse);

    /// Clonable exit watch (resolves when the child exits, any cause).
    pub fn exited(&self) -> tokio::sync::watch::Receiver<Option<i32>>;

    /// Close stdin (pi: `onInputEnd` → `shutdown()` → clean exit), wait up to
    /// 5 s for the exit, then kill. IDEMPOTENT — callable from `close_session`
    /// AND the driver teardown (the `PiRpc` owner is dropped when the spawn
    /// site returns, so the handle is the only reachable close path).
    pub fn close(&self);
}
```

**Wire-format rules (CRITICAL — the research report §F1 is the source of truth):**
- Only the `type`/`method` **discriminators** are snake_case; **payload fields are camelCase** (as in the `.d.ts` files). Do NOT snake_case any field name.
- Deserialization must be **permissive of unknown events**: parse each stdout line as `serde_json::Value` first, route by the `type` field, and deserialize known types into their variants; an unknown `type` becomes `RpcEvent::Unknown { raw: Value }` (log at debug; **never** a `Parse` error — the RPC surface is unversioned and pi will add events). A malformed JSON line (not an unknown type) is `RpcError::Parse` and the reader task fails all in-flight sends.

Type definitions (all in `rpc.rs`; derive `Debug` + serde where noted — **copy every field name from the .d.ts files / research §F1 before writing**):
- `RpcEvent` — `#[serde(tag = "type")]` enum, one variant per event in the §F1 table, **with the exact camelCase field names**:
  - `agent_start`, `agent_end { messages: Vec<Value>, willRetry: bool }`, `agent_settled`, `turn_start` (**`Value` — the wire carries `AgentMessage` objects; no local wire type is needed**)
  - `turn_end { message, toolResults }` — **both required** (no `messageEntryId`/`toolResultEntryIds` in `pi-agent-core/dist/types.d.ts:428-432`)
  - `message_start { message }` / `message_end { message }` — **`AgentMessage` has NO `id` field** (`UserMessage {role, content, timestamp}` / `AssistantMessage {role, content, api, provider, model, usage, stopReason, …}` per `pi-ai/dist/types.d.ts`)
  - `message_update { usage: Usage, assistantMessageEvent: AssistantMessageEvent }` — **`usage` is REQUIRED** (not optional)
  - `text_end` / `thinking_end` — the wire carries `contentIndex` + a `content: string` field (the authoritative text; `json-event.d.ts` strips only the cumulative `partial`), **but the normalizer does NOT re-emit it** (the delta stream already delivered the content)
  - `tool_execution_start { toolCallId, toolName, args }` / `tool_execution_update { toolCallId, toolName, args, partialResult }` (**`args` and `partialResult` are REQUIRED** — `partialResult` is `null`/empty when there is no partial) / `tool_execution_end { toolCallId, toolName, result, isError }`
  - `queue_update { steering: Vec<String>, followUp: Vec<String> }`
  - `entry_appended { entry }` / `session_info_changed { name? }` / `thinking_level_changed { level }`
  - `compaction_start { reason }` / `compaction_end { reason, result?, aborted, willRetry, errorMessage? }`
  - `auto_retry_start { attempt, maxAttempts, delayMs, errorMessage }` / `auto_retry_end { success, attempt, finalError? }`
  - `summarization_retry_scheduled { attempt, maxAttempts, delayMs, errorMessage }` / `summarization_retry_attempt_start { source, reason? }` / `summarization_retry_finished`
  - `bash_execution_update { id?, delta }` / `extension_error { extensionPath, event, error }`
  - `Unknown { raw: Value }` (the catch-all above)
- `AssistantMessageEvent` — the nested `assistantMessageEvent` union from `json-event.d.ts:9-27` (**camelCase**): `start`, `text_start`, `text_delta { contentIndex, delta: String }`, `text_end { contentIndex, content: String }` (the authoritative text; NOT re-emitted by the normalizer — see above), `thinking_start`, `thinking_delta { contentIndex, delta: String }`, `thinking_end { contentIndex, content: String }`, `toolcall_start { contentIndex, id, toolName }` (via `ToJsonAssistantMessageEvent`, `json-event.d.ts:3-14`), `toolcall_delta { contentIndex, delta: String }` (serialized partial args), `toolcall_end { toolCall: { id, name, arguments } }`, `done`, `error { … }`
- `ExtensionUiRequest` — `#[serde(tag = "method")]` enum (all variants carry `id`): `select { title, options: Vec<String>, timeout? }`, `confirm { title, message, timeout? }`, `input { title, placeholder?, timeout? }`, `editor { title, prefill? }`, `notify { message, notifyType? }`, `setStatus { statusKey, statusText? }`, `setWidget { widgetKey, widgetLines?, widgetPlacement? }`, `setTitle { title }`, `set_editor_text { text }` — **note pi's own casing is inconsistent**: `setStatus`/`setWidget`/`setTitle`/`notifyType` are camelCase while `set_editor_text` is snake (verified in `rpc-types.d.ts:357-420`).
- `ExtensionUiResponse` — three shapes, one Rust enum: `Value { id, value: String }` (**`value` is a REQUIRED string** per the .d.ts — `undefined` is never sent; the client sends `Cancelled` instead), `Confirmed { id, confirmed: bool }`, `Cancelled { id }` (serialized as `{"type":"extension_ui_response","id":…,"cancelled":true}`).

`src-tauri/src/bin/fake_pi.rs` — a deterministic test double (no LLM, no sleeps). Behavior:
- Always: read JSONL from stdin; for each command line, reply `{"type":"response","command":<echo of the command's type>,"id":<echo>,"success":true,"data":…}` unless the command's type is `"__fail"` → `{"type":"response","command":"__fail","id":…,"success":false,"error":"boom"}`. Close stdin → exit 0.
- **Mode-independent responses** (always available regardless of env — the establisher always calls `get_state`, and `FAKE_PI_PROMPT`-driven tests assert on its data): `get_state` → `{"sessionId":"fake-pi-1","sessionFile":"/tmp/fake-pi-session.jsonl","model":{"provider":"fake","id":"fake-model","name":"Fake Model","contextWindow":100000,"cost":{}},"thinkingLevel":"off","isStreaming":false,"steeringMode":"all","followUpMode":"all","autoCompactionEnabled":true,"messageCount":0,"pendingMessageCount":0}`; `get_available_models` → `{"models":[{"provider":"fake","id":"fake-model","name":"Fake Model",…},{"provider":"fake","id":"fake-model-2","name":"Fake Model 2",…}]}`; `get_available_thinking_levels` → `{"levels":["off","low","medium"]}`; `get_messages` → `{"messages":[<user message "hello">,<assistant message: text "world">]}` (shapes per `pi-ai/dist/types.d.ts` — **no `id` fields**); `set_model` → `success: true` + `model` = **the REQUESTED `provider`/`modelId` echoed in a `Model` object** (so `set_config_option_roundtrip` can assert `currentValue` == the value it sent — NOT a static model); `set_thinking_level` → `{"success":true}`.
- `FAKE_PI_PROMPT=1`: on `prompt`, emit in order: a **user** `message_start` (`message.role === "user"` — the wire's `message_start` carries any `AgentMessage`, so the first one is the user message, mirroring the real stream) → an **assistant** `message_start` (a message object, NO id) → `message_update` ×2 (`assistantMessageEvent` = `text_delta` with `delta` "Hel" then "lo", each with a `usage` object) → `message_end` (message with the full text "Hello") → `agent_end` (`willRetry:false`) → `agent_settled`; on `abort`, emit `agent_settled`. (The user-first ordering is what pins the Task 3 `messageId` role rule — see there.)
- `FAKE_PI_HANG=1` (positional-arg mode, like `fake_agent`'s hang mode): never respond to ANY command (notably `get_state`) — for the rewritten `establishment_times_out_when_the_agent_hangs` test.
- `FAKE_PI_GATE=1`: on `prompt`, first emit `{"type":"extension_ui_request","id":"gate-1","method":"confirm","title":"Allow bash?","message":"ls -la"}`; after the matching `extension_ui_response` arrives, emit the same `prompt` sequence as `FAKE_PI_PROMPT=1`.
- All emitted events use the **camelCase** field names above (so the round-trip tests at least prove serde self-consistency — **caveat: `fake_pi` is written by the same executor from the same table, so these tests CANNOT catch a systematic casing error; the .d.ts files are the authority, and Task 6's real-`pi` smoke test is the true wire check**).

**Test pattern (CRITICAL — the existing codebase documents the constraint):** unit tests inside `src/` CANNOT use `env!("CARGO_BIN_EXE_fake_pi")` — it is only set for integration tests (see the explicit note at `src/agent/subagent.rs:625-628`). Unit tests construct the path `concat!(env!("CARGO_MANIFEST_DIR"), "/target/debug/fake_pi")` and — mirroring `unique_fake_agent` (`session.rs:1745`) — **copy the binary to a per-test unique path** before spawning (the copy is load-bearing: `tests/acp_flow.rs:87`'s `find_fake_agent_pid` asserts process reaping by matching the binary path; a shared path false-positives across concurrent tests). Integration tests in `tests/` MAY use `env!("CARGO_BIN_EXE_fake_pi")`.

**Steps:**
- [ ] Write the failing tests in `rpc.rs` `#[cfg(test)]` (spawning `fake_pi` per the pattern above):
      - `spawn_get_state_roundtrip`: spawn → `send(get_state)` → assert `sessionId == "fake-pi-1"` and `sessionFile == "/tmp/fake-pi-session.jsonl"`.
      - `prompt_event_stream` (`FAKE_PI_PROMPT=1`): `send(prompt)` → collect `events()` until `agent_settled` → assert the exact sequence (user `message_start`, assistant `message_start`, 2× `message_update` with the two `delta` strings and a `usage` field, `message_end`, `agent_end`, `agent_settled`) and the `assistantMessageEvent` sub-shapes (`text_delta` with `contentIndex` + `delta`).
      - `extension_ui_roundtrip` (`FAKE_PI_GATE=1`): `send(prompt)` → read `extension_ui()` (assert `confirm` with `id "gate-1"`) → `respond_extension_ui(Confirmed { confirmed: true })` → collect to `agent_settled` (assert it arrives).
      - `failed_command`: `send({"type":"__fail"})` → assert `RpcError::Command { error: "boom" }`.
      - `process_exit_fails_inflight`: spawn, `handle.close()`, then `send(get_state)` → assert `RpcError::ProcessExited`.
      - `concurrent_commands`: two in-flight `send`s resolve independently (correlation by `id`, not order).
      - `unknown_event_is_not_an_error`: (extend `fake_pi` with a `FAKE_PI_UNKNOWN=1` mode that emits `{"type":"brand_new_event","foo":1}` before settling) → assert `RpcEvent::Unknown` is received and the stream continues.
- [ ] Run `cargo test agent::rpc` — confirm it fails (module doesn't exist / tests fail).
- [ ] Implement `fake_pi.rs`, then `rpc.rs` until the tests pass.
- [ ] Run `cargo test` (full) — all pass. `cargo clippy --all-targets` — 0 warnings. `cargo fmt` — clean.
- [ ] Commit with message: "feat(agent): add PiRpc JSONL client + fake_pi test double"

**Acceptance criteria:**
- [ ] All seven `rpc.rs` tests pass; `fake_pi` is deterministic (no sleeps, no LLM).
- [ ] `RpcError` derives `Serialize` (crosses Tauri IPC) and `PartialEq`.
- [ ] `AcpError` still compiles and all pre-existing tests still pass.

---

### Task 3: The session driver speaks RPC

**Context:** The heart of the swap. `SessionDriver` (currently `agent/session.rs`) keeps its generic machinery — the `sessions`/`pending_*` maps, the `EventSink` re-emission, `persist_update` persistence, the `WorkerRuntime` handoff, the establish timeout — but its protocol layer becomes `PiRpc`. The establisher becomes `get_state` (new) / `get_messages` (resume); `send_prompt` resolves on `agent_settled`; the `session-update` JSON the frontend consumes is FROZEN (the normalizer below maps pi events onto the exact current ACP-shaped envelopes). The default registry entry flips to `pi --mode rpc` so the app runs the new path end-to-end. **This task also rewrites the Tauri command bodies** (the ACP crate types they use die with the swap) and moves the image/payload helpers out of `prompt.rs` (which dies in **THIS** task — it uses `AcpError`, which this task deletes).

**Files:**
- Modify: `src-tauri/src/agent/session.rs` (the driver rewrite — the largest task)
- Modify: `src-tauri/src/agent/errors.rs` (delete `AcpError`; `RpcError` becomes the only error type)
- Modify: `src-tauri/src/agent/mod.rs` (re-exports: drop `AcpError`, `FsBackend`, `ImagePayload`; add `RpcError`; `ImagePayload` now lives in `session.rs`)
- Modify: `src-tauri/src/agent/subagent.rs` (compile fixes for the `AcpError` → `RpcError` rename **AND the `SessionInfo` field-type changes** — the `SessionInfo` constructors at `subagent.rs:300/853/984` and the `Result<SessionInfo, AcpError>` generic at `:1031-1034` are hit by item 1's shape change; its behavior rewrite is Task 5. **The unit tests adapt for the `SessionInfo`/`StopReason`/`RpcError` type changes — including a REWRITE of the two ACP-driven tests — `text_capture_and_last_message_id_track_distinct_messages` (:790) and `external_close_tears_down_during_establish` (:921) — against the new `drive_session` signature (fake_pi-based establisher); do NOT delete them** (the external-close-during-establish + text-capture coverage has no other home); the dispatch behavior coverage lives in the two `tests/` files below)
- Modify: `src-tauri/src/commands/sessions.rs` — **real rewrites, not renames** (see items 6-8 below: `send_prompt`/`send_prompt_with_images` delegate to the manager; `resume_session` pre-check replaced; `set_session_config_option` return type; `respond_permission`/`respond_bridge_request` unchanged)
- Modify: `src-tauri/src/commands/history.rs` (builds `SessionInfo` from `AgentCapabilities`/`SessionId` crate types — item 7)
- Modify: `src-tauri/src/commands/clipboard.rs` (`use crate::acp::prompt::MAX_IMAGE_BYTES` → the new home in `session.rs`, item 9)
- Modify: `src-tauri/src/config/registry.rs` (default `pi` entry: `command: "pi"`, `args: ["--mode", "rpc", "--no-themes"]`, `env: {}`, `bridge: true`)
- Modify: `src-tauri/src/test_support.rs` (`run_with_retry`'s `AcpError` → `RpcError` mapping — see item 13)
- Delete: `src-tauri/src/agent/prompt.rs` (the WHOLE file — the `ImagePayload`/`MAX_IMAGE_BYTES`/validation/`user_message_payload` helpers move to `session.rs` per item 5; `build_prompt_blocks` dies, replaced by item 5's prompt construction; the TEST MODULE at `prompt.rs:137-391` constructs `SessionInfo` with `AgentCapabilities` — move the still-valid tests to `session.rs` alongside the moved helpers, delete the superseded ones)
- Modify: `src-tauri/src/agent/fs_backend.rs` (**STAYS** — `permission.rs`'s read-only pre-approval check consumes it; but its error type `AcpError` dies in this task → a new local `FsError { Io(String), PathEscape(String) }` defined in `fs_backend.rs` (same derives as `AcpError`); no behavior change)
- Modify: `src-tauri/tests/fs_backend.rs` (imports adapt: `AcpError` → `FsError`; no behavior change)
- Modify: `src-tauri/tests/storage.rs` (constructs `SessionInfo { session_id: SessionId::new(…), capabilities: AgentCapabilities::default() }` at :10-21, :210-211 — item 1's shape change hits it: `session_id: String`, `capabilities: Value` with the item-1 JSON shape)
- Modify: `src-tauri/tests/ipc.rs` (spawns `CARGO_BIN_EXE_fake_agent` as the agent — after Task 3's RPC driver it can't answer `get_state`; adapt it to `fake_pi` (per-test-copy pattern), or delete it with a rationale in the commit message if its assertions are fully covered by `tests/rpc_flow.rs`)
- Modify: `src-tauri/tests/acp_flow.rs` (git mv → `src-tauri/tests/rpc_flow.rs`; its ACP flow assertions are rewritten against `fake_pi` — the `ImagePayload` usage moves with it; the `unique_fake_agent`/`write_agents_json_cmd` helpers duplicated in this file at :98-125 become `fake_pi` variants; the `establishment_times_out_when_the_agent_hangs` test at :369-370 is rewritten using `fake_pi`'s `FAKE_PI_HANG=1` mode — see item 13)
- Test: the integration tests in `session.rs` `#[cfg(test)]` + the rewritten `tests/rpc_flow.rs`

**What to implement:**

1. **`SessionInfo` shape change** (`session.rs`): `session_id: SessionId` → **`String`**; `capabilities: AgentCapabilities` → **`serde_json::Value`**; `config_options: Vec<SessionConfigOption>` → **`Option<Vec<serde_json::Value>>`**. **Consequential type changes (state explicitly — the plan's own grep criterion implies them):** `LiveSession.cx: ConnectionTo<Agent>` → `LiveSession.handle: PiRpcHandle`, `LiveSession.session_id: SessionId` → `String`, and `sessions: HashMap<SessionId, LiveSession>` → `HashMap<String, LiveSession>` (all lookups become `&str`-keyed). The `capabilities` value for the `pi` entry is:
```json
{
  "piSessionId": "<get_state().sessionId>",
  "piSessionFile": "<get_state().sessionFile>",          // ABSENT when get_state omits it
  "model": "<provider/modelId>",                            // ABSENT when get_state().model is absent (it is OPTIONAL in RpcSessionState)
  "thinkingLevel": "<level>",
  "loadSession": "<sessionFile was present>",          // bool — the frontend's Resume-button gate (ChatStream.tsx:318)
  "promptCapabilities": { "image": true, "audio": false, "embeddedContext": false }  // the frontend's image-send gate (chatAttachments.ts:130-140, fail-closed)
}
```
   **The two trailing keys are LOAD-BEARING, not decoration**: `ChatStream.tsx:318` gates the history Resume button on `capabilities.loadSession === true` and `chatAttachments.ts:130-140` fail-closes image sending on `capabilities.promptCapabilities.image === true`. Omitting them silently kills resume + images with no build warning (the TS type is already `Record<string, unknown>`). `record_session` stores the value in the existing `capabilities_json` column (no schema migration). **`sessionFile` is OPTIONAL in `get_state`** (`rpc-types.d.ts`) — absent (e.g. `--no-session` runs) → store the value without `piSessionFile` and with `loadSession: false` (the session is unresumable; same treatment as legacy rows, item 6).

2. **`drive_session` signature** (the one structural change to the machinery — note the exact param set and bounds; the current 7th param is `external_close: Option<ExternalClose>` — the **subagent cancel path** — and `SubagentSpawn` is a DIFFERENT thing, built INSIDE `drive_session` from `self.subagent` + cwd + agent_id at `session.rs:357-362` and handed to `bridge::start_listener` at :371-390):
```rust
pub async fn drive_session<Establisher, EstablisherFut>(
    &self,
    handle: PiRpcHandle,                       // cheap clone (replaces `agent: AcpAgent` / `cx: ConnectionTo<Agent>`)
    agent_id: &str,
    hint: String,
    cwd: PathBuf,
    sink: &Arc<dyn EventSink>,                // &Arc (NOT &dyn) — the driver task and bridge::start_listener move owned Arc clones into 'static closures (session.rs:404/405/423)
    bridge_setup: Option<(String, PathBuf)>,
    external_close: Option<ExternalClose>,    // UNCHANGED — the subagent cancel path
    establisher: Establisher,
) -> Result<SessionInfo, RpcError>
where
    Establisher: FnOnce(PiRpcHandle) -> EstablisherFut + Send + 'static,
    EstablisherFut: std::future::Future<Output = Result<SessionInfo, RpcError>> + Send + 'static;
```
   (**`Send + 'static` on BOTH bounds is required** — the establisher runs inside the `tokio::spawn`'d driver task; the current bounds at `session.rs:299-317` have them on the ACP versions. The establisher takes an **owned cheap `PiRpcHandle` clone** — the same reason the current pattern passes an owned `ConnectionTo<Agent>`; a `'static` generic `Future` bound works without the `futures` crate.) **Delete `SessionManager::connection()`** (session.rs:796) — it has TWO callers: `commands/sessions.rs::send_prompt` (rewritten in item 6) and `cancel_session` (`session.rs:1113` — rewritten in item 5 to send the `abort` command via the handle).
   The driver task: `select!` over `handle.events()`, `handle.extension_ui()`, `handle.exited()`, and the existing cancel/external-close handles (unchanged). Child exit → the existing `session-closed` (`reason: agent-exited`) teardown path, unchanged. **After the establisher resolves, call `bridge_handle.set_session_id(pi_session_id)` exactly as `session.rs:585-588` does today** — the id now comes from `get_state().sessionId`; the bridge-request payloads carry it and the frontend routes `respond_bridge_request` by it (tauri.ts:196-201). Dropping this one-liner breaks `respond_bridge_request` routing. **The `text_capture`/`last_message_id` capture hooks (currently fed inside the `on_receive_notification` handler, `session.rs:~450-490`, from ACP chunks) are re-fed by the driver from the normalized `agent_message_chunk` updates keyed by the derived `current_message_id`** — Task 5's `captures()` depends on this feed existing.

3. **Establishers** (inline in `start_session`/`resume_session`, replacing the ACP closures at `session.rs:895-933` and `:975-1043`):
   - **New session:** `let state = handle.send(json!({"type":"get_state"})).await?;` → build `SessionInfo` (per item 1) + `config_options` via the synthesizer (item 7).
   - **Resume — CRITICAL: the spawn must LOAD the pi session first.** `get_messages` returns the *current* session's messages, and a freshly spawned `pi --mode rpc` starts an EMPTY session — so a resume that only spawns + `get_messages` would (after `clear_messages_for`) wipe the stored transcript and repopulate nothing. The load mechanism is the CLI flag: **the resume spawn's args gain `--session <stored piSessionFile>`** (verified: `--session <path|id>` is a real flag, `dist/cli/args.js:89`; the desktop reads `piSessionFile` from the stored `capabilities_json` — it already parses it for the `NotResumable` check, item 6). Resume sequence: (a) `db.clear_messages_for(sid)` FIRST (same as today — the replay reuses known `messageId`s and must overwrite, not clobber); (b) spawn `pi --mode rpc --no-themes --session <file>`; (c) `get_state` (now reflects the loaded session — `SessionInfo` per item 1) + `get_messages` (now returns the loaded session's messages) → **persist each message**: user messages as `kind: "user"` rows with the `{"text": …, "images": […]?}` payload shape that `rowToMessages` (`sessions.ts:278-296`) reads — **an explicit improvement over the current ACP replay, which dropped user rows** (today user rows are written only at `send_prompt` time; `tests/acp_flow.rs::resume_replaces_stored_transcript` pinned `rows.len() == 1` after a replay that had 2 rows — the new test asserts 2+); assistant/toolResult messages through the normalizer per the **replay-feed rule below** (item 4) with the `messageId` derivation below (thinking blocks included **if** `get_messages` returns them — research gap G2; if it doesn't, that is accepted parity since the current ACP replay is text-only too); tool results as `tool_call` rows with `rawInput: null` (accepted parity — the current adapter's load also loses tool inputs); (d) `get_available_models` + `get_available_thinking_levels` → `config_options`; (e) `SessionInfo` with `loadSession` per item 1. **`fake_pi` models this**: it ignores `--session` (its mode-independent `get_state`/`get_messages` responses stand in for the loaded session); the real flag is verified by Task 6's manual smoke.
   - **`messageId` derivation (NEW — `AgentMessage` has NO `id` field)**: synthesize a per-session id — `TurnState.msg_counter: u64` starting at 1, **advanced ONLY when `message_start.message.role === "assistant"`** (`message_start` carries `AgentMessage`, which includes user and tool-result messages — `pi-agent-core/dist/types.d.ts:434-436` — so a role-blind counter would number the first assistant chunk `m2` and make the replay numbering irreproducible); `current_message_id = format!("m{}", counter)`. The `get_messages` replay uses the SAME rule in message order (iterate all messages; advance the counter only for `role === "assistant"`) so `resumeSession`'s dedupe keys (`sessions.ts:225-250`) match the live-session keys. **`fake_pi`'s `FAKE_PI_PROMPT` mode emits a user `message_start` FIRST, then the assistant `message_start`** — pinning the rule (the assistant chunks must still key `m1` in the `start_session_streams_and_persists` test).
   - **How the replay feeds the normalizer (the no-re-emit rule is LIVE-STREAM-only — a literal replay that fed stored messages as `message_end` would produce ZERO frames and `resume_replays_messages` would fail with 1 row):** per stored message the replay synthesizes the delta events the live stream would have produced: assistant message → an assistant `message_start` (advance the counter per the rule) + one `message_update` carrying a `text_delta` with the full block text as a single `delta` per text block + `thinking_delta` per thinking block + `toolcall_start` + `toolcall_end` (arguments = the block's `arguments` object) per toolCall; toolResult message → `tool_execution_end` (`toolCallId` from the preceding assistant message's matching toolCall, `result` = the toolResult content, `isError` from the message); user message → persisted DIRECTLY (bypasses the normalizer — the normalizer has no user-message input). The replay never feeds `message_end`/`text_end`/`thinking_end`.

4. **The event normalizer** — a pure function `fn normalize(e: &RpcEvent, st: &mut TurnState) -> Vec<Update>` where `Update` is the EXACT JSON the frontend consumes today (copy the shapes from the current `session.rs:433-486` + `persist_update` at `:1273` — the frozen contract). `TurnState` (per session): `msg_counter: u64`, `current_message_id: Option<String>`, `toolcall_args: HashMap<String, String>` (the accumulating partial-args buffer, cleared on `toolcall_end`).

| pi event | emitted `session-update` payload (frozen shape) |
|---|---|
| `message_start` | (no frame) — `msg_counter += 1`; `current_message_id = format!("m{}", msg_counter)` |
| `message_update` + `text_delta` | `{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":<delta>},"messageId":<current_message_id>}` |
| `message_update` + `text_end` / `thinking_end` | (no frame — the delta stream already delivered the content; the authoritative text in `message_end.message` is NOT re-emitted) |
| `message_update` + `thinking_delta` | `{"sessionUpdate":"agent_thought_chunk","content":{"type":"text","text":<delta>},"messageId":<current_message_id>}` |
| `message_update` + `toolcall_start` | `{"sessionUpdate":"tool_call","toolCallId":<id>,"title":<toolName>,"status":"in_progress","rawInput":{}}` |
| `message_update` + `toolcall_delta` | `{"sessionUpdate":"tool_call_update","toolCallId":<id>,"rawInput":<accumulated-partial-args>}` — accumulate the `delta` strings in `TurnState.toolcall_args`, `serde_json::from_str` each time; on parse failure send `{"partialArgs":<string>}` (mirrors the adapter's behavior, `pi-acp:967-1016`) |
| `message_update` + `toolcall_end` | `{"sessionUpdate":"tool_call_update","toolCallId":<id>,"rawInput":<full arguments>,"status":"in_progress"}` (clear the args buffer for this id) |
| `tool_execution_start` | `{"sessionUpdate":"tool_call_update","toolCallId":<id>,"status":"in_progress"}` |
| `tool_execution_update` | `{"sessionUpdate":"tool_call_update","toolCallId":<id>,"rawOutput":<partialResult>}` (**persisted-only, not consumed by the frontend** — `AcpSessionUpdate` in `tauri.ts:128-155` has no `rawOutput`; it lands in the persisted payload via `merge_json` and is harmless) |
| `tool_execution_end` | `{"sessionUpdate":"tool_call_update","toolCallId":<id>,"status":<completed if !isError else failed>,"rawOutput":<result>}` |
| `session_info_changed` | `{"sessionUpdate":"session_info_update","title":<name or null>}` |
| `thinking_level_changed` | `{"sessionUpdate":"config_option_update","configOptions":<re-synthesized with the new level>}` |
| `compaction_start` / `compaction_end` / `auto_retry_start` / `auto_retry_end` | `agent_message_chunk` text (human-readable one-liners, e.g. `"Compacting context…"`, `"Compaction finished"`, `"Retrying (attempt n/m…)"`, `"Retry succeeded"` — the adapter's de-structured parity; keep the strings stable for tests) |
| `agent_start` / `agent_end` / `turn_start` / `turn_end` / `queue_update` / `entry_appended` / `bash_execution_update` | (no frame — bookkeeping; `agent_end.willRetry` is ignored in Phase 1) |
| `extension_error` | `agent_message_chunk` text: `"Extension error: <error>"` |
| `Unknown` | (no frame — debug-log) |
| `agent_settled` | (no frame) — resolves the pending turn (item 5) |

   **Tool diffs: accepted Phase-1 parity loss.** The frontend's `content: ToolCallContent[]` diff channel (`extractDiffs`/`diffMessages` in the reducer, `has_diff` in `persist_update:1357-1365`) has NO mapping: pi's tool results don't carry diff-structured content (the adapter computed diffs client-side by re-reading files; the desktop-side tool executor that would produce real diffs is Phase 3). Document in the ADR (Task 6). The "byte-identical" acceptance criterion below applies to the **consumed** fields.

   **`persist_update` is REWRITTEN in this task** (its current signature takes `&SessionUpdate` — the ACP crate enum — and reads `chunk.message_id`, `tool_call.tool_call_id`, `tool_call_update.fields.content`, `ToolCallContent::Diff` (`has_diff`, :1357-1365): all crate types that die). The rewrite consumes the JSON `Update` shapes emitted by the normalizer; its **persistence semantics stay unchanged** — upsert keys (`(session_id, kind, message_key)`), agent-text accumulation per `messageId`, thought segmentation (`{messageId}#{segment}` boundaries), tool-call `merge_json` shallow-merge. The `has_diff`/`content` branch has no RPC input in Phase 1 (item 4's parity-loss note) and is dropped with the crate types.

5. **`send_prompt` / `send_prompt_with_images`** (replacing `session.rs:1050-1110`): build `{"type":"prompt","message":<text>,"images":<…>}` — the existing `ImagePayload` (currently `agent/prompt.rs`) maps to pi's `ImageContent` = `{"type":"image","data":<base64>,"mimeType":<mime>}` (verified in `pi-ai/dist/types.d.ts`); **move the `ImagePayload` struct + `MAX_IMAGE_BYTES` + the image validation + `user_message_payload` (the ADR-0008 transcript payload `{"text","images":[…]}`) into `session.rs` in THIS task** (their consumers — `commands/sessions.rs` and `commands/clipboard.rs` — are updated to the new import in this task; `prompt.rs` then has no remaining users and dies in **THIS** task — see the Files entry). If `get_state().isStreaming` is true → add `"streamingBehavior":"steer"`. **User-row persistence lives SOLELY in the manager method** (it persists the `user_message_payload` row, as `session.rs:1091` does today — the command does NOT also call `record_message` for the user row, or the row is written twice: `db.rs:164-169`'s `ON CONFLICT` never fires for user rows' `message_key: NULL` and `rowToMessages` would show a duplicate user bubble). Then: store a `pending_turn: oneshot::Sender<StopReason>` for the session (replacing the ACP "await the prompt response" pattern — `send_prompt` awaits the oneshot, unbounded, same as today's unbounded ACP await). The driver resolves it: `agent_settled` → `EndTurn`; a `response success:false` for the prompt → `Refusal` (and emit one `agent_message_chunk` with the error text first); `cancel_session` → `Cancelled`. **`cancel_session` (replacing `session.rs:1112-1127`) sends `{"type":"abort"}` over the handle** (the `cancel_requested` flag semantics are kept: a late `agent_settled` after an abort maps to `Cancelled`, not `EndTurn`). **`StopReason` is a NEW local enum in `session.rs`** (the crate type dies): `#[derive(Clone, Debug, PartialEq, serde::Serialize)] #[serde(rename_all = "snake_case")] pub enum StopReason { EndTurn, MaxTokens, Refusal, MaxTurnRequests, Cancelled }` — serializes to exactly the strings the frontend expects (`tauri.ts:76-82`).

6. **`commands/sessions.rs` rewrites** (the ACP crate types in signatures die; the frontend's command **names** and the `respond_permission`/`respond_bridge_request` bodies stay unchanged):
   - `send_prompt` + `send_prompt_with_images`: **replace the ACP flow** (currently `prompt::build_prompt_blocks` + `PromptRequest::new` + `state.connection()` → `ConnectionTo<Agent>` + `cx.send_request(…).block_task()` + `response.stop_reason`) with a delegation to the manager: `state.send_prompt_with_images(session_id, text, images).await` (the manager method at `session.rs:1063` now owns the prompt-send + user-row persistence + `pending_turn` oneshot + `StopReason` mapping per item 5 — **the command adds NO persistence of its own**).
   - `resume_session`: **replace the pre-check** (currently `serde_json::from_str::<AgentCapabilities>(&row.capabilities_json)` → `NotResumable` when `!caps.load_session` — which would reject EVERY row after item 1's storage change) with: normalize the stored capabilities (item 6b) and return `RpcError::NotResumable` only when `loadSession` is not `true`.
   - **(6b) Legacy-row normalization (NEW helper, used by the `list_sessions` path — NOT `load_history`, which returns raw `MessageRow`s and never touches `capabilities_json`):** a stored `capabilities_json` that (a) fails to parse as JSON or (b) parses but lacks the `loadSession` key (pre-swap ACP rows) is normalized to `loadSession: false` (and `promptCapabilities` absent) before reaching the frontend — so legacy rows show **no Resume button** (the history-only banner is a FRONTEND-side decision driven by `capabilities.loadSession`, `ChatStream.tsx:314-322`; there is NO error-kind matching in the frontend, so an error toast is not the honest path — the normalized capabilities ARE the honest path).
   - `set_session_config_option`: return type `Vec<SessionConfigOption>` (crate) → `Vec<serde_json::Value>` (the manager's synthesizer output, item 7).
   - `respond_permission` / `respond_bridge_request`: **unchanged** (verified: `PermissionPrompt.tsx:55-61` reads only `optionId`/`name`; `PermissionOutcome` is crate-free serde `{selected:{option_id}}|"cancelled"`; the main→subagent routing is unchanged).

7. **The config synthesizer** (in `session.rs`): `fn synthesize_config_options(state, models, levels) -> Option<Vec<serde_json::Value>>`:
```json
[
  {"id":"model","name":"Model","category":"model","type":"select","currentValue":"<provider/modelId>","options":[{"value":"<provider/modelId>","name":"<model name>"}…]},
  {"id":"thought_level","name":"Thinking","category":"thought_level","type":"select","currentValue":"<level>","options":[{"value":"<level>","name":"<Level>"}…]}
]
```
   (Field names mirror the frontend's `SessionConfigOption` type, `tauri.ts:35-45`; the frontend also supports GROUPED options (`SessionConfigSelectGroup`) but the synthesizer emits flat lists only — parity with what the agent advertises today.) `set_config_option` (manager, replacing `session.rs:1129-1164`): config id `"model"` → parse the value as `"<provider>/<modelId>"` → `{"type":"set_model","provider":…,"modelId":…}`; id `"thought_level"` → `{"type":"set_thinking_level","level":…}`. On the response, re-synthesize and emit `config_option_update`.

8. **`commands/history.rs`**: builds `SessionInfo` from `AgentCapabilities`/`SessionId` crate types (history.rs:21-32) → parse `capabilities_json` as `serde_json::Value` (with the item 6b normalization) + `session_id: String`.

9. **`commands/clipboard.rs`**: `use crate::acp::prompt::MAX_IMAGE_BYTES` (line 12) → `use crate::agent::session::MAX_IMAGE_BYTES` (moved in item 5).

10. **`close_session`** (replacing `session.rs:1221-…`): `handle` → `handle.close()` (idempotent — the driver teardown may also call it; stdin close → clean pi shutdown) + the existing teardown (bridge listener teardown + pending drains + `session-closed` event) unchanged.

11. **`respond_permission` wiring** (replacing the ACP handler at `session.rs:520-530` + `permission.rs`): the `pending_permissions` machinery (oneshot keyed `"{session_id}/{request_id}"`, 300 s waiter, `PermissionOutcome`) is KEPT verbatim (verified: trailing-slash prefix drain, `PERMISSION_TIMEOUT`, serde shape — all as the existing code shows); its input source changes from ACP `session/request_permission` to `extension_ui_request` frames:
    - `confirm` frame → emit `permission-request` Tauri event with the EXACT current shape, synthesized: `{"sessionId":…,"requestId":<frame id>,"request":{"sessionId":…,"toolCall":{"toolCallId":<frame id>,"title":<confirm title>},"options":[{"optionId":"allow","name":"Allow"},{"optionId":"reject","name":"Block"}]}}` (the `request` sub-object mirrors the frontend's `PermissionRequest` type, `tauri.ts:96-104` — the authoritative consumed contract).
    - `select` frame → same with `options` = `[{"optionId":<label>,"name":<label>}…]` (the `optionId` = the label string itself, so the response round-trips trivially).
    - `input`/`editor` frames → **answered `{"type":"extension_ui_response","id":…,"cancelled":true}` immediately** (the gate extension only uses `confirm` in Phase 1; document in a comment).
    - `notify`/`setStatus`/`setWidget`/`setTitle`/`set_editor_text` frames → ignore (no UI frame in Phase 1).
    - `respond_permission` (existing Tauri command, UNCHANGED signature): `Selected { option_id }` → for a `confirm` frame: `ExtensionUiResponse::Confirmed { confirmed: option_id == "allow" }`; for a `select` frame: `ExtensionUiResponse::Value { value: option_id }`; `Cancelled` → `ExtensionUiResponse::Cancelled { id }`. The 300 s waiter expiry → `Cancelled` (the extension's own 300 s `timeout` auto-resolves to `false` ≈ the same moment; a late duplicate response is dropped by pi — safe).
    - Delete `permission.rs`'s ACP-specific code (the `RequestPermissionRequest` handling) but KEEP the `PendingPermissions` map type + `permission_key` + `PermissionOutcome` (re-exported by `mod.rs`, used by `commands/sessions.rs` unchanged).

12. **`spawn_hint`** (session.rs:1397-1403): the user-facing text currently says "make sure `pi` and the `pi-acp` adapter are installed… `npm install -g pi-acp`" → rewrite to "make sure `pi` is installed (`npm install -g @earendil-works/pi-coding-agent`)".

13. **`test_support.rs`**: `run_with_retry`'s `AcpError` → `RpcError` mapping: `AcpError::SpawnFailed { hint }` → `RpcError::Spawn(hint)` (the real variant is `SpawnFailed { hint }`, NOT `Spawn`); `test_support.rs:32` also constructs `AcpError::Protocol` → map to `RpcError::Parse`. **`AcpError::InitializeFailed` → `RpcError::EstablishTimeout { detail }`** — this mapping covers `map_establish_error` (`session.rs:1252-1268`: timeout marker → `EstablishTimeout`), the no-delivery fallback (`session.rs:665-676` → `EstablishTimeout`), and `subagent.rs:1014` (matches `InitializeFailed` → `EstablishTimeout`). The `establishment_times_out_when_the_agent_hangs` test (`tests/acp_flow.rs:369-370`) is rewritten in `tests/rpc_flow.rs` using `fake_pi`'s `FAKE_PI_HANG=1` mode (Task 2). The `unique_fake_agent`/`write_agents_json_cmd` helpers live in `session.rs`'s test module (NOT `test_support.rs` — that file is 35 lines and holds only `run_with_retry`) and are **duplicated** in `tests/acp_flow.rs:98-125`: update BOTH copies to `fake_pi` variants (a `fake_pi`-pointing agents.json entry with the mode env vars; a "resume" mode = the mode-independent `get_state`/`get_messages` responses).

**Steps:**
- [ ] Write the failing integration tests in `session.rs` `#[cfg(test)]` (spawning `fake_pi` via the per-test-copy pattern from Task 2; mirror the existing ACP test structure at `session.rs:1739+`):
      - `start_session_streams_and_persists` (`FAKE_PI_PROMPT=1`): `start_session` → `send_prompt("hi")` → assert: the `session-update` event sequence (2× `agent_message_chunk` with `messageId "m1"`, then the turn resolves `EndTurn`), the `messages` table has ONE `agent-text` row with payload `"Hello"` (accumulated), and the `sessions` row's `capabilities_json` carries `piSessionFile` **AND `loadSession: true` AND `promptCapabilities.image: true`** (the load-bearing keys, item 1).
      - `resume_replays_messages` (pre-seed a `sessions` row with `capabilities_json` = the item-1 shape with `piSessionFile: "/tmp/fake-pi-session.jsonl"`): `resume_session` → assert the stored rows were cleared then re-populated from `get_messages` — **2+ rows, including a `kind: "user"` row with the `{"text": "hello"}` payload** (item 3's explicit improvement over the current drop) — and a legacy row WITHOUT `loadSession` in `capabilities_json` → `list_sessions`-normalized capabilities have `loadSession: false` (item 6b) and `resume_session` → `RpcError::NotResumable`.
      - `cancel_resolves_cancelled` (`FAKE_PI_PROMPT=1`): `send_prompt` → `cancel_session` → assert `send_prompt` resolves `Cancelled`.
      - `set_config_option_roundtrip`: `set_config_option("model", "fake/fake-model-2")` → assert a `config_option_update` event whose `currentValue` is `"fake/fake-model-2"`.
      - `permission_gate_roundtrip` (`FAKE_PI_GATE=1`): `send_prompt` → assert a `permission-request` event with options `[Allow, Block]` → `respond_permission(Selected("allow"))` → assert the turn still resolves `EndTurn` (fake_pi received `confirmed: true` — it only settles after the response).
- [ ] Run `cargo test agent::session` — confirm failure.
- [ ] Implement the driver rewrite (items 1-13) until the tests pass.
- [ ] Apply the `commands/*` rewrites (items 6-9) + `test_support.rs` + the `tests/acp_flow.rs` → `tests/rpc_flow.rs` rewrite (git mv + replace the ACP assertions with the `fake_pi` equivalents, keeping the process-reaping assertions' per-test-copy pattern).
- [ ] Fix all remaining `AcpError` references (`errors.rs`, `mod.rs`, `subagent.rs` compile-only) — `cargo build` clean.
- [ ] Run `cargo test` (full) — all pass (the old ACP tests that exercised `fake_agent` are REPLACED by the new `fake_pi` tests; delete the superseded ACP tests in this task).
- [ ] `cargo clippy --all-targets` — 0 warnings. `cargo fmt` — clean.
- [ ] Commit with message: "feat(agent): drive sessions over pi RPC (get_state establisher, event normalizer, settled-turn resolution, extension-UI permission gate)"

**Acceptance criteria:**
- [ ] All five new integration tests pass; the old ACP-specific tests are gone.
- [ ] `grep -rn "AcpError\|agent_client_protocol\|AcpAgent\|ConnectionTo" src-tauri/src/agent/session.rs src-tauri/src/agent/errors.rs src-tauri/src/commands/ src-tauri/src/lib.rs` returns nothing (the crate dep itself dies in Task 6).
- [ ] The `session-update` JSON emitted by the normalizer is identical in the **consumed** fields to the current ACP envelopes (the frozen contract — the frontend needs no changes; `rawOutput` is persisted-only).
- [ ] `pnpm build` + `pnpm test` (repo root) still green (the ONLY permitted frontend change is a type widening if the build breaks — and the item-1 semantic keys are asserted in Rust, not the frontend).

---

### Task 4: The bundled gate extension + spawn injection

**Context:** Pi RPC has no permission protocol; the de-facto permission model for RPC clients is an in-process extension whose `tool_call` hook calls `ctx.ui.confirm` → pi emits `extension_ui_request` → the client (the desktop) renders a prompt and answers `extension_ui_response`. This task ships that extension (a single small TS file, embedded in the Rust binary, written to disk at startup) and injects it into every agent spawn. The extension is self-gated on an env var so it is inert outside desktop-spawned sessions (the desktop is the only thing that sets the var). Design decision (Phase 1): gate the mutating/privileged tools — `bash`, `edit`, `write`, `sudo_exec` (the ADR 0003 double-prompt: gate confirm → bridge confirm modal → bridge password modal, for `sudo_exec`; verified: registered tools like `sudo_exec` DO fire `tool_call` events — `CustomToolCallEvent`); read-only tools (`read`/`find`/`grep`/`ls`) and the suite's own `ask`/`subagent`/`manage_todo_list` are ungated (they have their own UI or are harmless).

**Files:**
- Create: `src-tauri/assets/gate.ts` (the extension source; embedded via `include_str!`)
- Create: `src-tauri/src/agent/gate.rs` (`install_gate_extension` + the spawn-arg/env helpers)
- Modify: `src-tauri/src/agent/session.rs` (the main-session spawn site: append the gate args/env)
- Modify: `src-tauri/src/agent/subagent.rs` (the subagent spawn site: same injection — Task 5's rewrite consumes it; wire it now so the injection is complete for both spawn paths)
- Modify: `src-tauri/src/agent/mod.rs` (`pub mod gate;`)
- Test: `src-tauri/src/agent/gate.rs` `#[cfg(test)]`

**What to implement:**

`src-tauri/assets/gate.ts` (verified against `examples/extensions/permission-gate.ts` + `dist/core/extensions/types.d.ts`: the factory is a single-arg `export default (pi: ExtensionAPI) => …`; `pi.on("tool_call", async (event, ctx) => …)` receives `event.toolName`/`event.input` (including `CustomToolCallEvent` for registered tools like `sudo_exec`); `ctx.ui.confirm(title: string, message: string, opts?: {timeout?: number}): Promise<boolean>` — a `cancelled` response resolves to `false` → block, the safe posture; the first-party example's `ctx.hasUI` guard is correctly OMITTED here — in RPC mode `ctx.hasUI` is `true` and `confirm` is implemented via `extension_ui_request` (`rpc-mode.js:85`)):
```ts
// Desktop-owned tool gate. Inert unless PI_ARCHIMEDES_GATE=1 (the desktop is
// the only spawner that sets it). Gated tools prompt the user via the
// extension-UI subprotocol; a rejection BLOCKS the tool (the LLM sees the
// reason as an error result).
export default (pi: any) => {
  if (process.env.PI_ARCHIMEDES_GATE !== "1") return;
  const GATED = new Set(["bash", "edit", "write", "sudo_exec"]);
  const TIMEOUT_MS = 300_000; // matches the desktop's 300 s permission waiter
  pi.on("tool_call", async (event: any, ctx: any) => {
    if (!GATED.has(event.toolName)) return; // undefined = proceed
    const detail = JSON.stringify(event.input ?? {}).slice(0, 500);
    const ok = await ctx.ui.confirm(
      `${event.toolName}: allow this tool call?`,
      detail,
      { timeout: TIMEOUT_MS },
    );
    if (!ok) return { block: true, reason: "User rejected the tool call." };
  });
};
```

`src-tauri/src/agent/gate.rs`:
```rust
/// Write the embedded gate extension to `config_dir/pi-gate/gate.ts`
/// (0644; idempotent — skip the write when the file already matches).
/// Returns the path (used for the `-e` arg).
pub fn install_gate_extension(config_dir: &std::path::Path) -> std::io::Result<std::path::PathBuf>;

/// The spawn args additions for a gated agent: args gain `["-e", <gate path>]`.
pub fn gate_spawn_args(gate_path: &std::path::Path, args: &[String]) -> Vec<String>;

/// The spawn env additions: `PI_ARCHIMEDES_GATE=1` (merged into the
/// `BTreeMap<String, String>` env the spawn site already builds).
pub fn gate_env(env: &mut BTreeMap<String, String>);
```
Call `install_gate_extension` from **BOTH** `SessionManager::new` AND `SubagentSessionManager::new` (both have `config_dir`; the idempotent write makes double-install safe; store the `PathBuf` on each manager). Apply `gate_spawn_args`/`gate_env` at the `PiRpc::spawn` site(s) in `session.rs` (main sessions) and `subagent.rs` (the subagent spawn — Task 5's rewrite calls the same helpers).

**Steps:**
- [ ] Write the failing tests in `gate.rs`:
      - `install_gate_extension_writes_file`: temp `config_dir` → file exists, mode 0644, content == the embedded string; calling again does not rewrite (mtime unchanged).
      - `gate_spawn_args_appends_e_flag` + `gate_env_appends_var`.
- [ ] Run `cargo test agent::gate` — confirm failure.
- [ ] Implement `gate.ts` + `gate.rs` + the `session.rs`/`subagent.rs` spawn-site injection.
- [ ] Add the end-to-end test: a `start_session` + `send_prompt` against `fake_pi` with `FAKE_PI_GATE=1` (re-run the Task 3 `permission_gate_roundtrip` scenario through the REAL injection path — assert the `permission-request` event arrives with the synthesized `Allow`/`Block` options and the tool name in `title`).
- [ ] `cargo test` (full) — green. `cargo clippy --all-targets` — 0 warnings. `cargo fmt` — clean.
- [ ] Commit with message: "feat(agent): bundled gate extension (tool_call → ctx.ui.confirm) + spawn injection"

**Acceptance criteria:**
- [ ] The gate file is written at startup (both managers) and passed via `-e` (asserted by the `gate_spawn_args` unit tests + the e2e flow).
- [ ] Without `PI_ARCHIMEDES_GATE=1` the extension is inert (the env check is the first line of the factory).
- [ ] The desktop's `permission-request` event for a gated tool carries the tool name in `title` (the frontend's `PermissionPrompt` renders it).

---

### Task 5: Subagents spawn `pi --mode rpc` directly (the launch wrapper dies)

**Context:** Today a subagent dispatch writes a per-dispatch wrapper script (`launch_wrapper.rs`: `exec pi <resolved flags> "$@"`) and passes it to a fresh `pi-acp` via `PI_ACP_PI_COMMAND`, because the third-party adapter had no CLI surface for per-dispatch config. With the desktop speaking RPC directly, the desktop passes the flags itself — the wrapper script is deleted. **Constraint discovered in review: `bridge.rs` imports `LaunchConfig` from `launch_wrapper.rs`** (`bridge.rs:40`: `use crate::acp::launch_wrapper::LaunchConfig;`; `dispatch_params` at :659 builds it from the `dispatch_subagent` frame params) — so `LaunchConfig` must be MOVED before the module dies (an allowed exception to the bridge no-touch rule: the import retarget only). The `SubagentSessionManager` shape, the `WorkerRuntime` handoff, the per-dispatch `SessionDriver` pattern, the capture logic (`captures()` at `subagent.rs:540`), the bridge env for the child, and the `subagent-session-started`/`subagent-closed` events are all KEPT.

**Files:**
- Modify: `src-tauri/src/agent/subagent.rs` (the `dispatch` spawn path + the `LaunchConfig` struct moved here + the `pi` flag resolution)
- Modify: `src-tauri/src/agent/bridge.rs` (the `LaunchConfig` import retarget ONLY: `use crate::acp::launch_wrapper::LaunchConfig;` → `use crate::agent::subagent::LaunchConfig;` — no logic changes)
- Delete: `src-tauri/src/agent/launch_wrapper.rs`
- Modify: `src-tauri/src/agent/mod.rs` (drop `pub mod launch_wrapper;`; `LaunchConfig` is now re-exported from `subagent`)
- Modify: `src-tauri/tests/subagent_dispatch.rs` (the dispatch BEHAVIOR tests — NOT in `subagent.rs`, which the prior draft mis-anchored: `subagent.rs`'s test module at :606 never tests `dispatch`; these live at `tests/subagent_dispatch.rs:26,38,155` and use the `StopReason` crate type, `env!("CARGO_BIN_EXE_fake_agent")`, `SessionInfo` params, and exercise the wrapper/`PI_ACP_PI_COMMAND` mechanism this task deletes — adapt to `fake_pi` + the local `StopReason` + the item-1 `SessionInfo` shape)
- Modify: `src-tauri/tests/subagent_concurrency.rs` (same adaptation — `:32,41` use the `StopReason` crate type + `fake_agent`)
- Test: `subagent.rs` `#[cfg(test)]` (the unit tests adapt for the `SessionInfo`/`StopReason`/`RpcError` type changes; the dispatch behavior coverage lives in the two `tests/` files above)

**What to implement:**

1. **Move `LaunchConfig`** from `launch_wrapper.rs` into `subagent.rs` **verbatim** (keep the name and the exact fields — verified: `system_prompt: Option<String>`, `model: Option<String>`, `thinking: Option<String>`, `tools: Option<Vec<String>>` — there is NO `name` field; `agent_name` is a separate `dispatch(...)` parameter parsed at `bridge.rs:745-746`). Retarget the `bridge.rs` import.

2. **Port the flag resolution** from `launch_wrapper.rs`'s `config_flags()` into `subagent.rs` as a pure function — KEEP the exact flag semantics (read `launch_wrapper.rs` line by line; verified: each of `--system-prompt`/`--model`/`--thinking` is emitted only when its field is `Some`; `--tools a,b` is emitted when `tools: Some`, and `--exclude-tools subagent` is emitted **only when `tools: None`**; `--no-session` is always appended; the wrapper's `"$@"` forwarding of `--mode rpc --no-themes` was the adapter's own args — the desktop now adds those itself):
```rust
/// Build the `pi` CLI args for a subagent dispatch (the flags the wrapper
/// script used to `exec`, now passed directly).
fn subagent_pi_args(cfg: &LaunchConfig) -> Vec<String>;
// == ["--mode", "rpc", "--no-themes"]
//    + (cfg.system_prompt.is_some() → ["--system-prompt", v])
//    + (cfg.model.is_some()       → ["--model", v])
//    + (cfg.thinking.is_some()    → ["--thinking", v])
//    + (cfg.tools.as_ref().map(|t| t.join(",")).is_some()
//         → ["--tools", <csv>]  ELSE  ["--exclude-tools", "subagent"])
//    + ["--no-session"]
// (The program is NOT part of this function — the spawn site passes
// `entry.command`; in tests that is the copied `fake_pi` path.)
```

3. **The spawn path** (`dispatch`, `subagent.rs:155-…`): replace the wrapper-write + `PI_ACP_PI_COMMAND` env + `AcpAgent` spawn with: `PiRpc::spawn(&entry.command, &subagent_pi_args(cfg) + gate args, &env, cwd)` — **`entry.command` is the parent's registry entry command** (the current `subagent.rs:233-237` spawns `AcpAgentConfig::new(entry.command)` — the literal `"pi"` at `subagent.rs:226` is the WRAPPER's exec target, not the spawn program; in tests the agents.json entry points at the copied `fake_pi` path, so the entry command is the only thing that makes the tests work). `subagent_pi_args` already starts with `["--mode","rpc","--no-themes"]` (the same base `entry.args` carries in production — do NOT double-merge `entry.args`; the subagent's explicit base wins). `env` = the existing bridge env (`bridge_spawn_setup` — the child's suite still bridges: cost `cost_update` flows for the metrics capture) + the Task 4 gate env/args + `"ARCHIMEDES_SUBAGENT=1"` (a dedicated discriminator env — harmless in production; lets a `fake_pi`-based test distinguish subagent children if ever needed). Then the existing per-dispatch `SessionDriver` (db: `None`, subagent: `None`) + `drive_session` with the Task 3 establisher + the first `prompt` (unbounded, as today). On settle → `captures()` (UNCHANGED: last-`messageId` text + accumulated `cost_update` + wall clock — the `messageId` derivation from Task 3 item 3 feeds `last_message_id` via the driver's capture feed, Task 3 item 2) → `subagent-closed`.

4. **Delete `launch_wrapper.rs`** and its tests; delete the `PI_ACP_PI_COMMAND` references.

**Steps:**
- [ ] Adapt `tests/subagent_dispatch.rs` + `tests/subagent_concurrency.rs` to `fake_pi` (use `FAKE_PI_PROMPT=1` — the child responds to `prompt` with text deltas + `agent_settled`; the main-vs-subagent distinction is NOT needed: one agents.json entry feeds both, and both behave identically):
      - `dispatch_spawns_rpc_child_and_captures` (in `tests/subagent_dispatch.rs`): assert `subagent-session-started` → the prompt flows → `subagent-closed` with `metrics` (text == the accumulated last-`messageId` text, `durationMs > 0`; cost from a `cost_update` the child's bridge emits if the existing test pattern covers it — mirror what the existing test asserts).
      - `subagent_pi_args_resolve` (unit test in `subagent.rs`): assert the exact flag vector for a known `LaunchConfig` (pins the port from `launch_wrapper.rs` — compare against the wrapper's current `config_flags()` output for the same input, all four field combinations).
      - Adapt the `subagent.rs` unit tests (mod at :606) for the `SessionInfo`/`StopReason`/`RpcError` type changes (they never tested `dispatch` — do not invent dispatch coverage there).
- [ ] Run `cargo test agent::subagent` — confirm failure.
- [ ] Implement (move `LaunchConfig`, port the resolution, rewrite the spawn path, delete the wrapper, retarget the `bridge.rs` import).
- [ ] `cargo test` (full) — green. `cargo clippy --all-targets` — 0 warnings. `cargo fmt` — clean.
- [ ] Commit with message: "feat(agent): subagents spawn pi RPC directly (launch wrapper deleted)"

**Acceptance criteria:**
- [ ] No `launch_wrapper` / `PI_ACP_PI_COMMAND` / wrapper-script references remain in `src-tauri/` (grep).
- [ ] `bridge.rs` changed only in the `LaunchConfig` import line.
- [ ] The subagent capture (text/cost/duration) is byte-identical in semantics to today's (the existing `captures()` untouched).
- [ ] The child's bridge env is still set (the suite's `cost_update`/`session` pushes still flow — the metrics source is unchanged).

---

### Task 6: Delete the ACP artifacts, write the ADR, run the full validation gate

**Context:** With the RPC path complete (Tasks 2-5), the ACP layer is dead code. This task removes it, writes the ADR recording the decision (superseding ADRs 0001/0002/0005), updates the project docs, and runs the full validation gate from `AGENTS.md`.

**Files:**
- Delete: `src-tauri/src/bin/fake_agent.rs` (superseded by `fake_pi`) — the other ACP artifacts were already handled in Task 3 (`prompt.rs` deleted there; `fs_backend.rs`/`tests/fs_backend.rs` **STAY** with the `AcpError` → `FsError` error-type switch — `permission.rs`'s read-only pre-approval check consumes `FsBackend`; that path is effectively unused in Phase 1 — the gate extension never sends read-only requests — but it compiles and is the Phase 2/3 desktop-tool-executor foundation)
- Modify: `src-tauri/src/agent/mod.rs` (drop the `prompt` module + the `ImagePayload` re-export if Task 3 did not already — idempotent check; `fs_backend` + `FsBackend` STAY)
- Modify: `src-tauri/src/agent/bridge.rs` + `src-tauri/src/config/registry.rs` (**docs-only**: the stale `pi-acp` doc comments — `bridge.rs:21`/`:371` (the old topology description), `registry.rs:8`/`:42` — updated to the new topology; no behavioral change)
- Modify: `src-tauri/Cargo.toml` (remove `agent-client-protocol` from `[dependencies]` (line 33) AND `[dev-dependencies]` (line 44); remove any now-unused deps — clippy will surface them; update the "Two binaries exist" comment to name `fake_pi`)
- Modify: `src-tauri/tests/acp_concurrency_real.rs` (git mv → `tests/rpc_concurrency_real.rs`; adapt the real-binary concurrency test to spawn `pi --mode rpc` directly — **skip the test when `pi` is not on PATH** (the existing test's availability guard pattern); if the test cannot be adapted cleanly, delete it and note why in the commit message)
- Create: `docs/decisions/0009-rpc-replaces-acp.md`
- Modify: `docs/decisions/0001-acp-only-internal-protocol.md`, `0002-one-live-acp-session.md`, `0005-subagent-launch-wrapper.md` (front-matter `status: superseded` + `superseded-by: 0009-rpc-replaces-acp.md`)
- Modify: `AGENTS.md` (the "Rust backend … ACP" framing → the core speaks pi's RPC natively; the bridge paragraph — "The desktop is the **Client** of the bridge channel" — stays verbatim)
- Modify: `CONTEXT.md` (terminology: ACP-protocol references → pi RPC; the bridge terms stay as-is)

**What to implement:**
- The deletions + `Cargo.toml` cleanup. If `cargo build` surfaces a `use` of a deleted item (a missed reference), fix the reference (it should be gone after Tasks 3-5 — any remaining one is a bug to fix, not a reason to keep the ACP code).
- `docs/decisions/0009-rpc-replaces-acp.md` (ADR format, matching the existing ADRs' structure — front-matter `status: accepted`, `date: 2026-09-24`, `superseded-by:` empty, and a `supersedes: 0001, 0002, 0005` note in the body):
  - **Decision:** the Rust core speaks pi's RPC mode (`pi --mode rpc`) natively; the ACP protocol and the third-party `pi-acp` adapter are removed from the dependency graph.
  - **Why:** the evidence in `docs/research/pi-rpc-replaces-acp.md` — the 17 lossy areas of the third-party adapter (cost, todos, subagent visibility, thinking on resume, masked stopReasons, …), the removal of the third-party dependency, the macOS interactive gap (the extension-UI subprotocol rides stdio and works where the bridge is fail-closed), and the end-state expressibility (`registerTool` same-name override + `--no-builtin-tools` — the Gondolin pattern — makes "tools live in the desktop" natively achievable in pi, which ACP cannot express for arbitrary agents).
  - **Consequences:** the RPC surface is stable-but-unversioned (pin the pi version; contract-test on upgrades — the `rpc_flow` integration tests + the real-`pi` smoke test are the wire checks); `prompt` success ≠ completion (turns resolve on `agent_settled`); the permission model is the bundled gate extension (`tool_call` → `ctx.ui.confirm` → `extension_ui_request`) — the ADR 0003 double-prompt is now real for `sudo_exec` (gate confirm → bridge confirm modal → bridge password modal); the bridge is UNCHANGED in Phase 1 (it is the suite's own channel and becomes redundant only in Phase 3); `resume` reads the pi session file from the stored `capabilities_json` (legacy ACP rows are history-only — the frontend's Resume button is gated on the normalized `loadSession: false`, and `resume_session` returns `NotResumable`); **accepted Phase-1 parity losses: tool diffs (no `content: ToolCallContent[]` frames — the desktop-side tool executor that would produce them is Phase 3) and thinking blocks on resume if `get_messages` doesn't return them (gap G2)**; ADR 0004 (worker runtime) and 0006-0008 are unaffected.
  - **Phases:** Phase 1 = this decision (the swap); Phase 2 = the suite's tools (ask/sudo_exec/subagent/manage_todo_list) move into the desktop via `registerTool` same-name override with `execute()` = desktop round-trip; Phase 3 = the built-in tools move too (`--no-builtin-tools` + re-registered delegated tools, the Gondolin pattern — the `*Operations` seams, `dist/core/tools/bash.d.ts:24-27`) and the bridge retires (one channel, stdio, replaces two).
- `AGENTS.md`: update the project framing (the core speaks pi RPC; the bridge paragraph stays verbatim).
- `CONTEXT.md`: terminology updates (ACP protocol references → pi RPC; keep the bridge terms).

**Steps:**
- Delete `fake_agent.rs` + the `Cargo.toml` entries + update the stale `pi-acp` doc comments (`bridge.rs:21`/`:371`, `registry.rs:8`/`:42` — docs-only); `cargo build` clean (fix any missed reference as a bug).
- [ ] Write ADR 0009 + the superseded front-matter updates + `AGENTS.md`/`CONTEXT.md` edits.
- [ ] Run the FULL validation gate (from `AGENTS.md`): `pnpm test` (repo root) → `pnpm build` (repo root) → `cargo test` (in `src-tauri/`) → `cargo clippy --all-targets` (0 warnings) → `cargo fmt --check` (in `src-tauri/`).
- [ ] If any gate fails: fix the failure (a frontend type breakage is allowed ONLY as a widening — e.g. `capabilities` in `src/lib/tauri.ts` — and `pnpm test`/`pnpm build` must pass after).
- [ ] **Manual wire smoke (the one check `fake_pi` cannot do — see Task 2's circularity caveat):** if a `pi` binary is available on the dev machine: start a session in the desktop, send a prompt, watch text stream + a gated tool call trigger the `PermissionPrompt`, approve it, watch the turn settle; resume the session from history (replay from the pi session file). This is the authoritative wire-format check (real camelCase from real pi).
- [ ] Commit with message: "feat: remove ACP (crate dep, fake_agent) — prompt.rs died in Task 3, fs_backend.rs stays (FsError) — the core speaks pi RPC natively; ADR 0009"

**Acceptance criteria (the plan's `done-when`):**
- [ ] `grep -ri "agent-client-protocol\|AcpAgent\|AcpError\|pi-acp" src-tauri/` returns nothing (the `pi-acp` hits in `bridge.rs`/`registry.rs` doc comments are updated in this task — see the Files/Steps entries above).
- [ ] `docs/decisions/0009-rpc-replaces-acp.md` exists; 0001/0002/0005 are `status: superseded` pointing at it.
- [ ] All five validation gates green: `pnpm test`, `pnpm build`, `cargo test`, `cargo clippy --all-targets` (0 warnings), `cargo fmt --check`.
- [ ] The manual wire smoke (if `pi` is available) passes: live streaming + gate + settle + resume against a real `pi --mode rpc` process.
