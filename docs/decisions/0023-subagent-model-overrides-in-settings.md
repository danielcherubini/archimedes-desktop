---
status: accepted
date: 2026-10-03
superseded-by:
---

# Per-agent subagent model overrides live in the desktop settings (not the agent files)

A subagent's model could only be changed by editing the agent's markdown file (the frontmatter `model`, ADR 0020) or the main agent's explicit `subagent` tool param. We decided: the desktop's **Settings page owns a per-agent model override** — a `subagentModels` map in `settings.json` (agent name → model key) — layered **between the explicit tool param and the frontmatter** (explicit > override > frontmatter > parent model). The agent markdown files are never written to; an override is the user's current intent for a possibly-shared, possibly-stale file.

**Why:** the agent files are user-authored (and can be repo-controlled, shared across Spaces); a settings-page override lets the user re-point a named agent's model without touching the file — the same "user-managed per-entity settings live in `settings.json`" home as providers (ADR 0014) and MCP servers (ADR 0019).

**Considered Options**

- **SQLite table** (`subagent_overrides` in the app DB): rejected — a second settings location; the established home for user-managed per-entity settings is `settings.json`, and a DB table fragments the settings + adds a schema migration + is hard to hand-edit.
- **Sidecar file per agent** (`scout.override.json` next to `scout.md`): rejected — touches the agent dirs the user explicitly wants untouched; adds a discovery concern; muddles the scope precedence (space vs user).
- **Override beats the explicit tool param** (absolute user control): rejected — the explicit param is the model's current intent (ADR 0020's layering rationale); the main agent keeps the ability to partially override ("use scout but with the fast model").
- **Override as a fallback only** (frontmatter `model` beats the override): rejected — a file with a `model:` line would ignore the override, defeating the point.
- **A global default subagent model** (one selector for all subagents): rejected in favor of per-agent overrides (the chosen scope) — a single global model is too coarse for a multi-agent setup.

**Consequences**

- **The override is SOFT (stale-safe)** — like the frontmatter it layers over: an override value whose bare key resolves to nothing in the effective catalog degrades to the next layer (never fails the dispatch). Only the explicit param stays strict.
- **Overrides are keyed by NAME, not file** — the same identity the `agentName` resolution uses (case-insensitive, first-wins dedupe: a space-level definition shadows a user-level one of the same name, and an override for that name applies to whichever definition wins in the dispatching Space).
- **Orphaned overrides are inert** — a deleted/renamed agent file leaves an ignored map entry; no Rust-side cleanup (the Settings UI's remove button is the only cleanup path).
- **`AgentLoop` keeps `config_dir`** (today it only forwards it to the MCP manager) so `resolve_launch` reads `settings.json` at dispatch time — a settings edit takes effect on the NEXT dispatch, no restart (matching ADR 0020's "resolved at dispatch time, not a startup snapshot").
- The companion thinking-order fix (explicit > model-key `:<level>` suffix > frontmatter `thinking`) resolves a doc/code contradiction in `dispatch_native_inner`'s `launch.thinking.or(level_suffix)` — the feature doc's order wins.
