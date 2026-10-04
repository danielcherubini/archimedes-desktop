# Codebase Improvement Report — 2026-10-04

## Summary

20 findings across 8 categories. **5 high, 10 medium, 5 low.**

**Audit caveat (read first):** ADR 0025 (*sessions in Worker processes*) merged to `main` **between Phase 2 (scan) and Phase 3 (deep-dive)** — commits `77fb1be`…`ebbe8ed`, 2026-10-04. The scan was run against the pre-0025 tree; the deep-dive and all line-number evidence in this report were **re-verified against the current tree at `ebbe8ed`** (clean). Two pre-0025 findings were invalidated by 0025 (dead `LiveSession.cwd` field; unused `@tauri-apps/plugin-opener` — now used by the stalled-session banner) and are excluded. The `AgentLoop::new` 23-arg-duplication finding was also invalidated (0025 centralized construction in `worker/core.rs::default_build_loop`).

**Process gaps:** the Naming lens subagent (Phase 2) failed to return — its territory was substantially covered by the Inconsistent Patterns lens. 5 of 6 Phase 3 deep-dive subagents (3 reviewers + 2 explores) failed to return; the Rust-core explore returned and its claims were spot-verified directly. Findings therefore rest on lens-level evidence + direct verification, not reviewer sign-off.

## Context

- CONTEXT.md: **loaded** (domain glossary; no Engineering Rules section — defaults applied: 200-line threshold, per file, excl. comments/blanks)
- ADRs reviewed: 15 (0006–0025, by mtime)
- Plans reviewed: 6 (`docs/features/`: image-attachments, model-selection, subagent-sessions, thinking-display, tool-call-output, tool-card-zcode-style)

## Findings

### 🔴 High Severity

#### 1. `agent/harness/loop.rs` — 6,432-line god file (10 concerns, ~71% tests)
- **Lens:** File Length + Structure
- **Files:** `src-tauri/src/agent/harness/loop.rs`
- **Severity:** High
- **Confidence:** High
- **Problem:** 6,432 raw / ~1,850 production lines. Ten distinct concerns in one file: AgentLoop struct + lifecycle (~3%), pump loop (~1%), the `handle_prompt` turn core (536–977, **~440 lines** — user-message build, compaction, model call + retry + cancel race, stream accumulation, mid-stream retry, settlement), settle/cancel machinery, permission gate, tool dispatch + the new concurrent-subagent batch (ebbe8ed), ask-flow, subagent dispatch + `resolve_launch` (1326–1664, agent-definition frontmatter layering — a pure function with no loop state), model-request/tool-specs, persistence + event emission. Tests (2282–6432, **~4,150 lines, 71%**) inflate the file and hide the production surface. `handle_prompt` alone is a 6-subsystem god function (store, retry, compactor, provider, permissions, accumulator).
- **Proposal:** Extract, in order: (1) `resolve_launch` + its 15 `resolve_launch_*` tests → `harness/launch.rs` (pure fn, zero loop state — cleanest seam); (2) ask-flow + ask-result shaping fns → `harness/ask.rs`; (3) `dispatch_subagent` + subagent-call predicates → join existing `harness/dispatch.rs` or `agent/subagent.rs`; (4) move `run_compaction`/`summarize` into existing `compact.rs`. Split the test module per extraction (or move to `tests/` integration files where items are `pub`). Production file drops to ~900 lines. `handle_prompt` stays as the turn core (it *is* the loop).

