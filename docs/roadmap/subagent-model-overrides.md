---
status: committed
done-when: The Settings page has a Subagents section where a discovered agent's model override is settable; a subagent session for that agent runs with the overridden model (observable via the `subagent-session-started` event / the Delegating card) without the agent's markdown file being modified; the thinking-order code fix lands (explicit > suffix > frontmatter `thinking`); all validation gates green (pnpm test, pnpm build, cargo test, cargo clippy --all-targets, cargo fmt --check)
---

# Per-agent subagent model overrides in Settings — Plan

**Goal:** Let the user override a named agent's model from the Settings page (a `subagentModels` map in `settings.json`), layered between the explicit `subagent` tool param and the frontmatter (explicit > override > frontmatter > parent model — ADR 0023), without ever writing to the agent markdown files; plus the thinking-order code fix (explicit > `:<level>` suffix > frontmatter `thinking`).

**Architecture:** A new `Settings.subagent_models` field (agent name → model key) read at dispatch time by `AgentLoop::resolve_launch` (which gains a `config_dir` field) and inserted as a soft layer between the explicit param and the frontmatter. A new stateless `list_agent_definitions` Tauri command feeds a new **Subagents** section on the Settings page (one row per user-level discovered agent, a model `Select` defaulting to "File value (no override)", immediate save). The `LaunchConfig.thinking` field is split so `dispatch_native_inner` can resolve thinking in the doc-correct order.

**Tech Stack:** Rust (Tauri 2 backend, serde JSON settings) + React 19 / TypeScript frontend. TDD throughout: failing test first, confirm it fails, then make it pass.

**Context for the executing agent (no conversation memory — read this fully):**
- `Settings` lives in `src-tauri/src/commands/settings.rs` (`settings.json` in the config dir; all fields `#[serde(default)]`; `load_settings` returns defaults for a missing/corrupt file and NEVER errors).
- `LaunchConfig` lives in `src-tauri/src/agent/subagent.rs`; `resolve_launch` lives in `src-tauri/src/agent/harness/loop.rs` (the `AgentLoop` impl); the model/thinking resolution for the child happens in `SubagentSessionManager::dispatch_native_inner` (`src-tauri/src/agent/subagent.rs`).
- Current model layering (ADR 0020): explicit param (strict — unknown fails the dispatch) → frontmatter `model` (soft — a stale value degrades to the next layer) → parent model. Current thinking: `launch.thinking.or(level_suffix)` where `launch.thinking` was already merged with the frontmatter `thinking` in `resolve_launch` — i.e. the code's order is explicit > frontmatter > suffix, but the feature doc (`docs/features/subagent-sessions.md`) says explicit > suffix > frontmatter. **This plan fixes the code to match the doc** (the user decided the doc is right).
- `AgentDefinition` / `discover_agents` live in `src-tauri/src/agents.rs` (flat `*.md` files; `discover_agents(None)` = user-level roots only: `~/.agents/agents` + `~/.pi/agent/agents`).
- The frontend Settings page (`src/components/settings/SettingsPage.tsx`) has sections `general` / `appearance` / `providers` / `mcp` (a `Section` type union at the top); immediate save = `update(patch)` → `saveSettings` with the COMPLETE document; `remapModelRef(s)` (defined in the same file) remaps model refs when a provider name is committed. Tauri bindings live in `src/lib/tauri.ts` (`AppSettings` interface + `invoke` wrappers).
- Rust test infra (in the `#[cfg(test)] mod tests` of `loop.rs`): `build_loop_with_db` / `build_loop` (build a test `AgentLoop`, single-model catalog, `config_dir: None`), `build_loop_with_subagent` (parent loop + a `SubagentSessionManager` with `NativeDeps`, multi-model catalog, `config_dir: None`), `make_native_manager`, `RecordingSink` (captures EVERY event incl. `subagent-session-started`), `fake_model(id)` (a `fake` provider model, empty `thinking_levels` = soft-pass), `ScriptedProvider`, `wait_for_event`. The reference e2e test is `a_native_subagent_with_a_matching_agent_name_runs_the_frontmatter_config` (writes an agent file into `loop_.space_cwd/.agents/agents/`, drives a parent turn, asserts the `subagent-session-started` payload). The `resolve_launch_*` unit tests use the `env_lock()` + `RestoreHome` + empty-scratch-`HOME` pattern to isolate the user-level agent roots (see `resolve_launch_unknown_agent_name_returns_the_launch_verbatim`).
- **Clippy is a hard gate** (`cargo clippy --all-targets` must be 0 warnings) — keep new code warning-free (e.g. no unused fields, no needless clones).

---

### Task 1: `Settings.subagent_models` field (schema)

**Context:**
The override values live in `settings.json` next to the other user-managed settings (providers ADR 0014, MCP ADR 0019). This task adds the field only — no behavior yet. A pre-feature file must parse to an empty map (no migration); a corrupt file already yields the defaults (existing contract, untouched).

**Files:**
- Modify: `src-tauri/src/commands/settings.rs`

