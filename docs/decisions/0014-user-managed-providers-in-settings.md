---
status: accepted
date: 2026-09-30
superseded-by:
---

# Desktop-owned provider connections in Settings (the sole model source)

The first-generation native harness seeded its model catalog + auth from the user's pi config files and **deferred** a desktop-native config surface ("a later decision, not v1"). That later decision is now: the desktop's **Settings page owns a provider-connections list** — each entry a `{ id, name, base_url, api_key }` stored in the desktop's own `settings.json` — with each provider's models **discovered live** from its `GET /v1/models` endpoint (the existing `discover_models` path). The Settings list is the **sole** model source (the pi-config seeding it superseded as a transitional source was removed by the native-harness-only rip-out, ADR 0022). The `merge_catalog` machinery remains (the effective catalog is the user's providers' discovered models; a provider-id shadow is a no-op with a single source).

**Why:** the desktop is the harness (ADR 0011) and pi is removed from it (ADR 0022) — a desktop-owned provider store is the durable home for "which model endpoints does this app talk to", and it lets a user who does NOT run pi configure the desktop standalone. The user list is the sole model source.

**Considered Options**

- **User list REPLACES seeding** (the Settings list is the only source): rejected during the transition — it broke the first-generation seeding's zero-reconfig promise for existing pi users. It became the de-facto state automatically once the seeding path was removed.
- **User list primary + an "import from pi config" action:** rejected — the merge already gave every pi user their models for free; an import step is friction that the merge made unnecessary.
- **Keep seeding + merge user providers (chosen during the transition):** no regression for pi users, and the merge rule (user wins on clash) meant the user's explicit entries always beat a stale seeded one. Superseded — the seeding path was removed (ADR 0022); the user list is the sole source.

**Consequences**

- **`settings.json` is an API-key store** — keys are plaintext in the user's config dir (the same trust model as pi's `auth.json`). No encryption in v1 (a keyring integration is a later decision).
- **The effective catalog is the user's providers' discovered models** — a user provider with no key and an unreachable endpoint simply yields 0 models (best-effort; the provider row still exists, a refresh re-attempts).
- **The desktop has ONE model source** — the Settings list (the seeding path and its pi-config file-format coupling were removed, ADR 0022).
- **A user provider's models are assumed OpenAI-compatible** (`supports_tools: true`, `api: "openai-completions"`) — v1 is OpenAI-compatible only.
