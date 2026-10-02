---
status: live
last-verified: 2026-09-23
verified-by: PR #2 (squash-merged to main as 4598028) — pnpm test (240) + pnpm build; cargo test (full suite) + cargo clippy (0 warnings) + cargo fmt; Greptile + CI green
---

# Model + thinking-level selection

The desktop lets the user select a **live** session's model and thinking level from the session header, over the `configOptions` mechanism — the options are SYNTHESIZED in-process by the desktop's native harness (ADR 0011 / 0022: the desktop IS the harness — there is no external agent to advertise them).

## How it works

- **Harness (in-process, `src-tauri/src/agent/session.rs`)** is the source of truth: `synthesize_catalog_config_options` builds the `config_options` from the `ModelCatalog` (the EFFECTIVE catalog — the base models + the user's Settings providers, ADR 0014 — so a user-provider model can be switched TO mid-session, not just the base ones):
  - a **model** selector (`id: "model"`, `name: "Model"`, `category: "model"`, `type: "select"`) — options are the catalog's `openai_compatible()` models: `value` the composed `"<provider>/<id>"` key, `name` the bare model id; `currentValue` the session's current model key;
  - a **thinking-level** selector (`id: "thought_level"`, `name: "Thinking"`, `category: "thought_level"`, `type: "select"`) — options are the CURRENT model's advertised `thinking_levels` (display name the capitalized level, `"medium"` → `"Medium"`); `currentValue` the session's current level (empty when none is set); the selector is ABSENT when the model advertises no levels.
  - It runs at session start AND resume (the `SessionInfo.config_options` — a resume re-synthesizes fresh state from the stored `capabilities.model` + the catalog, with the stored `thinkingLevel`), and after every `set_config_option`.
- **Rust (Client, `src-tauri/src/commands/sessions.rs`)**: the `set_session_config_option` command → `SessionManager::set_config_option`: validates the value (a model: the composed key must resolve in the effective catalog; a level: non-empty), applies it through the loop's control channel (`ControlCmd::SetModel` / `SetThinkingLevel` — the loop applies them when idle; a model switch also applies the ADR 0015 minimal-surprise thinking-level reset: the level is KEPT when valid for the new model, replaced — the new model's remembered level, or `None` — only when the new model doesn't support it), mirrors the change on the handle's config state, RE-SYNTHESIZES the options from the catalog, and emits the `config_option_update` frame (the loop does NOT emit one itself — the client owns the frame). Mirror + emit ONLY when the `try_send` SUCCEEDS: a full / closed control queue means the loop never applies the change — claiming success (a mirrored state + a `config_option_update`) would silently diverge from the model / level the loop is actually running.
- **Persistence**: the Client does NOT persist config options — a stored session's `SessionInfo.config_options` is `None` (the options come back with the next start / resume, re-synthesized from the stored `capabilities` — a stale / unknown model key falls back to the resolution chain, never a hard error). The harness is the source of truth.
- **Frontend** keeps per-session `configOptions` in the sessions store — seeded on start/resume (the `SessionInfo.configOptions`), replaced wholesale by `config_option_update` notifications (the `set_session_config_option` command's response is applied through `applyConfigOptions` — the same wholesale replace), cleared on close — and renders two selectors in the session header (Model + Thinking; live sessions only; `category` match with `id` fallback, `type === "select"` + non-empty `options`; the selector is disabled while a set request is in flight; a transient inline error on failure auto-clears after 5 s).

## Wire shape facts (the synthesized JSON — do not re-research)

`synthesize_catalog_config_options` emits plain JSON (no crate types — the `agent-client-protocol` crate is gone, ADR 0022):

- Each option: `{ id, name, category, type: "select", currentValue, options }` (camelCase `currentValue`; `category` snake_case — `"model"` / `"thought_level"`). The frontend's `SessionConfigOption` (`src/lib/tauri.ts`) models `type: "select" | "boolean"` and `options` as a flat `(SessionConfigSelectOption | SessionConfigSelectGroup)[]` — the synthesizer emits FLAT `{ value, name }` entries (no groups, no `description`).
- The `set_session_config_option` command returns the re-synthesized `config_options` array (the frontend applies it through `applyConfigOptions`); the emitted `config_option_update` frame carries the same array — the store replaces wholesale, so the order of the two paths is irrelevant.