**What to implement:**
- Add to the `Settings` struct (after the `default_thinking_levels` field, keeping the doc-comment style of the neighbors):
  ```rust
  /// (ADR 0023) Per-agent subagent model overrides: agent name → model key
  /// (`"provider/id"`, or a hand-edited `"provider/id:<level>"` — the
  /// `:<level>` suffix is picked up by the dispatch's existing thinking
  /// candidate handling). `#[serde(default)]` — a pre-feature file parses
  /// to an empty map (no migration).
  #[serde(default)]
  pub subagent_models: HashMap<String, String>,
  ```
- Add `subagent_models: HashMap::new(),` to the hand-written `impl Default for Settings` (the `FontSettings`-style hand-written-default rule applies to `Settings` too — the impl is the single source of defaults).
- Do NOT change any other field, loader, or command.

**Steps:**
- [ ] In the `#[cfg(test)] mod tests` of `settings.rs`: (1) extend `default_settings_are_dark_with_empty_layout` with `assert!(settings.subagent_models.is_empty());`; (2) extend `a_pre_feature_settings_json_parses_to_the_new_defaults` with `assert!(settings.subagent_models.is_empty());`; (3) add a new test `subagent_models_round_trips_in_camel_case` mirroring `default_thinking_levels_defaults_to_empty_and_round_trips`: a file WITHOUT the field parses to an empty map; a file WITH `{ "subagentModels": { "scout": "tama/m-1" } }` parses to the entry; a populated map serializes with the `"subagentModels"` key present; (4) extend the pre-existing `settings_round_trip_with_the_new_fields` (~line 443 — the ONLY full `Settings` struct literal in the codebase outside the `Default` impl — the other two `Settings {` literals at ~419 / ~544 use `..Settings::default()` and need no change): add `subagent_models: std::collections::HashMap::from([("scout".to_string(), "tama/m-1".to_string())]),` to its literal and `"subagentModels"` to its key-presence loop.
- [ ] Run `cargo test --lib settings`
  - Did it fail to COMPILE (the field does not exist yet — E0063 in the extended literals/asserts)? That compile failure is the red state — if the tests somehow pass or compile, stop and investigate why.
- [ ] Implement the field + the `Default` entry in `settings.rs`.
- [ ] Run `cargo test --lib settings`
  - Did all settings tests pass? If not, fix and re-run before continuing.
- [ ] Run `cargo fmt`
  - Did it succeed? If not, fix and re-run before continuing.
- [ ] Run `cargo clippy --all-targets`
  - Did it succeed with 0 warnings? If not, fix and re-run before continuing.
- [ ] Commit with message: `feat: add the subagentModels settings field (per-agent subagent model overrides, ADR 0023)`

**Acceptance criteria:**
- [ ] `Settings` has a `subagent_models: HashMap<String, String>` field with `#[serde(default)]`.
- [ ] `Settings::default()` has an empty map; a pre-feature JSON parses to an empty map; a populated map round-trips in camelCase (`"subagentModels"`).
- [ ] All pre-existing settings tests still pass; clippy 0 warnings.

---

### Task 2: `list_agent_definitions` Tauri command

**Context:**
The Settings page's Subagents section needs the discovered agent list (name / description / the file's current `model` / scope). The `list_agents` model tool reads `discover_agents` in-process; the UI needs an IPC command. The command is stateless (no `State` param — like `list_tools`) and returns a camelCase DTO that EXCLUDES the fields the UI doesn't need (`path` / `system_prompt` / `tools` / `thinking`). Scope: user-level definitions only (`discover_agents(None)`) — the Settings page is app-global; space-level definitions are overridable by name via hand-edited JSON but not listed.

**Files:**
- Create: `src-tauri/src/commands/agents.rs`
- Modify: `src-tauri/src/commands/mod.rs` (add `pub mod agents;` — alphabetical: before `clipboard`)
- Modify: `src-tauri/src/lib.rs` (register `commands::agents::list_agent_definitions` in `generate_handler!` — next to the `commands::settings::*` entries)

**What to implement:**
- `src-tauri/src/commands/agents.rs`:
  ```rust
  //! Agent-definition listing for the Settings page (the Subagents
  //! section — ADR 0023).

  use serde::Serialize;
  use tauri::command;

  /// The camelCase wire shape. NOT the `AgentDefinition` struct itself —
  /// a DTO keeps the command's contract stable and excludes the fields
  /// the UI doesn't need (`path` / `system_prompt` / `tools` /
  /// `thinking`).
  #[derive(Debug, Clone, PartialEq, Serialize)]
  #[serde(rename_all = "camelCase")]
  pub struct AgentDefinitionDto {
      pub name: String,
      pub description: String,
      pub model: Option<String>,
      /// `"space" | "user"` (the `AgentScope` Display form).
      pub scope: String,
  }

  /// The mapping lives in a `From` impl (testable in isolation — the
  /// command body is a thin `discover_agents(None).map(...)` wrapper).
  impl From<AgentDefinition> for AgentDefinitionDto {
      fn from(d: AgentDefinition) -> Self {
          Self {
              name: d.name,
              description: d.description,
              model: d.model,
              scope: d.scope.to_string(),
          }
      }
  }

  /// The USER-level Agent definitions (the Settings page's Subagents
  /// section — the page is app-global: `discover_agents(None)` =
  /// user-level only; a space-level definition is NOT listed, but an
  /// override for it still applies by name at dispatch time and can be
  /// added by hand-editing `settings.json`).
  #[tauri::command]
  pub async fn list_agent_definitions() -> Result<Vec<AgentDefinitionDto>, String> {
      Ok(crate::agents::discover_agents(None)
          .into_iter()
          .map(AgentDefinitionDto::from)
          .collect())
  }
  ```
  (plus the `use crate::agents::AgentDefinition;` import the `From` impl needs).
