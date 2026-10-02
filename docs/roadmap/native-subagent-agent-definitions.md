---
status: approved
done-when: In a native session, `subagent({ task, agentName })` resolves `agentName` against a discovered Agent definition and the child runs with its frontmatter model/thinking/tools + system-prompt body (layered under explicit params); `list_agents` lists the discovered definitions; a user's existing `~/.pi/agent/agents/*.md` files resolve with zero config; ADR 0020 + CONTEXT.md committed.
---

# Native subagents: resolve `agentName` against user-authored Agent definitions

## Problem

The native harness's `subagent` tool takes `agentName` as a **free-form UI label only** (ADR 0017) — it is never resolved against anything. A subagent's model/tools/system-prompt must all be hand-typed into the tool call by the parent model. Agent definition files (markdown + YAML frontmatter — the format pi's subagent extension already uses) are ignored by the native harness: `agentName: "scout"` does not pick up `scout.md`'s `model`/`tools`/body.

## Goals

- `subagent({ task, agentName })` in a native session resolves `agentName` against **Agent definitions** discovered from standard locations, and applies the frontmatter (`model`/`thinking`/`tools`) + body (system prompt) to the child.
- A `list_agents` harness tool advertises the discovered definitions (name, description, scope, overrides) so the parent model can discover them without guessing.
- A user's existing pi agent files (`~/.pi/agent/agents/*.md`) work with zero configuration (ADR 0012's consumer contract).
- Backward compatible: an unknown or omitted `agentName` is never an error — today's label-only, config-less behavior.

## Non-goals

