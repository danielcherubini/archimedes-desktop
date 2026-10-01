---
status: approved
done-when: A native session starts with a persisted system prompt (preamble + tools + rules + project_context + skills + cwd) and replays it verbatim on resume; a native subagent child starts with [launch systemPrompt, if any] + the todo guidance line (when it has the manage_todo_list tool), persisted the same way; the subagent tool's agentName is a free-form UI label (no lookup).
---

# Native session system prompt

## Problem

Native sessions (the desktop's in-process Rust `AgentLoop`, ADR 0011) call the model with the user/tool transcript only — **no system prompt at all**. The only system-message path is an optional subagent `launch.systemPrompt` (prepended once via `AgentLoop::prepend_system`, **never persisted** — `persist_transcript_message` persists only the LAST message, so a resume of such a child silently loses its system prompt). Versus pi (the first-class agent, whose prompt is deliberately minimal), the native harness lacks a base prompt, project context (AGENTS.md), and skill metadata (progressive disclosure) — and ships tools pi's built-ins don't (`manage_todo_list`, `subagent`) with no guidance for them.

## Decision

The desktop authors a minimal, pi-shaped system prompt for native sessions.

### Main session

Built once at session start, persisted as the transcript's system message (seq 0), replayed **verbatim** on resume (static per session — a resumed conversation keeps the exact prompt the model originally saw; a changed AGENTS.md applies from the next NEW session):

```
You are an expert coding assistant operating inside Archimedes Desktop, a desktop app
that connects to coding agents. You help users by reading files, executing commands,
editing code, and writing new files.

<tools>
- <name>: <the tool's existing description>        (per selected tool)
</tools>

<rules>
- Use manage_todo_list to track multi-step work — write the plan before starting, mark items completed as you go   [if manage_todo_list selected]
- Delegate independent subtasks with subagent — give each a systemPrompt describing its role and constraints (e.g. a read-only researcher, a focused reviewer)   [if subagent selected]
- Be concise in your responses
- Show file paths clearly when working with files
</rules>

<project_context>    [omitted when no context file]
Project-specific instructions and guidelines:

<project_instructions path="...">
...
</project_instructions>
</project_context>

<skills>    [omitted when no skills, or when neither read nor bash is selected]
[pi's formatSkillsForPrompt output verbatim]
</skills>

<cwd>
...
</cwd>
```

- **Preamble**: pi's one sentence with the app name swapped — nothing else authored.
- **`<tools>`**: the session's SELECTED tools (the harness `enabled_tools` convention: `[]` = all) with their existing `ToolSpec` descriptions verbatim, so a disabled tool is absent from both the schema list and this section.
- **`<rules>`**: pi's two rules + the two new guidance lines, each **conditional on its tool being selected** (pi's `toolGuidelines` pattern — a disabled tool's guidance can't reference it).
- **`<project_context>`**: mirrors pi's `loadProjectContextFiles` — the candidate list `AGENTS.override.md` → `AGENTS.md` → `AGENTS.MD` → `CLAUDE.md` → `CLAUDE.MD` (first found wins per dir), walked up from the Space's cwd to the **repo root** (first ancestor with a `.git`; filesystem root if none), order outermost-first, dedup by canonical path, BOM stripped; plus the global `~/.pi/agent/` context file first (the desktop is a consumer of the user's pi setup — ADR 0012).
- **`<skills>`**: the existing `discover_skills(space_path)` (ADR 0013), formatted with pi's `formatSkillsForPrompt` output verbatim (metadata only — the full `SKILL.md` is loaded on demand via `read`); omitted when no skills, or when neither `read` nor `bash` is selected (pi's `fileReadTool` logic).

### Subagent child

The child's system message = `[the launch systemPrompt, if any]` + the todo guidance line (**when the child has the `manage_todo_list` tool** — its tools = the parent's minus `subagent`); if the child has neither → no system message (today's behavior). No preamble, no subagent guidance (the child has no `subagent` tool), no tools list (the tool schemas in the API call already carry names + descriptions).

### Subagents are nameless

The desktop ships **NO named subagent presets**. `subagent` = `task` (required) + optional `systemPrompt`/`tools`/`model` + optional `agentName` — a **free-form UI label only** (no lookup; it flows to the `subagent-session-started` event, which the UI renders as the bold row label above the task text). "The correct subagent" = one the parent SHAPES via `systemPrompt`/`tools`/`model`. *(Considered and rejected: named presets + a `list_agents` tool — the desktop would "ship agents"; a single-entry list is pointless, and the shaping mechanism is already the `launch` config.)*

### Persistence

The system message is persisted at session start: `store.insert_message(session_id, 0, "system", …)` — a new `persist_system_message` beside `persist_transcript_message` (which persists only the LAST message — that's why today's `prepend_system` message is never saved). Resume replays it **verbatim** via `load_transcript` (no rebuild). This **fixes the latent gap**: a resumed child with a `launch.systemPrompt` currently loses it. `model_request` is unchanged — it already sends `self.messages.clone()`, so the system message at index 0 leads every model call. The compactor is unchanged — `prepend_system` already re-estimates.

## Consequences

- A native session's first model call carries a system message (previously none); the transcript record and the model's effective prompt are the same string.
- A resumed native session keeps its original prompt; project-context changes (AGENTS.md edits) apply from the next new session.
- The `subagent` tool's `agentName` remains a no-op for behavior — now explicitly a display label.
- The desktop reads `~/.pi/agent/` context files — one more consumer-side coupling to pi's config dir (best-effort: a missing file is a no-op).
- Mid-session prompt updates (pi's section diffing) are a later feature.

## Out of scope (v1)

- Mid-session prompt updates / section diffing
- `list_agents` tool / named subagent presets / user-defined presets (settings, ADR 0014 territory)
- A `docs` section; `ask`/`sudo_exec` guidance
- Any change to External (pi) sessions — pi owns its prompt
- Worktree context-file shadowing (pi's edge case)
- Frontend changes (the UI already renders `agentName` + `task`)

## Tests (TDD — failing first, per AGENTS.md)

- `prompt.rs` unit tests: main-prompt section order; `<tools>` respects `enabled_tools`; guidance lines conditional on tool selection; child prompt (with/without `systemPrompt`, with/without the todo tool, no-system-message case); `load_project_context` (candidate precedence, walk-up, repo-root stop, dedup, BOM); skills format (byte-identical to pi's `formatSkillsForPrompt` output for the same input).
- `loop.rs` / `session.rs` integration: new session persists the system message at seq 0; resume replays it verbatim (byte-identical, no rebuild); a child with `launch.systemPrompt` persists it (regression test for the latent loss); compactor re-estimation on `prepend_system`.
