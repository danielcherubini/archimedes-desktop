---
status: accepted
date: 2026-09-28
superseded-by:
---

# Skills are desktop-owned (discovery + mention expansion)

The desktop (Client) is the owner of the skill layer: Rust discovers skills from the standard roots (the Space's `.agents/skills` + `.pi/skills` walked up to the repo root, plus user-level `~/.agents/skills` + `~/.pi/agent/skills` — the same roots pi scans), lists them in the left pane, and expands `$name` composer mentions into the skill's full content (in pi's native `/skill:name` expansion format) **before** the user message is recorded and dispatched — so native and external sessions behave identically. The agent's own native skill support (pi: catalog in the system prompt + on-demand loading + `/skill:name`) is untouched and composes with this.

**Why:** one code path, identical behavior across both session types — the user's mental model is one, not two. The desktop has to parse `SKILL.md` frontmatter anyway (name + description for the UI), so expansion is a cheap extension of it. The scanned roots mirror pi's, so the left pane matches what pi advertises in the common case.

**Considered Options**

- **Agent-owned** (the left pane queries the agent's own catalog over the RPC `get_commands` (source `skill`); mentions are rewritten to `/skill:name` for agent-side expansion): rejected — native sessions have no skill support at all (the harness has no discovery), so the feature set would differ per session type and the left pane would be empty for native sessions.
- **Hybrid** (per-session-type catalog + per-session-type expansion): rejected — the most code for the most accuracy, and the accuracy only pays off for the edge case of skills added by pi packages/extensions.

**Consequences**

- **Dual discovery for external sessions** — the desktop and pi each scan the same roots (both read-only, harmless). The left pane shows the desktop's view.
- **Drift from pi package/extension-added skills** — the desktop scans the standard roots only; a package-added skill is visible to the agent (pi's own discovery) but absent from the left pane until the desktop gains package awareness.
- **Recorded transcripts contain the expanded skill content** — the expansion happens BEFORE the message is recorded, so the recorded user message = what the agent saw. A later switch to agent-side expansion would change what history means — changing this decision has real cost.
- **The desktop is a READER of skill directories, never a writer** (v1 has no skill management).
