---
status: done
done-when: The gear icon in the left sidebar opens a ZCode-parity settings page (full view + 268px section sidebar + immediate save) where the user can set the theme (system/dark/light), fonts (size + UI/code family), default agent, trust-new-Spaces default, default model, and CRUD provider connections — all persisted in `settings.json` and taking effect per the wiring below.
---

# Settings Plan

**Goal:** Give the desktop its first user-facing settings surface — a ZCode-parity settings page (gear icon → full content-area view) covering theme, fonts, default agent, trust-new-Spaces default, default model, and user-managed provider connections (ADR 0014).

**Architecture:** One extended `Settings` document in `settings.json` (new fields all `#[serde(default)]`); the effective model catalog = the ADR 0012 pi-config seeded catalog merged with live-discovered user-provider models (user wins on provider-id clash, ADR 0014); settings take effect at session start (native model resolution chain, external `set_model`, `upsert_space` trust threading) and live in the UI (theme/font CSS application). The frontend settings page is a ZCode port: `SettingsGroupCard`/`SettingsRow`/`SettingsSidebarButton` primitives, 268px section sidebar, immediate save.

**Tech Stack:** Rust (Tauri 2 commands, serde, tokio, reqwest) · React 19 + TypeScript (ZCode-ported primitives, existing design-system tokens in `src/index.css`) · vitest + @testing-library/react · cargo test.

**Global rules (every task):**
- TDD: failing test first, confirm it fails, then make it pass.
- Rust verification (run from `src-tauri/`): `cargo test` → `cargo clippy --all-targets` (0 warnings) → `cargo fmt --check`.
- Frontend verification (run from the repo root): `pnpm test` → `pnpm build`.
- `settings.json` field names are camelCase over IPC (the Rust structs use `#[serde(rename_all = "camelCase")]`).
- Do NOT change the existing `paneLayout` behavior, the agent registry's built-in merge (`Registry::with_builtins`), or the `dark:`/`theme-zai-*` CSS token layer.

---

### Task 1: Rust — the extended Settings document