- Tests in the same file's `#[cfg(test)] mod tests` (stateless — build `AgentDefinition` values directly; the `discover_agents` I/O is covered by `agents.rs`' own tests): the mapping is asserted via `AgentDefinitionDto::from(def)` (the `From` impl — NOT via the command, which is a thin wrapper):
  - `agent_definition_dto_mapping`: a full `AgentDefinition` (all 8 fields: `name: "scout"`, `description: "Fast recon."`, `model: Some("tama/m-1")`, `thinking: Some("high")`, `tools: Some(vec!["read"])`, `system_prompt: "You are a scout."`, `scope: AgentScope::User`, `path: "/home/u/.agents/agents/scout.md"`) maps via `AgentDefinitionDto::from` to `name` / `description` / `model: "tama/m-1"` / `scope: "user"`; the serialized JSON does NOT contain `systemPrompt` / `path` / `tools` / `thinking` keys.
  - `agent_definition_dto_a_none_model_is_null_on_the_wire`: `model: None` serializes as JSON `null`; `scope: AgentScope::Space` serializes as `"space"`.

**Steps:**
- [ ] Write the two tests + the module with a STUB `From` impl (`fn from(_d: AgentDefinition) -> Self { unimplemented!() }` or a deliberately-wrong field) in `src-tauri/src/commands/agents.rs` (the `AgentDefinitionDto` struct + the command body + `use crate::agents::AgentDefinition;`), AND add `pub mod agents;` to `src-tauri/src/commands/mod.rs` (BEFORE the red run — until the module is declared, `cargo test --lib commands::agents` matches 0 tests and exits 0, which is NOT the red state).
- [ ] Run `cargo test --lib commands::agents`
  - Did it fail (the mapping test fails against the stub `From` impl)? That failure is the red state — if the tests pass unexpectedly, stop and investigate why.
- [ ] Replace the stub `From` impl with the real one (the `AgentDefinitionDto::from` above) and implement the real `list_agent_definitions` body (the `discover_agents(None)` + `.map(AgentDefinitionDto::from)` above).
- [ ] Register `commands::agents::list_agent_definitions` in `lib.rs`'s `generate_handler!` (next to the `commands::settings::*` entries).
- [ ] Run `cargo test --lib commands::agents`
  - Did all tests pass? If not, fix and re-run before continuing.
- [ ] Run `cargo fmt`
  - Did it succeed? If not, fix and re-run before continuing.
- [ ] Run `cargo clippy --all-targets`
  - Did it succeed with 0 warnings? If not, fix and re-run before continuing.
- [ ] Commit with message: `feat: add the list_agent_definitions command (the Settings page's Subagents data source, ADR 0023)`

**Acceptance criteria:**
- [ ] `list_agent_definitions` is a registered, stateless Tauri command returning camelCase `AgentDefinitionDto` rows (`name` / `description` / `model` / `scope`).
- [ ] The DTO excludes `path` / `system_prompt` / `tools` / `thinking` on the wire; a `None` model is JSON `null`.
- [ ] All pre-existing tests still pass; clippy 0 warnings.

---

### Task 3: Override layering in `resolve_launch` + the thinking-order fix

**Context:**
The core behavior (ADR 0023): on a matched Agent definition, the child model becomes **explicit param → settings override → frontmatter → parent model**. The override is SOFT (like the frontmatter it layers over): a value whose bare key resolves to nothing in the effective catalog degrades to the next layer — a stale override never fails a dispatch. It is keyed by NAME (case-insensitive, like the `agentName` match) and applies ONLY when a definition matches (a config-less dispatch is untouched). The settings are read at dispatch time via `load_settings` (missing/corrupt → defaults → no override; a settings edit takes effect on the NEXT dispatch, no restart).

The companion fix: the feature doc's thinking order (explicit > model-key `:<level>` suffix > frontmatter `thinking`) wins the doc/code contradiction. The code today computes `launch.thinking.or(level_suffix)` where `launch.thinking` was already merged with the frontmatter `thinking` — so the frontmatter beats the suffix. The fix splits the layers: `LaunchConfig.thinking` holds the EXPLICIT param only; a new `LaunchConfig.frontmatter_thinking` field carries the frontmatter's `thinking`; `dispatch_native_inner` computes `explicit ∨ suffix ∨ frontmatter_thinking`, then the EXISTING validation (empty `thinking_levels` soft-passes; a mismatch is dropped) runs unchanged on the result.

**Files:**
- Modify: `src-tauri/src/agent/subagent.rs` (`LaunchConfig` struct + `dispatch_native_inner` step 2 + the `default_native_launch()` test helper at ~line 1320)
- Modify: `src-tauri/src/agent/harness/loop.rs` (`AgentLoop` struct + `new` + `resolve_launch` + the 12 test-module `LaunchConfig` literals + the `build_loop_with_db` / `build_loop` / `build_loop_with_subagent` test helpers + their 7 call sites + the 4 pre-existing `resolve_launch_*` tests' frontmatter-`thinking` assertions)
- Modify: `src-tauri/src/agent/interactive.rs` (the `dispatch_params` `LaunchConfig` literal at ~line 669)
- Modify: `src-tauri/tests/harness_dispatch_native.rs` (the `default_launch()` full `LaunchConfig` literal at ~line 163 — its 15 call sites spread from it and need no change)

**What to implement:**
1. `LaunchConfig` (`subagent.rs`): add `Default` to the derive list (`#[derive(Clone, Debug, Default, PartialEq)]`) and a new field:
   ```rust
   /// The frontmatter's `thinking` (the dispatch resolves the thinking in
   /// the doc-correct order: explicit > model-key `:<level>` suffix >
   /// this — the frontmatter's `thinking` is the LAST rung). `None` for
   /// a config-less dispatch.
   pub frontmatter_thinking: Option<String>,
   ```
   Update the `thinking` field's doc comment: "Explicit thinking level from the tool call ONLY (the frontmatter's `thinking` moved to `frontmatter_thinking` — the dispatch layers it LAST)."