#### 2. `agent/harness/provider.rs` — 4,599-line file holding 3 wire providers + shared types
- **Lens:** File Length + Structure (DRY + Weak Abstractions ride along)
- **Files:** `src-tauri/src/agent/harness/provider.rs`
- **Severity:** High
- **Confidence:** High
- **Problem:** Three full provider stacks in one file — OpenAI-completions (SSE 377–640 + impl 1447–1550), Anthropic (741–1068 + 1633–1952), OpenAI-Responses (1069–1446 + 2049–2206) — plus shared wire types/trait (30–324) and one 2,393-line `mod tests` (52%). The three `complete()` impls share a **verbatim ~13-line HTTP-status→`ProviderError` map** (401/403→Auth, 429/5xx→Retryable, 4xx→Fatal) at 1497/1926/2172, an identical `build_client(30s, 5min)` construction, and an identical `x-litellm-session-id` header block (1476/1908/2148). The two `event:`-tagged SSE parsers (`AnthropicStream`, `ResponsesStream`) have **byte-identical `handle_line`** and the same `poll_next` skeleton. ADR 0024 justified three separate providers (wire shapes differ) but rejected a generic adapter only for the *chunk-dispatch* logic — the *transport* discipline is wire-agnostic and is triplicated. The `anthropic-version: "2023-06-01"` literal appears in both `provider.rs:1905` and `catalog.rs:298`.
- **Proposal:** Split into `harness/provider/{types.rs (wire types + trait), openai.rs, anthropic.rs, responses.rs}` + a shared `transport.rs` owning `build_client`, the status→`ProviderError` map, and the session-id headers; each provider keeps only its request-body builder + SSE parser. Move the per-provider test groups with their providers. Introduce `enum WireApi { OpenAiCompletions, AnthropicMessages, OpenAiResponses }` (serde rename — wire format unchanged) so `build_provider` becomes an exhaustive `match` and the ~25 free-floating literals (5 Rust files + 2 TS files, 80 total occurrences) collapse.