- No parallel `tasks` array (full live-harness parity scoped out).
- No `thinking` param on the `subagent` tool (the `model: "provider/id:<level>"` suffix already covers it).
- No external (pi) session path changes (native-only scope).
- No desktop-shipped default agents (ADR 0017's product decision stands: the desktop ships nothing; the user writes files).
- No UI surface for Agent definitions (file-based, like skills).
- No `disableModelInvocation`-style frontmatter flags (skills' ADR 0013 nuance doesn't apply).

## Design

### 1. Discovery — a new pure `src-tauri/src/agents.rs` module

`discover_agents(space_path: Option<&Path>) -> Vec<AgentDefinition>` — pure (fs reads only) and **total**: never panics, never errors; a missing root, unreadable file, or malformed frontmatter is silently skipped (the `skills.rs` contract).

`AgentDefinition { name, description, model: Option<String>, thinking: Option<String>, tools: Option<Vec<String>>, system_prompt: String, scope: AgentScope, path }`, `AgentScope = Space | User` (mirrors `SkillScope`).

- **Roots** — mirror `skills.rs` verbatim: space-level — walk up from the Space to the repo root, at each level `<level>/.agents/agents` + `<level>/.pi/agents` (innermost first); user-level — `~/.agents/agents` + `~/.pi/agent/agents`.
- **File layout** — flat `*.md` files directly in the root (pi's layout; no subdir walk, no depth limit). Symlinked files allowed; canonical-path dedupe shared across roots (a symlink collapse, like skills).
- **Frontmatter** — hand-rolled tolerant parser (skills.rs style, no new crate): `name` (string; **fallback = the file's stem name** — `scout.md` works with no `name` field; more forgiving than pi, consistent with skills' "frontmatter name, else directory name"), `description` (optional, default `""`), `model` (string, optional — `"provider/id"` or `"provider/id:<level>"`), `thinking` (string, optional — a native extension beyond pi's format; pi ignores unknown fields, so the format stays pi-compatible), `tools` (comma-string **or** YAML array — both spellings, per pi's `parseToolList`); body = system prompt (trimmed; empty body → no system prompt).
- **Dedupe** — first-wins by lowercased name: innermost space level shadows outer, space shadows user (skills' precedence). Output sorted by `(name, path)` — deterministic.
- **Per-dispatch discovery** — called fresh on every `subagent`/`list_agents` invocation (a user edit to an agent file applies immediately; matches pi's extension "discovered fresh on each invocation"; cost is a few `read_dir`s).

### 2. Resolution — in `dispatch_subagent` (`harness/loop.rs`)

The resolution lives in the native handler only (native-only scope — `dispatch_params` in bridge.rs stays untouched, so the external pi path is unchanged):

1. `agentName` empty/absent → **no resolution** — config-less dispatch (parent model, parent tools minus `subagent`, no system prompt). `agentName` still rides along as the UI label, exactly as today.
2. Otherwise → `discover_agents(Some(&self.space_cwd))` fresh, **case-insensitive exact-name match**.
3. **Match** → layered `LaunchConfig` (the approved precedence):
   - `model` = explicit param ?? frontmatter `model`
   - `thinking` = explicit ?? frontmatter `thinking`
   - `tools` = explicit param ?? frontmatter `tools`
   - `system_prompt` = explicit param ?? frontmatter body (only if the body is non-empty)
4. **No match** → today's behavior verbatim: `agentName` is a label only, config-less dispatch. **Never an error** — an unknown name degrades silently (the model may have a stale name; erroring would be noisier than the label-only outcome).
5. The downstream `dispatch_native` is **unchanged** — the layered config flows through its existing machinery: catalog model resolution (the `:<level>` suffix handling), thinking-level validation (mismatch → dropped), tools filtering (minus `subagent`), and the `subagent-session-started` payload (which already carries the *resolved* `model`/`thinkingLevel`/`enabledTools` — so the effective config is observable in the UI).

New semantics folded into the existing machinery:

- **Unresolvable frontmatter `model`** (not in the catalog — a stale file): **degrade** to the next layer (parent model) instead of failing the dispatch. Rationale: an explicit `model` param is the model's current intent (unknown → still `Failed`, unchanged); a frontmatter value is a user file that can go stale, and a stale file must not break a dispatch. The model actually used stays visible via the `subagent-session-started` `model` field.
- **Unknown tool names in frontmatter `tools`**: silently dropped (a file hint, not precise intent). If the list empties out after dropping, the frontmatter `tools` is treated as absent → parent tools minus `subagent`. A child is **never zeroed out** by a stale file. (Explicit `tools` param semantics stay verbatim, including today's "empties out → no tools" edge.)

### 3. `list_agents` harness tool + system prompt updates

- **New `ToolSpec`** in `tool_specs()`: `list_agents`, no parameters. Description (what the model sees): *"List available subagent configurations (name, description, source, model/tools overrides). Call before dispatching if unsure which agents exist or which fits the task."* — matches the live harness.
- **Handler** (in `dispatch_tool`'s match): `discover_agents(Some(&self.space_cwd))` → one line per agent: `name (space|user): description` + a bracketed override note when present (`[model: provider/id, tools: a, b, thinking: low]`); `"none"` when empty; deterministic (name-sorted).
- **System prompt** (`prompt.rs`):
  - `<tools>` section: a `list_agents` line (one line per advertised spec, in order — the existing format).
  - `<rules>`: the existing `SUBAGENT_GUIDANCE` line gets a short addendum pointing at `list_agents` (e.g. *"…or a discovered agent (list_agents)"*), still gated on the `subagent` tool being advertised.
- **Recursion guard**: the child's tool set is the parent's minus `subagent` — now also minus `list_agents` (a child can't dispatch, so listing dispatch targets is pointless token burn). Both the `tool_specs()`-names branch and the explicit-`tools`-inheritance branch get the second filter.
- **Naming note**: the Tauri IPC command `list_agents` (the process registry, `spaces.rs:23`) keeps its name — different layer (Client IPC vs. model tool), no runtime collision; the ADR disambiguates.
- **`subagent` ToolSpec description** updated to: *"Delegate tasks to subagents. `task` is required. Optional: `agentName` (a discovered Agent definition — its frontmatter model/thinking/tools + system-prompt body apply, layered under any explicit params), `model`, `systemPrompt`, `tools`. Omit `agentName` for a config-less dispatch (parent model, all tools, no system prompt). Model override is rarely needed."*

### 4. ADR + CONTEXT.md

- **New ADR 0020** (`docs/decisions/0020-native-subagent-agent-definitions.md`): *"The native harness resolves `subagent`'s `agentName` against user-authored Agent definitions (discovered like skills); `list_agents` advertises them."*
  - **Supersedes** ADR 0017's "nameless subagents" decision — explicitly, per 0017's own warning ("a future reader must not 'fix' it into a lookup without superseding this decision"). The ADR states the distinction that makes the supersede defensible: 0017 rejected the desktop **shipping** named presets (a static `general`/`researcher`/`reviewer` table — "a product-level concept the user rejected"); this feature ships *nothing* — the user writes their own definition files, the desktop only discovers them (the ADR 0013 skills model). 0017's main-session prompt decisions stand untouched.
  - Records the real trade-offs: layered precedence (flexibility) over authoritative definitions (simplicity); degrade-not-fail for stale frontmatter (availability) over strict failure (precision); **no trust-gating** of space-level agent files (same trust class as the Space's AGENTS.md/skills — the user opened the Space; a repo-controlled agent file is the same class of repo-controlled prompt the desktop already injects; pi's extension gates project agents behind a confirm, but the desktop's own pattern doesn't gate space-level prompt content).
- **CONTEXT.md** — add:
  > **Agent definition:** A user-authored markdown file (flat, in a standard agents dir) with YAML frontmatter (`name`, `description`, `model`, `thinking`, `tools`) + a system-prompt body — discovered by the desktop like a **Skill** (space-level `.agents/agents` + `.pi/agents` walked to the repo root; user-level `~/.agents/agents` + `~/.pi/agent/agents`). Selectable in a **Subagent session** via the `subagent` tool's `agentName` (layered under explicit params); advertised by the `list_agents` tool. Distinct from **Agent** (the conversation partner) and **Agent registry** (the Client's spawn-command list). _Avoid_: Agent config, agent preset, subagent preset.
  - Plus a one-line update to the **Subagent session** entry: `agentName` now resolves against discovered Agent definitions (ADR 0020) — the "free-form UI label only, ADR 0017" note is superseded there.

## Acceptance (done-when)

- In a native session, `subagent({ task, agentName: "scout" })` with a `scout.md` carrying `model: X` runs the child on `X` with the file's body as system prompt; an explicit `model` in the same call wins over the frontmatter; an unknown/omitted `agentName` dispatches config-less without error.
- `list_agents` lists the discovered definitions (name, scope, description, overrides) and returns `none` when none exist.
- A user's existing `~/.pi/agent/agents/*.md` files resolve with zero configuration.
- A stale frontmatter `model` degrades to the parent model (dispatch succeeds; the `subagent-session-started` payload shows the effective model); a stale frontmatter `tools` never zeroes the child's tools.
- Subagent children never receive the `subagent` or `list_agents` tools.
- ADR 0020 + CONTEXT.md committed; `cargo test` / `cargo clippy --all-targets` / `cargo fmt --check` green.
