---
status: approved
done-when: The gear icon in the left sidebar opens a ZCode-parity settings page (full view + 268px section sidebar + immediate save) where the user can set the theme (system/dark/light), fonts (size + UI/code family), default agent, trust-new-Spaces default, default model, and CRUD provider connections — all persisted in `settings.json` and taking effect per the wiring below.
---

# Settings (ZCode-parity settings page)

The desktop's first user-facing settings surface (ADR 0006 deferred it: "dark by default, no user-facing theme switcher — the Client has no settings surface in this scope"). The UI is a ZCode port (ADR 0006's design source): a full content-area settings page with a 268px section sidebar, `SettingsGroupCard`/`SettingsRow` primitives, and immediate save. Provider connections are the desktop-owned config surface ADR 0012 deferred (ADR 0014).

## 1. Entry point and UI shape (ZCode parity)

- **Gear icon** in the left sidebar footer (bottom right, mirroring ZCode's `WorkspaceSidebarFooter` position).
- **The settings page is a full content-area view**: it replaces the main area; while open, the app's left sidebar is hidden/inert (ZCode's `opacity-0 pointer-events-none` + `inert` pattern).
- **Back button** (ArrowLeft) at the top of the section sidebar → returns to the workspace (the last active session view is preserved — sessions run in Rust, so nothing is lost).
- **Layout**: `grid-cols-[268px_minmax(0,1fr)]` — 268px section sidebar + content.
- **Sections** (a single group): **General** · **Appearance** · **Providers**.
- **Ported primitives** (from ZCode, same design-system tokens already in `index.css`): `SettingsGroupCard` (rounded-xl card), `SettingsRow` (label + description left, control right in a 192px column, `border-t` rows), `SettingsSidebarButton` (icon + label nav button, active = `bg-surface-hover`).
- **Immediate save**: a control change → `save_settings` (the complete document); text fields commit on blur/Enter; no save button, no dirty state.

## 2. Data model (one `settings.json`, extended)

```rust
pub struct Settings {
    // existing:
    pub theme: String,                    // "dark" | "light"
    pub pane_layout: Value,               // free-form, frontend-owned (unchanged)

    // new (all #[serde(default)] — existing files parse unchanged):
    pub default_agent: Option<String>,    // agent registry `id`; None = the registry default (agents[0])
    pub default_trust_new_spaces: bool,   // default false
    pub default_model: Option<String>,    // composed key "provider/id"; None = system default
    pub providers: Vec<ProviderConfig>,   // default []
    pub font: FontSettings,              // default { size_px: 14, ui_family: None, code_family: None }
}

pub struct ProviderConfig {
    pub id: String,        // stable slug, auto-generated from the name on add (internal — not user-editable)
    pub name: String,      // display name
    pub base_url: String,  // normalized via the existing `normalize_base_url` (…/v1)
    pub api_key: String,   // plaintext (same trust model as pi's `auth.json` — the user's config dir)
}

pub struct FontSettings {
    pub size_px: u32,              // default 14; the UI clamps 12–20
    pub ui_family: Option<String>, // None = the design system's pinned sans stack
    pub code_family: Option<String> // None = the design system's pinned mono stack
}
```

