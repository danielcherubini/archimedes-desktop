---
status: live
last-verified: 2026-09-23
verified-by: PR #2 (squash-merged to main as 4598028) — pnpm test (240) + pnpm build; cargo test (full suite) + cargo clippy (0 warnings) + cargo fmt; Greptile + CI green
---

# Model + thinking-level selection

The desktop lets the user select a **live** session's model and thinking level from the session header, over the ACP `configOptions` mechanism.

## How it works

- **Agent (pi-acp)** advertises `configOptions` in the `session/new` / `session/load` responses and pushes `config_option_update` notifications when the agent-side state changes.
- **Rust (Client)** carries the options in `SessionInfo` (populated from the `newSession` / `loadSession` responses — a stored session reports `None`; the agent is the source of truth, nothing is persisted) and relays `session/set_config_option` to the live agent via the `set_session_config_option` Tauri command (the agent's response carries the updated `configOptions`).
- **Frontend** keeps per-session `configOptions` in the sessions store — seeded on start/resume, replaced wholesale by `config_option_update` notifications, cleared on close — and renders two selectors in the session header (Model + Thinking; live sessions only; `category` match with `id` fallback; the selector is disabled while a set request is in flight; a transient inline error on failure auto-clears after 5 s).

## `agent-client-protocol` 2.1.0 schema facts (do not re-research)

`agent_client_protocol::schema::v1` (crate `agent-client-protocol` 2.1.0):

- `SessionConfigOption` — fields `id: SessionConfigId`, `name: String`, `description: Option<String>`, `category: Option<SessionConfigOptionCategory>`, `kind: SessionConfigKind`, `meta`; serde camelCase (`currentValue`, `options`, …).
- `SessionConfigKind::{Select(SessionConfigSelect), Boolean(SessionConfigBoolean)}`; `SessionConfigSelect { current_value: SessionConfigValueId, options: SessionConfigSelectOptions }`.
- **`SessionConfigSelectOptions` is a `#[serde(untagged)]` ENUM**: `Ungrouped(Vec<SessionConfigSelectOption>) | Grouped(Vec<SessionConfigSelectGroup>)` — a flat wire array deserializes to `Ungrouped`; there is **NO `.len()` on the enum** (match it). The frontend models the union as `(SessionConfigSelectOption | SessionConfigSelectGroup)[]` and renders groups with Radix `SelectGroup`/`SelectLabel`.
- `SessionConfigSelectOption { value: SessionConfigValueId, name, description }`.
- `SessionConfigOptionCategory::{Mode, Model, ModelConfig, ThoughtLevel, Other(String)}` — serde **snake_case** (`"model"`, `"thought_level"`).
- `SetSessionConfigOptionRequest::new(session_id, config_id: impl Into<SessionConfigId>, value: impl Into<SessionConfigOptionValue>)` (method `session/set_config_option`). **`From<&str>` exists for the VALUE, but `SessionConfigId` has `From` ONLY for `Arc<str>` / `String` / `&'static str`** — a borrowed `&str` does NOT convert; wrap it: `SessionConfigId::new(config_id)` (`new` takes `impl Into<Arc<str>>`).
- `SetSessionConfigOptionResponse { config_options: Vec<SessionConfigOption> }`; `NewSessionResponse.config_options: Option<Vec<SessionConfigOption>>`; `LoadSessionResponse.config_options: Option<Vec<SessionConfigOption>>`; `SessionUpdate::ConfigOptionUpdate(ConfigOptionUpdate { config_options: Vec<SessionConfigOption> })`.
- `load_session(...).block_task().start_session().await` returns `RestoredSession { session, response }` with **PRIVATE fields** — access the response via the accessor `restored.response() -> &LoadSessionResponse` (or `into_parts()` / `into_session()`); direct field access does NOT compile.
- **pi-acp ordering**: the `config_option_update` notification is emitted BEFORE the `session/set_config_option` response (the desktop relays the response's `configOptions` as the source of truth — the order is irrelevant to the store, which replaces wholesale).
