---
status: approved
done-when: A provider row with `api: "litellm"` against a LiteLLM proxy (verified: `llm.wizards.town/v1`) discovers its models via `GET {base}/model/info` (correct context windows + verbatim thinking levels in the Thinking selector, e.g. None/Low/Medium/Xhigh), a prompt streams a Thinking block and executes tool calls on the existing OpenAI chat-completions wire, and `cargo test` / `cargo clippy --all-targets` (0 warnings) / `cargo fmt --check` (src-tauri/) + `pnpm test` + `pnpm build` (repo root) are all green.
---

# Spec: LiteLLM provider support (`api: "litellm"`)

## Context

The desktop's providers are user-managed rows in `settings.json` (ADR 0014), each discovered live. A LiteLLM proxy (e.g. `llm.wizards.town`) advertises models, thinking levels, and capabilities only at its non-OpenAI discovery endpoint `GET {base}/model/info` (the endpoint the user called `/v1/info` 404s; `/v1/model/info` and `/model/info` serve it — verified 200). The proxy's standard `GET {base}/models` carries neither `max_model_len` nor reasoning fields, so a plain `openai-completions` row against it discovers models with the `DEFAULT_CONTEXT_WINDOW` (128K) fallback and no thinking levels.

Verified live against the proxy: the **completion** path is standard OpenAI chat-completions — `reasoning_content` deltas (the existing `SseStream` already parses them as `ThinkingDelta`), `tool_calls` (existing accumulation), and `reasoning_effort: "none"` suppresses reasoning — and the harness already sends `x-litellm-session-id` / `x-request-id` on every completions request.

## Decision

