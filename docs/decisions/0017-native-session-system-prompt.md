---
status: accepted
date: 2026-10-01
superseded-by: 0020-native-subagent-agent-definitions.md
---

# The desktop authors the native session's system prompt; subagents are nameless

A native session (ADR 0011) previously had NO system prompt — the `AgentLoop` called the model with the user/tool transcript only, and the optional subagent `launch.systemPrompt` was prepended but never persisted (a resume lost it). We decided: the desktop builds a **minimal, pi-shaped** prompt for the main session (a one-sentence preamble + a tools list + a short rules list + the `project_context`/`skills`/`cwd` data sections — mirroring pi's `buildSystemPromptSections`), persists it as the transcript's system message (seq 0), and replays it **verbatim** on resume (static per session). Subagent children get a REDUCED message: `[launch.systemPrompt, if any]` + the `manage_todo_list` guidance line (when the child has the tool) — no preamble, no tools list. And the desktop ships **NO named subagent presets**: `subagent` = `task` + optional `systemPrompt`/`tools`/`model` + optional `agentName`, where `agentName` is a **free-form UI label only** (no lookup) — "the correct subagent" is one the parent SHAPES via `systemPrompt`/`tools`/`model`.

**Considered Options**

- **Named subagent presets + a `list_agents` tool** (a static table of `general`/`researcher`/`reviewer`; `agentName` selects a preset): rejected — the desktop would "ship agents" (a product-level concept the user rejected); a single-entry list is pointless, and the shaping mechanism is already the `launch` config.
- **Rebuild the prompt per model call / on resume** (the effective prompt tracks the current AGENTS.md/skills): rejected for v1 — pi's model is persist-and-replay (the transcript is the record); a changed AGENTS.md applies from the next new session. Mid-session section diffing (pi's `diffSystemPromptSections`) is a later feature.
- **No system prompt at all (status quo)**: rejected — a native session without project context ignores the project's conventions (TDD, build commands), a functional gap versus pi.

**Consequences**

- A native session's first model call carries a system message; the transcript record and the model's effective prompt are the same string (resume stability).
- `agentName` remains a no-op for behavior — a future reader must not "fix" it into a lookup without superseding this decision.
- The desktop reads `~/.pi/agent/` context files (the global candidate) — one more consumer-side coupling to pi's config dir (best-effort: a missing file is a no-op).
- The `persist_system_message` addition fixes the latent resume-loss of existing `launch.systemPrompt` children.