#### 3. `agent/session.rs` — 4,357-line file; 1,430-line `SessionManager` impl; post-0025 router/crash/stall cluster
- **Lens:** File Length + Structure
- **Files:** `src-tauri/src/agent/session.rs`
- **Severity:** High
- **Confidence:** High
- **Problem:** ~2,650 production lines with ~10 concerns: shared types (`EventSink` — the cycle keystone, `TurnState`, `SessionInfo`, image validation, `StalledInfo`), a now-vestigial 19-line `SessionDriver`, `EffectiveCatalog`, a **~1,430-line `SessionManager` impl** (588–2017: worker-session establishment, all lifecycle commands, the ADR 0025 **event router + crash/stall cluster** (1750–2017, ~270 lines: `route_event`, `handle_crash`, `stalled_info`, `resolve_turn`, `on_session_exited_expected`, `cleanup_pending_modals`), model/thinking resolution helpers, and the **event-normalization block** (2209–2656: `normalize` ~275 ln + `normalize_capabilities` + `compute_display_rows` ~140 ln — pure fns on `TurnState`). Three test mods (39% of the file) map 1:1 to the split boundaries. `set_config_option` (~150 ln) reaches into 4 domains (catalog, settings file, loop control channel, re-synthesis).
- **Proposal:** Extract (1) the normalization block (2209–2656) + its test mod → `agent/normalize.rs` (pure fns; `pub(crate)` — integration-test-safe to move); (2) the router/crash/stall cluster (1750–2017) → `agent/session_router.rs` (mirrors the `worker/sink.rs` precedent); (3) `EffectiveCatalog` + the model/thinking-resolution helpers → a `model_resolution` unit; (4) split the `SessionManager` impl at the lifecycle (948–1124 establishment) vs command (1125–1703) boundary. The extraction of `EventSink` + `TurnState` + `normalize` into a neutral module is also the cycle-break (finding 7).

#### 4. 4-way module cycle: `session ⇄ loop ⇄ subagent ⇄ interactive` (+ `permission` in the middle)
- **Lens:** Coupling
- **Files:** `src-tauri/src/agent/{session,subagent,interactive,permission}.rs`, `src-tauri/src/agent/harness/loop.rs`
- **Severity:** High
- **Confidence:** High (edges verified directly at `ebbe8ed`)
- **Problem:** Confirmed bidirectional edges: `loop.rs:52` → `session::{normalize, compute_display_rows, EventSink, ThoughtState, TurnState}` while `session.rs:42–45` → `harness::{… AgentLoop re-exports …}`; `subagent.rs:25` → `session::{mint_session_id, EventSink}` while `session.rs` → `harness::Model`; `interactive.rs:40` → `session::EventSink` + `:41` → `subagent::LaunchConfig` while `loop.rs:46–49` → `interactive::{…}`; `permission.rs:45` → `session::EventSink` while `session.rs:46` → `permission::{self}`. `EventSink` is the **keystone**: imported by all 4 cycle modules, 6 `worker/` modules, and 7 integration test files. The cycle is 5 items wide; the items are mostly `pub(crate)` (integration tests can't see them — moving them between private modules is integration-test-safe) except `EventSink` + `TurnState` (pub; `EventSink` facade-re-exported at `agent/mod.rs:32`). Net effect: no seam to break — "who owns what" is invisible at the module boundary, and every refactor of one god file ripples through all five.
- **Proposal:** Create `agent/types.rs` (or fold into `agent/events.rs`) hosting the shared items by their natural owner: `EventSink` (→ `events.rs`, it's an event-emission trait), `TurnState`/`ThoughtState` + `normalize`/`compute_display_rows` (→ new `agent/normalize.rs` per finding 3), `mint_session_id` (→ `types.rs`). The 4 modules then import from the hub; the `session ⇄ loop` and `session ⇄ {subagent,interactive,permission}` edges dissolve. Keep the `agent/mod.rs` re-exports stable so the 7 integration test files' `archimedes_lib::agent::EventSink` paths survive.

#### 5. Model-key / wire-API / thinking-level vocabulary is stringly-typed and scattered
- **Lens:** Weak Abstractions
- **Files:** `agent/harness/{catalog,provider}.rs`, `agent/session.rs`, `agent/subagent.rs`, `agent/worker/manager.rs`, `commands/settings.rs`, `src/lib/toolOutput.ts`, `src/components/settings/SettingsPage.tsx`
- **Severity:** High
- **Confidence:** High
- **Problem:** Three related concepts have no named types: (a) **model key** — composed `format!("{}/{}")` at 9 sites, split `split_once('/')` at 2, no `ModelKey` type (the "model ids contain `/`" ambiguity is documented in a comment only); (b) **wire API** — the 3 ADR 0024 literals free-floating in 5 Rust files (80 occurrences incl. tests; ~25 production, 18 in the `KNOWN_PROVIDERS` table) + 2 TS files, no shared constant; (c) **thinking-level resolution** — the ADR 0023 chain (explicit > settings override > frontmatter > parent) is implemented **twice** (`worker/manager.rs:782ff` and `loop.rs:1326ff` `resolve_launch`), each re-inlining the `rsplit_once(':')` suffix strip (3 sites, identical 4-line idiom) and the `is_empty() || iter().any(|t| t == level)` membership predicate (5 sites, 2 inverted). A new wire (the obvious ADR 0024 follow-up) or a 4th model-key rule means touching 6+ files blind.
- **Proposal:** (1) `enum WireApi` (serde rename — `settings.json` format unchanged) used by `ProviderConfig.api`, `Model.api`, `build_provider` (exhaustive match), `selectable()`, settings validation, + a TS const mirror; (2) `ModelKey` newtype with a `parse()` encoding the documented split rule, replacing the 9 `format!`/2 `split_once` pairs; (3) `Model::supports_thinking_level()` replacing the 5 predicates; (4) one `resolve_model_ref(...)` implementing the ADR 0023 chain once, shared by both dispatch sites (both degrade on stale values per ADR 0020/0023 — a policy parameter keeps the strict/degrading distinction if it exists).

### 🟡 Medium Severity

#### 6. `agent/interactive.rs` — 2,787 lines, 3 unrelated concerns
- **Lens:** File Length + Structure
- **Files:** `src-tauri/src/agent/interactive.rs`
- **Severity:** Medium
- **Confidence:** High
- **Problem:** ~1,160 production lines: real-sudo **process execution** (125–503: `RealSudoRunner`, `run_sudo_real`, process-group kill/reap, stream reading — no interactive-channel dependency), argv/secret helpers (504–652), `todo_apply` (707–881, an unrelated suite tool), the interactive **sudo flow** (882–1163), shared keying types (55–102). 58% tests.
- **Proposal:** Extract `interactive/sudo_exec.rs` (125–503) + `interactive/sudo_argv.rs` (504–652); move `todo_apply` to `agent/todo.rs`. File drops to ~600 production lines (the flow — the file's identity).

#### 7. Frontend god files: `SettingsPage.tsx` (1,607), `ChatStream.tsx` (1,255), `braille-loader.ts` (1,279), `sessions.ts` (1,222)
- **Lens:** File Length + Structure
- **Files:** `src/components/settings/SettingsPage.tsx`, `src/components/ChatStream.tsx`, `src/lib/braille-loader.ts`, `src/store/sessions.ts`
- **Severity:** Medium
- **Confidence:** High
- **Problem:** `ChatStream.tsx` is a **single 1,186-line component** (94% of the file, 24 imports — the de-facto composition god: 9 sibling components, 4 stores, 5 hooks) with 4 unextracted regions (skills ` $`-picker + send-path expansion ~310 ln, image-attachment handlers ~190 ln, message rendering + ask/confirm/password cards + composer + modals ~380 ln). `SettingsPage.tsx`: ~760-line main component rendering all 5 sections inline, with 4 extractable sub-component blocks (MCP 210 ln, `McpDialog` 158 ln, `ProviderRow` 155 ln, small controls 167 ln). `braille-loader.ts`: 27 animation variants inlined in one 1,100-line `VARIANT_CONFIGS` blob (86% of the file; each variant is a self-contained object). `sessions.ts`: ~600-line store object with ~25 actions (49% of the file) + 17 importers (the FE's largest coupling sink; components read the store directly rather than via props).
- **Proposal:** `ChatStream` → extract `ComposerSkills`/`AttachmentStrip`/`MessageList`/`ComposerModals` (the regions are already marker-commented). `SettingsPage` → extract the 4 blocks into `settings/*` sub-components (the seams are clean). `braille-loader` → one file per variant under `lib/braille/variants/` + a registry (or accept the size — it's self-contained and cosmetic; lowest priority of the four). `sessions.ts` → split action groups into separate stores or a `sessions/actions/` module only if the store exceeds ~1,500; the 17-importer sink is a *pattern* choice (zustand direct-read), not a defect — defer.

#### 8. Inverted layering: `agent` → `commands::settings` and `storage` → `agent::SessionInfo`; no repository seam over `Db`
- **Lens:** Coupling
- **Files:** `agent/session.rs:52`, `agent/mcp/manager.rs:19`, `agent/harness/loop.rs:1282ff`, `storage/db.rs:19`, `commands/settings.rs`
- **Severity:** Medium
- **Confidence:** High
- **Problem:** The domain layer imports the IPC layer: `load_settings`/`write_settings` (pure file I/O, zero Tauri in them) live in `commands/settings.rs` and are called from `agent/session.rs` (7 sites incl. the ADR 0015 remembered-thinking-level write), `agent/mcp/manager.rs`, and `agent/harness/loop.rs` (ADR 0023 override read). `storage/db.rs:19` imports `agent::SessionInfo` (storage → domain; one production signature, `Db::record_session`). `commands/settings.rs` (992 ln) is thus a de-facto **config service masquerading as IPC** (45% of it is the `Settings`/`ProviderConfig` serde model + load/write + the 156-line `KNOWN_PROVIDERS` catalog). Separately, the concrete `Arc<Db>` is held by 5 modules with no repository trait (only `harness/store.rs`'s `SessionStore` wraps it, used by 1 module) — a shared-mutable kernel.
- **Proposal:** (1) Move `Settings`/`ProviderConfig`/`FontSettings` + `load_settings`/`write_settings` + the `KNOWN_PROVIDERS` catalog into a `config/` module; `commands/settings.rs` keeps only `#[tauri::command]` wrappers. (2) Move `SessionInfo` to a shared types module (keep the `agent/mod.rs` re-export for the 2 test files + commands). (3) Defer a `Db` repository trait — the SQL is contained and the 5 holders are all in-crate; a trait is YAGNI until a second backend appears.

#### 9. `worker/manager.rs` — 1,987 lines; the subagent-dispatch block is a self-contained state machine
- **Lens:** File Length + Structure
- **Files:** `src-tauri/src/agent/worker/manager.rs`
- **Severity:** Medium
- **Confidence:** High
- **Problem:** 4 concerns: subagent outcome capture (`SubagentCapture` 105–218), manager lifecycle + read-loop pump + crash detection (310–710), the **subagent dispatch machinery** (747–1214, ~470 lines: child model/level resolution — the re-homed ADR 0023 chain — launch resolution, child attach, settle-timeout watchdog, `finish_subagent` graceful-close), config broadcast. 37% tests. It's a *justified* manager, borderline god: the dispatch block only takes `Arc<ManagerState>`, not the manager's identity.
- **Proposal:** Extract `worker/subagent_dispatch.rs` (747–1214) + `worker/capture.rs` (105–218) → ~700-line manager (lifecycle + pump + crash detection). Do this *after* finding 5's `resolve_model_ref` extraction (the dispatch block contains one of the two duplicated resolution chains).

#### 10. TS model-key parsing diverges from Rust; hardcoded context windows ignore the catalog
- **Lens:** Weak Abstractions (Inconsistent Patterns)
- **Files:** `src/lib/toolOutput.ts:160-184`, `src/components/SubagentDelegatingCard.tsx:464,581`
- **Severity:** Medium
- **Confidence:** High
- **Problem:** Rust strips the `:<level>` suffix with `rsplit_once(':')` (last colon); `cleanModelName` uses `model.split(":")[0]` (first colon) — **divergent for model ids containing colons** (TS drops the id tail). `getModelContextWindow` hardcodes context windows by name-substring (gemini→1M, claude→200k, …) and is the *primary* code path in `SubagentDelegatingCard` (the context-usage progress bar), **ignoring the `contextWindow` field the Rust `ModelDto` already computes and sends** — provider knowledge re-implemented client-side, guaranteed to drift.
- **Proposal:** (1) `cleanModelName` → `lastIndexOf(":")` (mirror Rust). (2) Thread `contextWindow` through the `subagent-session-started` payload (it already carries the model) or read it from the catalog via the settings store; keep the hardcoded table only as a no-catalog fallback.

#### 11. MCP DRY cluster: protocol constants + `tools/list` parse duplicated across the two transports; OAuth flow block duplicated
- **Lens:** DRY Violations
- **Files:** `agent/mcp/{stdio,http,callback,oauth,manager}.rs`
- **Severity:** Medium
- **Confidence:** High
- **Problem:** `PROTOCOL_VERSION = "2025-06-18"` + the `initialize` params JSON (`protocolVersion`/`capabilities`/`clientInfo`) + the 17-line `tools/list`→`ToolInfo` parse are **identical** in `stdio.rs` and `http.rs` (the JSON-RPC *request* building is properly shared via `rpc.rs` — this response-shape parse was left duplicated). `url_decode` (17-line byte loop) is duplicated in `callback.rs` (production) and `oauth.rs` (test); the query-string `&`/`=` parse loop is triplicated. `McpManager::authenticate` (231–267) and `authenticate_server` (338–362) share a ~20-line block (open-browser closure → `oauth::authenticate` → load/insert/save credentials).
- **Proposal:** One `mcp::rpc`-level `PROTOCOL_VERSION` + `initialize_params()` helper; move the `tools/list` parse into `mcp/types.rs` (next to the existing `extract_tool_call_result`); one `url_decode` in a shared util; a `run_oauth_flow(name, def, url)` collapsing the two authenticate fns.

#### 12. `skills.rs` / `agents.rs` — `space_roots` walk + frontmatter skeleton duplicated (beyond the ADR-sanctioned `parse_value`)
- **Lens:** DRY Violations
- **Files:** `src-tauri/src/skills.rs`, `src-tauri/src/agents.rs`
- **Severity:** Medium
- **Confidence:** High
- **Problem:** `parse_value` is a sanctioned copy (ADR 0020 — byte-identical, maintenance rule: apply fixes to BOTH). But **two more unflagged copies** exist: `space_roots` (the ~35-line walk-up-to-repo-root discovery walk — identical except the `Scope` type + two path suffixes) and the `parse_*_file` prelude (CRLF normalization, `---` boundary handling, body extraction, top-level key scan — verbatim-parallel). A fix to `---` boundary or CRLF handling in one file will not propagate. ADR 0020's "modules stay independent" was about the frontmatter *shapes*; a shared shape-agnostic *skeleton* with shape-specific key tables is compatible with the ADR's intent but needs an ADR addendum.
- **Proposal:** Extract a `frontmatter.rs` module: `space_roots_with(suffixes)` + the shared parse prelude; `skills.rs`/`agents.rs` keep their shape-specific key tables. Add an ADR 0020 addendum (or a line in the next ADR) recording the shared-skeleton decision.

#### 13. Frontend DRY: message-kind→component dispatch and the `agent-thought`→`Reasoning` wiring are triplicated
- **Lens:** DRY Violations
- **Files:** `src/components/{ChatStream,SubagentTranscript,MessageBubble}.tsx`
- **Severity:** Medium
- **Confidence:** High
- **Problem:** The `units.map` render loop mapping `Message.kind` → component (`agent-text`→`MessageBubble`, `agent-thought`→`Reasoning` + `ReasoningTrigger`/`ReasoningContent` with `autoCollapseKey`, `tool-call`→`ToolCallCard`/`AskQuestionCard`, `diff`→`DiffBlock`) is structurally the same 5-branch dispatch in `ChatStream.tsx:898-950` and `SubagentTranscript.tsx:97-135`, differing in the `isStreaming` derivation (`inTurn && last` vs `entry.status === "running" && last` — the most likely drift point) and ask-anchoring. The `Reasoning` trio wiring is spelled in both `MessageBubble` and `SubagentTranscript` (a third copy via ChatStream).
- **Proposal:** A `<MessageUnit unit isStreaming onAskAnchor>` component collapsing both loops; `SubagentTranscript`'s `agent-thought` case just calls `MessageBubble` (it already does for `agent-text`).

#### 14. No time injection — the permission-gate timeout (the ADR 0010 fail-safe) and all backoff/TTL paths are untestable
- **Lens:** Missing Tests / Testability
- **Files:** `agent/permission.rs:72`, `agent/harness/{retry,loop}.rs`, `agent/interactive.rs`
- **Severity:** Medium
- **Confidence:** High
- **Problem:** `PERMISSION_TIMEOUT = 300s` (the auto-cancel fail-safe of the ADR 0010 permission gate) is a hard constant — **the timeout path has no test** (it would take 5 minutes or a fake clock). `retry.rs` calls `tokio::time::sleep` directly (tests race a real 30 s sleep against cancel); `ASK_TIMEOUT` and the sudo-password TTL are equally non-injectable. The timeout/backoff-duration semantics — the security-relevant edges of the permission + sudo gates — are unverified. (Rest of the test story is strong: the model loop IS testable without a live endpoint via the `Provider` trait + scripted fakes; provider tests run against raw-socket SSE servers; DB migrations are well-tested.)
- **Proposal:** A `Clock` seam in `RetryPolicy` (or a `tokio::time`-independent `sleep` closure) + injectable `PERMISSION_TIMEOUT`/`ASK_TIMEOUT`/TTL constants → test the timeout paths at millisecond scales. This is also the enabler for the other duration-boundary tests.

#### 15. `src/lib/tauri.ts` — the entire frontend↔Rust IPC contract (693 ln, 72 wrappers + 8 listeners) has zero tests
- **Lens:** Missing Tests / Testability
- **Files:** `src/lib/tauri.ts`
- **Severity:** Medium
- **Confidence:** High
- **Problem:** Every command wrapper, camelCase/snake_case mapping, and event-listener signature in the 693-line contract file is untested; a rename on either side is caught only at runtime (pairs with finding 16 — the contract is hand-synced string literals). The `vi.mock('../lib/tauri')` boilerplate is re-stubbed per test file (no shared `mockTauri()` helper).
- **Proposal:** A `tauri.contract.test.ts` that walks the wrapper inventory (command names + a fixture payload per listener) — cheap, high value; add the shared `mockTauri()` helper to `test/`.

#### 16. The Rust↔TS wire contract is 100% hand-synced string/JSON — no typed owner, no codegen, no contract test
- **Lens:** Coupling
- **Files:** `agent/events.rs`, `agent/session.rs:2209-2656` (`normalize`), `agent/worker/manager.rs` (`SubagentCapture`), `src/lib/tauri.ts`, `src/App.tsx:33-100`
- **Severity:** Medium
- **Confidence:** High
- **Problem:** Rust emits 10 string-named Tauri events with untyped `Value` payloads; TS hand-declares 8 payload interfaces + 8 listen wrappers. The payload discriminators (`agent_message_chunk`, `tool_call_update`, …) are `json!` string literals built in `normalize` (session.rs:2278-2541) and re-matched as strings in the TS store (sessions.ts:275-346) + components. `MessageRow` hand-mirrors the SQLite `messages` row (kind values hand-synced with `db.record_message` call sites); `PermissionOutcome` hand-mirrors a Rust serde shape. `SubagentCapture` (worker/manager.rs) **pattern-matches the wire JSON field names** (`payload["update"]["sessionUpdate"]`, `payload["content"]["text"]`) — Rust code knowing the frontend's camelCase field names. The `RpcEvent` enum (257 ln, typed, per-variant `rename_all`) never crosses the Tauri boundary — it's re-serialized to `Value` and `normalize` **re-extracts the same type tags from the Value** (string-matching the enum's own serialized form). A rename in Rust = silent no-op in TS. ADR 0011 froze the `session-update` frame shape (the contract is load-bearing folklore, documented as such in `events.rs:17-22`).
- **Proposal (staged, smallest first):** (1) **Centralize**: one Rust `event_names` const module + a `SessionUpdateKind` serde enum for the discriminators (kills the free-floating literals on both sides; wire format unchanged); (2) **Type `normalize`'s input**: `normalize` stops re-extracting `RpcEvent`'s own type tags — match the enum directly (the harness already built it); (3) **Contract test**: a shared JSON fixture (event names + a payload sample per event) read by a Rust test and a TS test so drift fails CI; (4) **Later**: ts-rs/serde_ts codegen of the typed payload structs into `src/lib/generated/` with a CI sync check. Steps 1–3 are non-breaking; step 4 is the only one that could change the TS surface. Defer (4) until the 0025 worker protocol settles.

#### 17. Duplicated test infrastructure — no shared fake Provider / event recorder / temp-DB helper
- **Lens:** Missing Tests / Testability
- **Files:** `agent/harness/loop.rs:2251ff`, `agent/session.rs:3263ff`, `agent/worker/manager.rs` (tests), `src-tauri/src/test_support.rs`
- **Severity:** Medium
- **Confidence:** High
- **Problem:** The `ScriptedProvider`/`FailingProvider`/recording-`EventSink` fakes are **private to `loop.rs`'s test module**; `session.rs` and the worker tests each re-declare their own `native_test_model`/`native_test_catalog`/recording sinks; each test module re-inlines `temp_config_dir()` + `Db::open(dir/t.db)`; `test_support.rs` (61 ln) offers only `env_lock` + `raw_json_server` (and `catalog.rs` keeps a private duplicate of `raw_json_server` because the shared one is unreachable). Every new test module re-invents the harness.
- **Proposal:** Promote `ScriptedProvider`, the event recorder, and a `temp_db()` helper into `test_support.rs` (make `raw_json_server` public there, delete the `catalog.rs` copy); the per-module fakes become thin wrappers. Pairs with finding 1 (the test-module split should land the fakes in the shared home, not copy them into the new modules).

### 🟢 Low Severity

#### 18. Dead code sweep (Rust + frontend, verified at `ebbe8ed`)
- **Lens:** Dead Code
- **Files:** `agent/harness/loop.rs:440`, `agent/session.rs:813`, `agent/interactive.rs:101`, `agent/mcp/callback.rs:179-186`, `src/lib/version.ts`, `src/lib/tauri.ts:461`, `src/store/interactive.ts:47,53`, `src/lib/braille-loader.ts:32`
- **Severity:** Low
- **Confidence:** High
- **Problem:** Zero-caller `pub` items: `AgentLoop::cancel_turn` (superseded by the worker cancel path), `SessionManager::session_count` (the 9 `*_count` refs are the *different* `WorkerManager::test_session_count`), `TOOL_EXEC_FAST_TIMEOUT` const (its 30 s value is re-inlined literally in a test), `_free_port()` (its own doc comment concedes it's unneeded; the `#[allow(dead_code)]` is the codebase's only two — the other was on the deleted `LiveSession`). Frontend: `lib/version.ts` (whole module — the `app_info` command is registered but never invoked), `closeSession()` wrapper (no UI path closes a session; the `close_session` command is registered but unreachable), `ToolCallDiff`/`TodoUpdatePayload`/`TodoClearPayload` interfaces (orphaned — the live `diff` variant is an anonymous inline type), `speedToDuration` const. (Two pre-0025 dead-code findings were fixed by 0025 and are excluded.)
- **Proposal:** Delete all eight; remove the `app_info` + `close_session` command registrations if the wrappers go (or keep the commands and delete only the wrappers — decide per the "is a close-session UI planned?" question).

#### 19. Naming leftovers from the ADR 0022 bridge→interactive rename + `test_` prefix
- **Lens:** Inconsistent Patterns (Naming)
- **Files:** `agent/harness/loop.rs:179,249,288,1163,1176,1195`, `agent/interactive.rs:15,958`, `agent/session.rs:1654,1658`, `agent/worker/core.rs` (8 sites), `src/store/interactive.test.ts:147,148,167`
- **Severity:** Low
- **Confidence:** High
- **Problem:** ADR 0022 renamed the type `PendingBridge` → `PendingInteractive` but **not the field**: `pending_bridge: PendingInteractive` persists at 20 sites across 5 files (0025 added 8 more in `worker/core.rs`, keeping the old name). A test payload key `bridgeSession` survives in `interactive.test.ts`. Separately, 21 Rust test fns use a `test_` prefix (the convention is bare sentence-style — `#[test]` already marks them) and "fetch data" fns use 5 prefixes (`load_`/`list_`/`get_`/`read_`/`find_`).
- **Proposal:** Rename `pending_bridge` → `pending_interactive` (mechanical, 20 sites); replace `bridgeSession` with a neutral key; drop the `test_` prefix (21 sites); standardize `load_` (single entity from storage) vs `list_` (collection) — leave `read_`/`get_` alone (they read external sources).

#### 20. `Result<Value, ()>` — a third error-handling variant in the sudo flow
- **Lens:** Inconsistent Patterns
- **Files:** `agent/interactive.rs:940,1013`
- **Severity:** Low
- **Confidence:** Medium
- **Problem:** The layer convention is `thiserror` enums in domain code + `String` at the IPC boundary; the two `sudo_run_flow`-adjacent fns return `Result<Value, ()>` — a unit error that discards the failure reason the flow is trying to report.
- **Proposal:** `Result<Value, String>` (or a small `SudoFlowError` thiserror) so the error text survives to the event payload.

## Top Recommendation

**Tackle the god files as a staged decomposition, starting with `provider.rs`** — it's the most isolated (2 local imports, no cycle participation) and its split is the cleanest (3 self-contained provider stacks with seams already at 1633/1953). Suggested order, one squash-merge commit each (AGENTS.md convention):

1. **Vocabulary first (finding 5, step 1):** `enum WireApi` + `ModelKey` — small, non-breaking (serde rename keeps the wire format), and it makes the provider split's `build_provider` exhaustive instead of a string match.
2. **`provider.rs` 3-way split + shared `transport.rs`** (findings 2 + the DRY ride-along) — kills the triplicated status-map/client/header blocks.
3. **`loop.rs` extractions** (finding 1): `launch.rs` → `ask.rs` → dispatch merge → `compact.rs` merge, test modules splitting with the code (fakes promoted to `test_support` per finding 17).
4. **`session.rs` + the cycle break** (findings 3 + 4): `normalize.rs` + `session_router.rs` + `EffectiveCatalog` extraction, `EventSink`/`TurnState` to the neutral hub, `agent/mod.rs` re-exports kept stable.
5. **Layering fix** (finding 8): `config/` module + `SessionInfo` move.
6. **Dead-code + naming sweep** (findings 18, 19) — one hygiene commit.

`interactive.rs` (6) and `worker/manager.rs` (9) can slot in after 3; the frontend (7, 13) is an independent track; the wire contract (16, 15) is staged 1–3 now, codegen deferred.