**Context:**
The backend already persists app settings in `<config_dir>/settings.json` via `Settings { theme, pane_layout }` with `get_settings` / `save_settings` Tauri commands (`src-tauri/src/commands/settings.rs`). This task extends that document with the new settings (default agent, trust-new-Spaces default, default model, provider list, font) and hardens `get_settings` against corrupt files. All new fields are `#[serde(default)]` so a pre-feature file (`{ "theme": "dark", "paneLayout": {} }`) parses to the defaults. The `theme` value space extends from `"dark" | "light"` to `"system" | "dark" | "light"` (ZCode's `THEME_MODES`); `Default` stays `"dark"`. A new pure helper `load_settings` (no Tauri state) is extracted so later tasks (session start) can read settings without the command layer.

**Files:**
- Modify: `src-tauri/src/commands/settings.rs`
- Test: same file (`#[cfg(test)] mod tests` — the existing tests there)

**What to implement:**
In `src-tauri/src/commands/settings.rs`:

1. Add the new types (all `Serialize + Deserialize + Clone + Debug + PartialEq` — `PartialEq` is REQUIRED: the specified tests compare with `==`; `serde_json::Value` is `PartialEq`, so `Settings` derives it fine):
   ```rust
   #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
   #[serde(rename_all = "camelCase")]
   pub struct ProviderConfig {
       pub id: String,        // stable slug (internal — generated ONCE at add time, never changes afterwards; see Task 5)
       pub name: String,
       pub base_url: String,  // normalized (…/v1)
       pub api_key: String,   // plaintext (empty = local gateway, no key)
   }

   #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
   #[serde(rename_all = "camelCase")]
   pub struct FontSettings {
       #[serde(default = "default_font_size")]
       pub size_px: u32,              // default 14
       #[serde(default)]
       pub ui_family: Option<String>, // None = the design system's pinned sans stack
       #[serde(default)]
       pub code_family: Option<String>, // None = the design system's pinned mono stack
   }
   fn default_font_size() -> u32 { 14 }

   /// HAND-WRITTEN (do NOT `#[derive(Default)]` — a derived `Default` would
   /// give `size_px: 0`, a second "default" that diverges from the serde
   /// missing-field default of 14):
   impl Default for FontSettings {
       fn default() -> Self {
           Self { size_px: default_font_size(), ui_family: None, code_family: None }
       }
   }
   ```
2. Extend `Settings` (the derive list gains `PartialEq` — the specified tests compare whole `Settings` values with `==`; it derives cleanly, `Value` is `PartialEq`):
   ```rust
   #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
   #[serde(rename_all = "camelCase")]
   pub struct Settings {
       #[serde(default = "default_theme")]
       pub theme: String,                    // "system" | "dark" | "light" (default "dark")
       fn default_theme() -> String { "dark".to_string() }  // (a private fn next to `default_font_size`)
       #[serde(default)]
       pub pane_layout: Value,               // unchanged
       #[serde(default)]
       pub default_agent: Option<String>,    // registry id; None = agents[0]
       #[serde(default)]
       pub default_trust_new_spaces: bool,   // default false
       #[serde(default)]
       pub default_model: Option<String>,    // "provider/id"; None = system default
       #[serde(default)]
       pub providers: Vec<ProviderConfig>,   // default []
       #[serde(default)]
       pub font: FontSettings,
   }
   ```
   (`Default` impl: `theme: "dark"` — which now equals the serde default, so a file WITHOUT a `theme` key parses to the same value; everything else the serde defaults — update the existing `default_settings_are_dark_with_empty_layout` test to assert the new defaults too. **The `theme` serde default matters for the Task 2/3 fixtures: a partial `settings.json` (e.g. `{"defaultModel": "x/y"}`) must PARSE — without this default it would be "corrupt" and silently yield `Settings::default()` with `providers: []` / `default_model: None`, producing assertion diffs that point nowhere near the real cause.**)
3. Extract a pure loader (no `State`, no Tauri):
   ```rust
   /// Read the settings from the config dir. A MISSING file → the defaults
   /// (written to disk, as today — a write failure is LOGGED via `eprintln!`
   /// and the defaults are still returned: the write is best-effort, the
   /// app must not fail to load). A CORRUPT file (unreadable OR unparseable)
   /// → the defaults + a logged warning (`eprintln!`), NOT an `Err` — a bad
   /// file must never block app startup; the file is left untouched until
   /// the next `save_settings`.
   pub fn load_settings(config_dir: &Path) -> Settings
   ```
   `get_settings` becomes a thin wrapper: `Ok(load_settings(&state.config_dir()))` — it now **always succeeds** (the `Result` is kept for command-shape stability but is never `Err`).
4. `save_settings` is unchanged (whole-file overwrite).

**Steps:**
- [ ] Write failing tests in `settings.rs`'s `mod tests` (the `PartialEq` derives make the `==` assertions compile):
  - `a_pre_feature_settings_json_parses_to_the_new_defaults` — `serde_json::from_str::<Settings>(r#"{ "theme": "dark", "paneLayout": {} }"#)` → `default_agent: None`, `default_trust_new_spaces: false`, `default_model: None`, `providers: []`, `font == FontSettings::default()` (the hand-written `Default` — `size_px: 14`).
  - `font_settings_default_is_size_14_not_zero` — `FontSettings::default().size_px == 14` (guards against a regression to a derived `Default`).
  - `settings_round_trip_with_the_new_fields` — full struct (incl. a provider + a font with families) → JSON (assert `"defaultAgent"`, `"defaultTrustNewSpaces"`, `"defaultModel"`, `"providers"`, `"font"`, `"sizePx"` present) → back `==` the original.
  - `load_settings_on_a_corrupt_file_returns_the_defaults` — temp dir, write `{ not json` to `settings.json` → `load_settings` == `Settings::default()` (and the file is NOT rewritten — re-read the raw text and assert it's still corrupt).
  - `load_settings_on_a_missing_file_writes_the_defaults` — temp dir, no file → defaults returned AND `settings.json` exists afterwards (the existing first-run behavior).
  - `get_settings_never_errors` — temp dir with a corrupt `settings.json` → `get_settings` (call the fn body's logic via `load_settings` — the command itself takes `State` and is covered by the `tests/ipc.rs` integration) returns `Ok` (the `== Settings::default()` assertion).
- [ ] Run `cargo test settings` (from `src-tauri/`)
  - Did the new tests fail (missing fields / `Err` on corrupt)? If they passed unexpectedly, stop and investigate why.
- [ ] Implement the changes above in `settings.rs`.
- [ ] Run `cargo test` (from `src-tauri/`)
  - Did all tests pass? If not, fix and re-run before continuing.
- [ ] Run `cargo clippy --all-targets` (0 warnings) and `cargo fmt`
- [ ] Commit with message: "feat(settings): extend the settings document (agents, trust, model, providers, font) + corrupt-file fallback"

**Acceptance criteria:**
- [ ] A pre-feature `settings.json` deserializes to the new defaults (no migration needed).
- [ ] `FontSettings::default()` is `size_px: 14` (hand-written `Default` — NOT a derived `0`).
- [ ] A corrupt `settings.json` yields the defaults + a logged warning (`get_settings` returns `Ok`).
- [ ] First run still writes the (now richer) defaults file (a write failure is logged, not fatal).
- [ ] `cargo test` + `cargo clippy --all-targets` + `cargo fmt --check` all green.

---

### Task 2: Rust — the effective catalog (seeded + user providers merged) + `list_models`

**Context:**
The native model catalog is seeded from pi's config at startup (`ModelCatalog::seed_from_pi_config`, `src-tauri/src/agent/harness/catalog.rs` — ADR 0012, transitional). ADR 0014 adds the desktop-owned provider store: the effective catalog = seeded models + models discovered from the user's `Settings.providers` (each via `GET {base_url}/models` — the existing `discover_models` in `catalog.rs`, 5s connect / 10s read, best-effort), with the **user provider winning on a provider-id clash — even if its discovery failed** (a transient failure must not resurrect stale seeded models under the same id; the provider row shows `unreachable` + refresh). This task adds the merge + a `list_models` command (the frontend's Default-model select + provider-row discovery status both read it).

**Files:**
- Modify: `src-tauri/src/agent/session.rs` (the `SessionManager` impl — the `discovery_cache` field lives there)
- Modify: `src-tauri/src/commands/settings.rs` (new command)
- Modify: `src-tauri/src/lib.rs` (register the command — next to `get_settings` / `save_settings`)
- Test: `session.rs` `mod tests` + `settings.rs` `mod tests`

**What to implement:**

1. A pure merge function (put it in `src-tauri/src/agent/harness/catalog.rs`, `pub`, unit-tested there):
   ```rust
   /// Merge the seeded catalog with the user-provider models (ADR 0014).
   /// `user_models` are the discovered models; `shadowed_provider_ids` are
   /// the ids of EVERY user provider configured in `settings.json` (regardless
   /// of whether its discovery succeeded — a provider that discovered 0 models
   /// STILL shadows the seeded models for its id: user-wins-on-clash, even
   /// on failure). For every id in `shadowed_provider_ids` the seeded models
   /// for that id are REPLACED by the user models with that id (possibly none).
   /// `default_model` is the seeded default unless it belongs to a shadowed
   /// provider (then `None` — the caller's resolution chain degrades).
   pub fn merge_catalog(
       seeded: &ModelCatalog,
       user_models: &[Model],
       shadowed_provider_ids: &[String],
   ) -> ModelCatalog
   ```
   (The `compaction` field carries over from `seeded`. The three-argument shape is REQUIRED — a `user_models`-only signature cannot express "a provider that discovered 0 models still shadows": its id would be absent from `user_models` and the merge would silently keep the stale seeded models.)
2. On `SessionManager` (`src-tauri/src/agent/session.rs`):
   ```rust
   /// The effective catalog: the seeded catalog (ADR 0012) + the user's
   /// providers from `settings.json` (fresh read via `load_settings`),
   /// discovered via `discover_models` (best-effort; the existing
   /// per-provider `discovery_cache` — `force_refresh` bypasses the cache
   /// for provider `force_refresh` when `Some`). A provider whose discovery
   /// fails contributes 0 models but still shadows the seeded models for
   /// its id (ADR 0014 — via `merge_catalog`'s `shadowed_provider_ids`).
   pub async fn effective_catalog(&self, force_refresh: Option<&str>) -> ModelCatalog
   ```
   (Implementation: `let settings = load_settings(&self.config_dir);` → for each `settings.providers` entry, discover (cache-aware; a failed/unreachable endpoint contributes `[]`) and build the `Model` rows → `merge_catalog(&self.catalog, &all_user_models, &settings.providers.iter().map(|p| p.id.clone()).collect::<Vec<_>>())`.)
   A discovered model becomes a `Model`: `id` = the discovered id, `provider` = the provider's `id`, `base_url` / `api_key` from the provider config, `context_window` = `meta.context_window.unwrap_or(DEFAULT_CONTEXT_WINDOW)` (`DiscoveredMeta.context_window` is `Option<u32>` — a user model has no static metadata, so the `catalog.rs` `DEFAULT_CONTEXT_WINDOW` constant (128000) is the fallback), `thinking_levels` / `supports_thinking` from the `DiscoveredMeta` (`None` → `vec![]` / `false` — the existing `refresh_model_metadata` field-mapping pattern, minus the static-value fallback), `cost_per_mtok_*: 0.0`, `supports_tools: true`, `api: Some("openai-completions")` (v1 is OpenAI-compatible only — ADR 0012).
3. New command in `src-tauri/src/commands/settings.rs` — with an explicit camelCase DTO (the `Model` struct has NO `#[serde(rename_all)]` — `serde_json::to_value(&model)` would yield `base_url` / `context_window` (snake_case) and break the camelCase-over-IPC rule):
   ```rust
   /// The effective catalog's models (the frontend's Default-model select +
   /// the provider rows' discovery status).
   /// `force_refresh` = a provider id whose discovery cache entry is bypassed
   /// (the settings page's refresh affordance); `None` = cached.
   #[tauri::command]
   pub async fn list_models(
       state: State<'_, Arc<SessionManager>>,
       force_refresh: Option<String>,
   ) -> Result<Vec<ModelDto>, String> {
       Ok(state.effective_catalog(force_refresh.as_deref()).await.models.iter().map(ModelDto::from).collect())
   }

   /// The camelCase wire shape (the `Model` struct itself is NOT renamed —
   /// `capabilities_json` embeds composed keys, not `Model`, so a DTO here
   /// is the safe choice over renaming the struct):
   #[derive(Debug, Clone, PartialEq, Serialize)]
   #[serde(rename_all = "camelCase")]
   pub struct ModelDto {
       pub id: String,
       pub provider: String,
       pub context_window: u32,
       pub supports_thinking: bool,
       pub thinking_levels: Vec<String>,
   }
   impl From<&Model> for ModelDto { /* field-by-field copy */ }
   ```
   (The frontend reads `id`, `provider`, `contextWindow`; the composed key is `"{provider}/{id}"`.) Register in `lib.rs` `generate_handler!` AND in `src-tauri/tests/ipc.rs` `build_app`'s SEPARATE `generate_handler!` list (the IPC integration tests have their own — a command registered only in `lib.rs` is invisible to them).

**Steps:**
- [ ] Make `DEFAULT_CONTEXT_WINDOW` visible to `session.rs`: in `src-tauri/src/agent/harness/catalog.rs` it is a PRIVATE `const DEFAULT_CONTEXT_WINDOW: u32 = 128000;` (line ~21) — make it `pub` and add it to `src-tauri/src/agent/harness/mod.rs`'s `pub use catalog::{…}` re-export (the `effective_catalog` impl + the `session.rs` tests reference it — as-is it is `E0603` private cross-module).
- [ ] Add the shared test helper to `src-tauri/src/test_support.rs` (the existing `#[doc(hidden)] pub mod` — already used cross-module as `crate::test_support::run_with_retry`): a MULTI-accept `raw_json_server` (the `catalog.rs` private one accepts exactly ONCE and is unreachable from other modules):
  ```rust
  /// Serve a fixed `GET` response for as many requests as arrive (until the
  /// `JoinHandle` is aborted); `counter` (when `Some`) is incremented per
  /// request (the cache tests count fetches). `status` + `body` mirror the
  /// `catalog.rs` `raw_json_server` (copy its raw-HTTP-response construction).
  pub async fn raw_json_server(
      listener: tokio::net::TcpListener,
      status: u16,
      body: &str,
      counter: Option<std::sync::Arc<std::sync::atomic::AtomicU32>>,
  ) -> tokio::task::JoinHandle<()>
  ```
- [ ] Write failing tests:
  - In `catalog.rs` `mod tests` (reuse the module's existing private `model(id, supports_tools, api)` helper where it fits; where a `provider` field is needed, use a full `Model` literal — `Model` has 11 fields and no `Default` derive):
    - `merge_catalog_replaces_shadowed_providers_even_with_zero_models` — seeded has `p/a` + `q/b` (default `p/a`); `user_models = [a model with provider "p", id "x"]`, `shadowed_provider_ids = ["p"]` → result has `p/x` (NOT `p/a`) + `q/b`; `default_model` → `None` (shadowed). Then `user_models = []`, `shadowed_provider_ids = ["p"]` → `p/a` is STILL replaced (zero user models → 0 models for `p`) — the case a two-argument signature cannot express.
    - `merge_catalog_with_no_user_providers_is_the_seeded_catalog` — `user_models = []`, `shadowed_provider_ids = []` → the seeded catalog unchanged (models + `default_model` + `compaction`).
  - In `session.rs` `mod tests` (use the NEW `crate::test_support::raw_json_server` multi-accept helper; a local temp-dir + `agents.json` setup as the existing `SessionManager::new` tests, and `set_catalog` to control the seeded side):
    - `effective_catalog_discovers_a_user_provider` — temp config dir with a `settings.json` whose `providers` = one entry (`base_url` = the local server, `api_key: "k"`, `id: "tama"`); `set_catalog` a seeded catalog with a DIFFERENT provider id; `effective_catalog(None)` → contains the discovered model (`provider == "tama"`, `api == Some("openai-completions")`, `context_window == DEFAULT_CONTEXT_WINDOW` when the response lacks `max_model_len`).
    - `effective_catalog_a_failed_discovery_still_shadows_the_seeded_provider` — same but the provider's `base_url` = `http://127.0.0.1:1/v1` (unreachable) and the seeded catalog has a model under the SAME provider id → the effective catalog has 0 models for that id (the seeded one is shadowed).
    - `effective_catalog_caches_and_force_refresh_bypasses_the_cache` — `effective_catalog(None)` twice with the `counter` → the server is hit exactly ONCE (the second call is cache-served); `effective_catalog(Some("tama"))` → hit again (the counter increments) — a new response body can be asserted via the counter-driven flow (serve body v1, then re-point the test at a second listener with body v2 for the force-refresh call).
  - In `settings.rs` `mod tests`: `list_models_dto_mapping` — a `ModelDto::from` over a `Model` literal → the serialized output carries the camelCase `contextWindow` key (`serde_json::to_value(&dto)`). NOTE: the `#[tauri::command]` fn takes `State` (unconstructable in a unit test — private field); the command is integration-tested in `tests/ipc.rs` (add it to that `generate_handler!` list + a test case there using the file's LOCAL `invoke(&webview, "list_models", serde_json::json!({}))` helper — `get_ipc_response` is tauri's non-generic two-arg API, NOT the project's call path — asserting on the returned `serde_json::Value` shape: `[{"id": …, "provider": …, "contextWindow": …}]`. Do NOT add `Deserialize` to `ModelDto` — it is wire-out-only by design; the Value-shape assertion is the mechanism.
- [ ] Run `cargo test` (from `src-tauri/`)
  - Did the new tests fail (function/command missing)? If they passed unexpectedly, stop and investigate why.
- [ ] Implement `merge_catalog`, `effective_catalog`, `list_models`; register the command in `lib.rs`.
- [ ] Run `cargo test` (from `src-tauri/`)
  - Did all tests pass? If not, fix and re-run before continuing.
- [ ] Run `cargo clippy --all-targets` (0 warnings) and `cargo fmt`
- [ ] Commit with message: "feat(settings): effective catalog (seeded + user providers, user-wins-on-clash) + list_models command"

**Acceptance criteria:**
- [ ] `merge_catalog` is pure + unit-tested (shadowing works with zero discovered models via `shadowed_provider_ids`; `default_model` degrades correctly; no user providers → seeded unchanged).
- [ ] `effective_catalog` reads `settings.json` fresh (a provider saved via `save_settings` is visible on the next call — no in-memory sync needed).
- [ ] Discovery is best-effort + cached per provider (the existing `discovery_cache`); `force_refresh` bypasses the cache for one provider.
- [ ] `list_models` is registered in BOTH `lib.rs` and `tests/ipc.rs`, and returns the camelCase `ModelDto` shape.
- [ ] `cargo test` + `cargo clippy --all-targets` + `cargo fmt --check` all green.

---

### Task 3: Rust — behavior wiring (native model chain, external `set_model`, trust threading)

**Context:**
With the settings document (Task 1) and the effective catalog (Task 2) in place, this task makes the settings take effect: (a) the native model resolution chain gains a middle rung (`Settings.default_model` between the per-agent `HarnessConfig.default_model` and the catalog default); (b) an external (pi) session started with a `Settings.default_model` of valid shape gets a `set_model` RPC at establishment (the existing `set_config_option` `model` path shape: `{ "type": "set_model", "provider", "modelId" }` — lenient: a failure is logged, the session still establishes on pi's own default); (c) new Space rows are born trusted per `Settings.default_trust_new_spaces` (`Db::upsert_space` threads the flag: INSERT sets `trusted`, CONFLICT leaves the existing flag untouched — no retroactive trust). **Scope decision (reviewer finding): the effective catalog is threaded through ALL session-facing catalog consumers** — `build_native_session` (the `AgentLoop::new` `catalog` arg + `synthesize_catalog_config_options` — the in-session model picker must list user-provider models), `set_config_option`'s native branch (switching to/from a user-provider model mid-session), and `resume_native_session` (the stored-key lookup + the resolution fallback). **Documented asymmetry (v1 scope): `set_subagent_manager`'s `NativeDeps.catalog` keeps the SEEDED catalog** — subagent-launch model overrides resolve against the seeded catalog (a user-provider model as a subagent launch override degrades to the default); re-injecting after discovery is v2 machinery.

**Files:**
- Modify: `src-tauri/src/agent/session.rs` (`resolve_native_model`, `build_native_session`, `resume_native_session`, the external establisher closure in `start_session`, `record_session`)
- Modify: `src-tauri/src/storage/db.rs` (`upsert_space`)
- Test: `session.rs` `mod tests` + `db.rs` `mod tests`

**What to implement:**

1. `resolve_native_model` — the new chain (keep the function, change the resolution order):
   ```rust
   /// per-agent `HarnessConfig.default_model` → `Settings.default_model`
   /// (the new middle rung — a fresh `load_settings` read) → the catalog's
   /// `default_model` → the v1-selectable (`openai_compatible`) set.
   fn resolve_native_model(
       catalog: &ModelCatalog,
       harness_default: &Option<String>,
       settings_default: &Option<String>,
   ) -> Result<Model, RpcError>
   ```
   **Fall-through semantics (reviewer finding — the OR-chain shape `harness_default.or(settings_default).or(catalog.default_model)` + one resolve + `openai_compatible().first()` FAILS the specified test: with `harness: None`, `settings: Some("gone/m1")` (unresolvable), it resolves `key = "gone/m1"`, fails, and falls straight to `openai_compatible().first()` — skipping the catalog default rung):** each rung is tried **in order** (harness → settings → catalog default), and an UNRESOLVABLE key at any rung falls through to the NEXT rung (only when all three rungs are absent/unresolvable does it fall to `openai_compatible().first()`). This also changes the current behavior for a stale harness key (it now falls through the settings/catalog rungs instead of straight to the set) — intended.
   Call sites: `build_native_session` (pass `load_settings(&self.config_dir).default_model`) and `resume_native_session` (the stored-model fallback — same chain). Note `catalog` here is the EFFECTIVE catalog (Task 2) — update the call sites to `let catalog = self.effective_catalog(None).await;` and pass `&catalog` (the function takes a reference).
2. External `set_model` at start — in `start_session`'s establisher closure (the `move |handle: PiRpcHandle| async move { ... }` block): **send `set_model` BEFORE the first `get_state`** (the `config_dir` is captured into the closure — `self.config_dir` is a `PathBuf`; clone it before the closure). "Resolvable" is defined as: `Settings.default_model` is `Some(key)` AND `key.split_once('/')` succeeds (a valid `"provider/id"` shape) — the CATALOG is NOT consulted: pi's own `get_available_models` is the real source for an external session, and a model pi doesn't know about is rejected by pi (the lenient path below). The sequence:
   ```rust
   // The default model (settings): a validly-shaped `default_model` →
   // `set_model` sent BEFORE `get_state` (LENIENT — a failure is logged and
   // the session establishes on pi's own default; an absent/unset setting
   // sends nothing). The `get_state` response then reflects the applied
   // model (`build_capabilities` picks up `state.model`), which the test
   // asserts on `info.capabilities.model`.
   if let Some(key) = load_settings(&config_dir).default_model {
       if let Some((provider, model_id)) = key.split_once('/') {
           if let Err(e) = handle.send(json!({ "type": "set_model", "provider": provider, "modelId": model_id })).await {
               eprintln!("settings default model: set_model failed at start: {e} (establishing on pi's default)");
           }
       }
   }
   // ... then the EXISTING `get_state` + `get_available_models` + `get_available_thinking_levels`
   // (UNCHANGED — `info` is built from this first `get_state`, which now
   // reflects the applied default model).
   ```
   (Only for `kind: external` — this code is already in the external branch. `fake_pi` supports this: its `set_model` handler updates `current_model_id` and its `get_state` response substitutes it — verified in `src-tauri/src/bin/fake_pi.rs`.)
3. Trust threading — `Db::upsert_space` (`src-tauri/src/storage/db.rs`):
   ```rust
   pub fn upsert_space(&self, path: &str, default_trusted: bool) -> Result<(), DbError>
   ```
   SQL: `INSERT INTO spaces (path, created_at, last_opened_at, trusted) VALUES (?1, ?2, ?2, ?3) ON CONFLICT(path) DO UPDATE SET last_opened_at = excluded.last_opened_at` (the `trusted` column is written ONLY on INSERT — the CONFLICT branch never touches it). Update the single call site in `record_session` (`session.rs`): `db.upsert_space(&info.cwd.display().to_string(), load_settings(&self.config_dir).default_trust_new_spaces)` — `record_session` is sync, so use the sync `load_settings` (Task 1) directly.
   **Update every other `upsert_space` call site** (search `upsert_space` — there are ~13: `permission.rs` tests ×3, `tests/rpc_flow.rs` ×3, `tests/subagent_trust.rs` ×1, `tests/harness_dispatch_native.rs` ×1, `tests/storage.rs` ×6+) to the new signature, passing **`false`** (the pre-change behavior — rows born untrusted via the schema default; `true` would break tests whose comments state "The space row exists (an UNTRUSTED row…)"). Only `record_session` passes the settings flag.
4. **Thread the effective catalog through the remaining consumers** (the reviewer finding — without this, user-provider models are half-integrated: the in-session picker wouldn't list them, `set_config_option` couldn't switch to them, and a resume on a user-provider model would fall back): in `build_native_session`, `set_config_option` (the native branch), and `resume_native_session`, replace `self.catalog` with `let catalog = self.effective_catalog(None).await;` (one fetch per entry point — the discovery cache makes repeated calls cheap): `AgentLoop::new`'s `catalog` arg, `synthesize_catalog_config_options(&catalog, …)`, `resolve_composed_model(&catalog, key)`, **the `set_config_option` native branch's direct model lookup (`self.catalog.models.iter().find(…)` — session.rs ~line 2655 — WITHOUT this a user-provider model can never be switched TO mid-session)**, and the `resolve_native_model` calls all take the effective catalog. (`refresh_model_metadata` is unchanged — it operates on a single `Model` + the `discovery_cache`.) `set_subagent_manager`'s `NativeDeps.catalog` STAYS `self.catalog.clone()` (the documented v1 asymmetry above).

**Steps:**
- [ ] Write failing tests:
  - In `session.rs` `mod tests` (the existing native-session test pattern — `SessionManager::new` on a temp config dir + `set_catalog` + a `set_provider_factory` mock):
    - `the_native_model_chain_settings_default_sits_between_per_agent_and_catalog` — temp config dir with a `settings.json` `default_model: "s/m1"`; a native entry with `harness.default_model: None`; a catalog with models `s/m1` + `c/m2` and `default_model: Some("c/m2")` (build the catalog with the existing test-catalog pattern — a full `Model` literal per model, or a local helper mirroring `catalog.rs`'s `model()` helper) → resolved model = `s/m1`. Then set `harness.default_model: Some("h/m3")` (add `h/m3` to the catalog) → resolved = `h/m3` (per-agent wins). Then `settings.json` `default_model: "gone/m1"` (not in the catalog) → resolved = the catalog default `c/m2` (the chain degrades).
    - `an_external_session_start_sends_set_model_for_the_settings_default` — the existing fake_pi pattern (the `fake_pi.rs` binary handles `set_model` — the existing test at ~line 4677 "the `set_model` command reaches the agent" shows the pattern): temp config dir `settings.json` `default_model: "fake/fake-model-2"` (a model `fake_pi`'s `GET_AVAILABLE_MODELS_DATA` lists) → `start_session` (the external `pi` entry — the existing test's agent setup) → assert `info.capabilities.model == "fake/fake-model-2"` (`build_capabilities` reads `state.model` from the `get_state` that `fake_pi` answers AFTER applying the `set_model` sent before it). Plus a negative: `default_model: None` → `info.capabilities.model == "fake/fake-model"` (`fake_pi`'s default — no `set_model` sent).
  - In `db.rs` `mod tests` (the existing temp-dir `Db::open` pattern — use **real temp dirs created with `std::env::temp_dir().join(uuid)` + `create_dir_all`** for the paths, NOT bare `/tmp/x`: a nonexistent path fails canonicalize and `Db::space_trusted` fails closed to `false` on a canonicalize failure, so asserting via `space_trusted` on a nonexistent path gives a misleading red):
    - `upsert_space_insert_sets_trusted_from_the_flag` — fresh db, `upsert_space(<real temp dir a>, true)` → the `spaces` row's `trusted` = 1; `upsert_space(<real temp dir b>, false)` → 0. **Assert the row's `trusted` via `list_spaces()` (or a direct row query, the `tests/storage.rs` style) — NOT via `space_trusted` (fail-closed on canonicalize failure).**
    - `upsert_space_conflict_leaves_the_existing_trust_untouched` — `upsert_space(<real temp dir c>, false)` then `set_space_trusted(<the same path>, true)` then `upsert_space(<the same path>, false)` → the row's `trusted` is STILL 1 (the CONFLICT branch doesn't touch it).
- [ ] Run `cargo test` (from `src-tauri/`)
  - Did the new tests fail (chain not threaded / flag not written)? If they passed unexpectedly, stop and investigate why.
- [ ] Implement the changes above.
- [ ] Run `cargo test` (from `src-tauri/`)
  - Did all tests pass (including the pre-existing `upsert_space` call sites — the signature change is a compile gate)? If not, fix and re-run before continuing.
- [ ] Run `cargo clippy --all-targets` (0 warnings) and `cargo fmt`
- [ ] Commit with message: "feat(settings): settings take effect (native model chain, external set_model, space trust default)"

**Acceptance criteria:**
- [ ] Native model resolution: per-agent `HarnessConfig.default_model` > `Settings.default_model` > catalog default > `openai_compatible` first (an unresolvable settings value degrades, never errors).
- [ ] An external session with a validly-shaped `Settings.default_model` gets a `set_model` at establishment (lenient — a failure never blocks the session; the `get_state`-based `info.capabilities.model` reflects it in the `fake_pi` test).
- [ ] A new Space row is born `trusted` per the setting; an existing Space's flag is never touched by a re-upsert.
- [ ] The effective catalog is threaded through `build_native_session` (the `AgentLoop` + the synthesized config options), `set_config_option` (native), and `resume_native_session` — the in-session model picker lists user-provider models and can switch to them.
- [ ] `NativeDeps.catalog` (subagents) stays seeded (the documented v1 asymmetry).
- [ ] `cargo test` + `cargo clippy --all-targets` + `cargo fmt --check` all green.

---

### Task 4: Frontend — settings load + theme/font application

**Context:**
The frontend has the `AppSettings` interface + `getSettings` / `saveSettings` bindings (`src/lib/tauri.ts`), but nothing consumes them: `main.tsx` hardcodes `applyThemeToDocument("zai-dark")` ("The app is dark by default (no switcher)"), and the font tokens (`--ui-font-size: 14px`, `--font-sans`, `--font-mono` in `src/index.css`) are static. This task wires the stored settings to the document: extend the `AppSettings` interface to the new shape (Task 1, camelCase), make `src/lib/theme.ts` resolve `"system"` (a `matchMedia("(prefers-color-scheme: dark)")` listener, applied live on OS change), and add a settings-application module that sets the font CSS variables (with clamping). It is the foundation Task 5's settings page saves into.

**Files:**
- Modify: `src/lib/tauri.ts` (the `AppSettings` interface)
- Modify: `src/lib/theme.ts` (+ its test `src/lib/theme.test.ts`)
- Create: `src/lib/settings.ts` (+ `src/lib/settings.test.ts`)
- Modify: `src/main.tsx`

**What to implement:**

1. `src/lib/tauri.ts` — replace the `AppSettings` interface:
   ```ts
   export interface ProviderConfig {
     id: string;
     name: string;
     baseUrl: string;
     apiKey: string;
   }
   export interface FontSettings {
     sizePx: number;
     uiFamily: string | null;
     codeFamily: string | null;
   }
   export interface AppSettings {
     theme: "system" | "dark" | "light";
     paneLayout: Record<string, unknown>;
     defaultAgent: string | null;
     defaultTrustNewSpaces: boolean;
     defaultModel: string | null;
     providers: ProviderConfig[];
     font: FontSettings;
   }
   ```
   Add the `listModels` binding (Task 2's command):
   ```ts
   /** The effective catalog's models (the Default-model select + provider discovery status). `forceRefresh` = a provider id whose discovery cache is bypassed. */
   export interface ModelDto {
     id: string;
     provider: string;
     contextWindow: number;
     supportsThinking: boolean;
     thinkingLevels: string[];
   }
   export async function listModels(forceRefresh?: string): Promise<ModelDto[]> {
     return invoke("list_models", { forceRefresh: forceRefresh ?? null });
   }
   ```
2. `src/lib/theme.ts` — extend (keep `applyThemeToDocument` working for the concrete themes):
   ```ts
   export type AppTheme = "zai-light" | "zai-dark";
   export type SettingsTheme = "system" | "dark" | "light";

   /** Resolve a settings theme to a concrete app theme ("system" → the OS scheme). */
   export function resolveTheme(theme: SettingsTheme): AppTheme;
   // "dark" → "zai-dark"; "light" → "zai-light"; "system" →
   // window.matchMedia("(prefers-color-scheme: dark)").matches ? "zai-dark" : "zai-light"
   // (jsdom: a missing `matchMedia` → "zai-dark" — the existing default; the
   // NewSpaceDialog tests already stub `matchMedia` with `matches: false`.)

   /** Apply a settings theme: resolve + `applyThemeToDocument`. Returns a
       cleanup function. For "system" it subscribes a `matchMedia`
       `"change"` listener that re-applies live (the listener is removed by
       the cleanup). For concrete themes the cleanup is a no-op function. */
   export function applySettingsTheme(theme: SettingsTheme): () => void;
   ```
3. `src/lib/settings.ts` — new module:
   ```ts
   import { getSettings, type AppSettings } from "./tauri";
   import { applySettingsTheme } from "./theme";

   /** The font-size clamp bounds (the settings UI's slider range). */
   export const MIN_FONT_PX = 12;
   export const MAX_FONT_PX = 20;

   /** Apply the settings' font to `document.documentElement` as CSS custom
       properties: `--ui-font-size: <clamped>px` always (set via
       `style.setProperty`); `--font-sans: "<family>, <tail>"` when
       `font.uiFamily` is non-null, else `style.removeProperty("--font-sans")`
       (a PREVIOUSLY-set inline override must be REMOVED when the family goes
       back to `null` — the `index.css` `@theme` value cannot override a stale
       inline custom property; same for `--font-mono`). The tails are the
       design system's existing stacks from `src/index.css` (reuse the
       literals verbatim: the sans tail `"ui-sans-serif, system-ui,
       sans-serif, "Apple Color Emoji", "Segoe UI Emoji", "Segoe UI Symbol",
       "Noto Color Emoji"` and the mono tail `"ui-monospace, SFMono-Regular,
       Menlo, Monaco, Consolas, "Liberation Mono", "Courier New", "Microsoft
       YaHei UI", "Microsoft YaHei", "PingFang SC", "Noto Sans CJK SC",
       monospace`). An out-of-range `sizePx` (a hand-edited file) is CLAMPED
       to [12, 20] (the file is not rewritten). */
   export function applySettingsFont(font: FontSettings): void;

   /** Apply BOTH (theme + font) — returns the theme cleanup. */
   export function applySettingsToDocument(settings: AppSettings): () => void;

   /** Load the settings + apply them (boot path). A `getSettings` failure
       (the backend's `get_settings` always succeeds in practice after Task 1
       — `load_settings` swallows the write-defaults IO error) → log + the
       concrete dark theme (the first-frame state stands). */
   export async function loadAndApplySettings(): Promise<AppSettings | null>;
   ```
4. `src/main.tsx` — replace the hardcoded `applyThemeToDocument("zai-dark")` with: keep the first-frame `applyThemeToDocument("zai-dark")` (no flash — the CSS default is dark), then `void loadAndApplySettings();` (the stored value applies as soon as it arrives; a `"system"` value subscribes the live `matchMedia` listener for the app's lifetime).

**Steps:**
- [ ] Write failing tests:
  - In `src/lib/theme.test.ts` (extend the existing file — it already tests `applyThemeToDocument`'s class toggling):
    - `resolveTheme_maps_concrete_themes` — `"dark"` → `"zai-dark"`, `"light"` → `"zai-light"`.
    - `resolveTheme_system_uses_the_os_scheme` — stub `window.matchMedia` (the pattern from `NewSpaceDialog.test.tsx` `beforeAll`: a factory returning `{ matches, addEventListener, ... }`) — `matches: true` → `"zai-dark"`, `matches: false` → `"zai-light"`.
    - `applySettingsTheme_system_re_applies_on_os_change` — use a CAPTURING `matchMedia` stub (the `NewSpaceDialog.test.tsx` stub's `addEventListener: vi.fn()` DISCARDS the callback — with it the listener can never be invoked and the cleanup assertion is vacuous): `addEventListener: (event: string, cb: () => void) => { listeners[event] = cb }` (a test-local `Record<string, () => void>`; `removeEventListener: vi.fn()`). Flow: `matches: true`, `applySettingsTheme("system")` → `zai-dark` classes; flip the stub's `matches` to `false` + invoke the captured `"change"` listener → `zai-light` classes; call the returned cleanup → assert `removeEventListener("change", cb)` was called with the same `cb` (the listener is removed).
  - In `src/lib/settings.test.ts` (new — jsdom, the real `document.documentElement`; **all custom-property assertions read via `document.documentElement.style.getPropertyValue(...)` — jsdom's `getComputedStyle` cascade for custom properties is unreliable, while the inline `style` read is reliable**):
    - `applySettingsFont_sets_the_size_variable_clamped` — `sizePx: 500` → `style.getPropertyValue("--ui-font-size") === "20px"`; `sizePx: 5` → `"12px"`; `sizePx: 14` → `"14px"`.
    - `applySettingsFont_families_override_with_the_existing_tails` — `uiFamily: "Inter"` → `--font-sans` starts with `"Inter, "`; `codeFamily: "JetBrains Mono"` → `--font-mono` starts with `"JetBrains Mono, "` and still contains `"Noto Sans CJK SC"` (the CJK tail is preserved); both `null` → `style.getPropertyValue` is `""` for both (the `index.css` values stand — nothing inline-set).
    - `applySettingsFont_removes_a_stale_family_override_when_set_back_to_null` — `applySettingsFont({ sizePx: 14, uiFamily: "Serif", codeFamily: null })` → `--font-sans` set; then `applySettingsFont({ sizePx: 14, uiFamily: null, codeFamily: null })` → `style.getPropertyValue("--font-sans") === ""` (the stale inline override is removed, not left behind).
    - `loadAndApplySettings_applies_the_stored_settings` — mock `../lib/tauri` (`vi.mock` with a `getSettings` resolving a full `AppSettings` incl. `theme: "system"` + a font) → the document has the dark classes (stub `matchMedia` `matches: true`) + the font variables; a `getSettings` rejection → resolves `null` + the dark classes stand (no throw).
- [ ] Run `pnpm test` (from the repo root)
  - Did the new tests fail (functions missing)? If they passed unexpectedly, stop and investigate why.
- [ ] Implement the changes above.
- [ ] Run `pnpm test` (from the repo root)
  - Did all tests pass (the pre-existing `theme.test.ts` + `main.tsx` behavior — the first-frame dark still applies)? If not, fix and re-run before continuing.
- [ ] Run `pnpm build` (from the repo root)
- [ ] Commit with message: "feat(settings): apply the stored theme (incl. system) + font to the document"

**Acceptance criteria:**
- [ ] Boot: first frame is dark (no flash), then the stored theme applies; `system` follows the OS scheme live.
- [ ] Font: `--ui-font-size` (clamped 12–20) + optional `--font-sans` / `--font-mono` overrides (existing tails preserved, CJK tail always kept; a `null` family REMOVES any stale inline override).
- [ ] The `AppSettings` interface matches the Rust struct's camelCase shape exactly (a `saveSettings` round-trip loses no field).
- [ ] `pnpm test` + `pnpm build` green.

---

### Task 5: Frontend — the settings page (ZCode port)

**Context:**
The ZCode-parity settings page itself (the approved design, spec §1/§4): a full content-area view — 268px section sidebar (back button + 3 sections: General / Appearance / Providers) + content, using ported ZCode primitives (`SettingsGroupCard`, `SettingsRow`, `SettingsSidebarButton` — same design-system tokens already in `src/index.css`) and immediate save (a control change → `saveSettings` with the complete document; text fields commit on blur/Enter; no save button, no dirty state). This task also ports the `NewSpaceDialog` default-agent precedence (selected > settings > `agents[0]`). The entry point (gear icon + view swap) is Task 6.

**Files:**
- Create: `src/components/settings/primitives.tsx` (+ `src/components/settings/primitives.test.tsx`)
- Create: `src/components/settings/SettingsPage.tsx` (+ `src/components/settings/SettingsPage.test.tsx`)
- Modify: `src/components/NewSpaceDialog.tsx` (+ `src/components/NewSpaceDialog.test.tsx`)

**What to implement:**

1. `src/components/settings/primitives.tsx` — port the three ZCode primitives (ZCode source, absolute paths — the ZCode repo is the sibling at `/home/daniel/Coding/AI/ZCode/`: `packages/ui/src/settings/SettingsPageParts.tsx` (`SettingsGroupCard` / `SettingsRow` / `SettingsBadge`) + `packages/ui/src/SettingsPage.tsx` (`SettingsSidebarButton`); the app's existing `src/components/ui/*` primitives — `button.tsx`, `select.tsx`, `input.tsx`, `card.tsx`, `tooltip.tsx`. **`switch.tsx` does NOT exist yet in `src/components/ui/` — create it in this task** from the ZCode `packages/ui/src/components/ui/switch.tsx` port, same Radix pattern as the existing `select.tsx` (a `Switch` component: a `button[role="switch"]` with `aria-checked`, `onCheckedChange`, the existing design tokens). **Return type: `ReactElement` (`import type { ReactElement } from "react"`) — do NOT use `JSX.Element`: the project's `@types/react@19` removed the global `JSX` namespace (`tsc` fails with TS2503; no existing component uses it).**
   ```tsx
   /** A settings section card: a rounded-xl card hosting rows (ZCode
       `SettingsGroupCard`). */
   export function SettingsGroupCard({ children }: { children: ReactNode }): ReactElement;
   // <Card className="overflow-hidden rounded-xl border border-border bg-card py-0 shadow-none">
   //   <CardContent className="space-y-0 px-0">{children}</CardContent>
   // </Card>

   /** One settings row: label + description left, control right (ZCode
       `SettingsRow` — `grid-cols-[minmax(0,1fr)_192px]`, `border-t` rows,
       `first:border-t-0`, `px-4 py-3`). */
   export function SettingsRow({ label, description, control, detail, controlLayout = "default" }: {
     label: ReactNode;
     description?: ReactNode;
     control: ReactNode;
     detail?: ReactNode;
     controlLayout?: "default" | "wide";
   }): ReactElement;

   /** A section-nav button (ZCode `SettingsSidebarButton` — `h-8 w-full
       rounded-xl px-2.5`, icon + label, active = `bg-surface-hover
       text-foreground`, inactive = `text-foreground-subtle
       hover:bg-surface-hover hover:text-foreground`; a `ControlHintTooltip`
       equivalent = the existing `src/components/ui/tooltip.tsx`). */
   export function SettingsSidebarButton({ icon: Icon, label, active, onClick }: {
     icon: LucideIcon;
     label: string;
     active?: boolean;
     onClick?: () => void;
 }): ReactElement;
   ```
2. `src/components/settings/SettingsPage.tsx` — the page:
   ```tsx
   export default function SettingsPage({ onBack }: { onBack: () => void }): ReactElement
   ```
   - **Layout**: `grid h-full grid-cols-[268px_minmax(0,1fr)]` — left: the section sidebar (`flex flex-col`: the back button — `ArrowLeftIcon` + "Back", the ZCode `SettingsSidebarButton` ghost style — then the 3 `SettingsSidebarButton`s: `Settings2Icon` "General", `PaletteIcon` "Appearance", `PackageIcon` "Providers"); right: `overflow-y-auto` content for the active section. Local `useState` for the active section (default `"general"`).
   - **Data**: on mount, `getSettings()` → local `settings` state (the single source of truth for the page); `listAgents()` (the General default-agent select) and `listModels()` (the General default-model select + the Providers rows' discovery status) on mount.
   - **The immediate-save pattern** (one helper, used by every control — the theme cleanup is HELD IN A REF and replaced on each apply: `applySettingsToDocument` (Task 4) returns a `matchMedia` listener cleanup for `"system"` themes, and discarding it would leak a listener per control change):
     ```ts
     const themeCleanupRef = useRef<(() => void) | null>(null);
     const update = (patch: Partial<AppSettings>) => {
       const next = { ...settings, ...patch };
       setSettings(next);
       void saveSettings(next);        // the complete document
       themeCleanupRef.current?.();    // drop the previous theme listener
       themeCleanupRef.current = applySettingsToDocument(next); // live theme/font
     };
     // + a `useEffect` cleanup on unmount: `return () => themeCleanupRef.current?.();`
     ```
   - **General section** (one `SettingsGroupCard`):
     - `SettingsRow` "Default agent" — description "The agent new sessions start with" — a `Select` (the existing `src/components/ui/select.tsx`) over `listAgents()` options + an implicit default: selecting an agent sets `defaultAgent: agent.id`; add a "Default (first in the list)" option with value `""` that sets `defaultAgent: null`.
     - `SettingsRow` "Trust new Spaces by default" — description "New Spaces start trusted (skip permission prompts for bash/edit/write); existing Spaces are unaffected" — a `Switch` bound to `defaultTrustNewSpaces`.
     - `SettingsRow` "Default model" — description "The model new sessions start with (both native and external sessions)" — a `Select` over `listModels()` options (label = `"{provider}/{id}"`, value = the composed key) + a "System default" option (value `""` → `defaultModel: null`).
   - **Appearance section** (one `SettingsGroupCard`):
     - `SettingsRow` "Theme" — a `Select` over `system` / `dark` / `light` (labels "System (follow the OS)" / "Dark" / "Light") → `update({ theme })`.
     - `SettingsRow` "Font size" — description "The UI base size (12–20px)" — the ZCode `FontSizeInput` pattern: a `w-28` `Input type="number"` (`min=12 max=20 step=1`, a `px` suffix span, local draft state, commit on blur/Enter (clamped + `update({ font: { ...settings.font, sizePx: n } })`), Escape cancels the draft.
     - `SettingsRow` "UI font" — a `Select`: "System (default)" (value `""` → `uiFamily: null`) / "Serif" (`"ui-serif, Georgia, serif"`) / "Monospace" (`"ui-monospace, SFMono-Regular, Menlo, monospace"`) → `update({ font: { ...settings.font, uiFamily } })`.
     - `SettingsRow` "Code font" — a `Select`: "System (default)" (`""` → `null`) / "JetBrains Mono" / "Fira Code" / "Cascadia Code" / "Menlo" / "Consolas" (each value = the bare family name) → `update({ font: { ...settings.font, codeFamily } })`.
   - **Providers section** — a `SettingsGroupCard` with one `SettingsRow` per `settings.providers` entry + an "Add provider" button below the card:
     - Per-row fields (the row's `control` is a `flex` of the inputs; use `controlLayout="wide"`): **Name** (`Input`, `w-40`), **Base URL** (`Input`, `w-56`, placeholder `https://example.com/v1`), **API key** (`Input type="password"` — the ZCode masking pattern: a local `showKey` state per row + an eye `Button` (`EyeIcon` / `EyeOffIcon`) toggling `type` between `password` / `text`; the real value is always what's saved; empty = "no key (local gateway)"), **discovery status** (a `span`: `{n} models` when the provider's models are present in the `listModels()` result, `unreachable` when the provider exists in settings but has 0 models, `checking…` while a refresh is in flight) + a refresh `Button` (**`RefreshCwIcon`** — `RefreshIcon` does NOT exist in the installed `lucide-react@1.47`; `RefreshCw` is the family's canonical name → `RefreshCwIcon`) → `listModels(provider.id)` → update the local models state), **remove** `Button` (`TrashIcon`).
     - **Field commit** (immediate save): each text field commits on blur/Enter — `update({ providers: nextProviders })` then `void listModels(changedProviderId).then(setModels)` (the changed provider's discovery re-runs — Task 2's `force_refresh` bypasses its cache).
     - **Add provider** (a `Button` below the card): append an empty editable row `{ id, name: "", baseUrl: "", apiKey: "" }` — **the `id` is generated ONCE at add time and NEVER changes afterwards** (including when the name is committed later — a later name commit must NOT regenerate it, or `defaultModel` references + the discovery cache entries would silently orphan): `id = slug(name)` when a name is already known at add time, else `provider-N` (N = the list length + 1). `slug` = lowercase alphanumeric + `-` (the ZCode-style slug); de-duped against existing ids with a `-2` / `-3` suffix. The new row is editable immediately (immediate save on commit).
     - **Remove**: a `AlertDialog` (the existing `src/components/ui/alert-dialog.tsx`) — title "Remove provider {name}?", description "Its {n} models will leave the model picker. Existing sessions are unaffected." — confirm → `update({ providers: without })`.
     - **Empty state** (no providers): a `SettingsRow`-less empty slot in the card — "No providers yet — add one to connect a model provider."
3. `src/components/NewSpaceDialog.tsx` — the default-agent precedence (the current line 45: `const effectiveAgentId = selectedAgentId || agents[0]?.id || "";`):
   - Read the settings once on mount, **following the file's existing `listAgents()` fetch pattern (a `.catch(…)` — a settings failure → `null`, NEVER an unhandled rejection: `getSettings` rejects in jsdom without the Tauri internals, and an unhandled rejection fails the vitest run)** (`getSettings().then(setSettings).catch(() => null)` → `settingsRef` / state — the dialog is modal, a single fetch is fine):
   ```ts
   const effectiveAgentId =
     selectedAgentId ||
     (settings?.defaultAgent && agents.some((a) => a.id === settings.defaultAgent)
       ? settings.defaultAgent
       : null) ||
     agents[0]?.id ||
     "";
   ```
   (An unknown `defaultAgent` — the agent was removed from `agents.json` — falls back to `agents[0]`.)

**Steps:**
- [ ] Write failing tests:
  - In `src/components/settings/primitives.test.tsx` (new — render each primitive):
    - `SettingsRow_renders_label_description_and_control` — the label + description + control are in the document; the row has the `border-t` class (and `first:border-t-0` is present in the className).
    - `SettingsSidebarButton_active_state` — `active` → `bg-surface-hover` in the className; `onClick` fires on click.
    - `SettingsGroupCard_renders_children_in_a_card`.
  - In `src/components/settings/SettingsPage.test.tsx` (new — `vi.mock("../../lib/tauri")` — **note the `../../` depth: the test lives in `src/components/settings/`, so `../lib/tauri` would resolve to the non-existent `src/components/lib/tauri`** — with `getSettings` resolving a full `AppSettings` (theme `"dark"`, `font` defaults, one provider `{ id: "tama", name: "Tama", baseUrl: "https://tama.wizards.town/v1", apiKey: "k" }`), `listAgents` resolving `[{ id: "pi", name: "Pi" }, { id: "archimedes", name: "Archimedes" }]`, `listModels` resolving `[{ id: "Qwen3.8", provider: "tama", contextWindow: 128000, supportsThinking: false, thinkingLevels: [] }]` (the `ModelDto` camelCase shape — Task 2), `saveSettings` a `vi.fn().mockResolvedValue(undefined)`, `listModels` spyable; **`beforeAll` stubs: the pointer-capture set from `SessionConfigSelect.test.tsx` lines 5–19** (`Element.prototype.scrollIntoView` / `hasPointerCapture` / `setPointerCapture` / `releasePointerCapture` — REQUIRED: the page's selects are the Radix `ui/select.tsx` port, and opening one in jsdom throws `hasPointerCapture is not a function` without the stubs; the `NewSpaceDialog.test.tsx` pattern alone is NOT sufficient — it only stubs `matchMedia` + `scrollIntoView` for a native `<select>`) **plus the `matchMedia` stub from the same `SessionConfigSelect.test.tsx` `beforeAll`**):
    - `renders_the_three_sections_and_the_back_button` — "General" / "Appearance" / "Providers" nav buttons + the back button; clicking "Appearance" shows the Theme + Font size rows; clicking "Providers" shows the provider row (name "Tama").
    - `immediate_save_a_control_change_saves_the_complete_document` — open the Theme select, choose "Light" → `saveSettings` called once with a complete `AppSettings` (assert `theme: "light"` AND the other fields intact — `defaultAgent: null`, `providers` length 1).
    - `a_provider_field_commits_on_blur_and_refreshes_discovery` — type a name in the provider row's Name input + `fireEvent.blur` → `saveSettings` called with the updated `providers[0].name` AND `listModels` called with `"tama"` (the force-refresh).
    - `add_provider_appends_a_row_with_a_generated_id` — click "Add provider" → a new empty row exists (a Name input with the placeholder) whose `id` is `provider-2` (generated at add time — the list already has one provider); type "Tama" (the same name as the existing one) + `fireEvent.blur` → `saveSettings` called with `providers[1].name === "Tama"` AND `providers[1].id === "provider-2"` (the id is UNCHANGED by the name commit — the stability rule above).
    - `remove_provider_confirms_then_saves` — click the row's remove button → the `AlertDialog` appears ("Remove provider Tama?"); confirm → `saveSettings` called with `providers: []`.
    - `the_default_model_select_lists_the_catalog_and_system_default` — the Default model select shows `tama/Qwen3.8` + "System default"; choosing the model → `saveSettings` with `defaultModel: "tama/Qwen3.8"`; choosing "System default" → `defaultModel: null`.
  - In `src/components/NewSpaceDialog.test.tsx` (extend — add a `getSettings` mock to the existing `vi.mock` block; **DEFAULT the mock to a full `AppSettings` with `defaultAgent: null` so the existing tests' `agents[0]` behavior holds**):
    - `the_settings_default_agent_wins_over_agents_0` — `getSettings` resolving `defaultAgent: "archimedes"` → the dialog's effective selection is `archimedes` (assert via the existing test's select-value mechanism); `getSettings` resolving `defaultAgent: "gone"` (not in `listAgents`) → the effective selection is `agents[0]` (`pi`).
  - In `src/components/SpacesList.test.tsx` (extend — **REQUIRED: three existing tests mount `NewSpaceDialog` ("opens the Open Space dialog from the action button" / "…from the New Session button when no session is active" / "…for an orphaned active session"), and the file's `vi.mock("../lib/tauri")` spreads `importActual` but does NOT mock `getSettings` — after Task 5's `NewSpaceDialog` change, the real `getSettings()` runs in jsdom → `@tauri-apps/api/core`'s `invoke` rejects; the `NewSpaceDialog` `.catch` (above) keeps it from being an unhandled rejection, but the mock is still added so the dialog's effective selection is deterministic in those tests**: add `getSettings: vi.fn().mockResolvedValue(<a full `AppSettings` with `defaultAgent: null`>)` to the existing mock block).
- [ ] Run `pnpm test` (from the repo root)
  - Did the new tests fail (components missing)? If they passed unexpectedly, stop and investigate why.
- [ ] Implement the primitives, the page, and the `NewSpaceDialog` precedence.
- [ ] Run `pnpm test` (from the repo root)
  - Did all tests pass (the pre-existing `NewSpaceDialog.test.tsx` + `SpacesList.test.tsx` — the `getSettings` mocks default to `defaultAgent: null` so the existing `agents[0]` behavior holds; the `NewSpaceDialog` fetch's `.catch` prevents unhandled rejections)? If not, fix and re-run before continuing.
- [ ] Run `pnpm build` (from the repo root)
- [ ] Commit with message: "feat(settings): the settings page (ZCode port — sections, immediate save, provider CRUD) + NewSpaceDialog default-agent precedence"

**Acceptance criteria:**
- [ ] The page renders all three sections with the ported primitives (`ReactElement` return types — no `JSX.Element`); the back button calls `onBack`.
- [ ] Every control change saves the COMPLETE document (immediate save; no save button, no dirty state); theme/font apply live via Task 4's `applySettingsToDocument` (the theme cleanup is ref-held — no `matchMedia` listener leak per change).
- [ ] Provider CRUD: add (id generated once at add time, de-duped, never regenerated on name commit), field commit (blur/Enter → save + force-refreshed discovery status), remove (confirm dialog), empty state.
- [ ] `NewSpaceDialog`: selected > `settings.defaultAgent` (when in the registry) > `agents[0]`.
- [ ] `pnpm test` + `pnpm build` green.

---

### Task 6: Frontend — the entry point (gear icon + view swap)

**Context:**
The last piece: the gear icon in the left sidebar footer (bottom right, mirroring ZCode's `WorkspaceSidebarFooter` position) opens the settings page (Task 5) as a full content-area view — while open, the app's left sidebar is hidden/inert (the ZCode pattern: `opacity-0 pointer-events-none` + the `inert` attribute, so the sidebar is neither visible nor focusable/clickable). The back button returns to the workspace (the last active session view is preserved — the sessions live in Rust, so nothing is lost; the `SpacesList` / `ChatStream` / `SidePane` components simply unmount + remount, re-reading the stores).

**Files:**
- Modify: `src/components/SpacesList.tsx` (+ `src/components/SpacesList.test.tsx`)
- Modify: `src/App.tsx` (+ create `src/App.test.tsx`)

**What to implement:**

1. `src/components/SpacesList.tsx` — the footer:
   - New prop: `onOpenSettings?: () => void` (**optional** — the component currently takes no props; optional keeps the existing `SpacesList.test.tsx` render calls untouched. Add to the component's signature; it reads the stores).
   - A footer row at the bottom of the `aside` (after the scrollable session list, before the closing tag): `border-t border-border/50 p-2` — a right-aligned `SettingsIcon` button (lucide `Settings` icon, `size-4`, `aria-label="Settings"`, the existing `rounded-md size-6 hover:bg-surface-hover` button pattern from the `SpaceGroup` hover actions) → `onOpenSettings?.()`.
2. `src/App.tsx` — the view swap:
   ```tsx
   const [view, setView] = useState<"workspace" | "settings">("workspace");
   ```
   - The content row (`<div className="flex min-h-0 flex-1">`): when `view === "settings"` → render `<SettingsPage onBack={() => setView("workspace")} />` (full width — the `SpacesList` is NOT rendered in the settings view; the `SettingsPage`'s own 268px section sidebar is the left edge). When `view === "workspace"` → the existing `<SpacesList onOpenSettings={() => setView("settings")} /> <ChatStream /> <SidePane /> <SubagentDetailHost />` (unchanged).
   - (The ZCode `opacity-0 + inert` pattern is for keeping the workspace MOUNTED while settings is open; here the workspace unmounts — the stores are the source of truth and re-hydrate on remount, so the simpler unmount is used. The `inert`/`opacity` classes are therefore NOT needed.)
   - Do NOT change the boot `useEffect`s (session/space loading) or the Tauri listener `useEffect` — they are view-independent.

**Steps:**
- [ ] Write failing tests:
  - In `src/components/SpacesList.test.tsx` (extend — the existing file renders `SpacesList` with mocked stores):
    - `the_gear_icon_opens_settings` — render with `onOpenSettings` as a `vi.fn()` → a button with `aria-label="Settings"` exists; click it → the callback is called. Render WITHOUT the prop (the existing tests' shape) → the button still renders (the optional prop is guarded with `onOpenSettings?.()`).
  - In `src/App.test.tsx` (new — `vi.mock("./lib/tauri")` with the full mock pattern from `NewSpaceDialog.test.tsx`: `listSessions` / `listSpaces` resolving `[]`, every `listen*` function resolving a no-op unlisten, `listSkills` resolving `[]` (the `useSkillCatalog` hook in both `SpacesList` and `ChatStream` calls it on mount — the hook catches rejections, but the mock avoids noisy failures with a bare `vi.mock` factory); the `matchMedia` stub; **AND `vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => ({ minimize: vi.fn(), toggleMaximize: vi.fn(), close: vi.fn() }) })` — REQUIRED: `App` renders `WindowControls`, which calls `getCurrentWindow()` from `@tauri-apps/api/window` on EVERY render (its own test file documents this and mocks the module); in jsdom `window.__TAURI_INTERNALS__` is undefined → `TypeError` during render → every case fails without it** (the `WindowControls.test.tsx` mock pattern)):
    - `the_gear_icon_swaps_to_the_settings_view` — render `App` → assert the workspace is visible (a "Sessions" heading from `SpacesList`); click the `aria-label="Settings"` button → the settings view is visible (assert the "General" / "Appearance" / "Providers" section nav from `SettingsPage` — mock `SettingsPage`'s data deps: `getSettings` / `listAgents` / `listModels` in the same `vi.mock` block) AND the workspace is gone (the "Sessions" heading is absent).
    - `the_back_button_returns_to_the_workspace` — from the settings view, click the back button → the "Sessions" heading is visible again.
- [ ] Run `pnpm test` (from the repo root)
  - Did the new tests fail (view swap missing)? If they passed unexpectedly, stop and investigate why.
- [ ] Implement the footer + the view swap.
- [ ] Run `pnpm test` (from the repo root)
  - Did all tests pass (the pre-existing `SpacesList.test.tsx` — the `onOpenSettings` prop is OPTIONAL, so the existing render calls are untouched)? If not, fix and re-run before continuing.
- [ ] Run `pnpm build` (from the repo root)
- [ ] Run the full verification suite (the branch is ready when ALL of these are green): `pnpm test` + `pnpm build` (repo root), `cargo test` + `cargo clippy --all-targets` + `cargo fmt --check` (`src-tauri/`).
- [ ] Commit with message: "feat(settings): the gear icon + settings view swap"

**Acceptance criteria:**
- [ ] The gear icon (left sidebar footer, bottom right) opens the settings page; the back button returns to the workspace (the last active session view is preserved).
- [ ] While the settings view is open, the workspace (incl. the left sidebar) is unmounted; the app's Tauri listeners + stores are view-independent (a session running in the background keeps streaming into the stores).
- [ ] The full verification suite (AGENTS.md) is green.
- [ ] `done-when` met: the user can open the settings page and change every setting listed in the front-matter exit condition, see the change apply (theme/font live; the rest on the next session/Space), and have it survive a restart (`settings.json`).
