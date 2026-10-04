---
status: committed
done-when: The harness's god files (`harness/loop.rs`, `harness/provider.rs`, `session.rs`, `interactive.rs`) and the frontend god files (`ChatStream.tsx`, `SettingsPage.tsx`) are decomposed into per-concern modules; the 4-way `session⇄loop⇄subagent⇄interactive` cycle is broken; the wire-API + model-key + thinking-level vocabulary is a named type (no free-floating literals); the `agent`→`commands::settings` and `storage`→`agent::SessionInfo` layer inversions are fixed. NO behavior change — `pnpm test` + `pnpm build`, `cargo test` + `cargo clippy --all-targets` (0 warnings) + `cargo fmt --check` all green, and the wire format / `settings.json` / event contract are byte-identical to before.
---

# God-file decomposition plan

**Goal:** Decompose the oversized god files and the structural problems they encode (module cycle, stringly-typed model/wire vocabulary, inverted layering) into per-concern modules with no behavior change.

**Architecture:** A staged, dependency-ordered refactor of the Rust agent core (`src-tauri/src/agent/`) and the frontend. Each task is independently committable and leaves the full validation gate green. The order is chosen so no two tasks edit the same lines: the model/wire vocabulary (Tasks 1–2) lands first because it makes later splits exhaustive; the isolated files (provider, interactive) split next; the two big coupled files (loop, session+cycle) split after their vocabulary is in place; the layering fix and the frontend run last.

**Tech Stack:** Rust (Tauri 2 backend, `src-tauri/`), React 19 + TypeScript + zustand (frontend, `src/`). Validation gate: `pnpm test` + `pnpm build` (repo root), `cargo test` + `cargo clippy --all-targets` + `cargo fmt --check` (in `src-tauri/`).

**Conventions (from AGENTS.md):** TDD (failing test first where there is NEW behavior; for pure moves the existing suite is the safety net — run it green before and after). Squash-merge, one commit per task. Durable decisions go to `docs/decisions/`; this in-flight plan is deleted on ship.