- **Theme extends to `"system" | "dark" | "light"`** (ZCode's `THEME_MODES`). `system` = follow the OS scheme via a `matchMedia("(prefers-color-scheme: dark)")` listener, applied live on OS change.
- **Backward compatibility**: every new field is `#[serde(default)]` — a pre-feature `settings.json` (`{ "theme": "dark", "paneLayout": {} }`) deserializes to the defaults.
- `get_settings` still writes the (now richer) defaults on first run.
- **Robustness**: parse failure → return the defaults + log a warning (the corrupt file is left untouched until the next save) — a bad file can never block app startup (replaces the current `Err` on parse failure).
- `save_settings` still overwrites the whole file (the frontend sends the complete document).
- API keys are plaintext in the config dir (the same trust model as pi's `auth.json`; no encryption in v1 — a keyring integration is a later decision).

## 3. Behavior wiring (how each setting takes effect)

**Theme**
- Boot: `main.tsx` keeps the first-frame `applyThemeToDocument("zai-dark")` (no flash), then the frontend fetches `get_settings` and applies the stored value (replacing the "no switcher" hardcode).
- `system`: a `matchMedia` listener re-applies live on OS scheme change.
- A change in the settings page applies immediately (the same function).

**Font**
- Boot + on change: set CSS custom properties on `document.documentElement`:
  - `--ui-font-size: <size>px` (out-of-range values from a hand-edited file are clamped to 12–20 when applying — the file is NOT rewritten)
  - `--font-sans: <family>, <existing fallback tail>` (only when `ui_family` is set)
  - `--font-mono: <family>, <existing CJK tail>` (only when `code_family` is set — the CJK fallback is always preserved)

**Default agent**
- `NewSpaceDialog`: `effectiveAgentId = selectedAgentId || settings.default_agent (when present in the registry) || agents[0]?.id` — an unknown id (agent removed from `agents.json`) falls back to `agents[0]`.

**Trust new Spaces by default**
- `upsert_space` becomes `upsert_space(path, default_trusted)`: on INSERT, `trusted = ?3`; on conflict the existing flag is **untouched** (a Space already created while the flag was off is NOT retro-trusted when it's flipped on — the flag only affects *new* Spaces).
- The session-start caller reads the current `Settings` and passes `default_trust_new_spaces` (default `false` — ADR 0010's per-Space semantics are unchanged).

**Default model**
- **Native**: the resolution chain gains a middle rung: `HarnessConfig.default_model` (per-agent registry) → `Settings.default_model` → `ModelCatalog.default_model` (the seeded pi default).
- **External**: at session start, if `Settings.default_model` is set **and** resolves in the merged catalog, the desktop sends `set_model` (the existing RPC path — the same one the composer's model picker uses). Unset or unresolvable (provider deleted, discovery failed) → no RPC; the session starts on pi's own default.

**Providers + catalog merge**
- The model catalog used by native sessions and the model picker = **seeded models (transitional, ADR 0012) + models discovered from user providers** (ADR 0014).
- **Merge rule**: on a provider-id clash, the **user provider wins — even if its discovery failed** (a transient failure does not resurrect stale seeded models under the same id; the provider row shows `unreachable` + a refresh affordance).
- **Discovery**: on provider add/edit commit → `GET {base_url}/models` (the existing `discover_models` — 5s connect / 10s read, best-effort). A discovered model becomes a `Model` entry: id = the discovered id, provider = the provider's `id`, `base_url`/`api_key` from the provider config, `supports_tools: true`, `api: Some("openai-completions")` (v1 is OpenAI-compatible only — the same assumption the metadata-less seeded path makes).
- The existing per-provider discovery cache is reused (at most one fetch per provider per boot; a failed endpoint is not retried per session) — the settings page's **refresh affordance** bypasses the "already attempted" cache for that provider.
- Provider **removal** → its models drop out of the catalog; existing sessions are unaffected (the model is pinned per session).
- The **Default model** select lists the merged catalog (seeded models remain selectable — they still work).

## 4. Providers section UI

- One `SettingsGroupCard` with one `SettingsRow` per provider, fields per row:
  - **Name** (text input)
  - **Base URL** (text input, e.g. `https://tama.wizards.town/v1`)
  - **API key** (masked input — `••••` — with a show/hide eye toggle; the toggle is local state only, the real value is always what's saved; empty = "no key (local gateway)")
  - **Discovery status** (right slot): `3 models` (success) · `0 models` (empty response) · `unreachable` (failure) · `checking…` (in flight), plus a small **refresh icon** that re-runs discovery (bypasses the per-boot "already attempted" cache)
  - **Remove** (trash icon)
- **Commit model** (the immediate-save rule): a field commits on blur/Enter → `save_settings` → if name/URL/key changed, discovery re-runs.
- **Add provider** (a button below the card): appends an empty editable row; on commit, the internal `id` is auto-generated (slugified name, de-duped with a numeric suffix, e.g. `tama-2`) and discovery runs.
- **Removal**: a confirm dialog (the existing `alert-dialog` primitive): *"Remove provider X? Its N models will leave the model picker. Existing sessions are unaffected."*
- **Empty state**: "No providers yet — add one to connect a model provider." + the Add button.

## 5. Edge cases

- **No `settings.json`** → `get_settings` writes the full defaults (existing behavior, now including the new fields).
- **Corrupt `settings.json`** → `get_settings` returns the defaults + logs a warning (the file is left untouched until the next save) — a bad file can never block app startup.
- **`theme: "system"` + OS scheme changes** → the `matchMedia` listener re-applies live.
- **`default_model` unresolvable** (its provider was removed, or discovery failed) → native: the resolution chain continues to the next rung; external: the `set_model` is skipped and the session starts on pi's own default.
- **`default_agent` unknown** (agent removed from `agents.json`) → falls back to `agents[0]`.
- **Font size out of range** (hand-edited file, e.g. `500`) → the frontend clamps to 12–20 when applying (the file is not rewritten).
- **Provider id generation** (two providers named "Tama") → the slug is de-duped with a numeric suffix (`tama`, `tama-2`).
- **Removing a provider that is the current `default_model`** → the setting is left as-is (it simply becomes unresolvable → the fallbacks above); the user is not forced to change it.

## 6. Tests (TDD — the repo's convention)

- **Rust**:
  - `Settings` serde round-trip with the new fields, including a pre-feature JSON (without the new keys) parsing to the defaults
  - `resolve_native_model` chain with a `Settings.default_model` middle rung
  - the catalog merge (user provider wins on a provider-id clash, including a failed discovery)
  - `upsert_space` trust-default threading (INSERT sets `trusted` from the flag; CONFLICT leaves it untouched)
  - `get_settings` corrupt-file fallback (defaults + warning, no `Err`)
- **Frontend**:
  - the settings page renders all three sections with the ZCode row primitives
  - immediate save (a control change → `save_settings` called with the complete document)
  - theme application including `system` (mocked `matchMedia`)
  - font variable application (including clamping)
  - provider CRUD (add → id generation + discovery status; remove → confirm dialog)
  - `NewSpaceDialog` default-agent precedence (selected > settings > `agents[0]`)
