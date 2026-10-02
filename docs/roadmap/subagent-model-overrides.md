---
status: approved
done-when: The Settings page has a Subagents section where a discovered agent's model override is settable; a subagent session for that agent runs with the overridden model (observable via the `subagent-session-started` event / the Delegating card) without the agent's markdown file being modified; the thinking-order code fix lands (explicit > suffix > frontmatter `thinking`); all validation gates green (pnpm test, pnpm build, cargo test, cargo clippy --all-targets, cargo fmt --check)
---

# Per-agent subagent model overrides in Settings

## Problem

Subagent model resolution is today: explicit `subagent` tool param → Agent definition frontmatter `model` → parent model (ADR 0020). To change a named agent's model, the user must edit the agent's markdown file. The user wants a Settings-page control: per-agent model overrides stored in the app's settings — the markdown files are never touched.

## Design

### Storage (settings.json)

- New `Settings` field: `subagent_models: HashMap<String, String>` — the effective agent's `name` → model key. `#[serde(default)]` (a pre-feature file parses to `{}`, no migration); `Default for Settings` → empty map.
- **Value shape**: a verbatim model key — a bare `"provider/id"`, or `"provider/id:<level>"` if hand-edited (the existing downstream suffix handling picks the `:<level>` up as a thinking candidate — zero new code for that path). The UI writes bare keys only.
- **Lookup normalization**: stored name and incoming `agentName` are both lowercased before comparison (mirrors the existing case-insensitive name match in `resolve_launch`).
- **Orphaned entries** (the agent file was deleted/renamed): ignored at dispatch time — no definition matches, no override applies. No Rust-side cleanup; the UI is the only cleanup path.
- The agent markdown files are **never written to** — that is the point of the feature.

### Resolution (Rust)

- `AgentLoop` gains a `config_dir: Option<PathBuf>` field (the constructor already receives it — today it is only forwarded to `McpManager`).
- The `subagent` tool handler loads the settings at dispatch time via the existing `load_settings` (missing/corrupt file → defaults, never blocks — the established contract).
- `resolve_launch` (unchanged for config-less dispatches — `agentName` empty or matching no definition returns the launch verbatim, no override applies): on a matched definition, the `model` layering becomes **explicit param** (unchanged, strict — unknown fails the dispatch) → **override** (new — soft: its bare key resolved against the effective catalog; a stale value degrades to the next layer, exactly like the frontmatter) → **frontmatter** (unchanged, soft) → parent model (unchanged, in `dispatch_native_inner`).
- **Thinking-order fix** (the doc-correct order — the feature doc's order wins the doc/code contradiction): `LaunchConfig` gains a `frontmatter_thinking` field; `resolve_launch` stops merging the frontmatter `thinking` into `launch.thinking` (which now holds the explicit param only). `dispatch_native_inner` computes `thinking = explicit ∨ model-key suffix ∨ frontmatter_thinking`, then the existing validation against `model.thinking_levels` (empty set soft-passes; a mismatch is DROPPED — never sent upstream as a bogus `reasoning_effort`) runs unchanged on the result.
- `tools` / system-prompt layering: unchanged.
- `dispatch_native_inner`'s model step is unchanged (it receives the already-layered `launch.model` — explicit-unknown → `Failed`; the `:<level>` suffix strip/candidate logic is untouched — a hand-edited override with a suffix flows through it verbatim). The `subagent-session-started` event already carries the resolved `model` + `thinkingLevel` — the effective override stays observable with no event change.

### UI (the Settings page's new **Subagents** section)

- New section in the Settings nav (General / Appearance / Providers / **Subagents** / MCP), rendered like the other sections.
- **Data source**: a new Tauri command `list_agent_definitions` (new `commands/agents.rs` module) → `discover_agents(None)` — the user-level definitions (the Settings page is app-global; space-level definitions are NOT listed — but an override for one still applies by name at dispatch time, and can be added by hand-editing the JSON). Returns a camelCase DTO (`name`, `description`, `model`, `scope`) — not the full `AgentDefinition` (no `path` / `system_prompt` / `tools` / `thinking` — the UI doesn't need them).
- **Row shape** (one per discovered agent): name + description (truncated) + the file's current `model` as read-only text ("file: `provider/id`" or "— (inherits parent model)") + a model `Select` defaulting to **"File value (no override)"**, options = the `listModels()` catalog (composed `provider/id` keys, same picker as the existing Default-model select).
- **Immediate save** (the existing `update` pattern — `saveSettings` with the COMPLETE document): picking a model sets `subagentModels[name] = key`; picking "File value" deletes the key.
- **Provider-rename remap**: `remapModelRefs` applied to `subagentModels` alongside `defaultModel` / `defaultThinkingLevels` when a provider name is committed.
- **Orphaned entries** (override keys matching no discovered agent): rendered as dim rows with a remove button — makes stale entries visible and deletable (the Rust side ignores them; the UI is the only cleanup path).
- Frontend `AppSettings` type (`src/lib/settings.ts`) gains `subagentModels: Record<string, string>`.

### Testing & validation (TDD — failing test first, then make it pass)

1. **`settings.rs` schema** — `subagent_models` defaults to empty (`Settings::default()`); a pre-feature JSON (without the field) parses to `{}`; a populated `subagentModels` map round-trips in camelCase (mirrors the existing `mcp_servers` / `default_thinking_levels` tests).
2. **`resolve_launch` layering** (the existing suite's temp-home agent-dir fixture pattern) — override present + resolvable + no explicit param → override beats frontmatter; explicit param + override → explicit wins; stale override → degrades to frontmatter (soft, never fails the dispatch); case-insensitive key match (`"Scout"` key, `"scout"` agentName); no override → today's behavior (existing tests stay green).
3. **Thinking-order fix** — a `dispatch_native` test asserting the `subagent-session-started` `thinkingLevel`: a model key with a `:<level>` suffix + a conflicting frontmatter `thinking` → the suffix wins.
4. **`list_agent_definitions` command** — DTO mapping test (stateless, like `list_tools`).
5. **`SettingsPage.test.tsx`** — the Subagents section renders rows (mocked `list_agent_definitions` + `list_models`); picking a model saves `subagentModels[name]`; picking "File value" deletes the key; an orphaned entry renders with a remove button.
6. **`settings.test.ts`** — `remapModelRefs` applied to `subagentModels` on a provider rename.
7. **Docs** — `docs/features/subagent-sessions.md` "Config resolution" section gains the override layer in the model precedence (its thinking line already matches the fixed code).

**Validation gate (AGENTS.md):** `pnpm test` + `pnpm build` (repo root); `cargo test` + `cargo clippy --all-targets` (0 warnings) + `cargo fmt --check` (`src-tauri/`).

## Out of scope (YAGNI)

- A global default subagent model (the per-agent override was the chosen scope).
- Per-agent thinking-level overrides (model only — the thinking order fix is a bug fix, not new scope).
- Per-Space overrides (app-global `settings.json` only; space-level agents are overridable by name via hand-edited JSON).
- Orphaned-entry cleanup in Rust (the UI's remove button is the only cleanup path).
