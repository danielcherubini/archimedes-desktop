---
status: committed
done-when: A provider row with `api: "litellm"` against a LiteLLM proxy (verified: `llm.wizards.town/v1`) discovers its models via `GET {base}/model/info` (correct context windows + verbatim thinking levels in the Thinking selector, e.g. None/Low/Medium/Xhigh), a prompt streams a Thinking block and executes tool calls on the existing OpenAI chat-completions wire, and `cargo test` / `cargo clippy --all-targets` (0 warnings) / `cargo fmt --check` (src-tauri/) + `pnpm test` + `pnpm build` (repo root) are all green.
---

# LiteLLM provider support Plan (`api: "litellm"`)

**Goal:** Add `"litellm"` as a fourth `settings.json` provider `api` value — a *discovery mode* (models + thinking levels from `GET {base}/model/info`) on top of the existing OpenAI chat-completions wire (ADR 0026).
**Architecture:** The provider row's `api` field already doubles as the discovery-shape selector (`discover_models` dispatches on it). `litellm` routes discovery to the LiteLLM `model/info` endpoint and leaves completion on the existing `OpenAiCompatibleProvider` — no new provider struct, request body, or SSE parser. Thinking levels are surfaced verbatim (`none`/`low`/`medium`/`xhigh` — no `off`↔`none` normalization); the chosen level flows onto the wire as `reasoning_effort` as-is.
**Tech Stack:** Rust (Tauri backend: `reqwest`, serde), React 19 + TypeScript (Settings page), existing tokio/TCP mock-server test patterns, wiremock + vitest.

**Spec & decisions:** `docs/decisions/0026-litellm-api-mode.md` (the ADR), `CONTEXT.md` (Provider + Thinking level entries). The design discussion is complete; this plan is the HOW-in-order.

**Validation roots:** Rust from `src-tauri/` (`cargo test`, `cargo clippy --all-targets` — 0 warnings, `cargo fmt --check`); frontend from repo root (`pnpm test`, `pnpm build`).

---

### Task 1: `selectable()` accepts `api: "litellm"` (catalog.rs)

**Context:**
A provider row's `api` value flows onto every discovered model (`EffectiveCatalog` copies `provider.api` verbatim), and `ModelCatalog::selectable()` decides which models the model picker / config options offer. Today it matches exactly three `api` strings; a fourth value (`"litellm"`) would make LiteLLM models unselectable even though their completion wire is the standard OpenAI one. This task makes the catalog accept the new value. It is deliberately first and independent: no other task depends on it at compile time, and it is the purest expression of the ADR 0026 decision ("`litellm` is a discovery mode, not a fourth wire — but it IS selectable").

**Files:**
- Modify: `src-tauri/src/agent/harness/catalog.rs` (the `ModelCatalog::selectable()` method, ~line 109; its `#[cfg(test)] mod tests` at the bottom of the same file)
- Test: `src-tauri/tests/catalog.rs` (the integration test for `selectable()`)

**What to implement:**
In `ModelCatalog::selectable()` (catalog.rs), the filter's `matches!` currently reads:

```rust
matches!(
    m.api.as_deref(),
    Some("openai-completions")
        | Some("anthropic-messages")
        | Some("openai-responses")
)
```

Add `| Some("litellm")` as a fourth alternative, and update the surrounding doc comments on `selectable()` and on `Model.api` (both say "the three wires the harness speaks") + the stale inline comment inside `selectable_includes_the_three_wires` ("one model per wire … all three selectable" → four) to reflect that the selectable set is the three wires PLUS the `litellm` discovery mode (its wire is `openai-completions`; ADR 0026). Do NOT touch anything else in `catalog.rs` — no new discovery code in this task (Task 2 adds it).

