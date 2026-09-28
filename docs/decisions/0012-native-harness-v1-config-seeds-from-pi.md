---
status: accepted
date: 2026-09-27
superseded-by:
---

# Native harness v1 config: seed from pi's existing config (no new config surface)

The native harness (ADR 0011) needs a model catalog and provider auth to run a native session. **v1 adds no new user-facing config surface**: instead, the desktop **seeds** its model catalog + auth from the user's existing pi config files — `~/.pi/agent/settings.json` (`defaultProvider`, `defaultModel`, `enabledModels`), `~/.pi/agent/auth.json` (API keys), and `~/.pi/agent/models-store.json` (model metadata: context window, cost, capabilities). The user's existing pi setup works in the native harness with zero reconfiguration.

**Auth is ambient (env-first), not only `auth.json`.** A provider's `api_key` is resolved **env-first** (matching the `pi-provider-litellm` ambient-auth pattern): for provider `p` (uppercased `P`), try `P_API_KEY` → `P_TOKEN` → `P_KEY` and use the first that is set and **non-empty** (a `tama` gateway is keyed by `TAMA_TOKEN`; `eurouter`/`openrouter`/`google` by `<NAME>_API_KEY`); then `auth.json[provider].key`; a provider with neither (a local gateway needing no key) → empty. Env-first means a stored key can be overridden without editing `auth.json`, and a provider absent from `auth.json` (like `tama`) still authenticates (fixes the `tama` 401 — the harness now sends `Authorization: Bearer $TAMA_TOKEN`).

**Model metadata can be refreshed from the live endpoint.** Beyond the static `models-store.json`, the harness can query a provider's `GET /v1/models` (the standard OpenAI discovery endpoint — `tama` and OpenAI-compatible gateways expose it) for fresh model metadata (`max_model_len` → context window, `reasoningLevels`, `supportsReasoningEffort`), mirroring the `pi-provider-litellm` `fetchModels` pattern. This is **best-effort + cached per-provider** (at most one fetch per provider; a failed/unreachable endpoint is not retried every session); a failure or an absent model degrades to the static `models-store.json` metadata. The OpenAI endpoint thus "supplies everything" — the static file is a fallback, not the only source.

**Why:** the desktop is a *consumer* of the user's existing agent setup, not a competitor to it. Re-seeding from pi's config means a user who already uses pi (which this user does — the desktop's only v1 agent is pi) gets a working native agent for free. A new desktop config surface would force the user to re-enter providers, models, and API keys from scratch — friction with no v1 benefit.

**Considered Options**

- **A new desktop config surface** (the desktop owns its own provider/model/auth config, independent of pi): rejected for v1 — clean in the long run, but forces the user to reconfigure everything; the cost is not justified before the native harness is proven. Revisit once the native harness is the default and pi is optional.
- **A Node sidecar that reuses pi's `@earendil-works/pi-ai` provider SDK** (all pi providers for free): rejected — a second binary + an inference protocol (ADR 0011), and the user's entire current model set is OpenAI-compatible, so a single Rust OpenAI-compatible client covers it.

**Consequences**

- **The desktop reads pi's config files** — a coupling to pi's config file *format*. If pi changes those files' shape, the desktop's seeding breaks (a maintenance dependency). Mitigation: the seeding is a *best-effort bootstrap* — a missing/unparseable file degrades to "no seeded model" (the user can still add models manually), not a crash.
- **Auth is ambient (env-first)** — a provider's `api_key` comes from an env var (`<P>_API_KEY` → `<P>_TOKEN` → `<P>_KEY`) before `auth.json`. This couples to the env-var *naming convention* (a provider must export its key as `<P>_<SUFFIX>`); a provider that uses a non-standard env name is not auto-authenticated (it needs an `auth.json` entry). This matches how the user's pi setup already authenticates (e.g. `TAMA_TOKEN`), so no new config surface is added.
- **The desktop does not write to pi's config files** — it reads them (seed + refresh on startup). The user's pi config remains the source of truth for pi; the desktop's model catalog is a *derived* view (seeded from pi's config, plus any models the user adds in the desktop).
- **A desktop-native config surface is deferred** — it becomes the source of truth once the native harness is the default agent and pi is optional (a later decision, not v1). Until then, "just works with your pi setup" is the v1 contract.
- **v1 provider scope is OpenAI-compatible only** — the seeded catalog is filtered to OpenAI-compatible models (the desktop's single provider client); non-OpenAI-compatible pi models are present in the catalog but not selectable in v1 (added later as the desktop gains more provider clients).
