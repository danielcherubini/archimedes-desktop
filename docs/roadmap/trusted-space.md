---
status: approved
done-when: A Space can be marked trusted (the prompt's third option or the sidebar shield button); a trusted Space's bash/edit/write tool calls run without a permission prompt — main and subagent Sessions, immediate effect, persisted across restarts; sudo_exec and ask are unaffected
---

# Trusted Space — per-Space permission auto-approve

## Problem

The desktop's permission model (ADR 0009) prompts the user for **every** `bash`/`edit`/`write` call from a desktop-spawned session. For a folder the user has already vetted, this is pure friction — every exploratory command asks. There is no way to stop being asked: no auto-approve, no trust mode; a 300 s timeout = block.

## Decision (approved 2026-09-26)

A **Trusted Space** is a Space flagged as trusted — its Sessions (including Subagent Sessions) skip the Permission prompt for the gated tools (`bash`/`edit`/`write`); the Client auto-approves them. `sudo_exec` and `ask` are unaffected.

- **Per-Space, not per-Session** — the user's trust decision is about the *folder*, not the conversation. Trust outlives sessions and restarts.
- **Enforced desktop-side** (ADR 0010) — the gate extension (`gate.ts`) is unchanged; the desktop auto-answers.

## Behavior

### Trust lookup (per request)

1. A `Confirm` request arrives. `Confirm` is the gate's by invariant — `ask` emits `Select`, `sudo_exec` is a desktop modal.
2. The desktop looks up the session's Space: `cwd` → canonicalize → `spaces` row.
3. **Trusted** → respond `Confirmed { confirmed: true }` immediately. No `permission-request` event, no prompt, no oneshot.
4. **Untrusted / no row / canonicalize failure** → today's flow unchanged (prompt, 300 s timeout → block, fail-closed).

### Scope

- Main Sessions and Subagent Sessions (both flow through `handle_extension_ui_request`).
- Takes effect on the *next* tool call — no restart, no session reload.
- `ask` (`Select`) is never auto-answered; `sudo_exec` (desktop-owned confirm + password modal) is never auto-approved.
- `input`/`editor` immediate-cancel and `notify`/`set_status`/… ignore — unchanged.

### Setting trust

- **On the Permission prompt** — a third option, always present: `[Allow, Block, Don't ask again for this Space]` (`optionId: "trust-space"`). Picking it: the waiter answers `confirmed: true` **first**, then sets the Space trusted best-effort (logged on failure — the Space stays untrusted, the next call prompts again).
- **On the Space** — a shield icon button in the `SpaceGroup` hover actions (alongside `+` and the chevron — no context-menu pattern exists in the codebase): untrusted = muted icon + tooltip "Trust this Space — skip permission prompts for bash/edit/write"; trusted = persistently colored (success treatment) + tooltip "Stop trusting this Space". Click → `set_space_trusted` command + optimistic store update (roll back on error).
- **Default**: untrusted.

## Persistence

- `trusted INTEGER NOT NULL DEFAULT 0` on `spaces` (PK = canonical path).
- Fresh DBs: the column joins `SCHEMA`'s `CREATE TABLE spaces`.
- Existing DBs: a one-time `ALTER TABLE spaces ADD COLUMN trusted INTEGER NOT NULL DEFAULT 0` at open, ignoring the duplicate-column error (the codebase's idempotent-at-open pattern — same shape as the cwd→spaces backfill).
- `SpaceRow` gains `trusted: bool` (camelCase) — flows through the existing `list_spaces` path.
- One new Tauri command: `set_space_trusted { space_path, trusted }`.

## UI

- `PermissionPrompt` — **zero frontend change**: it renders one button per payload option (first = primary, the rest = outline); the third option appears automatically. (A vitest case with a 3-option payload guards this assumption.)
- `SpacesList`/`SpaceGroup` — the shield button above; the `SpaceView` type gains `trusted`; a small `setSpaceTrusted` store action (the spaces store has no live refresh).
- No new dialogs, no new stores.

## Edge cases

| Scenario | Behavior |
|---|---|
| Toggle trust **off** mid-session | Next `Confirm` prompts again |
| Toggle trust **on** mid-session | Next call auto-approves |
| Several live Sessions in one Space | All share the flag live (per-request lookup) |
| Subagent Session | Inherits; its prompt's third option works via its own `cwd` |
| `ask` in a trusted Space | Never auto-answered (`Select` ≠ `Confirm`) |
| `sudo_exec` in a trusted Space | Always prompts (separate path) |
| No `spaces` row / canonicalize fails | Fail-closed: prompt as today |
| User hits **Block** | Tool blocked with reason; trust state untouched (only the explicit option / shield sets it) |
| Third option picked, flag write fails | Answer first, flag second (best-effort, logged) |
| Two prompts already pending when trust flips | Both stay up; the flag affects the *next* prompt (no auto-dismiss) |
| `input`/`editor`, timeouts, session-close drain | Unchanged |
| Restart | Flag persisted; read on next open |

## Out of scope (YAGNI)

- Allowlists / per-tool rules / path-scoped trust (a future permission policy — desktop-side per ADR 0010)
- Auto-dismissing pending prompts when trust flips
- A trust indicator outside the sidebar (e.g. the center header)
- Agent-side awareness (no Config option advertised over RPC — trust is a desktop concept)

## Test strategy (TDD)

- `permission.rs` unit tests: trusted → `Confirmed{true}` with **no `permission-request` event emitted**; untrusted → today's flow; `trust-space` outcome → flag set + `Confirmed{true}`; `Select` never auto-answered
- `db.rs` tests: the `ALTER` idempotent across two opens of a pre-existing DB; `set_space_trusted` writes; reads return the flag
- Frontend vitest: `SpacesList` shield button (both states, click → command + optimistic store update, rollback on error); `PermissionPrompt` 3-option payload case
