---
status: accepted
date: 2026-10-04
superseded-by:
---

# LiteLLM as a fourth `api` value (a discovery mode, not a fourth wire)

LiteLLM-proxy-backed providers (e.g. `llm.wizards.town`) advertise their models, thinking levels, and per-model capabilities at a NON-OpenAI discovery endpoint — `GET {base}/model/info` — which the standard `GET {base}/models` (served by the same proxy) does not carry. We decided: `api: "litellm"` is a FOURTH `api` value whose only new behavior is the **discovery mode** (`GET {base}/model/info`, Bearer auth, LiteLLM shape). The **wire stays `openai-completions`**: `build_provider` builds the existing `OpenAiCompatibleProvider` for it, `ModelCatalog::selectable()` accepts it alongside the three wires, and thinking levels are surfaced **verbatim** from `model_info.reasoning_effort_levels` (e.g. `["none","low","medium","xhigh"]`) with no `off`↔`none` normalization — the level string flows onto the wire as `reasoning_effort` as-is.

**Why:** a LiteLLM proxy always speaks OpenAI-compatible chat-completions for completion (verified live: `reasoning_content` deltas and `tool_calls` stream in the exact shapes the existing `SseStream` parses, and the harness already sends `x-litellm-session-id` / `x-request-id` on completions requests) — so there is no new wire to write. The genuinely different axis is DISCOVERY, and `api` already doubles as the discovery-shape selector (`discover_models` dispatches on it for the Anthropic vs. OpenAI `/models` shapes — ADR 0024), so extending it keeps one field, one dispatch point, and no settings migration.

**Considered Options**

- **A known-provider template only** (no new discovery): rejected — it adds nothing the user can't already do (a `openai-completions` row against the same base URL discovers via `/models`), and `/models` carries no reasoning fields, so thinking levels would be lost.
- **A separate `discovery` field on the provider row** (keeps `api` purely the wire): rejected — it doubles the surface (new persisted field, validation, a second UI select) for a distinction that always travels with the wire in practice; `api`'s meaning becomes "endpoint flavor: wire + discovery", which is what it already is (the `anthropic-messages` value does exactly this).
- **A fourth WIRE** (`OpenAiLiteLLMProvider`): rejected — the completion wire is empirically identical to `openai-completions`; a new struct would duplicate a request body and an SSE parser that already work.
- **Normalizing the level vocabulary** (`none`→`off` in the catalog, `off`→`none` on the wire): rejected — the desktop is already "the endpoint supplies everything" (`thinking_levels` is a free-form vec, the selector displays the provider's own words, per-model remembered levels are keyed by `provider/model`); a bidirectional mapping risks sending a level the proxy rejects and assumes LiteLLM's vocabulary is stable across deployments.

**Consequences**

- **`settings.json` `providers[].api` gains `"litellm"`** (no migration — a new value on an existing `String` field with a serde default).
- **`discover_models` gains a `"litellm"` arm**: `GET {base_url}/model/info`, Bearer auth (empty key → no header), parsing `data[].model_name` / `model_info.{max_input_tokens (+ max_output_tokens fallback), reasoning_effort_levels, supports_reasoning}`; `supports_function_calling`, the cost fields, and `default_reasoning_effort` are deliberately ignored for v1 (the `cost_per_mtok_*` seam stays at the documented 0.0).
- **`build_provider` routes `"litellm"` to the default (OpenAI) arm** — the arm that looks like it "should" build a LiteLLM-specific client; this is intentional (the wire IS OpenAI-compatible).
- **`ModelCatalog::selectable()` accepts `"litellm"`** (fourth value in the match).
- **The Settings' API select gains a "LiteLLM" option** (no known-provider template is added — LiteLLM base URLs differ per deployment, so a pre-filled template has no canonical base URL).
- **`default_reasoning_effort` is NOT parsed (v1)**: the level-resolution chain is untouched, a fresh LiteLLM session starts with no thinking level (the proxy's server-side default applies).
- **A user's existing `openai-completions` row against a LiteLLM proxy keeps working** (discovers via `/models`); switching its `api` to `litellm` (the UI select is editable) upgrades discovery to `/model/info` (thinking levels + capabilities + cost).
- **Amendment 2026-10-05 (god-file decomposition, Task 1): `api` is now a named type internally.** `agent::harness::WireApi` (wire.rs) is the wire vocabulary — a 4-variant enum (`OpenAiCompletions` / `AnthropicMessages` / `OpenAiResponses` / `LiteLLM`) with explicit `#[serde(rename)]` kebab-case strings and `parse` / `as_str` (the parse failures are logged where settings parsing tolerates unknowns). The DISCRIMINANT STRINGS are unchanged — `settings.json` and the Worker IPC `StartEnv` stay byte-identical (permissive storage stays `String` at `Model.api` / `ProviderConfig.api` / `KnownProviderDto.api`; only the dispatch sites — `build_provider`, `discover_models`, `selectable`, `catalog` — convert through `WireApi`). The TS side mirrors it: the `WireApi` union in `src/lib/tauri.ts`. The ADR 0023 level chain intentionally stays `&str` (Degrade vs Strict resolution paths are not unifiable without behavior change).