In the inline unit tests (`catalog.rs` `mod tests`): the existing `selectable_includes_the_three_wires` test builds a 3-model catalog. Extend its model list with a fourth model `model("lit/1", true, Some("litellm"))` and assert it IS in the selectable set (the ids assertion becomes `vec!["a", "b", "c", "lit/1"]` in the existing order) — AND update the test's FIRST assertion `assert_eq!(catalog.all().len(), 3)` → `4` (it counts ALL models, not just selectable — a verbatim agent that forgets it hits an unexplained red). `selectable_excludes_none_and_unknown_api` stays as-is; `selectable_requires_supports_tools` gains a `model("d", false, Some("litellm"))` case (its ids assertion stays `vec!["a"]` — `supports_tools` still gates the new value).

In the integration test `src-tauri/tests/catalog.rs`: rename nothing; in `selectable_filters_to_the_three_wires_subset` add a fifth model `model("lit/1", "llm", Some("litellm"))` and assert `ids.contains(&"lit/1")` (the `assert_eq!(selectable.len(), 2)` becomes 3). Update the test's doc comment ("three wires") to say the selectable set is the three wires + the `litellm` discovery mode.

**NOT to change:** `merge_catalog`, `DiscoveredMeta`, `discover_models`, `build_provider`, the frontend — all later tasks.

**Steps:**
- [ ] In `src-tauri/tests/catalog.rs`, modify `selectable_filters_to_the_three_wires_subset`: add `model("lit/1", "llm", Some("litellm"))` to the catalog, assert `ids.contains(&"lit/1")`, update the length assertion 2 → 3 and the comment.
- [ ] In `catalog.rs` inline tests: extend `selectable_includes_the_three_wires` with the fourth `litellm` model + assertion; add the no-tools `litellm` model to `selectable_requires_supports_tools`.
- [ ] Run `cargo test selectable` from `src-tauri/`
  - Did the new/changed assertions FAIL (the litellm models missing from the selectable set)? If a test passed unexpectedly, stop and investigate why.
- [ ] Implement the `Some("litellm")` arm in `selectable()` + the doc-comment updates in `catalog.rs`.
- [ ] Run `cargo test` from `src-tauri/`
  - Did ALL tests pass? If not, fix the failures and re-run before continuing.
- [ ] Run `cargo clippy --all-targets` from `src-tauri/` — 0 warnings? Run `cargo fmt --check` — clean? Fix and re-run if not.
- [ ] Commit with message: `"feat(catalog): make litellm-api models selectable"`

**Acceptance criteria:**
- [ ] `cargo test` green from `src-tauri/`, including the updated `selectable_filters_to_the_three_wires_subset` (integration) and the two inline `selectable_*` tests.
- [ ] `selectable()` accepts `Some("litellm")` and still rejects `None`, unknown apis, and `supports_tools: false` models.
- [ ] Doc comments on `selectable()` and `Model.api` no longer claim "three wires".

---

### Task 2: LiteLLM discovery — `GET {base}/model/info` (catalog.rs)

**Context:**
A LiteLLM proxy advertises its models' metadata (context window, thinking levels, reasoning capability) ONLY at `GET {base}/model/info` — its standard `GET {base}/models` (already served) carries neither. A `litellm` provider row must therefore discover through the LiteLLM shape, exactly as `anthropic-messages` rows already do through a dedicated `discover_anthropic_models` function. This task adds that third discovery shape: a new private `discover_litellm_models` function + a `"litellm"` dispatch arm in `discover_models`, mapping the response into the EXISTING `DiscoveredMeta` struct (no new fields — the struct already has exactly the three `Option` fields needed).

The live response shape (verified against `llm.wizards.town/v1/model/info`):

```json
{ "data": [ {
    "model_name": "Qwen/Qwen3.8-27B",
    "model_info": {
      "max_input_tokens": 262144,
      "max_output_tokens": 32768,
      "supports_reasoning": true,
      "reasoning_effort_levels": ["none", "low", "medium", "xhigh"],
      "default_reasoning_effort": "xhigh",
      "supports_function_calling": null,
      "input_cost_per_token": null,
      "output_cost_per_token": null
    }
} ] }
```