2. `dispatch_native_inner` (subagent.rs, step 2 — the line `let mut thinking = launch.thinking.clone().or(level_suffix);`):
   ```rust
   // 2. The child thinking level: explicit > the `:<level>` suffix of
   // the resolved model key > the frontmatter's `thinking` (the
   // doc-correct order — the frontmatter's `thinking` is the LAST rung).
   // Validate against `model.thinking_levels` (when non-empty — an empty
   // set soft-passes) — a mismatch is DROPPED (never sent upstream as a
   // bogus `reasoning_effort`).
   let mut thinking = launch
       .thinking
       .clone()
       .or(level_suffix)
       .or(launch.frontmatter_thinking.clone());
   ```
   (the validation block immediately below is UNCHANGED).
3. `AgentLoop` (`loop.rs`): add a field (near `catalog`):
   ```rust
   /// The settings dir (the `settings.json` home — ADR 0023:
   /// `resolve_launch` reads the `subagentModels` override from here at
   /// dispatch time; `None` = no override layer).
   config_dir: Option<PathBuf>,
   ```
   In `new`: the `McpManager::new(..., config_dir.as_deref())` call stays; ADD `config_dir,` to the `Self { ... }` construction (the owned value is still available after the borrow).
4. `resolve_launch` (`loop.rs`): after the `def` match (and BEFORE the existing `model` block), add:
   ```rust
   // (ADR 0023) The per-agent model override (the settings'
   // `subagentModels` — read at dispatch time: a settings edit takes
   // effect on the NEXT dispatch, no restart; a missing/corrupt file
   // yields the defaults → no override). SOFT, like the frontmatter:
   // a value whose bare key resolves to NOTHING degrades to the next
   // layer (a stale override must not fail the dispatch). CASE-
   // INSENSITIVE by name (a case-insensitive SCAN — the stored key is
   // NOT rewritten: the UI saves `def.name` verbatim, and a hand-edited
   // mixed-case key must still match the case-insensitive `agentName`
   // resolution).
   let override_model = self
       .config_dir
       .as_deref()
       .and_then(|dir| {
           crate::commands::settings::load_settings(dir)
               .subagent_models
               .iter()
               .find(|(k, _)| k.eq_ignore_ascii_case(name))
               .map(|(_, v)| v.clone())
       });
   ```
   The `model` chain becomes (the frontmatter block is the EXISTING one, verbatim, as the last `or_else`). **NOTE — the resolvability check runs against the parent's SESSION-START catalog (`self.catalog`), NOT the dispatch-time `EffectiveCatalog`** — exactly like the EXISTING frontmatter check (a model added to a provider after the parent session started is "stale" at `resolve_launch` time); `resolve_launch` is SYNCHRONOUS (no async machinery for a fresh `EffectiveCatalog::resolve`), so do NOT "fix" the check to use the effective catalog — the degradation semantics are the same as the frontmatter's, by design.
   ```rust
   let model = launch.model.clone().or_else(|| {
       override_model.as_ref().and_then(|m| {
           let bare = m
               .rsplit_once(':')
               .map(|(b, _)| b.to_string())
               .unwrap_or_else(|| m.clone());
           crate::agent::session::resolve_composed_model(&self.catalog, &bare)
               .is_some()
               .then_some(m.clone())
       })
   }).or_else(|| {
       def.model.as_ref().and_then(|m| {
           /* the EXISTING frontmatter block body, verbatim */
       })
   });
   ```
   The `thinking` line changes from `let thinking = launch.thinking.clone().or_else(|| def.thinking.clone());` to:
   ```rust
   // The thinking split (the doc-correct order — the dispatch resolves
   // explicit > model-key suffix > frontmatter `thinking`): `thinking`
   // holds the EXPLICIT param only; the frontmatter's `thinking` moves
   // to its own field.
   let frontmatter_thinking = def.thinking.clone();
   ```
   The final construction becomes:
   ```rust
   LaunchConfig {
       system_prompt,
       model,
       thinking: launch.thinking.clone(),
       frontmatter_thinking,
       tools,
   }
   ```
   (the old `thinking,` shorthand is replaced by the explicit `thinking: launch.thinking.clone()` + `frontmatter_thinking`).
