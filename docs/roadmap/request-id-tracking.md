---
status: committed
done-when: Outgoing requests from OpenAiCompatibleProvider carry x-litellm-session-id and x-request-id headers with the session's stable ID, newly minted IDs follow arch_<uuid-v4>, and all tests pass with 0 clippy warnings.
---

# Request ID Tracking & Reuse Plan

**Goal:** Ensure all LLM completions send stable, prefixed session/request IDs (`arch_<uuid-v4>`) across initial turns, subsequent turns, tool loops, and compaction summaries.
**Architecture:** Mint `arch_<uuid-v4>` on session creation, plumb `session_id` through `ModelRequest` in `AgentLoop`, and inject `x-litellm-session-id` & `x-request-id` headers in `OpenAiCompatibleProvider`.
**Tech Stack:** Rust, Tauri 2, Reqwest, Tokio, WireMock.

---

### Task 1: `mint_session_id()` Helper and Session ID Prefixing

**Context:**
All newly minted session IDs (native sessions, external placeholder client sessions, native subagents, and external subagent placeholders) need to follow the `arch_<uuid-v4>` format, while resumed sessions preserve their existing stored ID (including legacy bare-UUID sessions).

**Files:**
- Modify: `src-tauri/src/agent/session.rs`
- Modify: `src-tauri/src/agent/subagent.rs`
- Test: `src-tauri/tests/session_native.rs`

**What to implement:**
- In `src-tauri/src/agent/session.rs`:
  - Add `pub fn mint_session_id() -> String { format!("arch_{}", uuid::Uuid::new_v4()) }`.
  - In `SessionManager::build_native_session` (~line 2009): use `mint_session_id()` for new session IDs when `resume` is `None`.
  - In `SessionManager::start_session` (~line 2238): use `mint_session_id()` for `client_session_id`.
  - In `session_tests` mod: add unit test `mint_session_id_format` verifying the returned string starts with `arch_` and the suffix parses as a valid UUID with `get_version_num() == 4`.
- In `src-tauri/src/agent/subagent.rs`:
  - In `SubagentSessionManager::dispatch_native_inner` (~line 885): use `crate::agent::session::mint_session_id()` for `child_id`.
  - In `SubagentSessionManager::dispatch` (~line 491): use `crate::agent::session::mint_session_id()` for `client_session_id`.
- In `src-tauri/tests/session_native.rs`:
  - In `start_session_streams_and_persists`, assert `info.session_id.starts_with("arch_")`.
  - Add a test `resuming_legacy_bare_uuid_session_preserves_id` verifying a session seeded with `uuid::Uuid::new_v4().to_string()` without prefix retains its exact ID on resume.

**Steps:**
- [ ] Write unit test `mint_session_id_format` in `src-tauri/src/agent/session.rs`.
- [ ] Run `cargo test --lib mint_session_id_format` from `src-tauri/`
  - Confirm failure before implementing `mint_session_id`.
- [ ] Implement `mint_session_id()` and replace raw `Uuid::new_v4().to_string()` calls for session minting in `session.rs` and `subagent.rs`.
- [ ] Update `src-tauri/tests/session_native.rs` with the `arch_` prefix assertion and legacy bare-UUID resume test.
- [ ] Run `cargo test --lib mint_session_id_format` and `cargo test --test session_native` from `src-tauri/`
  - Confirm all pass.
- [ ] Run `cargo fmt --check` and `cargo clippy --all-targets` from `src-tauri/` (must be 0 warnings).
- [ ] Commit with message: "feat(agent): mint session IDs with arch_ prefix"

**Acceptance criteria:**
- [ ] `mint_session_id()` produces strings formatted as `arch_<uuid-v4>`.
- [ ] Fresh native and subagent session IDs start with `arch_`.
- [ ] Resumed sessions (including legacy bare-UUID rows) continue using their stored ID without modification.
- [ ] All tests pass with 0 warnings.

---

### Task 2: `ModelRequest` Session ID & Provider Header Injection

**Context:**
`OpenAiCompatibleProvider` needs to attach `x-litellm-session-id` and `x-request-id` headers to outgoing HTTP requests when `session_id` is present on `ModelRequest`. All `ModelRequest` constructor sites must be updated so the crate compiles cleanly.

**Files:**
- Modify: `src-tauri/src/agent/harness/provider.rs`
- Modify: `src-tauri/src/agent/harness/loop.rs`
- Modify: `src-tauri/tests/provider.rs`
- Test: `src-tauri/tests/provider.rs`