Field mapping (the approved spec): `model_name` → the map key; `max_input_tokens` → `context_window` (fall back to `max_output_tokens` when `max_input_tokens` is absent, so a `max_output_tokens`-only entry still gets a window); `reasoning_effort_levels` → `thinking_levels` VERBATIM (absent → `None`); `supports_reasoning` → `supports_thinking` (absent → `None`). IGNORED: `default_reasoning_effort` (the level-resolution chain is untouched — the proxy's server-side default applies), `supports_function_calling` (discovered models already get `supports_tools: true`; the field is null on 3 of 4 live models), `input/output_cost_per_token` (no consumer — `cost_per_mtok_*` is hardcoded 0.0 for all discovered models).

**Files:**
- Modify: `src-tauri/src/agent/harness/catalog.rs` (`discover_models` dispatch + new `discover_litellm_models` + response types + inline tests)
- Test: inline `#[cfg(test)] mod tests` in `catalog.rs` (reuses the file's existing `raw_json_server` helper, ~line 425)

**What to implement:**

1. In `discover_models` (catalog.rs), after the existing `anthropic-messages` early-return, add:

```rust
if api == "litellm" {
    return discover_litellm_models(base_url, api_key).await;
}
```

and update `discover_models`'s doc comment (the `api` decides-the-wire list) with the `litellm` bullet.

2. New private function (mirror `discover_anthropic_models`'s isolation — own response types, own function; place it next to it):

```rust
/// The LiteLLM `GET {base_url}/model/info` (ADR 0026): `model_name` → the map
/// key; `model_info.max_input_tokens` (fallback `max_output_tokens`) →
/// `context_window`; `reasoning_effort_levels` → `thinking_levels` (VERBATIM);
/// `supports_reasoning` → `supports_thinking`. `default_reasoning_effort` /
/// `supports_function_calling` / the cost fields are parsed-by-omission
/// (ignored for v1 — see the ADR's Considered Options). Bearer auth when the
/// key is non-empty (empty key → no header); same 5s/10s bounded client;
/// non-2xx / network error → `Err`. NO base-URL normalization (LiteLLM serves
/// both `/v1/model/info` and `/model/info`).
async fn discover_litellm_models(
    base_url: &str,
    api_key: &str,
) -> Result<HashMap<String, DiscoveredMeta>, String> {
    let url = format!("{}/model/info", base_url.trim_end_matches('/'));
    // ... the SAME client builder as the OpenAI arm (5s connect / 10s read)
    // ... .bearer_auth(api_key) when !api_key.is_empty() (NOT when empty)
    // ... non-2xx → Err(format!("status {status}")); resp.json::<LiteLLMModelInfoResponse>()
    // map each entry: (entry.model_name, entry.model_info.map(|mi| DiscoveredMeta {
    //     context_window: mi.max_input_tokens.or(mi.max_output_tokens),
    //     thinking_levels: mi.reasoning_effort_levels,
    //     supports_thinking: mi.supports_reasoning,
    // }).unwrap_or_default())  — a missing / null `model_info` → all-`None` meta
}
```

3. New private response types (serde, like `ModelsResponse` / `AnthropicModelsResponse`):

```rust
#[derive(Debug, Deserialize)]
struct LiteLLMModelInfoResponse {
    #[serde(default)]
    data: Vec<LiteLLMModelInfoEntry>,
}

#[derive(Debug, Deserialize)]
struct LiteLLMModelInfoEntry {
    model_name: String,
    // `Option`: an absent OR an explicit `"model_info": null` (both observed
    // shapes on LiteLLM deployments) must map to an all-`None` meta entry
    // (the documented degradation, same as a bare OpenAI entry) — NOT fail
    // the whole response.
    #[serde(default)]
    model_info: Option<LiteLLMModelInfo>,
}

#[derive(Debug, Default, Deserialize)]
struct LiteLLMModelInfo {
    #[serde(rename = "max_input_tokens")]
    max_input_tokens: Option<u32>,
    #[serde(rename = "max_output_tokens")]
    max_output_tokens: Option<u32>,
    #[serde(rename = "reasoning_effort_levels")]
    reasoning_effort_levels: Option<Vec<String>>,
    #[serde(rename = "supports_reasoning")]
    supports_reasoning: Option<bool>,
}
```

(`Option<u32>` for the token counts: real LiteLLM values are small (≤ 1.3M on the verified instance — far under `u32::MAX`), `null` is handled by `Option`, and this is consistent with the existing `DiscoveredMeta.context_window: Option<u32>` and the OpenAI arm's `max_model_len: Option<u32>`.) The map-building step: `entry.model_info.map(|mi| DiscoveredMeta { context_window: mi.max_input_tokens.or(mi.max_output_tokens), thinking_levels: mi.reasoning_effort_levels, supports_thinking: mi.supports_reasoning }).unwrap_or_default()`.

4. Inline tests in `catalog.rs` `mod tests` (the file's `raw_json_server(listener, status, body)` helper already exists — 3-arg, single-accept, at ~line 425; bind a fresh listener per test and `server.abort()` after, exactly like `discover_models_anthropic_shape`). NOTE: `raw_json_server` ignores the request PATH and HEADERS (it serves its fixed body to any GET), so these tests pin the DISPATCH ARM + response parsing — the `{base}/model/info` path and the Bearer header are NOT asserted here (a path/header-asserting variant would require extending the helper — out of scope); the path + auth are verified by the Task 5 live smoke.
   - `discover_models_litellm_maps_the_model_info_shape`: body with TWO entries — one full (`model_name: "lit/1"`, `max_input_tokens: 262144`, `reasoning_effort_levels: ["none","low","medium","xhigh"]`, `supports_reasoning: true`) and one bare (`model_name: "lit/2"` only). Call `discover_models(&format!("http://{addr}/v1"), "test-key", "litellm")`. Assert the map has both keys; `lit/1` → `context_window: Some(262144)`, `thinking_levels: Some(vec!["none","low","medium","xhigh"])`, `supports_thinking: Some(true)`; `lit/2` → `DiscoveredMeta::default()` (all `None`).
   - `discover_models_litellm_falls_back_to_max_output_tokens`: body with one entry carrying ONLY `max_output_tokens: 999` → `context_window: Some(999)`.
   - `discover_models_litellm_errors_on_a_non_2xx`: `raw_json_server(listener, 404, r#"{"detail":"Not Found"}"#)` → `Err`.

**NOT to change:** the OpenAI / Anthropic discovery arms, `DiscoveredMeta` (no new fields), `EffectiveCatalog::resolve` (its existing call `discover_models(base, key, api)` picks up the new arm automatically), `build_provider`.

**Steps:**
- [ ] Write the three failing tests above in `catalog.rs` `mod tests`.
- [ ] Run `cargo test discover_models_litellm` from `src-tauri/`
  - EXPECTED red state: the two shape-mapping tests FAIL (before the arm exists, `"litellm"` routes to the OpenAI arm, which fetches `/models` with the OpenAI parser — the LiteLLM-shaped body has no `id` field → serde error → `Err` → the `.unwrap()` panics). The `discover_models_litellm_errors_on_a_non_2xx` test PASSES pre-implementation — that is CORRECT, not an anomaly: non-2xx → `Err` is dispatch-independent (every arm does it), and the test pins the *error contract*, not the routing. Do NOT "fix" the 404 test or stall on it. If a SHAPE-mapping test passed unexpectedly, stop and investigate why.
- [ ] Implement `discover_litellm_models` + the response types + the `"litellm"` dispatch arm in `discover_models` + the doc-comment update.
- [ ] Run `cargo test` from `src-tauri/`
  - Did ALL tests pass? If not, fix the failures and re-run before continuing.
- [ ] Run `cargo clippy --all-targets` (0 warnings) + `cargo fmt --check` from `src-tauri/` — fix and re-run if not.
- [ ] Commit with message: `"feat(catalog): litellm discovery via GET /model/info"`

**Acceptance criteria:**
- [ ] `discover_models(_, _, "litellm")` maps the four fields per the table (a bare / `model_info: null` entry yields all-`None` meta); the dispatch arm is exercised (the shape tests would fail without it). The `{base}/model/info` path + Bearer header are pinned by the Task 5 live smoke, not the mock (see the raw_json_server note above).
- [ ] The `max_input_tokens` → `max_output_tokens` fallback is covered by a test.
- [ ] Non-2xx → `Err` (the caller's existing degradation path: row shows `unreachable`, 0 models, still shadows the base id — unchanged).
- [ ] `cargo test` / `clippy` / `fmt` green.

---

### Task 3: `build_provider` + `EffectiveCatalog` routing for `litellm` (provider.rs, session.rs)

**Context:**
With discovery working, a `litellm` provider's models must (a) build the EXISTING `OpenAiCompatibleProvider` (the wire is standard OpenAI chat-completions — verified live: `reasoning_content` deltas and `tool_calls` stream in the exact shapes the existing `SseStream` parses, and `reasoning_effort` (including the value `"none"`) is sent verbatim by the existing `request_body` — there is no suppression/empty-check on this wire, by design), and (b) be stamped `api: Some("litellm")` on their `Model` rows so the catalog (Task 1) keeps them selectable. `build_provider` already routes `litellm` correctly via its default arm (the match's `_` catch-all) — but that routing is currently UNPINNED by a test: a future refactor of the match could silently move `litellm` to a different arm. This task pins the behavior with tests on both sides and adds the dispatch to the existing dispatch test.

**Files:**
- Modify: `src-tauri/src/agent/harness/provider.rs` (`build_provider` doc comment + the inline `build_provider_dispatches_on_the_api` test in `mod tests`)
- Modify: `src-tauri/src/agent/session.rs` (inline `mod tests` — the `write_settings_provider_api` + `temp_config_dir` helpers already exist there; the mock server comes from `crate::test_support::raw_json_server`, the 4-arg `counter: Option<…>` variant — pass `None`)

**What to implement:**

1. `provider.rs` — `build_provider` (~line 2187): NO code change to the match (the `_` arm already catches `"litellm"`). Update ONLY its arm comments: the `_` arm's comment currently says "`openai-completions` / `None` / unknown — the current default" — add `litellm` to that list with a pointer to ADR 0026 ("`litellm` is a DISCOVERY mode, not a wire — its completion wire IS `openai-completions`").

2. `provider.rs` inline test `build_provider_dispatches_on_the_api` (~line 4501): it performs THREE dispatches through `recorded_dispatch_server` (a loop-accept server that replies a wire-appropriate minimal SSE body per PATH — the else-branch already handles any non-`/messages` / non-`/responses` path with the OpenAI body). Append a FOURTH dispatch:

```rust
// (4) (ADR 0026) `litellm` → the OpenAI arm: `POST /chat/completions` +
// `authorization` (it is a discovery mode, NOT a fourth wire).
let provider = build_provider(&model(&base_url, Some("litellm")));
// ... same collect + Done(Stop) assert ...
let (path, has_auth, has_x_api_key) = rx.recv().await.expect("the server recorded the request");
assert!(path.starts_with("post ") && path.contains("/chat/completions"), "got {path:?}");
assert!(has_auth, "the litellm wire sends `authorization` ({path:?})");
assert!(!has_x_api_key, "the litellm wire sends NO `x-api-key` ({path:?})");
```

(Update the test's doc comment from "THREE dispatches" to "FOUR" — including the `recorded_dispatch_server` doc comment that says "performs THREE dispatches".)

3. `session.rs` inline test — new test modeled on the existing `resolve_uses_the_provider_api` (~line 4096, which does exactly this for `anthropic-messages`):

```rust
/// (ADR 0026) A provider with `api: "litellm"` yields an effective-catalog
/// model with `api: Some("litellm")` (the provider's `api` decides the wire
/// — and the wire for `litellm` is `openai-completions`, Task 3/ADR 0026).
#[tokio::test]
async fn resolve_uses_the_litellm_provider_api() {
    let dir = temp_config_dir();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    // The `litellm` discovery arm hits `{base}/model/info` — serve the
    // LiteLLM shape (the test_support `raw_json_server` is the 4-arg
    // `counter: Option<…>` variant; pass `None`).
    let server = crate::test_support::raw_json_server(
        listener,
        200,
        r#"{"data":[{"model_name":"lit/1","model_info":{"max_input_tokens":262144,"reasoning_effort_levels":["none","low"],"supports_reasoning":true}}]}"#,
        None,
    )
    .await;
    write_settings_provider_api(&dir, "llm", &format!("http://{addr}/v1"), "litellm");
    let manager = SessionManager::new(dir);
    let effective = manager.effective_catalog(None).await;
    let user: Vec<&Model> = effective.models.iter().filter(|m| m.provider == "llm").collect();
    assert_eq!(user.len(), 1, "the discovered model is in the catalog");
    // The provider's `api` is stamped verbatim (NOT `openai-completions`).
    assert_eq!(user[0].api.as_deref(), Some("litellm"));
    // ... and the metadata mapped (the discovery arm actually ran):
    assert_eq!(user[0].context_window, 262144);
    assert_eq!(user[0].thinking_levels, vec!["none".to_string(), "low".to_string()]);
    server.abort();
}
```

NOTE: this test's failure mode BEFORE Task 2's arm exists would be a discovery `Err` — the mock server answers 200 on ANY path, so the pre-Task-2 failure is the OpenAI parser choking on the LiteLLM-shaped body (no `id` field → serde error), NOT a 404 — either way 0 models, and `user.len()`'s assertion message says which side failed. Task 2 lands first, so by the time this task runs the discovery arm exists and only the stamping assertion is the new check.

**NOT to change:** `EffectiveCatalog::resolve` (the stamping `api: Some(provider.api.clone())` is already generic), `discover_models`, the Worker protocol, `synthesize_catalog_config_options`.

**Steps:**
- [ ] Add the 4th dispatch to `build_provider_dispatches_on_the_api` (provider.rs) — it should PASS immediately (the `_` arm already catches `litellm`); that is correct: it PINS existing behavior. If it FAILS, stop and investigate (it would mean the match already routes `litellm` elsewhere — a bug in Task 2 or a match refactor).
- [ ] Add `resolve_uses_the_litellm_provider_api` to session.rs `mod tests`.
- [ ] Update the `build_provider` arm comment + the two "THREE dispatches" doc comments.
- [ ] Run `cargo test build_provider_dispatches resolve_uses_the_litellm` from `src-tauri/`
  - Did all pass? If not, fix and re-run before continuing.
- [ ] Run `cargo test` (full) from `src-tauri/`; `cargo clippy --all-targets` (0 warnings) + `cargo fmt --check` — fix and re-run if not.
- [ ] Commit with message: `"feat(harness): route litellm models to the openai-compatible provider"`

**Acceptance criteria:**
- [ ] A `Model { api: Some("litellm") }` completes through `POST /chat/completions` with an `authorization` header (pinned by the 4th dispatch test).
- [ ] A `settings.json` provider row with `api: "litellm"` yields catalog models stamped `api: Some("litellm")` WITH mapped metadata (pinned by `resolve_uses_the_litellm_provider_api`).
- [ ] `cargo test` / `clippy` / `fmt` green.

---

### Task 4: Settings UI — the API select offers "LiteLLM" (frontend)

**Context:**
The provider row's API select (Settings → Providers) is the user's switch into the new discovery mode: a row against a LiteLLM proxy (e.g. the user's existing `openai-completions` "Wizards" row at `llm.wizards.town/v1`) upgrades its discovery by flipping the select to "LiteLLM". The select's options come from the `WIRE_APIS` const and the labels from `apiLabel` in `SettingsPage.tsx` — a two-line change plus the test. There is NO new known-provider template (no canonical base URL for arbitrary LiteLLM deployments — ADR 0026) and NO new persisted field (`api` is a plain `String` in `ProviderConfig` / `settings.json` with a serde default — `"litellm"` round-trips without migration).

**Files:**
- Modify: `src/components/settings/SettingsPage.tsx` (`WIRE_APIS` const ~line 91, `apiLabel` fn ~line 98)
- Modify: `src/components/settings/SettingsPage.test.tsx` (the `provider_row_api_select_commits_api` test ~line 1197 + a new offer/label test)
- Modify: `src/lib/toolOutput.ts` (`THINKING_GLYPHS` ~line 142 — add a `none` entry) + `src/lib/toolOutput.test.ts` (the existing `formatThinkingIndicator` assertion block ~line 505)
- Modify: `src/lib/tauri.ts` (`ProviderConfig.api` doc comment, line 347)
- Modify: `src-tauri/src/commands/settings.rs` (`ProviderConfig.api` doc comment, ~line 36)

**What to implement:**

1. `SettingsPage.tsx`:
   - `WIRE_APIS`: add `"litellm"` (fourth entry, order: openai-completions, anthropic-messages, openai-responses, litellm).
   - `apiLabel`: add `if (api === "litellm") return "LiteLLM";` (before the final default return).
   - Do NOT change the known-providers picker, `addProvider`, or any other row control.

2. `src/lib/toolOutput.ts` — the LiteLLM verbatim vocabulary includes `none`, but `THINKING_GLYPHS` has no entry for it, so the config selector's thinking indicator for the `none` level would fall back to the mid glyph `◑` (the fallback in `formatThinkingIndicator`). Add `none: "○"` (same as `off` / `minimal` — a thinking-OFF level). The LABEL is already correct ("none" → "None" via the existing capitalization in `synthesize_catalog_config_options`) — only the fill-ramp indicator needed the entry.

3. `SettingsPage.test.tsx`:
   - New test `provider_row_api_select_offers_litellm`: mock `getSettings` with one `openai-completions` provider row (copy the `provider_row_api_select_commits_api` fixture), render, `go("Providers")`, open the API `combobox` (name "API"), and assert the option `screen.findByRole("option", { name: "LiteLLM" })` exists.
   - Extend `provider_row_api_select_commits_api`: after the existing `anthropic-messages` commit assertion, reopen the combobox, click the "LiteLLM" option, and assert `saveSettings` called with `providers` containing `{ api: "litellm" }` (the immediate-save `commitProviderField` pattern — same shape as the existing assertion). FALLBACK: reopening a Radix select a second time within one render is a pattern not exercised elsewhere in this file — if the second open misbehaves under jsdom (the file's `hasPointerCapture`/`scrollIntoView`/`matchMedia` stubs should handle it, but this is the one step with residual flake risk), split the LiteLLM-commit assertion into its own test (a fresh render, one open) matching the file's established pattern.
   - `toolOutput.test.ts`: in the existing `formatThinkingIndicator` assertion block (~line 505), add `expect(formatThinkingIndicator("none")).toBe("○ none");`.

4. Doc comments (doc-only, zero behavior):
   - `src/lib/tauri.ts` line 347: `"openai-completions"` (default) / `"anthropic-messages"` / `"openai-responses"` / `"litellm"` (discovery via `GET /model/info`, wire = openai-completions — ADR 0026).
   - `src-tauri/src/commands/settings.rs` `ProviderConfig.api` doc (~line 36): add the fourth value in the same style (the field stays `#[serde(default = "default_provider_api")]` — NO schema change; a pre-feature file still parses to `openai-completions`).

**NOT to change:** `KnownProvider` / `KNOWN_PROVIDERS` (no new template), the `ProviderConfig` RUST struct's fields, `addProvider`'s signature, any store/persistence code.

**Steps:**
- [ ] Write the failing `provider_row_api_select_offers_litellm` test + the `litellm`-commit extension in `SettingsPage.test.tsx`, and add the failing `expect(formatThinkingIndicator("none")).toBe("○ none")` assertion to the existing `formatThinkingIndicator` block in `toolOutput.test.ts` (~line 505).
- [ ] Run `pnpm test -- SettingsPage toolOutput` from repo root
  - Did the new assertions FAIL (no "LiteLLM" option; `formatThinkingIndicator("none")` returns the `◑` fallback)? If they passed unexpectedly, stop and investigate why.
- [ ] Implement the `SettingsPage.tsx` changes, the `THINKING_GLYPHS` entry, and the two doc-comment updates.
- [ ] Run `pnpm test` from repo root
  - Did ALL tests pass? If not, fix and re-run before continuing.
- [ ] Run `pnpm build` from repo root
  - Did it succeed (type-check + build)? If not, fix and re-run before continuing.
- [ ] Commit with message: `"feat(settings): offer the litellm api in the provider select"`

**Acceptance criteria:**
- [ ] The API select offers "LiteLLM" and committing it saves `api: "litellm"` on the row (immediate save, no new dialog).
- [ ] `formatThinkingIndicator("none")` → `"○ none"` (the LiteLLM `none` level gets the thinking-OFF glyph, not the mid fallback).
- [ ] `pnpm test` + `pnpm build` green.
- [ ] The `tauri.ts` + `settings.rs` doc comments list the fourth value.

---

### Task 5: Full validation + live smoke (exit criterion)

**Context:**
The feature's exit condition (`done-when`) is a LIVE LiteLLM proxy working end-to-end — not just unit tests. This task runs the full validation matrix for both roots and the manual live smoke against `llm.wizards.town`. No code changes are expected; if the smoke surfaces a bug, fix it in the responsible module (tasks 1–4's files), re-run the relevant suite, and commit the fix — then re-run the matrix.

**Files:**
- (no source changes expected)
- Validate: `src-tauri/` (Rust), repo root (frontend), the running app against `llm.wizards.town/v1`

**Steps:**
- [ ] Run `cargo test` from `src-tauri/` — all green?
- [ ] Run `cargo clippy --all-targets` from `src-tauri/` — 0 warnings? Run `cargo fmt --check` — clean?
- [ ] Run `pnpm test` from repo root — all green?
- [ ] Run `pnpm build` from repo root — success?
- [ ] Live smoke (manual): with the app running, in Settings → Providers, ensure a row with `baseUrl: "https://llm.wizards.town/v1"`, the user's LiteLLM key, and `api: "LiteLLM"` (flip the existing "Wizards" row's select, or add a new row):
  - Does the row's badge show `4 models` after a refresh (deepseek/deepseek-v4-flash, z-ai/glm-5.3-prime, deepseek/deepseek-v4-pro, Qwen/Qwen3.8-27B)?
  - Start a session on `Qwen/Qwen3.8-27B`: does the Thinking selector show None / Low / Medium / Xhigh (the proxy's verbatim vocabulary)?
  - Set thinking to `Low` and send a prompt: does a Thinking block stream (a `reasoning_content` fragment) followed by the assistant text?
  - Does a tool call execute (e.g. ask the agent to list a file — the `read` tool round-trips through the proxy)?
  - Does the context window reflect `max_input_tokens` (Qwen → 262144, not the 128000 default)?
  - If ANY step fails: diagnose (the provider's error text surfaces in the session error / the row badge), fix in the responsible file, re-run that root's suite + `clippy`/`fmt`, commit the fix with message `"fix(litellm): <what the smoke surfaced>"`, and re-run the full matrix from the top of this task.
- [ ] If a fix was committed, squash-note it into this task's completion record (the branch squash-merges as one commit per AGENTS.md; the fixes ride along).
- [ ] Final commit check: `git log --oneline -5` shows tasks 1–4's commits (+ any smoke fix) ready for the branch squash-merge.

**Acceptance criteria (the `done-when`):**
- [ ] `cargo test` / `cargo clippy --all-targets` (0 warnings) / `cargo fmt --check` (src-tauri/) + `pnpm test` / `pnpm build` (repo root) ALL green.
- [ ] Live: `litellm` row at `llm.wizards.town/v1` → 4 models, correct context windows, verbatim thinking levels (None/Low/Medium/Xhigh) in the selector, a `Low`-thinking prompt streams a Thinking block, a tool call executes, row badge `4 models`.
