---
status: accepted
date: 2026-09-30
superseded-by:
---

# Desktop-owned provider connections in Settings (the config surface ADR 0012 deferred)

ADR 0012 seeded the native harness's model catalog + auth from the user's pi config files and **deferred** a desktop-native config surface ("a later decision, not v1"). That later decision is now: the desktop's **Settings page owns a provider-connections list** — each entry a `{ id, name, base_url, api_key }` stored in the desktop's own `settings.json` — with each provider's models **discovered live** from its `GET /v1/models` endpoint (the existing `discover_models` path). The pi-config seeding (ADR 0012) remains as a **transitional** catalog source: the effective catalog is the seeded models **merged with** the user's providers, and **on a provider-id clash the user's provider wins**. pi is being phased out of the desktop; the seeding path is scheduled for removal, after which the Settings list is the sole model source.

**Why:** the desktop is moving to be the harness (ADR 0011) and pi is being removed — a desktop-owned provider store is the durable home for "which model endpoints does this app talk to", and it lets a user who does NOT run pi configure the desktop standalone. The user list is the canonical surface; seeding is a bootstrap, not a competitor.

**Considered Options**

- **User list REPLACES seeding** (the Settings list is the only source): rejected for now — it breaks ADR 0012's zero-reconfig promise for existing pi users during the transition. It becomes the de-facto state automatically once the seeding path is removed.
- **User list primary + an "import from pi config" action:** rejected — the merge already gives every pi user their models for free; an import step is friction that the merge makes unnecessary.
- **Keep seeding + merge user providers (chosen):** no regression for pi users, and the merge rule (user wins on clash) means the user's explicit entries always beat a stale seeded one.

**Consequences**

- **`settings.json` becomes an API-key store** — keys are plaintext in the user's config dir (the same trust model as pi's `auth.json`). No encryption in v1 (a keyring integration is a later decision).
- **The effective catalog is a derived merge** — seeded + user providers, user-wins-on-clash. A user provider with no key and an unreachable endpoint simply yields 0 models (best-effort; the provider row still exists, a refresh re-attempts).
- **The desktop now has TWO model sources** during the transition — when pi is removed, the seeding path (and ADR 0012's file-format coupling) goes away and the Settings list is the sole source.
- **A user provider's models are assumed OpenAI-compatible** (`supports_tools: true`, `api: "openai-completions"`) — the same assumption the metadata-less seeded path makes (v1 is OpenAI-compatible only, ADR 0012).
