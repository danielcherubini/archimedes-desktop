---
status: accepted
date: 2026-10-06
superseded-by:
---

# Multi-wire model providers (Anthropic + OpenAI Responses) and a known-providers picker

The native harness spoke **OpenAI-compatible chat-completions only** (ADR 0012's "v1 is OpenAI-compatible only") and every user provider was ASSUMED OpenAI-compatible (ADR 0014). We decided: the harness speaks **three wire APIs** — `openai-completions` (the existing `OpenAiCompatibleProvider`), `anthropic-messages` (a new `AnthropicProvider`), and `openai-responses` (a new `OpenAiResponsesProvider`) — selected per model via the `Model.api` discriminator the catalog already carried, and the Settings' **Providers section gains a known-providers picker**: a built-in catalog of provider templates (name, base URL, wire API, key-management URL) the user can add as a **pre-filled provider row** with one click (the user still pastes the API key).

**Why:** the desktop's only model source is the user's provider list (ADR 0014), and adding a provider meant hand-typing a base URL — error-prone, and a dead end for the large providers whose best endpoints are NOT OpenAI-compatible (Anthropic, xAI, OpenRouter, the Chinese GLM/Kimi/MiniMax/DeepSeek/Bailian endpoints). The templates are seeded from the ZCode builtin provider catalog (`config/provider/zcode-builtin.json` — 20 user-facing templates). The picker does NOT bypass ADR 0014: a picked template becomes a normal `settings.json` provider row (the user's list remains the sole model source; the base catalog stays empty).

**Considered Options**

- **Pre-seed the provider list on first run** (every known provider as a keyless row): rejected — a keyless row whose endpoint needs a key shows `unreachable` / 0 models; the first-run screen would be a wall of failed rows. Opt-in per provider keeps the list the user actually uses.
- **Seed the base catalog with static models**: rejected — contradicts ADR 0014's "the user list is the sole model source" (the base catalog is empty by design) and the base catalog has no key storage, so the user would still add a provider row for the key — the static models would be dead weight.
- **A frontend-only known-providers constant** (no Rust involvement): rejected — the wire API is a harness concern (the `Provider` factory dispatches on `Model.api`), and the picker's pre-fill value (`api`) must match the harness's vocabulary; a Rust-owned catalog with a Tauri command keeps one source of truth.
- **Port the `anthropic-messages` wire from ZCode's TS adapters**: rejected — ZCode's adapters are a Vercel-AI-SDK TS stack; the harness is Rust and already owns its SSE parsing. The wire contracts are re-implemented natively (the `Provider` trait seam exists for exactly this, ADR 0012's "a second provider (Anthropic) can be added later without touching the loop").
- **One generic "adapter" with a protocol table**: rejected — the three wires differ structurally (Anthropic's `event:`-tagged SSE + `system`-as-top-level-param + alternating-role messages; Responses' `input`/`instructions` + `output` items) more than a table can express; three small, well-tested providers beat one giant switch.

**Consequences**

- **`ProviderConfig` gains `api`** (`"openai-completions"` (default) / `"anthropic-messages"` / `"openai-responses"`) + an optional `keyUrl` (set by the picker; a "Get key" link in the row). `#[serde(default)]` — a pre-feature `settings.json` parses to the OpenAI-compatible default (no migration). Discovered models carry the provider's `api` (no longer hard-coded `openai-completions`).
- **The selectable set generalizes** — `ModelCatalog::openai_compatible()` becomes `selectable()`: `supports_tools` + `api` ∈ the three supported wires (a `None`/unknown `api` stays unselectable, as before).
- **`ProviderFactory` dispatches on `Model.api`** (the `AgentLoop` is untouched — the seam ADR 0012 designed for).
- **Discovery is per-wire** — `openai-completions` / `openai-responses` keep the OpenAI `GET {base}/models` shape; `anthropic-messages` uses Anthropic's `GET {base}/models` (an id-only list — a discovered Anthropic model gets the `DEFAULT_CONTEXT_WINDOW` fallback and advertises no thinking levels; v1 does not fetch per-model metadata from Anthropic).
- **Thinking levels map per-wire** — `openai-completions` passes `reasoning_effort` through (today's behavior); `openai-responses` sends `reasoning: { effort }`; `anthropic-messages` maps the desktop's level vocabulary to a `thinking` budget (`low` 4096 / `medium` 10000 / `high` 20000 / `xhigh` 40000 / `max` 128000; an unrecognized level → `adaptive`), and the request's `max_tokens` is clamped above the budget (Anthropic requires `max_tokens` — a `None` option defaults to 32768).
- **The known-providers catalog is a Rust constant** (20 ZCode templates) exposed via a stateless `list_known_providers` command (the `list_tools` pattern); the picker pre-fills `name` / `baseUrl` / `api` / `keyUrl` and an empty key.
- **A user provider's models are NO LONGER assumed OpenAI-compatible** (ADR 0014's consequence updated) — the provider row's `api` decides the wire.
