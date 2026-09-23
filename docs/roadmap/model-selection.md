---
status: approved
done-when: In the desktop, a live pi session's header shows a model selector (populated from pi's model registry) and a thinking-level selector; choosing a value switches the session's model/thinking level mid-session (visible in pi's behavior), with the UI tracking agent-side changes.
---

# Spec: Session model + thinking-level selection (ACP config options)

## Context

The desktop (Client) speaks ACP to coding agents; pi (via `pi-acp`) is the
first-class agent. Users want to select the model — and the thinking level —
from the UI.

`pi-acp` already implements the full ACP config-option mechanism:
`newSession`/`loadSession` responses carry `configOptions` (a `model` select
built from pi's own model registry — `provider/id` entries — plus a
`thought_level` select); `session/set_config_option` switches the model
mid-session (pi's `set_model`); `config_option_update` notifications push the
updated state. **No agent-side work is required.** The desktop today drops
`configOptions` from the session responses and has no `set_config_option`
command.

## Design

**Approach: ACP-native config options** (agent-generic; any ACP agent that
advertises config options works; mid-session switching; the agent is the
source of truth — nothing is persisted).

### Rust backend

1. **`SessionInfo` carries config options** — new field
   `config_options: Option<Vec<SessionConfigOption>>` (camelCase
   `configOptions` over IPC):
   - `start_session` establisher: read from the `newSession` response
   - `resume_session` establisher: read from the `loadSession` response
     (`restored.response.config_options`)
   - `list_sessions` (stored sessions): `None`
   - No DB schema change: `record_session` ignores the field (a resume
     re-fetches fresh state from the agent; persisting would only create a
     stale copy)
2. **New command `set_session_config_option`** —
   `(session_id, config_id, value) → Result<Vec<SessionConfigOption>, AcpError>`:
   - `SessionManager::set_config_option` follows the `send_prompt` pattern:
     clone the live session's connection handle, drop the lock, send
     `SetSessionConfigOptionRequest`, return `response.config_options`
   - Unknown session → `AcpError::UnknownSession`; agent rejection →
     `AcpError::Protocol`
3. **`config_option_update` notifications: no new Rust code** — the existing
   `on_receive_notification` handler already forwards every `SessionUpdate`
   variant (including `config_option_update`) to the frontend as a raw
   `session-update` event
4. **`fake_agent` fixture** — `session/new` and `session/load` responses gain
   `configOptions` (a `model` select with 2–3 `provider/id` entries + a
   `thought_level` select with the 6 levels, `currentValue` set); a new
   handler for `session/set_config_option` (updates the internal
   `currentValue`, returns the updated `configOptions`, emits a
   `config_option_update` notification — mirroring real pi-acp); a new
   `set_config_option_error` variant (the request is rejected)

### Frontend

1. **Types** (`src/lib/tauri.ts`):
   - `SessionConfigOption { id, name, description?, category?, type: "select" | "boolean", currentValue: string | boolean, options?: { value, name, description? }[] }`
   - `SessionInfo.configOptions?: SessionConfigOption[]`
   - `setSessionConfigOption(sessionId, configId, value): Promise<SessionConfigOption[]>`
2. **Store** (`src/store/sessions.ts`):
   - New state `configOptions: Record<string, SessionConfigOption[]>` keyed by
     session id (live sessions only)
   - Seeded from `SessionInfo.configOptions` on start/resume
   - `applySessionUpdate` handles `config_option_update`: **replace the whole
     set** (the agent always sends the full set)
   - The set-command response is applied the same way (idempotent — the agent
     follows it with a `config_option_update` notification)
   - Cleared in `handleSessionClosed`
3. **UI** (the `ChatStream` session header):
   - Two `Select` components (existing `Select`, `variant="ghost"
     size="sm"` — same style as the conversation selector), placed after the
     conversation selector and before the more-menu
   - **Model** — the option with `category === "model"` (fallback:
     `id === "model"`), rendered only when `type === "select"` and `options`
     is non-empty
   - **Thinking** — the option with `category === "thought_level"` (fallback:
     `id === "thought_level"`), same rules
   - Live sessions only; hidden when the agent doesn't advertise the option
     (graceful handling per the ACP spec)
   - Trigger shows the selected option's `name` (e.g.
     `anthropic/claude-sonnet-4-5`, `Thinking: medium`), truncated with a
     max-width
   - On change → `setSessionConfigOption`; the selector is disabled while the
     request is in flight; on error a transient inline message next to the
     selector (`text-destructive`, auto-cleared after ~5 s — the app's
     existing inline-error pattern)
   - Out of scope: subagent sessions (they render in the `SubagentPanel`);
     `boolean`-kind options (not rendered in v1)

### Edge cases

| Case | Behavior |
|---|---|
| Agent doesn't advertise config options | Selectors hidden; everything else unchanged |
| Model list empty / auth error at session start | pi-acp fails `newSession` with the auth error → the existing session-start error path |
| `set_config_option` rejected | Transient inline error; the selector keeps its last known value |
| `config_option_update` without a client request | Store replaced; UI re-renders |
| App restart | On resume, fresh config options from `loadSession` (agent state) — nothing stale persisted |
| Multiple live sessions | Per-session state; the selectors reflect the ACTIVE session |
| `boolean`-kind option | Ignored in v1 |
| Stored (non-live) session | No selectors (no agent process) |

### TDD / verification (failing test first, per AGENTS.md)

**Rust** (`cargo test`, `cargo clippy --all-targets` 0 warnings,
`cargo fmt --check`):
1. `start_session` returns `configOptions` from the `newSession` response
   (fake_agent advertises them)
2. `resume_session` returns `configOptions` from the `loadSession` response
3. `set_session_config_option` round-trips: the request reaches the agent,
   the response's updated `configOptions` come back
4. `set_config_option_error` variant: the command surfaces the agent's
   rejection as `AcpError::Protocol`
5. A `config_option_update` notification arrives at the frontend as a
   `session-update` event with the `configOptions` payload

**Frontend** (`pnpm test`, `pnpm build`):
1. Store: seeded from `SessionInfo.configOptions`; replaced by
   `config_option_update`; cleared on close
2. Component: the model + thinking selectors render from config options
   (category match, select kind, non-empty options); hidden when the option
   is absent or the session isn't live
3. Component: a selection invokes `set_session_config_option`; the selector
   is disabled while in flight; an error shows the transient inline message
