---
status: accepted
date: 2026-10-02
superseded-by:
---

# The native harness resolves `subagent`'s `agentName` against user-authored Agent definitions

A native session's `subagent` tool took `agentName` as a free-form UI label only (ADR 0017) — a subagent's model/tools/system-prompt had to be hand-typed by the parent model, and the agent-definition files users already write (markdown + YAML frontmatter — the format pi's subagent extension uses) were ignored. We decided: the native harness DISCOVERS Agent definitions (flat `*.md` files; space-level `.agents/agents` + `.pi/agents` walked up to the repo root, user-level `~/.agents/agents` + `~/.pi/agent/agents`) like it discovers skills (ADR 0013), resolves `agentName` against them (case-insensitive exact match), and layers the frontmatter (`model` / `thinking` / `tools`) + body (system prompt) UNDER the explicit tool-call params (explicit > frontmatter > parent defaults). A `list_agents` tool advertises the definitions; a subagent child gets neither `subagent` nor `list_agents`.

**Supersedes** ADR 0017's "nameless subagents" decision (its "the desktop ships NO named subagent presets" consequence + the "a future reader must not 'fix' it into a lookup" warning). The supersession is PARTIAL — 0017's main-session prompt decisions (persist-and-replay, the reduced child message, no per-call rebuild) stand untouched. The distinction that makes it defensible: 0017 rejected the desktop SHIPPING named presets (a static `general`/`researcher`/`reviewer` table — "a product-level concept the user rejected"); this feature ships NOTHING — the user writes their own definition files, the desktop only discovers them (the ADR 0013 skills model). A user's existing `~/.pi/agent/agents/*.md` files work with zero configuration (ADR 0012's "just works with your pi setup" consumer contract).

**Considered Options**

- **Authoritative frontmatter** (a matched definition fully wins; explicit tool params are ignored): rejected — the model loses the ability to partially override ("use scout but with the fast model"); the layered precedence is strictly more flexible and degrades to the same config when no params are passed.
- **Fail the dispatch on a stale frontmatter `model`** (unknown model → `Failed`, like an explicit param): rejected — a frontmatter value is a USER FILE that can go stale (the provider renamed the model); a stale file must not break a dispatch. Explicit params stay strict (they are the model's current intent). The effective model stays observable via the `subagent-session-started` `model` field.
- **Gate space-level agent files behind the Trusted Space flag (ADR 0010) / a confirm prompt (pi's extension does this for project agents)**: rejected — a repo-controlled agent file is the SAME trust class as the Space's `AGENTS.md` / skills, which the desktop already injects ungated (the user opened the Space); adding a second gate for a subset of repo-controlled prompts would be inconsistent.
- **Native-only scope** (the external pi path's `dispatch_params` untouched): the fix targets the native harness only — the pi suite's `subagent` tool spec lives in the separate pi-archimedes repo and its own dispatch path is unchanged.

**Consequences**

- The desktop gains a second consumer of pi's config dir layout (`~/.pi/agent/agents/` — next to `settings.json` / `auth.json` / `models-store.json`, ADR 0012): a pi format change breaks discovery (mitigated: best-effort, a missing/unparseable file is skipped, never a crash — the skills.rs total contract).
- `agentName` is no longer "a no-op for behavior" (ADR 0017's consequence is superseded) — but an UNKNOWN or omitted `agentName` is still a no-op for behavior (label-only, config-less, never an error): the feature is additive, not a breaking change to existing tool calls.
- The `list_agents` NAME collides with the Tauri IPC command `list_agents` (the process registry, `commands/spaces.rs`) — different layers (model tool vs Client IPC), no runtime collision; future readers should not "unify" them.
- A frontmatter `thinking` field is a NATIVE extension beyond pi's agent format (`name` / `description` / `tools` / `model`); pi ignores unknown frontmatter fields, so the file format stays pi-compatible in both directions.
- `skills.rs` and `agents.rs` each carry a copy of the shape-agnostic `parse_value` helper (the reviewed plan kept the modules independent — the two frontmatter shapes differ, and `skills.rs`'s visibility is NOT changed): a bug in the value-reading logic (e.g. the NBSP/dedent byte-offset guard) must be applied to BOTH copies.