**What to implement:**
- In `src-tauri/src/agent/harness/provider.rs`:
  - Add `pub session_id: Option<String>` to `ModelRequest`.
  - In `OpenAiCompatibleProvider::complete`: when `req.session_id` is `Some(ref sid)`, attach `.header("x-litellm-session-id", sid)` and `.header("x-request-id", sid)` to the request builder.
  - Update `ModelRequest` literals in `provider.rs` tests (~lines 1043, 1150, 1232) with `session_id: None`.
- In `src-tauri/src/agent/harness/loop.rs`:
  - Update `ModelRequest` literals in `summarize` (~line 1247) and `model_request` (~line 1318) to set `session_id: None` (temporarily, so Task 2 compiles independently before Task 3 wires them to `self.session_id`).
- In `src-tauri/tests/provider.rs`:
  - Update the `request()` helper to include `session_id: None`.
  - Add test `openai_provider_sends_session_id_headers_when_present` using `wiremock` to assert that `x-litellm-session-id` and `x-request-id` headers match `req.session_id`.
  - Add test `openai_provider_omits_session_id_headers_when_absent` asserting that neither header is present when `session_id` is `None`.

**Steps:**
- [ ] Add failing wiremock tests in `src-tauri/tests/provider.rs`.
- [ ] Run `cargo test --test provider openai_provider_sends_session_id_headers_when_present` from `src-tauri/`
  - Confirm test failure.
- [ ] Update `ModelRequest` struct, `OpenAiCompatibleProvider::complete`, and the literal sites in `provider.rs`, `loop.rs`, and `tests/provider.rs`.
- [ ] Run `cargo test --test provider` from `src-tauri/`
  - Confirm all wiremock provider tests pass.
- [ ] Run `cargo fmt --check` and `cargo clippy --all-targets` from `src-tauri/` (must be 0 warnings).
- [ ] Commit with message: "feat(harness): attach x-litellm-session-id and x-request-id headers in OpenAiCompatibleProvider"

**Acceptance criteria:**
- [ ] Outgoing HTTP requests carry `x-litellm-session-id` and `x-request-id` headers when `session_id` is `Some(...)`.
- [ ] Outgoing HTTP requests omit both headers when `session_id` is `None`.
- [ ] `cargo test --test provider` passes and `cargo clippy --all-targets` has 0 warnings.

---

### Task 3: Plumb `session_id` from `AgentLoop` into `ModelRequest`

**Context:**
`AgentLoop` owns `self.session_id` throughout the entire turn lifecycle (first prompt, all tool-call iterations, retries, and compaction summaries). It must propagate `session_id: Some(self.session_id.clone())` into `ModelRequest` so that every request in a session shares the same identifier.

**Files:**
- Modify: `src-tauri/src/agent/harness/loop.rs`
- Test: `src-tauri/src/agent/harness/loop.rs`

**What to implement:**
- In `src-tauri/src/agent/harness/loop.rs`:
  - In `AgentLoop::model_request` (~line 1318): change `session_id: None` to `session_id: Some(self.session_id.clone())`.
  - In `AgentLoop::summarize` (~line 1247): change `session_id: None` to `session_id: Some(self.session_id.clone())`.
  - In `mod tests` of `loop.rs`:
    - Add a `SessionRecordingProvider` helper struct that records `req.session_id.clone()` into an `Arc<Mutex<Vec<Option<String>>>>` and returns a canned `text_then_done` stream.
    - Add unit test `model_requests_carry_the_session_id_across_turns_and_tool_calls` asserting all recorded `session_id` values equal `Some("s1")`.
    - Add unit test `summarize_model_request_carries_the_session_id` calling `loop_.summarize(...)` directly and asserting recorded `session_id` is `Some("s1")`.

**Steps:**
- [ ] Write failing unit tests in `src-tauri/src/agent/harness/loop.rs` using `SessionRecordingProvider`.
- [ ] Run `cargo test --lib model_requests_carry_the_session_id` from `src-tauri/`
  - Confirm failure on assertion (observing `None != Some("s1")`).
- [ ] Update `model_request` and `summarize` in `src-tauri/src/agent/harness/loop.rs` to set `session_id: Some(self.session_id.clone())`.
- [ ] Run `cargo test --lib harness::r#loop::tests` from `src-tauri/`
  - Confirm all loop tests pass.
- [ ] Run full test suite:
  - `cargo test` from `src-tauri/`
  - `cargo clippy --all-targets` from `src-tauri/` (must be 0 warnings)
  - `cargo fmt --check` from `src-tauri/`
  - `pnpm test` from repo root
  - `pnpm build` from repo root
- [ ] Commit with message: "feat(harness): propagate session_id from AgentLoop to ModelRequest"

**Acceptance criteria:**
- [ ] Every model call during conversational turns, tool loops, retries, and compaction summaries receives `Some(self.session_id.clone())`.
- [ ] Full backend and frontend test suites pass with 0 warnings.