5. `interactive.rs` `dispatch_params` (the `LaunchConfig { ... }` literal at ~line 669): append `..Default::default(),` (the tool params never set `frontmatter_thinking`).
6. The remaining `LaunchConfig { ... }` literals: the 12 in `loop.rs` tests (~lines 4014–4466) append `..Default::default(),`. The `default_native_launch()` helper (subagent.rs ~line 1320) is a FULL literal — add `frontmatter_thinking: None,` to it (the literals at subagent.rs ~1637 / ~1678 spread from it via `..default_native_launch()` — leave them UNTOUCHED; a literal with two `..` bases is a syntax error). Likewise `default_launch()` in `src-tauri/tests/harness_dispatch_native.rs` (~line 163, a full literal used by 15 call sites that spread from it): add `frontmatter_thinking: None,`. (Grep for `LaunchConfig {` across `src-tauri/` — including `tests/` — to find them all: after this task, no FULL literal may omit the new field.)
7. Test-helper changes (`loop.rs` `#[cfg(test)] mod tests`):
   - `build_loop_with_db`: add a `config_dir: Option<&std::path::Path>` parameter (pass it as the LAST `AgentLoop::new` arg — `AgentLoop::new` takes `Option<PathBuf>`, so convert: `config_dir.map(|p| p.to_path_buf())` — replacing the hardcoded `None`); also add a `models: Vec<Model>` parameter replacing the hardcoded single-model `catalog` (the `ModelCatalog` is built from `models` — the existing single-model callers pass `vec![fake_model("m1")]`… note: the existing body builds the `Model` inline; move that construction into a `let model = models[0].clone();` and use `models` for the catalog — the single-model behavior is preserved).
   - **Update ALL 8 `build_loop_with_db(` grep occurrences** (the 7 at loop.rs ~lines 2988, 3028, 3120, 3192, 3274, 3346, 4925 + `build_loop`'s internal delegation at ~2353 — grep for `build_loop_with_db(` to find them) with the two new args: `vec![fake_model("m1")]` (or the single-model vec the existing body used) + `None`.
   - `build_loop`: pass the same two args (`vec![fake_model("m1")]` + `None`) — `build_loop` stays a thin wrapper.
   - `build_loop_with_subagent` (~line 3860): add a `config_dir: Option<&std::path::Path>` parameter (pass it as the LAST `AgentLoop::new` arg — same `.map(|p| p.to_path_buf())` conversion — replacing the hardcoded `None`); its ONE existing call site (~line 4540, the frontmatter e2e test) gains a trailing `None`.
   - NOTE: `build_loop_with_db`'s revised body indexes `models[0]` — use `models[0].clone().expect("at least one model")` (every caller passes a non-empty vec; a silent panic on an empty vec is a poor failure mode for future edits).
8. NEW tests (all in the `loop.rs` test module):
   - `resolve_launch_settings_override_beats_frontmatter_model`: a temp `config_dir` with a `settings.json` of `{ "subagentModels": { "scout": "fake/m2" } }`; a temp HOME (the `env_lock()` + `RestoreHome` pattern) with `.agents/agents/scout.md` (frontmatter `name: scout`, `model: fake/m1` — resolvable); a loop built via `build_loop_with_db(..., vec![fake_model("m1"), fake_model("m2")], Some(&config_dir))`; `resolve_launch(&LaunchConfig::default(), "scout")` → assert `model == Some("fake/m2")`.
   - `resolve_launch_explicit_param_beats_settings_override`: same fixture; `LaunchConfig { model: Some("fake/m1".into()), ..Default::default() }` → assert `model == Some("fake/m1")`.
   - `resolve_launch_stale_override_degrades_to_frontmatter`: override `"scout": "fake/nope"` (NOT in the catalog) + frontmatter `model: fake/m1` → assert `model == Some("fake/m1")` (soft — no error, no `None`).
   - `resolve_launch_case_insensitive_override_key`: settings key `"Scout"` (capital) + file `name: scout` + `resolve_launch(..., "Scout")` → assert the override applies (`model == Some("fake/m2")` for the m2 override value).
   - `resolve_launch_no_override_keeps_the_frontmatter`: a `settings.json` with `{ "subagentModels": {} }` + frontmatter `model: fake/m1` → assert `model == Some("fake/m1")`.
   - `resolve_launch_frontmatter_thinking_migrates_to_its_own_field`: file with `thinking: low` (no `model`); `resolve_launch(&LaunchConfig::default(), "scout")` → assert `thinking == None` AND `frontmatter_thinking == Some("low")`.
   - `a_native_subagent_with_a_suffix_model_key_uses_the_suffix_thinking` (e2e, mirror `a_native_subagent_with_a_matching_agent_name_runs_the_frontmatter_config` VERBATIM in structure — `ScriptedProvider` with a `subagent` `ToolCall` of `agentName: "scout"`, `make_native_manager` with a 2-model catalog `[fake_model("m1"), fake_model("m2")]`, `build_loop_with_subagent(..., Some(manager), models, None)`, agent file `model: fake/m2:high` + `thinking: low`): assert the `subagent-session-started` payload `thinkingLevel == "high"` (the suffix beats the frontmatter — the OLD code produced `"low"`; this is the regression test for the order fix) and `model == "fake/m2"` (the suffix is stripped for resolution).
   - `a_native_subagent_with_a_settings_override_runs_the_override_model` (e2e, same mirror): a temp `config_dir` with `settings.json` `{ "subagentModels": { "scout": "fake/m3" } }`; a 3-model catalog `[fake_model("m1"), fake_model("m2"), fake_model("m3")]`; agent file `model: fake/m2` + `thinking: low`; `build_loop_with_subagent(..., Some(manager), models, Some(&config_dir))`: assert `model == "fake/m3"` (the override beats the frontmatter) and `thinkingLevel == "low"` (the frontmatter's `thinking` still applies — the override only replaces the model).

**Steps:**
- [ ] Write the 8 new tests from step 8 (they reference the new field / helper params — they will NOT compile yet: that compile failure is the red state).
- [ ] Run `cargo test --lib harness`
  - Did it fail to compile (unknown `frontmatter_thinking` field / wrong helper signatures)? If the tests compile and pass unexpectedly, stop and investigate why.
- [ ] Implement items 1–7 (the `LaunchConfig` field + `Default` derive, `dispatch_native_inner` step 2, the `AgentLoop` field + `new`, `resolve_launch`, the `interactive.rs` literal, the `loop.rs` test literals + `default_native_launch()` + `default_launch()` in `tests/harness_dispatch_native.rs`, the 3 test-helper signatures + ALL their call sites).
- [ ] Migrate the frontmatter-`thinking` assertions in the pre-existing `resolve_launch_*` tests: **grep for `resolved.thinking` in `loop.rs`'s test module — there are exactly 5 sites (~lines 4102, 4138, 4183, 4224, 4478, in `resolve_launch_case_insensitive_match` ~4076, `resolve_launch_frontmatter_fills_gaps` ~4111, `resolve_launch_explicit_params_win_over_frontmatter` ~4152, `resolve_launch_stale_frontmatter_model_degrades_to_explicit_param` ~4193, `resolve_launch_empty_body_means_no_system_prompt` ~4448) — every one of them carries the FRONTMATTER value (`Some("low")` from the agent file's `thinking:` field) and moves from `resolved.thinking` to `resolved.frontmatter_thinking` (the value is UNCHANGED — only the field moved). Assertions where the value comes from the EXPLICIT launch param stay on `resolved.thinking` (none of the 5 is such a case). Do NOT touch `resolve_launch_stale_frontmatter_model_degrades_to_none` (~4235 — it has NO `thinking` assertion).
- [ ] Run `cargo test --lib`
  - Did ALL tests pass (the 8 new ones + every pre-existing `resolve_launch_*` / subagent-dispatch test — the pre-existing tests' `LaunchConfig` literals got `..Default::default()`, the 4 frontmatter-`thinking` assertions moved to `frontmatter_thinking` with their values unchanged, and the `tests/harness_dispatch_native.rs` integration tests compile via the `default_launch()` fix)? If not, fix and re-run before continuing.
- [ ] Run `cargo fmt`
  - Did it succeed? If not, fix and re-run before continuing.
- [ ] Run `cargo clippy --all-targets`
  - Did it succeed with 0 warnings? If not, fix and re-run before continuing.
- [ ] Commit with message: `feat: layer the subagent model override between the explicit param and the frontmatter + fix the thinking order (ADR 0023)`

**Acceptance criteria:**
- [ ] A settings override (resolvable — against the parent's session-start catalog, like the frontmatter) beats the frontmatter `model`; an explicit param beats the override; a stale override degrades to the frontmatter (never fails the dispatch); the key match is case-insensitive (a capital settings key matches a lowercase `agentName` — the `eq_ignore_ascii_case` scan); no override → today's behavior (all pre-existing `resolve_launch_*` tests green, the 5 frontmatter-`thinking` assertions moved to `frontmatter_thinking` with unchanged values).
- [ ] `config_dir: None` (a loop built without a config dir) → no override layer (pre-existing tests green).
- [ ] Thinking resolves as explicit > `:<level>` suffix > frontmatter `thinking` (the e2e suffix test passes; the validation block is unchanged).
- [ ] No `LaunchConfig` full literal omits the new field (clippy 0 warnings, full `cargo test` green — including the `tests/harness_dispatch_native.rs` integration suite).

---

### Task 4: The Settings page's Subagents section (frontend)

**Context:**
The UI half of ADR 0023: a new **Subagents** section on the Settings page. One row per user-level discovered agent (from the new `list_agent_definitions` command — Task 2): name + description + the file's current `model` (read-only) + a model `Select` defaulting to "File value (no override)" (options = the `listModels()` catalog, the same picker as the existing Default-model select). Immediate save via the existing `update(patch)` pattern (a pick sets `subagentModels[name]`; "File value" deletes the key). Orphaned entries (override keys matching no discovered agent) render as dim rows with a remove button — the only cleanup path. The provider-rename remap (the existing `remapModelRefs` call in the name-commit effect) extends to `subagentModels`.

**Files:**
- Modify: `src/lib/tauri.ts` (the `AppSettings` interface + a new `AgentDefinitionDto` interface + a new `listAgentDefinitions` binding)
- Modify: `src/components/settings/SettingsPage.tsx` (the `Section` union + the nav + the section render + the orphan rows + the `commitProviderField` value remap)
- Modify: `src/components/settings/SettingsPage.test.tsx` (the `baseSettings` + inlined-mock fixtures + the `go()` helper union + the 5 new tests + the extended remap test)

**What to implement:**
1. `src/lib/tauri.ts`:
   - `AppSettings`: add `/** (ADR 0023) Per-agent subagent model overrides: agent name → model key. */ subagentModels: Record<string, string>;` (after `defaultThinkingLevels`).
   - New interface (next to `ModelDto`):
     ```ts
     /** One discovered agent definition (camelCase over IPC — the Rust `AgentDefinitionDto`). */
     export interface AgentDefinitionDto {
       name: string;
       description: string;
       model: string | null;
       scope: "space" | "user";
     }
     ```
   - New binding (next to `listTools`):
     ```ts
     export async function listAgentDefinitions(): Promise<AgentDefinitionDto[]> {
       return invoke<AgentDefinitionDto[]>("list_agent_definitions");
     }
     ```
2. `SettingsPage.tsx`:
   - `type Section = "general" | "appearance" | "providers" | "subagents" | "mcp";`
   - Import `BotIcon` from `lucide-react` (the section icon) + `listAgentDefinitions` / `AgentDefinitionDto` from `@/lib/tauri`.
   - Nav: a Subagents button BETWEEN Providers and MCP (same `SettingsSidebarButton` shape as the neighbors — from `./primitives`: `icon: BotIcon`, `label: "Subagents"`, `active: section === "subagents"`, `onClick: () => setSection("subagents")`).
   - Content: `section === "subagents" ? subagentsSection : ...` in the existing conditional chain.
   - `subagentsSection` (a `SettingsGroupCard`, like the other sections; return `null` while `settings === null`):
     - New state — declare it with the OTHER `useState` calls at the TOP of the component (~line 730, NOT inside the `subagentsSection` JSX expression — a `useState` there would violate the rules of hooks): `const [agentDefs, setAgentDefs] = useState<AgentDefinitionDto[] | null>(null);`
     - Load in the SAME effect that calls `listModels()` (the one at ~line 744): `void listAgentDefinitions().then(setAgentDefs).catch(() => setAgentDefs([]));`
     - Per discovered def, a `SettingsRow`: `label` = `def.name`; `description` = `${def.description || "No description"} · file: ${def.model ?? "— (inherits parent model)"}`; `control` = the `Select` (same shape as the Default-model select at ~line 917):
       ```tsx
       <Select
         value={settings.subagentModels[def.name] ?? ""}
         onValueChange={(value) => {
           const subagentModels = { ...settings.subagentModels };
           if (value === "") {
             delete subagentModels[def.name];
           } else {
             subagentModels[def.name] = value;
           }
           update({ subagentModels });
         }}
       >
         <SelectTrigger aria-label={`Subagent model for ${def.name}`} className="w-64">
           <SelectValue placeholder="File value (no override)" />
         </SelectTrigger>
         <SelectContent>
           <SelectItem value="">File value (no override)</SelectItem>
           {models.map((model) => (
             <SelectItem
               key={`${model.provider}/${model.id}`}
               value={`${model.provider}/${model.id}`}
             >
               {model.provider}/{model.id}
             </SelectItem>
           ))}
         </SelectContent>
       </Select>
       ```
       (copy the Default-model select's `SelectItem` rendering verbatim for the `models.map` block — match its label format exactly). **Stale-catalog display guard**: a stored override whose model key is no longer in the `models` list (a renamed/removed provider) matches no `SelectItem`, so Radix would fall back to the placeholder and misrepresent the saved state — when `settings.subagentModels[def.name]` is set AND not in the `models` list, render one EXTRA `SelectItem` (disabled) carrying the raw stored value as both its `value` and label (the select then displays the actual stored value instead of the placeholder — the same problem the thinking-level select solves with its `storedLevelOutsideUnion` pattern).
     - Orphaned entries: `const orphans = agentDefs === null ? [] : Object.entries(settings.subagentModels).filter(([name]) => !agentDefs.some((d) => d.name.toLowerCase() === name.toLowerCase()));` — **gate on `agentDefs !== null`** (while the definitions are still loading, `agentDefs` is `null` — treating "not loaded yet" as "no discovered agents" would flash every override as a stale orphan with a live remove button before the list lands). Each orphan as a muted `SettingsRow` (the `label` prop is a `ReactNode` — pass `<span className="text-foreground-subtle">{name}</span>`; `SettingsRow` has no `dim` prop): `description` = "No longer discovered (stale override)"; `control` = a `Button` (`variant: "ghost"`, `size: "icon-sm"`, `TrashIcon` — the same button shape the MCP rows use for their remove button, WITHOUT the `AlertDialog` confirm — a stale entry is inert, no confirm needed) whose `onClick` deletes the key via `update`.
3. The provider-rename remap — the `if (id !== current.id)` block inside the `commitProviderField` FUNCTION (~lines 826–841, where `nextSettings.defaultModel` / `nextSettings.defaultThinkingLevels` are remapped): add a VALUE remap — `subagentModels` is `agent name → model key`, so the model refs are the VALUES (the agent names are the keys — `remapModelRefs` remaps map KEYS and would be a no-op here — do NOT use it):
   ```ts
   const remappedSubagentModels: Record<string, string> = {};
   for (const [name, model] of Object.entries(settings.subagentModels)) {
     remappedSubagentModels[name] = remapModelRef(model, current.id, id) ?? model;
   }
   nextSettings.subagentModels = remappedSubagentModels;
   ```
   (inside the same `if` block, next to the existing two remap calls).
4. `SettingsPage.test.tsx` — follow the file's existing mock pattern (it mocks `@/lib/tauri`: `getSettings` / `saveSettings` / `listModels` / `listTools`; add `listAgentDefinitions` + `AgentDefinitionDto` to the mock):
   - **Mock fixtures first**: `baseSettings` (typed `AppSettings` — a new required field breaks `tsc`, and `pnpm build` typechecks `src/**` including tests) AND the inlined `getSettings` mock object in the `vi.mock` factory (untyped — without `subagentModels: {}` the `commitProviderField` remap runs `remapModelRef` on `undefined` at runtime in the existing provider-rename tests) both gain `subagentModels: {}`. (The per-test `mockResolvedValueOnce({ ...baseSettings, ... })` spreads are then fine.)
   - **`listAgentDefinitions` mock**: the `vi.mock` factory gains `listAgentDefinitions: vi.fn()` (default `mockResolvedValue([])`; the tests override per-test).
   - **`go()` helper** (line ~116: `async function go(section: "Appearance" | "Providers" | "MCP")`): extend the union with `"Subagents"`; the new branch clicks the Subagents nav button and returns WITHOUT awaiting a section marker (the Subagents card has no always-present button/heading — an empty agents list renders a nearly empty card — the per-test `findByText` on a row handles the wait).
   - `renders_the_subagents_section_rows_for_discovered_agents`: settings with `subagentModels: {}` + `listAgentDefinitions` resolving to two defs (one with `model: "p/m1"`, one with `model: null`); navigate to the Subagents section; assert both rows render (name + a `file: p/m1` description for the first, a `file: — (inherits parent model)` description for the second) with the "File value (no override)" select state.
   - `a_stale_override_value_renders_as_its_own_option`: settings with `subagentModels: { scout: "gone/m1" }` (NOT in the `listModels` catalog) + a def named `scout`; assert a (disabled) `SelectItem` with the raw value `gone/m1` renders — the select does NOT fall back to the placeholder for a stored-but-unknown value.
   - `selecting_a_model_saves_the_subagent_models_entry`: pre-select a def's select, pick a catalog model; assert `saveSettings` is called with `subagentModels: { [def.name]: "<picked key>" }` (the complete document — the other fields unchanged).
   - `selecting_file_value_deletes_the_subagent_models_entry`: settings pre-seeded with `subagentModels: { [def.name]: "p/m1" }`; pick "File value (no override)"; assert `saveSettings` is called with the key ABSENT from `subagentModels`.
   - `an_orphaned_override_renders_with_a_remove_button`: settings with `subagentModels: { ghost: "p/m1" }` + `listAgentDefinitions` resolving to a def named `scout`; assert a muted row for `ghost` ("No longer discovered") with a remove button; click it; assert `saveSettings` is called with `subagentModels: {}`.
   - Extend the existing provider-rename test (`a_name_commit_reidentifies_the_provider_and_remaps_the_model_refs`, ~line 210): seed `subagentModels: { scout: "<old provider id>/m1" }` and assert the saved document carries `subagentModels: { scout: "<new provider id>/m1" }` (the VALUE remap — a non-matching value stays untouched).

**Steps:**
- [ ] Write the 4 new tests in `SettingsPage.test.tsx` (they reference the new section / binding — they will fail: wrong section name, missing mock, or a render assertion failure).
- [ ] Run `pnpm test -- SettingsPage`
  - Did they fail for the expected reasons (the Subagents section / binding don't exist yet)? If any pass unexpectedly, stop and investigate why.
- [ ] Implement items 1–3 (`tauri.ts` type + binding, the `Section` union + nav + `subagentsSection` + orphans + remap).
- [ ] Run `pnpm test -- SettingsPage`
  - Did all SettingsPage tests pass (the 5 new + the extended remap test + every pre-existing — the `baseSettings` / inlined-mock `subagentModels: {}` additions keep the existing `getSettings` mock shape valid)? If not, fix and re-run before continuing.
- [ ] Run `pnpm build`
  - Did the type-check + build succeed? If not, fix and re-run before continuing.
- [ ] Commit with message: `feat: add the Settings page's Subagents section (per-agent model overrides, ADR 0023)`

**Acceptance criteria:**
- [ ] The Settings nav has a Subagents section (between Providers and MCP) listing the user-level discovered agents (name + description + the file's model, read-only).
- [ ] Picking a model saves `subagentModels[name]` (immediate save, complete document); picking "File value (no override)" deletes the key.
- [ ] Orphaned entries render as dim rows with a working remove button; the provider-rename remap covers `subagentModels`.
- [ ] `pnpm test` + `pnpm build` green.

---

### Task 5: Feature doc update

**Context:**
`docs/features/subagent-sessions.md` documents the config resolution; it must reflect the new override layer (its thinking line already matches the fixed code — no change there). The doc's front-matter `last-verified` / `verified-by` get refreshed.

**Files:**
- Modify: `docs/features/subagent-sessions.md`

**What to implement:**
- The "## Config resolution" heading: `(layered: explicit > frontmatter > parent defaults)` → `(layered: explicit > override > frontmatter > parent defaults)`.
- The `model` bullet: prepend the override layer — "the explicit param, else the **settings override** (the `settings.json` `subagentModels` entry for the agent's name — ADR 0023; read at dispatch time; a stale value degrades to the next layer, like the frontmatter), else the frontmatter `model`, else the parent's model." Keep the existing sentences about resolution-against-the-effective-catalog / explicit-unknown-fails / suffix-strip verbatim.
- The `thinking` bullet: NO change (it already says "the explicit param, else the model key's `:<level>` suffix, else the frontmatter `thinking`" — the code now matches it).
- Front-matter: `last-verified: <THE ACTUAL SHIP DATE — run `date +%Y-%m-%d` and substitute it; do NOT write a hard-coded date>`; `verified-by:` append `+ the subagent-model-overrides plan (docs/roadmap/subagent-model-overrides.md)`. (The model bullet's existing "Resolved at dispatch time against the effective catalog" sentence stays — it is true of the FINAL dispatch resolution in `dispatch_native_inner`; the soft-degrade check in `resolve_launch` runs against the session-start catalog, as Task 3 documents.)

**Steps:**
- [ ] Apply the three edits above.
- [ ] Re-read the "Config resolution" section end-to-end: does the model bullet read in the right order (explicit > override > frontmatter > parent), and is the thinking bullet untouched?
- [ ] Commit with message: `docs: update the subagent-sessions feature doc (the override layer, ADR 0023)`

**Acceptance criteria:**
- [ ] The config-resolution section documents the override layer in the model precedence with its soft-degradation semantics.
- [ ] The thinking bullet is unchanged; the front-matter dates are refreshed.

---

## Final validation (run after ALL tasks — the AGENTS.md gate)

- [ ] `pnpm test` (repo root) — all frontend unit tests green.
- [ ] `pnpm build` (repo root) — type-check + build green.
- [ ] `cargo test` (`src-tauri/`) — all Rust tests green.
- [ ] `cargo clippy --all-targets` (`src-tauri/`) — 0 warnings.
- [ ] `cargo fmt --check` (`src-tauri/`) — clean.
- [ ] On ship (the `finish` skill): fold the durable content into `docs/features/subagent-sessions.md` (Task 5 already does the doc update) and DELETE `docs/roadmap/subagent-model-overrides.md`.
