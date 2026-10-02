---
status: committed
done-when: Outgoing requests from OpenAiCompatibleProvider carry x-litellm-session-id and x-request-id headers with the session's stable ID, newly minted IDs follow arch_<uuid-v4>, and all tests pass with 0 clippy warnings.
---

# Request ID Tracking & Reuse Plan

**Goal:** Ensure all LLM completions send stable, prefixed session/request IDs (`arch_<uuid-v4>`) across initial turns, subsequent turns, tool loops, and compaction summaries.
**Architecture:** Mint `arch_<uuid-v4>` on session creation, plumb `session_id` through `ModelRequest` in `AgentLoop`, and inject `x-litellm-session-id` & `x-request-id` headers in `OpenAiCompatibleProvider`.
**Tech Stack:** Rust, Tauri 2, Reqwest, Tokio.

---

### Task 1: `mint_session_id()` Helper and Session ID Prefixing

**Context:**
All newly created session IDs (native sessions, external client sessions, native subagents, and external subagents) need to follow the `arch_<uuid-v4>` format, while resumed sessions preserve their existing stored ID.

**Files:**
- Modify: `src-tauri/src/agent/session.rs`
- Modify: `src-tauri/src/agent/subagent.rs`
- Test: `src-tauri/tests/session_native.rs`
- Test: `src-tauri/tests/harness_dispatch_native.rs`

**What to implement:**
- In `src-tauri/src/agent/session.rs`:
  - Add `pub fn mint_session_id() -> String { format!("arch_{}", uuid::Uuid::new_v4()) }`.
  - In `SessionManager::build_native_session`: use `mint_session_id()` for new session IDs when `resume` is `None`.
  - In `SessionManager::start_session`: use `mint_session_id()` for `client_session_id`.
- In `src-tauri/src/agent/subagent.rs`:
  - In `SubagentSessionManager::dispatch_native`: use `crate::agent::session::mint_session_id()` for `child_id`.
  - In `SubagentSessionManager::dispatch`: use `crate::agent::session::mint_session_id()` for `client_session_id`.
- Update any test assertions expecting bare UUIDs to accept `arch_<uuid>`.

**Steps:**
- [ ] Write unit test in `src-tauri/src/agent/session.rs` verifying `mint_session_id()` produces an ID starting with `arch_` followed by a valid UUID v4.
- [ ] Run `cargo test -p archimedes-lib mint_session_id`
  - Confirm test failure before implementing helper.
- [ ] Implement `mint_session_id()` and replace raw `Uuid::new_v4().to_string()` calls for session minting in `session.rs` and `subagent.rs`.
- [ ] Run `cargo test -p archimedes-lib` and integration tests.
- [ ] Run `cargo clippy --all-targets` from `src-tauri/` (must be 0 warnings).
- [ ] Commit with message: "feat(agent): mint session IDs with arch_ prefix"

**Acceptance criteria:**
- [ ] Fresh native and subagent session IDs start with `arch_`.
- [ ] Resumed sessions continue using their stored ID without modification.
- [ ] All session and subagent tests pass.

---

### Task 2: `ModelRequest` Session ID & Provider Header Injection

**Context:**
`OpenAiCompatibleProvider` needs to attach the session ID to outgoing requests so LiteLLM and proxy gateways can track and correlate all completions in a session.

**Files:**
- Modify: `src-tauri/src/agent/harness/provider.rs`
- Test: `src-tauri/src/agent/harness/provider.rs`

**What to implement:**
- In `src-tauri/src/agent/harness/provider.rs`:
  - Add `pub session_id: Option<String>` to `ModelRequest`.
  - In `OpenAiCompatibleProvider::complete`: when `req.session_id` is `Some(ref sid)`, add headers `.header("x-litellm-session-id", sid)` and `.header("x-request-id", sid)` to the request builder.
  - Update all internal `ModelRequest` test instances in `provider.rs` with `session_id: None` (or test session IDs).

**Steps:**
- [ ] Write failing test in `provider.rs` checking that `OpenAiCompatibleProvider::complete` sends `x-litellm-session-id` and `x-request-id` headers when `session_id` is set on `ModelRequest`.
- [ ] Run `cargo test -p archimedes-lib provider::tests`
  - Confirm failure.
- [ ] Update `ModelRequest` struct and `OpenAiCompatibleProvider::complete` implementation.
- [ ] Run `cargo test -p archimedes-lib provider::tests`
  - Confirm all provider tests pass.
- [ ] Run `cargo clippy --all-targets` from `src-tauri/` (must be 0 warnings).
- [ ] Commit with message: "feat(harness): attach x-litellm-session-id and x-request-id headers in OpenAiCompatibleProvider"

**Acceptance criteria:**
- [ ] Outgoing HTTP requests include both headers with the exact `session_id` string when provided.
- [ ] Requests without `session_id` do not include these headers.

---

### Task 3: Plumb `session_id` from `AgentLoop` into `ModelRequest`

**Context:**
`AgentLoop` owns the `session_id` for the lifetime of a turn and across multiple tool calls and compaction summaries. It must propagate this `session_id` into every `ModelRequest` it creates.

**Files:**
- Modify: `src-tauri/src/agent/harness/loop.rs`
- Test: `src-tauri/src/agent/harness/loop.rs`
- Test: `src-tauri/tests/harness_loop.rs`

**What to implement:**
- In `src-tauri/src/agent/harness/loop.rs`:
  - In `AgentLoop::step`: construct `ModelRequest` with `session_id: Some(self.session_id.clone())`.
  - In `AgentLoop::summarize`: construct `ModelRequest` with `session_id: Some(self.session_id.clone())`.
  - Update any test helper `ModelRequest` constructions in `loop.rs`.
- Add test verifying that `RecordingProvider` captures `session_id` matching `AgentLoop.session_id` across turns and summaries.

**Steps:**
- [ ] Write failing test in `loop.rs` checking that `complete()` receives `session_id` matching `AgentLoop.session_id`.
- [ ] Run `cargo test -p archimedes-lib harness::loop::tests`
  - Confirm failure.
- [ ] Update `step()` and `summarize()` in `loop.rs` to populate `session_id`.
- [ ] Run `cargo test` across all targets.
- [ ] Run `cargo clippy --all-targets` from `src-tauri/` (must be 0 warnings).
- [ ] Run `cargo fmt --check` from `src-tauri/`.
- [ ] Run `pnpm test` and `pnpm build` from repo root.
- [ ] Commit with message: "feat(harness): propagate session_id from AgentLoop to ModelRequest"

**Acceptance criteria:**
- [ ] Every model call during `step()` (initial prompt & tool loops) has `session_id == self.session_id`.
- [ ] Every model call during `summarize()` has `session_id == self.session_id`.
- [ ] Entire backend and frontend test suites pass with 0 warnings.