`"litellm"` is a **fourth `api` value** — a *discovery mode*, not a fourth wire (ADR 0026): discovery uses `GET {base}/model/info`; completion uses the existing `OpenAiCompatibleProvider`. Thinking levels are surfaced **verbatim** from `model_info.reasoning_effort_levels` (no `off`↔`none` normalization — the chosen level flows onto the wire as `reasoning_effort` as-is; the proxy's own words render in the selector).

## Behavior

### 1. Dispatch topology

- `discover_models` (`src-tauri/src/agent/harness/catalog.rs`): new `"litellm"` arm → a dedicated LiteLLM discovery function (mirroring the existing `discover_anthropic_models` isolation — own response types, own function).
- `build_provider` (`src-tauri/src/agent/harness/provider.rs`): `"litellm"` falls to the default arm → `OpenAiCompatibleProvider`. **No new provider struct, request body, or SSE parser.**
- `ModelCatalog::selectable()` (catalog.rs): accepts `Some("litellm")` alongside the three existing wires.
- Nothing else changes: `AgentLoop`, retry policy, Worker protocol, config-option synthesis, session-id headers.

### 2. Discovery — `GET {base_url}/model/info`

- **Auth**: `Authorization: Bearer` when the key is non-empty; empty key → no header (the existing rule). Same bounded client (5s connect / 10s read). Non-2xx / network error → `Err` (the caller degrades exactly as today: the row shows `unreachable`, 0 models, still shadows the base id).
- **Response mapping** (`data[]` entries → the existing `DiscoveredMeta`):

  | `model_info` field | → `DiscoveredMeta` |
  |---|---|
  | `model_name` | the map key (= the model id sent to completions) |
  | `max_input_tokens` (fallback `max_output_tokens`) | `context_window` (then `DEFAULT_CONTEXT_WINDOW` at the call site) |
  | `reasoning_effort_levels` | `thinking_levels` (verbatim; absent → `None`) |
  | `supports_reasoning` | `supports_thinking` (absent → `None`) |

- **Ignored for v1**: `input_cost_per_token` / `output_cost_per_token` (the `cost_per_mtok_*` `Model` fields are hardcoded 0.0 for every discovered model today and have no consumer — recorded as a future seam in ADR 0026); `default_reasoning_effort` (the level-resolution chain — remembered → settings default — is untouched; a fresh session starts with no level and the proxy's server-side default applies); `supports_function_calling` (discovered models already get `supports_tools: true`; the field is null on 3 of the 4 live models, so honoring it would hide more than protect).
- **Base URL**: no normalization — LiteLLM serves BOTH `/v1/model/info` and `/model/info` (verified), so a row's `…/v1` base works as-is.

### 3. UI / settings / catalog surface

- `src/components/settings/SettingsPage.tsx`: `WIRE_APIS` + `apiLabel` gain `"litellm"` → `"LiteLLM"`. The provider row's existing (editable) **API select** is the switch — the user's current `openai-completions` row against the proxy upgrades by flipping the select. **No new known-provider template** (no canonical base URL for arbitrary LiteLLM deployments).
- `src-tauri/src/commands/settings.rs`: **zero schema change** — `api` is a plain `String` with a serde default; `"litellm"` round-trips. The `ProviderConfig.api` doc comment gains the fourth value.
- `src/lib/tauri.ts`: the `api` doc comment gains the fourth value.
- Discovered models get `api: Some("litellm")` stamped from the provider row (`EffectiveCatalog` copies `provider.api` verbatim — no change).
- The **Thinking selector is untouched**: `synthesize_catalog_config_options` already renders `thinking_levels` verbatim (capitalized → "None / Low / Medium / Xhigh"), and the model-switch stale-level reset already handles cross-model vocabulary.

### 4. Edge cases

| Case | Outcome |
|---|---|
| Model ids containing `/` (e.g. `deepseek/deepseek-v4-flash`) | Unchanged — composed keys split at the first `/` (`resolve_composed_model`) |
| Aliased models (LiteLLM's same-model-under-two-names) | Each alias = its own `Model` row (the user picks the alias); exact-name duplicates → last-wins (existing `HashMap` behavior) |
| Discovery 404 / non-2xx / wrong API value on the row | Existing degradation verbatim: `unreachable`, 0 models, still shadows the base id; the user flips the API select back |
| Base URL entered without `/v1` | Works — both endpoint spellings are served (verified) |
| `max_input_tokens` absent | Fallback: `max_output_tokens`, then `DEFAULT_CONTEXT_WINDOW` |
| Empty `reasoning_effort_levels` | No Thinking selector for that model (existing rule) |
| Stale remembered level (a level change or model switch) | Existing switch-time reset: a level not in the model's advertised set → the remembered level or `None` |
| Empty API key (auth-less local proxy) | No `Authorization` header (existing rule) |
| Subagents on a `litellm` model | Unchanged — resolution runs against the parent's effective catalog; `litellm` is just a `Model` |

## Verification (TDD — failing test first)

- **Rust** (from `src-tauri/`: `cargo test`, `cargo clippy --all-targets` 0 warnings, `cargo fmt --check`):
  - `discover_models` `"litellm"` arm: happy path (all fields mapped from a fixture with `model_name` / `max_input_tokens` / `reasoning_effort_levels` / `supports_reasoning`), a bare entry → all-`None` meta, the `max_input_tokens` → `max_output_tokens` fallback, non-2xx → `Err` (reuse the existing `raw_json_server` test pattern).
  - `ModelCatalog::selectable()` includes a `litellm` model.
  - `build_provider` with `api: Some("litellm")` builds an `OpenAiCompatibleProvider`.
  - `EffectiveCatalog` stamps `api: Some("litellm")` on a LiteLLM provider's discovered models.
- **Frontend** (repo root: `pnpm test`, `pnpm build`): `SettingsPage.test.tsx` — the API select offers "LiteLLM"; `apiLabel("litellm")` renders.
- **Live smoke (manual, exit criterion)**: a provider row at `llm.wizards.town/v1` with `api: litellm` + key → 4 models with correct context windows; Qwen's Thinking selector shows None/Low/Medium/Xhigh; a prompt at `low` streams a Thinking block and tool calls work; the row's badge shows `4 models`.
