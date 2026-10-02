---
status: approved
done-when: Outgoing requests from OpenAiCompatibleProvider carry x-litellm-session-id and x-request-id headers with the session's stable ID, newly minted IDs follow arch_<uuid-v4>, and all tests pass with 0 clippy warnings.
---

# Request ID Tracking & Reuse Spec

## Context & Motivation
LiteLLM and upstream LLM gateways use session identifiers (`x-litellm-session-id`, `x-request-id`, `x-litellm-trace-id`) to group turns, tool-call loops, and compaction summaries of a conversation under a single trace in logs, spend tracking, and proxy caches.

Archimedes Desktop's native OpenAI-compatible client (`OpenAiCompatibleProvider`) currently sends `User-Agent: archimedes/<version>` and `Authorization: Bearer <key>`, but does not attach session/request ID headers. Additionally, Archimedes session IDs should follow the standard `arch_<REQUESTID-HASH>` format (where `<REQUESTID-HASH>` is a UUID v4) and be reused consistently across every request for that session.

## Detailed Design

### 1. ID Minting Helper (`mint_session_id`)
Introduce a helper function in `src-tauri/src/agent/session.rs`:
```rust
pub fn mint_session_id() -> String {
    format!("arch_{}", uuid::Uuid::new_v4())
}
```

Use `mint_session_id()` at all points where fresh session IDs are minted:
- `SessionManager::start_native_session`: `mint_session_id()`
- `SessionManager::start_external_session` & `bridge_spawn_setup`: `mint_session_id()` for `client_session_id`
- `SubagentSessionManager::dispatch_native`: `mint_session_id()` for `child_id`
- `SubagentSessionManager::dispatch`: `mint_session_id()` for `client_session_id`

When resuming an existing session from SQLite storage, the stored `session_id` is preserved as-is.

### 2. Request Plumbing & Propagation (`ModelRequest` & `AgentLoop`)
Extend `ModelRequest` in `src-tauri/src/agent/harness/provider.rs`:
```rust
pub struct ModelRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub tools: Vec<ToolSpec>,
    pub options: ModelOptions,
    pub session_id: Option<String>,
}
```

In `src-tauri/src/agent/harness/loop.rs`:
- `AgentLoop::step`: Construct `ModelRequest` with `session_id: Some(self.session_id.clone())`.
- `AgentLoop::summarize`: Construct `ModelRequest` with `session_id: Some(self.session_id.clone())`.

### 3. Header Injection in `OpenAiCompatibleProvider`
In `src-tauri/src/agent/harness/provider.rs` (`OpenAiCompatibleProvider::complete`):
```rust
let mut builder = client
    .post(&url)
    .bearer_auth(&self.api_key)
    .json(&request_body(req));

if let Some(session_id) = &req.session_id {
    builder = builder
        .header("x-litellm-session-id", session_id)
        .header("x-request-id", session_id);
}

let resp = builder
    .send()
    .await
    .map_err(|e| ProviderError::Retryable(format!("request failed: {e}")))?;
```

### 4. Verification & Testing
- Unit tests in `provider.rs` asserting HTTP requests receive `x-litellm-session-id` and `x-request-id` headers when `req.session_id` is present.
- Unit tests asserting `mint_session_id()` returns `arch_<uuid-v4>`.
- Loop integration tests asserting `AgentLoop` provides `session_id` to `complete()`.
- Full test pass: `cargo test`, `cargo clippy --all-targets` (0 warnings), `pnpm test`, `pnpm build`.