**Review status (2026-10-04):** The plan-execution reviewer subagents timed out (3 retries — subagent infra was flaky during this session; the same wave also lost 5 of 6 in the earlier deep-dive phase). In their place, the following load-bearing claims were verified **directly** against the tree: the Task 5 cycle-break import-site list (all `use crate::agent::session::` sites enumerated above — complete), `resolve_composed_model` staying in `session.rs`, the ChatStream render region being hook-free pure JSX with its exact closure set (Task 8a), `store.rs` not actually importing session items (doc comment only), and the per-file anchors for every task (line ranges from verified greps). **Before executing, run a fresh plan-execution review** (the specify skill's reviewer prompt) against Tasks 1, 3, 5, 6 — those are the ones whose details were verified by spot-check rather than full-file read.

**Scope guard:** This is a *move + rename + introduce-a-named-type* refactor. Do NOT change any wire format, `settings.json` field name, event name, payload shape, SQL schema, or user-visible behavior. If a task requires a behavior change to land cleanly, STOP and surface it — do not improvise.

---

### Task 1: Introduce the `WireApi` enum (kill the 3 free-floating wire literals)

**Context:** The three provider wire APIs (`openai-completions`, `anthropic-messages`, `openai-responses`, ADR 0024) are currently free-floating `&str` literals in 5 Rust files + 2 TS files (~80 occurrences, 18 of them in the `KNOWN_PROVIDERS` table). There is no shared constant, so a fourth wire or a rename means a blind multi-file search, and a typo in one `KNOWN_PROVIDERS` entry is only caught by one round-trip test. This task introduces a single `enum WireApi` (serde-renamed so the on-disk and on-wire strings are UNCHANGED) used by every site. It is the vocabulary foundation for Task 3 (the `build_provider` match becomes exhaustive). It is non-breaking: `#[serde(rename_all = "kebab-case")]` keeps `settings.json` byte-identical.

**Files:**
- Create: `src-tauri/src/agent/harness/wire.rs`
- Modify: `src-tauri/src/agent/harness/catalog.rs` (`Model.api`), `src-tauri/src/agent/harness/mod.rs` (re-export), `src-tauri/src/agent/harness/provider.rs` (`build_provider`), `src-tauri/src/agent/session.rs` (`unwrap_or("openai-completions")` at ~line 712), `src-tauri/src/commands/settings.rs` (`ProviderConfig.api`, `default_provider_api`, `KNOWN_PROVIDERS`, the validation match at ~569), `src/lib/tauri.ts` (`api` field type), `src/components/settings/SettingsPage.tsx` (the `WIRE_APIS`/`apiLabel` constants at ~92-100).
- Test: `src-tauri/src/agent/harness/wire.rs` (inline `#[cfg(test)]`), `src/components/settings/SettingsPage.test.tsx` (the `api` pre-fill assertions).

**What to implement:**
1. `src-tauri/src/agent/harness/wire.rs`:
   ```rust
   /// The provider wire API a model speaks (ADR 0024). Serde-renamed so the
   /// on-disk / on-wire values stay the existing kebab-case strings.
   #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
   #[serde(rename_all = "kebab-case")]
   pub enum WireApi {
       OpenAiCompletions,   // "openai-completions" (default)
       AnthropicMessages,   // "anthropic-messages"
       OpenAiResponses,     // "openai-responses"
   }
   impl Default for WireApi {
       fn default() -> Self { WireApi::OpenAiCompletions }
   }
   impl WireApi {
       pub const ALL: [WireApi; 3] = [
           WireApi::OpenAiCompletions,
           WireApi::AnthropicMessages,
           WireApi::OpenAiResponses,
       ];
       /// The three supported wires (the `selectable()` set, ADR 0024).
       pub fn is_supported(self) -> bool { Self::ALL.contains(&self) }
   }
   ```
   Add `pub mod wire;` + `pub use wire::WireApi;` to `src/agent/harness/mod.rs`.
2. `catalog.rs`: `Model.api: Option<String>` → `Model.api: Option<WireApi>` (keep `#[serde(default)]` if present). `selectable()` (lines ~116-118): replace the `matches!(api, Some("anthropic-messages") | Some("openai-responses") | Some("openai-completions"))` with `matches!(m.api, Some(WireApi::OpenAiCompletions) | Some(WireApi::AnthropicMessages) | Some(WireApi::OpenAiResponses))` (or `m.api.map_or(false, WireApi::is_supported)`). `discover_models` (~227): the branch that checks `api == "anthropic-messages"` becomes `api == WireApi::AnthropicMessages`.
3. `provider.rs::build_provider` (~2187): the `match api { Some("anthropic-messages") => …, Some("openai-responses") => …, _ => OpenAiCompatibleProvider }` becomes an exhaustive `match m.api { Some(WireApi::AnthropicMessages) => …, Some(WireApi::OpenAiResponses) => …, _ => … }`.
4. `session.rs` (~712): `unwrap_or("openai-completions")` → `unwrap_or(WireApi::OpenAiCompletions)` (adjust the surrounding expression to carry a `WireApi`).
5. `commands/settings.rs`: `ProviderConfig.api: String` → `ProviderConfig.api: WireApi` (keep `#[serde(default)]`); `default_provider_api()` returns `WireApi::OpenAiCompletions`; every `api: "openai-completions"` / `api: "anthropic-messages"` / `api: "openai-responses"` in `KNOWN_PROVIDERS` (lines ~322-477) → `api: WireApi::OpenAiCompletions` etc.; the validation match (~569) → `WireApi` match.
6. Frontend: `src/lib/tauri.ts` — `api: string` → `api: "openai-completions" | "anthropic-messages" | "openai-responses"` (a named `export type WireApi = …` union). `SettingsPage.tsx` — the `WIRE_APIS` array + `apiLabel` if-chain (~92-100) become a single `const WIRE_APIS: WireApi[] = […]` and a `Record<WireApi, string>` label map; the default `api: template?.api ?? "openai-completions"` (~1031) stays but typed.

**What NOT to change:** the JSON strings on the wire / in `settings.json` (serde rename guarantees this — verify with the round-trip test), the `Model.api` `Option` vs `ProviderConfig.api` non-`Option` distinction (they are two different encodings — one optional, one defaulted; keep both), anything in `build_provider`'s provider construction beyond the match discriminant.

**Steps:**
- [ ] Write failing tests in `wire.rs` `#[cfg(test)]`: `WireApi::OpenAiCompletions` serializes to `"openai-completions"` (and all three); `#[serde(default)]` on a missing `api` field yields `OpenAiCompletions`; `is_supported()` true for all three. Run `cargo test wire` — confirm they fail (type doesn't exist yet).
- [ ] Implement the enum + `wire` module + re-export. Run `cargo test wire` — green.
- [ ] Update `catalog.rs` (`Model.api`, `selectable`, `discover`). Run `cargo test catalog` + `cargo test selectable` — green (the existing selectable/discovery tests must still pass — they assert on the same models).
- [ ] Update `provider.rs::build_provider` + `session.rs`. Run `cargo test build_provider` + `cargo test` — green.
- [ ] Update `commands/settings.rs`. Run `cargo test` in `settings` — the `KNOWN_PROVIDERS` round-trip test (the one at ~569 that asserts `api` round-trips) must still pass. Green.
- [ ] Update the TS type + `SettingsPage.tsx`. Run `pnpm build` (type-check) + `pnpm test` — the `api` pre-fill test in `SettingsPage.test.tsx` must still pass. Green.
- [ ] Run `cargo fmt`. Then the full gate: `cd src-tauri && cargo test && cargo clippy --all-targets` (0 warnings) and `pnpm test && pnpm build`.
- [ ] Commit: "refactor: introduce WireApi enum for the 3 provider wires (ADR 0024 vocabulary)"

**Acceptance criteria:**
- [ ] `grep -rn '"openai-completions"\|"anthropic-messages"\|"openai-responses"' src-tauri/src` returns ONLY serde/test/doc occurrences — no production `match`/`if` discriminants on the raw strings (the `KNOWN_PROVIDERS` table + `build_provider` + `selectable` + `discover` all use `WireApi`).
- [ ] A `settings.json` written before this commit parses identically after (the serde default + rename preserve it) — verified by the existing round-trip test.
- [ ] `cargo clippy --all-targets` 0 warnings (the exhaustive `match` now satisfies clippy's non-exhaustive-literal heuristic).
- [ ] Frontend `WireApi` union type is the single source for the `api` field; `SettingsPage.test.tsx` green.

---

### Task 2: `ModelKey` newtype + `Model::supports_thinking_level` + a single `resolve_model_ref`

**Context:** The composed model key (`provider/id`), the `:<level>` thinking suffix, and the ADR 0023 model-resolution chain are all string fiddling inlined across 7 files. Concretely: the key is composed `format!("{}/{}", provider, id)` at 9 sites and split `split_once('/')` at 2 with no `ModelKey` type (the "model ids contain `/`" rule is a comment only); the `:<level>` suffix is stripped `rsplit_once(':')` at 3 identical 4-line sites (`loop.rs` ~1370/1381, `worker/manager.rs` ~782); the thinking-level membership predicate `levels.is_empty() || levels.iter().any(|l| l == level)` is inlined at 5 sites (2 inverted); and the ADR 0023 chain (explicit > settings `subagentModels` override > frontmatter > parent) is implemented **twice** — `loop.rs::resolve_launch` (degrading, reads settings) and `worker/manager.rs` child-model-resolution (strict, the child fails on unknown). This task names the key, names the predicate, and unifies the bare-key strip + predicate into one shared home. The full ADR 0023 *chain* unification is intentionally scoped as a sub-step with a policy parameter, because the two sites differ (strict vs degrading) and ADR 0020/0023 forbid a stale value failing a *frontmatter/override* dispatch.

**Files:**
- Create: `src-tauri/src/agent/harness/model_key.rs`
- Modify: `src-tauri/src/agent/harness/catalog.rs` (`Model::supports_thinking_level`), `src-tauri/src/agent/harness/mod.rs` (re-export), `src-tauri/src/agent/session.rs` (the 2 `split_once` + 3 predicate sites + `resolve_composed_model` at 2042), `src-tauri/src/agent/harness/loop.rs` (`resolve_launch` ~1326-1424 — the 2 `rsplit_once` + predicate), `src-tauri/src/agent/worker/manager.rs` (the child-model block ~760-860 — the `rsplit_once` + predicate), `src/lib/toolOutput.ts` (`cleanModelName` colon split ~160-167 → mirror Rust's last-colon).
- Test: `src-tauri/src/agent/harness/model_key.rs` (inline), `src/lib/toolOutput.test.ts`.

**What to implement:**
1. `model_key.rs`:
   ```rust
   /// A composed model key `<provider>/<id>`, plus an optional `:<level>`
   /// thinking-level suffix (a `ModelKey` carries the bare key; the level is
   /// a `ModelRef`). Provider ids do not contain `/`; model ids MAY, so the
   /// key splits on the FIRST `/` (the documented rule, `resolve_composed_model`).
   #[derive(Debug, Clone, PartialEq, Eq, Hash)]
   pub struct ModelKey { provider: String, id: String }
   impl ModelKey {
       pub fn new(provider: impl Into<String>, id: impl Into<String>) -> Self { … }
       pub fn parse(s: &str) -> Option<Self> {
           let (p, i) = s.split_once('/')?;
           ( !p.is_empty() ).then(|| ModelKey { provider: p.into(), id: i.into() })
       }
       pub fn to_string(&self) -> String { format!("{}/{}", self.provider, self.id) }
   }
   /// A model key + optional thinking level (`provider/id:level`).
   pub struct ModelRef { pub key: ModelKey, pub level: Option<String> }
   impl ModelRef {
       /// Splits on the LAST `:` (mirrors `rsplit_once(':')` — a model id may
       /// contain `:`, so the level is always the trailing segment).
       pub fn parse(s: &str) -> Option<Self> {
           let (bare, level) = match s.rsplit_once(':') {
               Some((b, l)) => (b, Some(l.to_string())),
               None => (s, None),
           };
           Some(ModelKey::parse(bare)?.then(|k| ModelRef { key: k, level }))
       }
   }
   ```
   Re-export from `harness/mod.rs`.
2. `catalog.rs`: add `pub fn supports_thinking_level(&self, level: &str) -> bool { self.thinking_levels.is_empty() || self.thinking_levels.iter().any(|l| l == level) }`. Replace the 5 inlined predicates with calls to it: `session.rs` ~994-995, ~1004-1005, ~1466-1467, `remembered_thinking_level` ~2056-2061, `worker/manager.rs` ~828 (the inverted form becomes `!model.supports_thinking_level(level)`).
3. `session.rs::resolve_composed_model` (2042): keep the signature but implement the split via `ModelKey::parse` (the behavior is identical — first `/`). The 2 `split_once('/')` sites at ~1436 (set_config_option "model") and 2043 become `ModelKey::parse`. The 9 `format!("{}/{}", provider, id)` sites become `ModelKey::new(provider, id).to_string()`.
4. **Single bare-key strip:** the 3 `rsplit_once(':')` sites (`loop.rs` ~1370/1381, `worker/manager.rs` ~782) all become `ModelRef::parse(m)` and read `.key` (resolvability) + `.level` (the thinking candidate). This is the core DRY win — one parse, one rule.
5. **ADR 0023 chain (scoped):** Introduce `pub enum ResolvePolicy { Degrade, Strict }` and `pub fn resolve_model_ref(catalog: &ModelCatalog, explicit: Option<&ModelRef>, override_model: Option<&str>, frontmatter: Option<&ModelRef>, parent_key: &str, policy: ResolvePolicy) -> …`. The two call sites (`loop.rs::resolve_launch` with `Degrade`, `worker/manager.rs` child-resolution with `Strict`) keep their current strict/degrading outcomes but route through the shared helper for the bare-key strip + predicate. **If the two sites' exact layering cannot be expressed by one helper without changing behavior, implement only steps 1–4 (the key + predicate + strip) and leave the two chain sites as-is with a `// NOTE: ADR 0023 chain — see resolve_model_ref (not yet unified)` — do NOT force a unification that changes strict/degrading semantics.** Surface which path you took in the commit message.
6. Frontend `toolOutput.ts::cleanModelName` (~160-167): `model.split(":")[0]` / `parts[parts.length-1]` → `const i = model.lastIndexOf(":"); const bare = i >= 0 ? model.slice(0, i) : model; const level = i >= 0 ? model.slice(i+1) : undefined;` (mirror Rust's last-colon). Do NOT touch `getModelContextWindow`'s hardcoded table here (that's finding 10, not in this plan).

**What NOT to change:** the strict-vs-degrading *outcomes* (a stale frontmatter/override still degrades — ADR 0020/0023; an explicit `model` param still fails on unknown). `getModelContextWindow` (out of scope). Any wire payload.

**Steps:**
- [ ] Write failing tests in `model_key.rs`: `ModelKey::parse("tama/m-1")` → `provider "tama"`, `id "m-1"`; `parse("a/b/c")` → `provider "a"`, `id "b/c"` (first-`/` rule); `parse("noid")` → `None`; `ModelRef::parse("tama/m-1:high")` → key `tama/m-1` + level `high`; `ModelRef::parse("weird:id:high")` → bare `weird` … (document the id-with-colon case per the last-`:` rule). Run `cargo test model_key` — fail first.
- [ ] Implement `model_key.rs` + re-export. Green.
- [ ] Add `Model::supports_thinking_level` + a test (empty set soft-passes; a level in the set passes; a level not in the set fails). Replace the 5 predicate sites. Run `cargo test` — green (the existing thinking-level tests must pass unchanged).
- [ ] Convert the 2 `split_once` + 9 `format!` + 3 `rsplit_once` sites to `ModelKey`/`ModelRef`. Run `cargo test` — green.
- [ ] Attempt the `resolve_model_ref` unification (step 5). Run `cargo test resolve_launch` (the ~19 `resolve_launch_*` tests) + the subagent dispatch tests — green. If it changes behavior, revert to steps 1–4 only and note it.
- [ ] Update `toolOutput.ts` + `toolOutput.test.ts` (add a case for an id containing a colon). Run `pnpm test` + `pnpm build` — green.
- [ ] `cargo fmt`; full gate. Commit: "refactor: ModelKey/ModelRef newtype + Model::supports_thinking_level + shared ADR 0023 bare-key strip"

**Acceptance criteria:**
- [ ] `grep -rn "rsplit_once(':')" src-tauri/src` → 0 production hits (all via `ModelRef::parse`).
- [ ] `grep -rn "split_once('/')\|format!(\"{}/{}\"" src-tauri/src` (non-test) → 0 hits (all via `ModelKey`).
- [ ] `supports_thinking_level` has exactly 5 callers (the old predicate sites); no `is_empty() || iter().any` thinking predicate remains inline.
- [ ] The 19 `resolve_launch_*` tests + subagent dispatch tests pass unchanged (no strict/degrading behavior shift).
- [ ] `toolOutput.test.ts` has a colon-in-id case; `pnpm test` green.

---

### Task 3: Split `harness/provider.rs` into per-provider modules + a shared `transport.rs`

**Context:** `provider.rs` (4,599 lines) holds three full provider stacks — OpenAI-completions (`SseStream` 360-640 + `request_body` 1549 + `impl Provider` 1447-1524), Anthropic (`AnthropicStream` 708-1045 + mappers 1647-1810 + `impl Provider` 1881-1952), OpenAI-Responses (`ResponsesStream` 1045-1446 + `responses_*` 1953-2066 + `impl Provider` 2132-2186) — plus shared wire types/trait (30-280) and one 2,393-line `mod tests` (2210-4599). ADR 0024 chose three separate providers because the *wire shapes* differ (justified), but the *transport discipline* — `build_client(30s, 5min)` (1527), the ~13-line HTTP-status→`ProviderError` map (401/403→Auth, 429/5xx→Retryable, 4xx→Fatal) at 1497/1926/2172, and the `x-litellm-session-id` header block (1476/1908/2148) — is wire-agnostic and is **triplicated verbatim**. This task splits by provider and pulls the shared transport into one helper. It is the most isolated file (2 local imports, no cycle participation), so it's a safe early win. Task 1's `WireApi` is already in place, so `build_provider`'s match is exhaustive.

**Files:**
- Create: `src-tauri/src/agent/harness/provider/{mod.rs, types.rs, transport.rs, openai.rs, anthropic.rs, responses.rs}`
- Delete: `src-tauri/src/agent/harness/provider.rs` (replaced by the directory)
- Modify: `src-tauri/src/agent/harness/mod.rs` (the `pub use provider::{…}` list must keep exporting the same public names), `src-tauri/src/agent/harness/catalog.rs` (`normalize_anthropic_base_url`/`anthropic-version` literal if it moves).
- Test: the `mod tests` (2210-4599) splits into per-provider `#[cfg(test)]` modules in the new files; the shared transport gets its own tests.

**What to implement:**
1. `provider/mod.rs`: `pub mod types; pub mod transport; pub mod openai; pub mod anthropic; pub mod responses;` + re-export the same public names the old file exposed (`build_provider`, `Provider`, `ProviderError`, `ProviderEvent`, `ModelRequest`, `ChatMessage`, `ChatRole`, `MessageContent`, `ToolSpec`, `ToolCall`, `ToolCallDelta`, `ModelOptions`, `Usage`, `FinishReason`, `AnthropicProvider`, `OpenAiCompatibleProvider`, `OpenAiResponsesProvider`) so `harness/mod.rs`'s `pub use provider::{…}` is unchanged.
2. `provider/types.rs`: the wire types + trait (old lines 30-280: `ModelRequest`, `MessageContent`, `ChatRole`, `ChatMessage`, `ToolSpec`, `ToolCall`, `ToolCallDelta`, `ModelOptions`, `Usage`, `FinishReason`, `ProviderEvent`, `ProviderError`, `trait Provider`).
3. `provider/transport.rs`: `build_client` (old 1527) + a `pub(crate) fn send_stream_request(client, url, headers, body) -> Result<impl Stream<…>, ProviderError>` (or the exact shape that fits the current `complete()` flow) that owns: the `build_client(30s, 5min)` construction, the 4-arm HTTP-status→`ProviderError` map (401/403→`Auth`, 429/5xx→`Retryable`, other 4xx→`Fatal`, else→`Fatal`), and the `x-litellm-session-id`/`x-request-id` header block. The `anthropic-version: "2023-06-01"` literal (old provider.rs 1905) and its copy in `catalog.rs` (~298) become one shared `const ANTHROPIC_API_VERSION: &str = "2023-06-01";` in `transport.rs` (or `types.rs`) used by both.
4. `provider/openai.rs`: `LineAssembler` (318), `SseStream` (360-640), `map_finish_reason` (527), `parse_arguments` (628), `OpenAiCompatibleProvider` (293), `request_body` (1549), `impl Provider for OpenAiCompatibleProvider` (1447-1524) — its `complete()` now calls the shared `transport` for client/status/headers and keeps only the URL suffix + `request_body` + `SseStream` wiring.
5. `provider/anthropic.rs`: `AnthropicStream` (708-1045), the mappers (`anthropic_block_wire` 1647, `anthropic_tool_result_content` 1664, `merge_content` 1678, `anthropic_messages` 1717, `anthropic_thinking` 1794, `anthropic_request_body` 1812), `AnthropicProvider` (1633), `map_anthropic_stop_reason` (541), `map_anthropic_error` (598), `normalize_anthropic_base_url` (566), `impl Provider for AnthropicProvider` (1881-1952).
6. `provider/responses.rs`: `ResponsesStream` (1045-1446), `responses_*` mappers (1953-2066), `OpenAiResponsesProvider` (2049), `impl Provider for OpenAiResponsesProvider` (2132-2186).
7. `build_provider` (old 2187) → `provider/mod.rs` (or `types.rs`), using `WireApi` (Task 1) for an exhaustive match.
8. Split `mod tests` (2210-4599) per provider (the existing comments already group them: object-safety/seam, the OpenAI SSE + request-body group, the `responses_stream_bytes`/Responses group at ~3784/4079, the Anthropic group). Shared-transport tests (the idle `read_timeout` group at ~3226, the no-`User-Agent` note at ~3353) go in `transport.rs`.

**What NOT to change:** the SSE parsing logic, the request-body wire shapes (ADR 0024 froze them — the per-provider request-body tests are the safety net), the `Provider` trait signature, `build_provider`'s public behavior. This is a pure move + de-dup of the transport.

**Steps:**
- [ ] Baseline: `cd src-tauri && cargo test provider` — record the count (all green).
- [ ] Create `provider/` dir; move types (30-280) → `types.rs`; move `build_provider` + the mod re-exports → `mod.rs`. `cargo build` — fix only import paths.
- [ ] Extract `transport.rs` (`build_client` + the status→error map + session-id headers + `ANTHROPIC_API_VERSION`). Add a failing test: the status map returns `Auth` for 401/403, `Retryable` for 429/5xx, `Fatal` for other 4xx. Run `cargo test transport` — green.
- [ ] Move the OpenAI stack → `openai.rs`, Anthropic → `anthropic.rs`, Responses → `responses.rs`; each `complete()` now calls the shared transport. Move the per-provider test groups with them.
- [ ] Run `cargo test provider` — the count must match the baseline (every raw-socket SSE + request-body test still passes). Green.
- [ ] Delete the old `provider.rs`. `cargo fmt`. Full gate (`cargo test && cargo clippy --all-targets` + `pnpm test && pnpm build`).
- [ ] Commit: "refactor: split provider.rs into per-provider modules + shared transport (de-dup 3× status-map/client/headers)"

**Acceptance criteria:**
- [ ] `provider.rs` is gone; `provider/{mod,types,transport,openai,anthropic,responses}.rs` exist, each < ~1,200 lines (production).
- [ ] `grep -rn "401 || .*403\|Retryable\|x-litellm-session-id" provider/` — the status-map + header block appear ONCE (in `transport.rs`), not three times.
- [ ] `ANTHROPIC_API_VERSION` is a single const; `grep -rn "2023-06-01"` → 1 definition + usages only.
- [ ] `cargo test provider` count == baseline; `cargo clippy` 0 warnings; `harness/mod.rs` public surface unchanged (no other file's imports break).

---

### Task 4: Split `harness/loop.rs` (extract `launch.rs`, `ask.rs`, merge dispatch + compaction)

**Context:** `loop.rs` (6,432 lines, ~71% tests) mixes 10 concerns. Four are cleanly extractable without touching the turn core: (a) `resolve_launch` (~1326-1485, the ADR 0020/0023 agent-definition layering — a pure function of `LaunchConfig`/`ModelCatalog`/settings with no loop state) + its ~15 `resolve_launch_*` tests → `launch.rs`; (b) the ask-flow (`ask_flow` 1171 + `AskQuestionResult` 2087 + `response_to_results` 2103 + `shape_ask_result` 2166 + `build_ask_session_content` 2204 + `selection_summary` 2222 + `question_context` 2249, all pure or near-pure) → `ask.rs`; (c) the subagent-dispatch half (`is_subagent_call` 1224, `is_enabled_subagent_call` 1232, `dispatch_subagent` 1240) → the existing `dispatch.rs` (or `agent/subagent.rs`); (d) `run_compaction` (1486) + `summarize` (1586) → the existing `compact.rs`. After the moves, `loop.rs` keeps the `AgentLoop` struct, the pump `run` (467), the `handle_prompt` turn core (536-977 — it *is* the loop, stays), settle/cancel, the permission gate, and persistence/emission, dropping to ~900 production lines. Task 2 already simplified `resolve_launch` (bare-key strip via `ModelRef`), so this task moves the already-slimmed function.

**Files:**
- Create: `src-tauri/src/agent/harness/launch.rs`, `src-tauri/src/agent/harness/ask.rs`
- Modify: `src-tauri/src/agent/harness/loop.rs` (remove the moved fns + their tests, add `use`), `src-tauri/src/agent/harness/dispatch.rs` (or `agent/subagent.rs`), `src-tauri/src/agent/harness/compact.rs`, `src-tauri/src/agent/harness/mod.rs` (re-exports if any moved item was `pub`).
- Test: the moved test fns travel with their code.

**What to implement:**
1. `launch.rs`: move `resolve_launch` (1326-1485) as `pub(crate) fn resolve_launch(launch: &LaunchConfig, agent_name: &str, catalog: &ModelCatalog, config_dir: Option<&Path>, space_cwd: &Path) -> LaunchConfig` (it currently reads `self.catalog`/`self.config_dir`/`self.space_cwd` — pass them as args since it's a pure fn; if that's awkward, keep it as a method on a small `LaunchResolver` struct that takes those three). Move its ~15 `resolve_launch_*` tests (grep `fn resolve_launch_` in the test module).
2. `ask.rs`: move the ask-shaping block (2087-2281: `AskQuestionResult`, `response_to_results`, `shape_ask_result`, `build_ask_session_content`, `selection_summary`, `question_context`) — they're pure fns, zero loop state. `ask_flow` (1171) is a method (uses `self.pending_bridge` oneshot) — keep it in `loop.rs` but have it call the pure `ask.rs` shapers. Move the `shape_ask_result_*` tests (6609, 6630) with the shapers.
3. Subagent dispatch: move `is_subagent_call` (1224) + `is_enabled_subagent_call` (1232) + `dispatch_subagent` (1240) to `dispatch.rs` (the `SubagentDispatcher` home) or `agent/subagent.rs`. `dispatch_subagent` uses `self.subagent` (the dispatcher) + `self.model` + `resolve_launch` — pass as args or make it a free fn taking those.
4. `compact.rs`: move `run_compaction` (1486) + `summarize` (1586) (they use `self.compactor`/`self.catalog`/`self.model`/`self.store` — pass as args or take `&AgentLoop`-fields via a small trait). Keep the `compact()` method (461) + `Compactor` where they are.
5. Update `loop.rs` imports + `harness/mod.rs` re-exports for any moved `pub`/`pub(crate)` item.

**What NOT to change:** `handle_prompt` (536-977) stays the turn core — do NOT split it further in this task (its 6 subsystems are interleaved; a deeper split is a separate, riskier effort). `run` (467), the `AgentLoop` struct, `new`, settle/cancel, the permission gate, `emit`/`persist_*` all stay in `loop.rs`. No behavior change.

**Steps:**
- [ ] Baseline: `cargo test loop` — record the count.
- [ ] Move `resolve_launch` + its tests → `launch.rs`. `cargo test launch` + `cargo test loop` — green (count preserved).
- [ ] Move the ask shapers + `shape_ask_result_*` tests → `ask.rs`; `ask_flow` calls them. Green.
- [ ] Move the subagent-dispatch fns → `dispatch.rs`. Green.
- [ ] Move `run_compaction`/`summarize` → `compact.rs`. Green.
- [ ] `cargo test loop` — count == baseline. `cargo fmt`. Full gate.
- [ ] Commit: "refactor: split loop.rs — extract launch.rs + ask.rs, merge subagent-dispatch + compaction into their homes"

**Acceptance criteria:**
- [ ] `loop.rs` production lines (excl. the `#[cfg(test)]` block) drop below ~1,000.
- [ ] `launch.rs` + `ask.rs` exist; `resolve_launch` + the ask shapers are no longer in `loop.rs`.
- [ ] The ~15 `resolve_launch_*` tests + `shape_ask_result_*` tests still pass, now in their new modules.
- [ ] `cargo test loop` count == baseline; `handle_prompt` still in `loop.rs` and unchanged.

---

### Task 5: Split `session.rs` + break the 4-way module cycle

**Context:** `session.rs` (4,357 lines) holds ~10 concerns, and it is the hub of a 4-way module cycle: `loop.rs:52` imports `{normalize, compute_display_rows, EventSink, ThoughtState, TurnState}` from `session` while `session` imports the `harness` re-exports (incl. `AgentLoop`); `subagent.rs` imports `{mint_session_id, EventSink}`; `interactive.rs` imports `{EventSink, LaunchConfig}`; `permission.rs` imports `EventSink` — and each is imported back. `EventSink` is the keystone (4 cycle modules + 6 `worker/` modules + 7 integration test files). This task does two things together (they share lines): (A) extract the self-contained blocks — the event-normalization block (`normalize` 2209, `normalize_capabilities` 2483, `compute_display_rows` 2515, `system_chunk` 2174, `merge_json` 2184, `now_ms` 2650) + its test mods (`normalize_tests` 2658, `compute_display_rows_tests` 4248) → `agent/normalize.rs`; the ADR 0025 router/crash/stall cluster (`route_event` 1750, `handle_crash` 1826, `stalled_info` 1860, `resolve_turn` 1872, `resolve_turn_forced` 1896, `on_session_exited_expected` 1912, `cleanup_pending_modals` 1970, `track_modal` 2003, `find_newest_crash_log` 2019) → `agent/session_router.rs`; (B) break the cycle by relocating the shared items to their natural owners so the 4 modules import from a neutral home instead of each other: `EventSink` (59) → `agent/events.rs` (it's an event-emission trait), `TurnState` (388) + `ThoughtState` (361) → `agent/normalize.rs` (they're normalized-state), `mint_session_id` (222) → a small `agent/types.rs`. The `agent/mod.rs` re-exports are kept **stable** so the 7 integration test files' `archimedes_lib::agent::EventSink` / `::SessionInfo` paths survive. The cycle items are mostly `pub(crate)` (integration tests can't see them — moving between private modules is integration-test-safe); only `EventSink` + `TurnState` are `pub` (keep their facade re-exports).

**Files:**
- Create: `src-tauri/src/agent/normalize.rs`, `src-tauri/src/agent/session_router.rs`, `src-tauri/src/agent/types.rs`
- Modify: `src-tauri/src/agent/session.rs` (remove moved fns + the `EventSink`/`TurnState`/`ThoughtState`/`mint_session_id` defs, add `use`), `src-tauri/src/agent/events.rs` (host `EventSink`), `src-tauri/src/agent/mod.rs` (add `mod normalize; mod session_router; mod types;` + keep the facade re-exports pointing at the new homes), `src-tauri/src/agent/{harness/loop, subagent, interactive, permission, harness/dispatch, worker/core, worker/dispatch, worker/manager, worker/sink}.rs` (update the `use crate::agent::session::{EventSink, TurnState, …}` imports to the new homes). **Complete import site list (verified by grep at planning time):** `harness/dispatch.rs:12` + `:166` (test) → `EventSink`; `harness/loop.rs:52` → `{compute_display_rows, normalize, EventSink, ThoughtState, TurnState}`; `interactive.rs:40` → `EventSink`; `permission.rs:45` → `EventSink`; `worker/core.rs:29` → `EventSink`; `worker/dispatch.rs:18` + `:111` (test) → `EventSink`; `worker/manager.rs:44` → `{mint_session_id, resolve_composed_model, EventSink}` + `:1260` (test); `worker/sink.rs:19` + `:81` (test) → `EventSink`; `subagent.rs:25` → `{mint_session_id, EventSink}` + `:271` (test). `resolve_composed_model` STAYS in `session.rs` (it is not a cycle item — `worker/manager.rs` and `loop.rs` may keep importing it from `session`).
- Test: `normalize_tests` + `compute_display_rows_tests` move with `normalize.rs`; `session_tests` (3064) stays in `session.rs`.

**What to implement:**
1. `agent/normalize.rs`: `EventSink`? No — `EventSink` goes to `events.rs`. `normalize.rs` hosts `TurnState` + `ThoughtState` + `normalize` + `normalize_capabilities` + `compute_display_rows` + `system_chunk` + `merge_json` + `now_ms` + the `normalize_tests`/`compute_display_rows_tests` mods. `normalize`/`compute_display_rows`/`ThoughtState` are `pub(crate)` — keep that visibility.
2. `agent/events.rs`: add `pub trait EventSink: Send + Sync { fn emit(&self, event: &str, payload: Value); }` (moved from `session.rs:59`). Update `session/mod.rs`-level: `agent/mod.rs` re-exports `EventSink` from `events` (path `archimedes_lib::agent::EventSink` unchanged).
3. `agent/types.rs`: `pub fn mint_session_id() -> String` (moved from 222) + `SessionInfo` if it's being moved in Task 6 (coordinate — if Task 6 moves `SessionInfo`, this file hosts only `mint_session_id` + any other cycle item; leave `SessionInfo` in `session.rs` for now and let Task 6 move it).
4. `agent/session_router.rs`: the 9 router/crash/stall fns. They're methods on `SessionManager` using `&self` state — either move them as `impl SessionManager` blocks in `session_router.rs` (Rust allows an `impl` in another module for a type in the same crate) or extract a `SessionRouter` struct. Prefer `impl SessionManager { … }` in `session_router.rs` (least change).
5. Update every `use crate::agent::session::{…}` in `loop.rs`, `subagent.rs`, `interactive.rs`, `permission.rs`, and the `worker/` modules to import `EventSink` from `events`, `TurnState`/`ThoughtState`/`normalize`/`compute_display_rows` from `normalize`, `mint_session_id` from `types`.

**What NOT to change:** the `SessionManager` impl's public methods, the `EffectiveCatalog` (449) + model/thinking helpers (those are a *candidate* further split but NOT in scope — leave them to keep this task focused), the `session_tests` mod, any wire shape. The `archimedes_lib::agent::{EventSink, SessionInfo, SessionManager, normalize_capabilities, user_message_payload, …}` public paths must all still resolve.

**Steps:**
- [ ] Baseline: `cargo test session` + `cargo test --test harness_loop --test session_native --test harness_subagent_dispatch --test harness_dispatch_native` (the 7 integration test files that use `EventSink`/`SessionInfo`) — record all green.
- [ ] Create `normalize.rs`; move `TurnState`/`ThoughtState` + the normalization fns + the 2 test mods. `cargo test normalize` + `cargo test session` — green.
- [ ] Move `EventSink` → `events.rs`; `mint_session_id` → `types.rs`; add the `mod` decls + keep the `agent/mod.rs` re-exports stable. `cargo build` — fix the import paths in `loop/subagent/interactive/permission/worker`. Green.
- [ ] Create `session_router.rs`; move the 9 fns as `impl SessionManager`. Green.
- [ ] Verify the cycle is broken: `grep -n "use crate::agent::session::{.*EventSink\|.*TurnState\|.*normalize" src/agent/harness/loop.rs src/agent/subagent.rs src/agent/interactive.rs src/agent/permission.rs` → the shared items now come from `events`/`normalize`/`types`, not `session`.
- [ ] Run the 7 integration test files — green (the `EventSink`/`SessionInfo` public paths survive). `cargo fmt`. Full gate.
- [ ] Commit: "refactor: split session.rs (normalize.rs + session_router.rs) and break the 4-way cycle (EventSink→events, TurnState/ThoughtState→normalize, mint_session_id→types)"

**Acceptance criteria:**
- [ ] `session.rs` production lines drop below ~1,800 (the 3 extracted blocks + 2 test mods are gone).
- [ ] No module in `{session, harness/loop, subagent, interactive, permission}` imports a shared cycle item from `session` — they import from `events`/`normalize`/`types`.
- [ ] `archimedes_lib::agent::EventSink` + `archimedes_lib::agent::SessionInfo` + all 7 integration test files compile and pass unchanged.
- [ ] `cargo test session` + the integration tests == baseline green.

---

### Task 6: Fix the layer inversions — `config/` module + `SessionInfo` move

**Context:** The domain layer imports the IPC layer: `load_settings`/`write_settings` (pure file I/O, zero Tauri in them) live in `commands/settings.rs` and are called from `agent/session.rs` (7 sites), `agent/mcp/manager.rs:83`, and `agent/harness/loop.rs:1358`. And `storage/db.rs:19` imports `agent::SessionInfo` (storage → domain, one production signature `Db::record_session`). `commands/settings.rs` (992 lines) is a de-facto config service masquerading as IPC. This task moves the config *model + I/O + catalog* into a new top-level `config/` module (so `commands/settings.rs` keeps only `#[tauri::command]` wrappers) and moves `SessionInfo` to a shared home so `storage/` doesn't import `agent/`. (A `Db` repository trait is deliberately NOT added — the SQL is contained and all 5 `Arc<Db>` holders are in-crate; a trait is YAGNI until a second backend.) Task 5 already moved `EventSink`/`TurnState`; this task is independent of it but should land after (both touch `session.rs`).

**Files:**
- Create: `src-tauri/src/config/mod.rs` (or `src-tauri/src/config.rs`), `src-tauri/src/types.rs` (or add `SessionInfo` to the `agent/types.rs` from Task 5 — pick ONE home and be consistent; recommend `src-tauri/src/types.rs` at the crate root so both `agent/` and `storage/` can import it without a layer inversion)
- Modify: `src-tauri/src/lib.rs` (`pub mod config; pub mod types;`), `src-tauri/src/commands/settings.rs` (keep only the `#[tauri::command]` fns; they now `use crate::config::{…}`), `src-tauri/src/agent/session.rs` (re-import `load_settings`/`write_settings` from `config`, and re-export `SessionInfo` from `types`), `src-tauri/src/agent/mcp/manager.rs` + `src-tauri/src/agent/harness/loop.rs` (the `load_settings` import), `src-tauri/src/storage/db.rs` (import `SessionInfo` from `types`), `src-tauri/src/agent/mod.rs` (keep the `SessionInfo` re-export stable for `commands/history.rs` + `commands/sessions.rs` + the 2 test files), `src-tauri/src/commands/{history,sessions}.rs` + `src-tauri/src/agent/persist.rs` (the `SessionInfo` imports).
- Test: the `settings` round-trip tests move with the config model to `config/` (or stay in `commands/settings.rs` if they test the command — keep the ones that test `load_settings`/`write_settings`/`Settings` serde in `config/`).

**What to implement:**
1. `config/mod.rs`: move `Settings` (82), `ProviderConfig` (25), `FontSettings` (55), `load_settings` (188), `write_settings` (237), `default_provider_api`, `KNOWN_PROVIDERS` (322), `KnownProvider`, `ModelDto`, and the serde/`Default` impls. `commands/settings.rs` keeps the `#[tauri::command] fn get_settings/save_settings/list_models/list_tools/list_known_providers/test_mcp_server/auth_mcp_server` wrappers, which now `use crate::config::{Settings, load_settings, write_settings, …}`.
2. `types.rs` (crate root): move `SessionInfo` (from `session.rs:244`) here. `agent/mod.rs` keeps `pub use crate::types::SessionInfo;` (so `archimedes_lib::agent::SessionInfo` is unchanged for `commands/history.rs`, `commands/sessions.rs`, `persist.rs`, `tests/storage.rs`, `tests/harness_store.rs`). `storage/db.rs` imports `crate::types::SessionInfo` (no more `agent::SessionInfo`).
3. Update the `load_settings`/`write_settings` call sites: `session.rs` (7), `mcp/manager.rs:83`, `loop.rs:1358` → `crate::config::{load_settings, write_settings}`.

**What NOT to change:** the `Settings`/`ProviderConfig`/`FontSettings` serde shapes (camelCase, `#[serde(default)]`), the `settings.json` format, the `#[tauri::command]` names/signatures (the frontend is unchanged), `Db::record_session`'s behavior.

**Steps:**
- [ ] Baseline: `cargo test settings` + `cargo test storage` — green.
- [ ] Create `config/mod.rs`; move the model + I/O + catalog + the serde/round-trip tests. `cargo build` — fix the `commands/settings.rs` wrappers to `use crate::config`. `cargo test settings` — the round-trip tests (now in `config/`) green.
- [ ] Create `types.rs`; move `SessionInfo`; update `agent/mod.rs` re-export + `storage/db.rs` + `commands/{history,sessions}.rs` + `persist.rs`. `cargo test storage` — green.
- [ ] Update the 3 module `load_settings` imports. Full gate.
- [ ] `cargo fmt`. Commit: "refactor: extract config/ module (settings I/O + catalog) + move SessionInfo to crate-root types — fixes agent→commands and storage→agent layer inversions"

**Acceptance criteria:**
- [ ] `grep -rn "use crate::commands::settings" src/agent src/storage` → 0 hits (the domain + storage no longer import the IPC layer).
- [ ] `grep -rn "use crate::agent::SessionInfo" src/storage` → 0 hits (storage imports from `types`).
- [ ] `commands/settings.rs` contains only `#[tauri::command]` fns + their thin wiring (no `Settings`/`load_settings` definitions).
- [ ] `archimedes_lib::agent::SessionInfo` still resolves (the 2 test files + commands compile); `cargo test` + `pnpm build` green.

---

### Task 7: Split `interactive.rs` (extract `sudo_exec.rs` + `sudo_argv.rs`)

**Context:** `interactive.rs` (2,787 lines, ~58% tests) holds 3 unrelated concerns: real-sudo **process execution** (`SudoRunner` trait 125, `RealSudoRunner` 142, `run_sudo_real` 156, `kill_process_group_and_reap` 350, `SudoGroupGuard` 374, `SudoPromptCleanup` 411, `read_stream_into` 480 — no interactive-channel dependency), argv/secret helpers (`split_command_into_argv` 504, `build_sudo_argv` 604, `scrub_secret` 624, `is_sudo_auth_failure` 645 — pure), and the interactive **sudo flow** (`sudo_run_flow` 882) + `dispatch_params` (659) + the keying types (`interactive_key` 55, `session_key_prefix` 63, `CachedPassword` 82, constants 89-102, `SudoRun` 108). (`todo_apply` 707 stays — it takes a `sink` and is the interactive suite tool.) This task extracts the two process/argv concerns so the file keeps its identity (the interactive flow). Independent of Tasks 1-6.

**Files:**
- Create: `src-tauri/src/agent/interactive/sudo_exec.rs`, `src-tauri/src/agent/interactive/sudo_argv.rs`
- Modify: `src-tauri/src/agent/interactive.rs` (becomes a `mod` dir OR stays a file with `pub mod` children — prefer converting `interactive.rs` → `interactive/mod.rs` + the two children), `src-tauri/src/agent/mod.rs` (the `pub use interactive::{…}` list must keep exporting `interactive_key`, `CachedPassword`, `PendingInteractive`, `PendingSudo`, `RealSudoRunner`, `SudoRun`, `SudoRunner`).
- Test: the `run_sudo_real_*` tests (2304-2750) + the `build_sudo_argv`/`scrub_secret` tests (2132-2191) move with their code.

**What to implement:**
1. Convert `agent/interactive.rs` → `agent/interactive/mod.rs` (same content). Add `pub mod sudo_exec; pub mod sudo_argv;`.
2. `interactive/sudo_exec.rs`: `SudoRunner` (125), `RealSudoRunner` (142) + its `impl` (144-349), `run_sudo_real` (156), `kill_process_group_and_reap` (350), `SudoGroupGuard` (374), `SudoPromptCleanup` (411), `read_stream_into` (480) + the `run_sudo_real_*` tests.
3. `interactive/sudo_argv.rs`: `split_command_into_argv` (504), `build_sudo_argv` (604), `scrub_secret` (624), `is_sudo_auth_failure` (645) + their tests.
4. `interactive/mod.rs` keeps: `interactive_key`, `session_key_prefix`, `CachedPassword`, the constants, `SudoRun`, `dispatch_params`, `sudo_run_flow` (882), `todo_apply` (707), `PendingInteractive`/`PendingSudo`, and re-exports `pub use sudo_exec::{SudoRunner, RealSudoRunner};` so `agent/mod.rs`'s `pub use interactive::{… RealSudoRunner, SudoRunner …}` is unchanged.

**What NOT to change:** the sudo flow state machine (confirm → password → TTL cache → run → timeout/abort/cleanup), the `SudoRunner` trait signature, the process-group kill behavior (the `run_sudo_real_timeout_kills_the_whole_process_group` test is the safety net), `todo_apply`.

**Steps:**
- [ ] Baseline: `cargo test interactive` — record the count.
- [ ] Convert to `interactive/mod.rs`; create `sudo_exec.rs` + `sudo_argv.rs`; move the fns + tests. `cargo build` — fix imports + the `mod.rs` re-exports.
- [ ] `cargo test interactive` — count == baseline (all `run_sudo_real_*` + `build_sudo_argv` + `scrub_secret` tests pass from their new homes). Green.
- [ ] `agent/mod.rs` public surface unchanged (`RealSudoRunner`/`SudoRunner` still re-exported). `cargo fmt`. Full gate.
- [ ] Commit: "refactor: split interactive.rs — extract sudo_exec.rs (process mgmt) + sudo_argv.rs (parsing)"

**Acceptance criteria:**
- [ ] `interactive/mod.rs` production lines drop below ~700 (the flow + keying + `todo_apply`).
- [ ] `sudo_exec.rs` holds all process execution + the `run_sudo_real_*` tests; `sudo_argv.rs` holds the pure parsing.
- [ ] `cargo test interactive` count == baseline; `agent/mod.rs` re-exports unchanged.

---

### Task 8: Decompose the frontend god files (`ChatStream.tsx`, `SettingsPage.tsx`, `braille-loader.ts`)

**Context:** The frontend's god files. `ChatStream.tsx` (1,260 lines) is a **single 1,186-line component** (the `return` JSX block alone is ~393 lines) with 24 imports — the de-facto composition god (9 sibling components, 4 stores, 5 hooks). `SettingsPage.tsx` (1,609 lines) has a ~760-line main component rendering all 5 sections inline, with 4 clean sub-component blocks. `braille-loader.ts` (1,279 lines) is a self-contained procedural animation engine (27 variants inlined in a ~1,100-line `VARIANT_CONFIGS` blob — cosmetic, low priority). `sessions.ts` (1,246) is deliberately NOT split (the 17-importer sink is a zustand direct-read pattern choice, not a defect). This task is the independent frontend track — it touches no Rust. Extract sub-components by lifting the already-marked regions into separate files; behavior is unchanged.

**Files:**
- Create: `src/components/chat/{ComposerSkills,AttachmentStrip,MessageList,ComposerModals}.tsx` (or colocate under `src/components/`), `src/components/settings/{McpSection,ProviderSection,GeneralSection,AppearanceSection,SubagentSection}.tsx`, `src/lib/braille/variants/` (one file per variant) + `src/lib/braille/registry.ts`.
- Modify: `src/components/ChatStream.tsx`, `src/components/settings/SettingsPage.tsx`, `src/lib/braille-loader.ts` (or keep `braille-loader.ts` as the entry and import the variant files).
- Test: the existing `ChatStream.test.tsx` (2,461) + `SettingsPage.test.tsx` (1,320) are the safety net — they must pass unchanged; add a render test per new sub-component only if one has non-trivial logic.

**What to implement:**
1. `ChatStream.tsx` → extract, in order of cleanliness: (a) **MessageList** — the `units.map` render loop (~898-950, the 5-branch `kind`→component dispatch, including the `prompts.map`/`stackedAskRequests.map`/empty-state blocks around it) → `chat/MessageList.tsx`. **Verified pure JSX — zero hooks in the region.** Required props (the full closure set): `units`, `activeSessionId`, `inTurn`, `askRequests`, `prompts`, `stackedAskRequests`, `hasPendingRequest`, `workingOrInTurn`, `agentState`. (b) **AttachmentStrip** — the image-attachment handlers + the thumbnail strip (region marked `// --- Image-attachment handlers ---` at ~689; **it contains the `attachments`/`attachmentsRef` state + effects — keep that state in `ChatStream` and pass `attachments`/`setAttachments`/the read handlers down as props**, do not relocate the `useState`); (c) **ComposerSkills** — the `$`-trigger picker + send-path expansion (region marked `// --- Skills (Task 5) ---` at ~377, incl. `useSkillCatalog` + the `filtered` memo + `activeSkillToken` + the `picker` state — **`useSkillCatalog` is a hook: it must either stay in `ChatStream` (pass `skills`/`picker` down) or move INTO `ComposerSkills` wholesale together with the `picker` `useState` + the keyboard effects that coordinate only with the picker — decide at execution time by checking which effects reference only picker state; the default is keep-in-parent + props-down**; (d) **ComposerModals** — the ask/confirm/password modals + working indicator (verify the region contains no coordinating hooks before moving; the modals take props only). `ChatStream` keeps the hooks/state (draft, error, attachments, scroll, the `send` fn) and composes the 4 extracted pieces. Each extracted piece receives the state it needs via props (do NOT make them read the stores directly unless they already do — `MessageList` must NOT call `useSessions`; everything comes via props).
2. `SettingsPage.tsx` → extract the 4 blocks into `settings/` sub-components: the MCP block (`McpRow` 325, `entryToDraft` 483, `linesToMap` 502, `draftToEntry` 512, `McpDialog` 537 → `McpSection.tsx`), `ProviderRow` (695) + the provider list → `ProviderSection.tsx`, the small controls (`FontSizeInput` 158, `SpinnerStylePicker` 218, `TextField` 272, `Field` 310 → `controls.tsx`), and the general/appearance/subagent sections → `GeneralSection.tsx`/`AppearanceSection.tsx`/`SubagentSection.tsx`. `SettingsPage` keeps the top-level state + the 5-section composition.
3. `braille-loader.ts` → move each of the 27 `VARIANT_CONFIGS` entries into `lib/braille/variants/<name>.ts` + a `registry.ts` that assembles them. Keep the math helpers + `generateFrames` + public API in `braille-loader.ts` (importing the registry). **If this is the least valuable, do it last or defer it** — it's cosmetic and self-contained; the ChatStream + SettingsPage extractions are the priority.

**What NOT to change:** any store, any `send`/`send_prompt` behavior, the event wiring, the rendered DOM (the extracted components must produce identical JSX), `sessions.ts`. No new state — lift, don't relocate, state (a `useState` that must stay in `ChatStream` to coordinate multiple pieces stays in `ChatStream` and is passed down).

**Steps:**
- [ ] Baseline: `pnpm test` + `pnpm build` — green. Note the `ChatStream.test.tsx` + `SettingsPage.test.tsx` counts.
- [ ] Extract `MessageList` from `ChatStream`. `pnpm test` (ChatStream tests) + `pnpm build` — green (identical DOM).
- [ ] Extract `AttachmentStrip`, `ComposerSkills`, `ComposerModals`. Green after each.
- [ ] Extract the 4 `SettingsPage` blocks. `pnpm test` (SettingsPage tests) + `pnpm build` — green.
- [ ] (Optional/deferred) Split `braille-loader.ts` variants. `pnpm test` (braille tests) + `pnpm build` — green.
- [ ] `pnpm test` (full) + `pnpm build`. Commit: "refactor: decompose ChatStream + SettingsPage into sub-components (+ braille variants)"

**Acceptance criteria:**
- [ ] `ChatStream.tsx` production lines drop below ~600 (the 4 extracted pieces are separate files); the main component is a composition of `<MessageList/>`/`<AttachmentStrip/>`/`<ComposerSkills/>`/`<ComposerModals/>` + the retained state.
- [ ] `SettingsPage.tsx` main component drops below ~400 lines; the 5 sections are separate `settings/*` files.
- [ ] `ChatStream.test.tsx` + `SettingsPage.test.tsx` counts == baseline (no test deleted; the DOM is unchanged).
- [ ] `pnpm test` + `pnpm build` green.

---

## Execution order + dependencies

1 → 2 → 3 → 4 → 5 → 6 → 7 → 8. Rationale: Tasks 1-2 (vocabulary) make later splits exhaustive and simplify `resolve_launch` before Task 4 moves it. Task 3 (provider) is isolated and safe early. Task 4 (loop) needs Task 2's simplified `resolve_launch`. Task 5 (session+cycle) and Task 6 (layering) both touch `session.rs` — run 5 before 6. Task 7 (interactive) is independent. Task 8 (frontend) is fully independent of all Rust tasks (can run in parallel on a separate branch if desired). Each task is independently committable and leaves the full gate green.

## Out of scope (deferred findings 9-20, not in this plan)

`worker/manager.rs` split (9), TS hardcoded context windows (10), MCP DRY cluster (11), skills/agents `space_roots` copy (12), frontend message-dispatch triplication (13), time injection / untested timeouts (14), `tauri.ts` contract tests (15), hand-synced wire contract codegen (16), shared test infra (17), dead-code sweep (18), `pending_bridge` rename (19), `Result<Value, ()>` (20). These remain open in `docs/reviews/2026-10-04-codebase-improvement.md`.
