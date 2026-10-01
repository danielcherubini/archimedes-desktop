---
status: accepted
date: 2026-10-01
superseded-by:
---

# Per-model remembered thinking level in Settings

The desktop **remembers the last explicitly-chosen thinking level PER MODEL** — a `defaultThinkingLevels` map (`"provider/id"` → level) in `settings.json` — instead of a global default. At session start (and resume) the remembered level for the session's model wins over the per-agent `HarnessConfig.default_thinking_level`, which wins over `None` (the model's own default); an entry is only applied when it is a member of the model's current `thinking_levels` (a stale entry — the provider changed its levels — is ignored). A mid-session level change writes the map for the session's current model (both native and external sessions); a native model switch keeps the current level when it is valid for the new model and otherwise resets to the new model's remembered level or `None`.

**Why:** the user's ask was "just remember what it was set to last" — and levels are a per-model property (each model advertises its own `reasoningLevels`; `xhigh` on one model is meaningless on another), so the natural unit of memory is the model, not the app. A global default would force one level onto models that don't support it, and a per-session-only memory would lose the choice on every restart.

**Considered Options**

- **A global `defaultThinkingLevel` setting (chosen shape's sibling):** rejected — one level cannot be right for every model's level set; per-model is strictly more expressive and is what "remember what I set" means in practice.
- **Memory in the session row's `capabilities_json`:** rejected — that row is written at start/resume only; a mid-session change is never re-recorded there, so it would go stale. `settings.json` is written on every explicit change, so it is the durable home.
- **A user-visible "thinking defaults" section in the Settings page:** deferred — the memory is implicit (the select shows the remembered level at session start); a visible/clearable list is a later UI decision if users ask for it.

**Consequences**

- **`settings.json` grows an unbounded-key map** — one entry per model the user ever set a level on. No pruning in v1 (a few dozen entries max in practice).
- **Resume prefers memory over the stored row** — the stored `thinkingLevel` is the start-of-session value; after a mid-session change the memory entry is newer and wins (both are validated against the model's live levels).
- **External (pi) sessions apply the remembered level only when the starting model is known** — i.e. `settings.defaultModel` is set (the `set_thinking_level` is sent leniently after `set_model`, before `get_state`, mirroring the ADR 0014-era `set_model` leniency). A session starting on pi's own default model gets no remembered level (the app can't know which model pi picked before `get_state`).
- **A native model switch is a minimal-surprise reset** — the level persists across the switch when valid for the new model; it is only replaced (new model's remembered level, or `None`) when the new model doesn't support it.
