---
status: live
last-verified: 2026-09-17
verified-by: src-tauri/tests/bridge_integration.rs (4 scenarios) + the unit suites (both repos) — the manual pass below is the release gate
---

---

# Bridge E2E Checklist (manual)

**How to run.** This is a manual pass — no automated harness. It exercises the *whole* bridge (real `pi` agent + real desktop listener + real UI), which the unit/integration tests only cover at the seams.

Prerequisites:
- **Linux** (the v1 bridge is fail-closed on macOS — the listener is not started — and a no-op on Windows; the checklist is for the supported Linux path).
- `archimedes-desktop` checked out; `pnpm install` done.
- `pi` + `pi-acp` + the **archimedes suite** (pi-archimedes with the bridge module) installed and on PATH.
- Start the app: `pnpm tauri dev` (or `cargo tauri dev`). The built-in `pi` registry entry has `bridge: true`, so the desktop sets the 4 env vars (`PI_ARCHIMEDES_BRIDGE=1`, `PI_ARCHIMEDES_BRIDGE_SOCKET`, `PI_ARCHIMEDES_BRIDGE_SESSION`, `PI_ARCHIMEDES_BRIDGE_SERVER_PID`) at spawn → the suite runs in bridge mode.

**Confirm bridge mode is active** (do this once, before the checklist): start a `pi` session and check that the agent's environment carries the 4 `PI_ARCHIMEDES_BRIDGE_*` vars (e.g. `cat /proc/<pi-pid>/environ | tr '\0' '\n' | grep PI_ARCHIMEDES_BRIDGE`), and that a `bridge-<uuid>.sock` file appears under `$XDG_RUNTIME_DIR/archimedes-bridge-<uid>/` (or the temp dir when `XDG_RUNTIME_DIR` is unset). If the vars are absent, the suite is not in bridge mode — fix the registry entry before continuing.

**What "the bridge works" means:** an interactive tool (`ask`/`sudo_exec`/todo) in a desktop `pi` session renders the **desktop UI** (not a dead TUI path) and the user's choice flows back through the bridge channel to the agent.

---

## 1. `ask`

Trigger by prompting `pi` to use the `ask` tool (e.g. "use the ask tool to let me pick X").

- [ ] **Single question** — an `AskQuestionCard` appears **inline** in the stream. Select an option → submit → the card collapses and `pi` receives the actual answer (`{cancelled:false, results:[…]}`), **not** `"cancelled"`.
- [ ] **Multi** — an `ask` with `multi: true` renders **checkboxes** (multiple selectable). Select several → submit → `pi` receives **all** selected options.
- [ ] **Other (free-text)** — an `ask` with an `Other (type your own)` option → type custom text → submit → `pi` receives the `customInput`.
- [ ] **Cancel** — an `ask` card → Cancel (or Esc) → `pi` receives `{cancelled:true}` and continues (no crash, no hang).
- [ ] **Timeout** — leave an `ask` unanswered until the 5-minute bridge timeout → the desktop's waiter (330 s cap) sends the terminal `error:"cancelled"` frame → the card collapses as cancelled. (Long; the mechanism is covered by the integration test — tick when observed.)
- [ ] **Concurrent asks** — get **two** `ask` tools in one turn → **two** `AskQuestionCard`s **stack** vertically (arrival order). Answer each independently → each gets **its own** response (no cross-talk; correlation is by `(source, toolCallId)`/frame `requestId`, never bare `toolCallId`).
- [ ] **Subagent ask** — a subagent that calls `ask` → a **labeled** card (anchored by `source: "subagent:<name>"`) → answer → the child receives it via the child socket (the bridge is the sole `ASK_RESPONSE` emitter; `refcount` returns to 0).

## 2. `sudo_exec`

Trigger by prompting `pi` to run a command via `sudo_exec`.

- [ ] **Decline** — `SudoConfirmModal` appears (shows `command` + `reason` **verbatim**). Click Cancel/Decline → the command is **not** run (the tool reports the decline error text, preserved).
- [ ] **Cancel (password)** — confirm → `SudoPasswordModal` appears (shows `command` + `reason` verbatim + a masked `•` field). Press Esc (or submit empty) → cancel (`password: ""`) → the command is **not** run.
- [ ] **Warm cache** — `sudo_exec` when the credential is already cached (a recent sudo) → the password prompt is **skipped** (the pre-existing in-memory credential cache is used) → the command runs without a prompt.
- [ ] **Cold cache** — `sudo_exec` with a cold cache → `SudoPasswordModal` → enter the password (masked: one `•` per char, the raw value **never** rendered) → Enter → the password flows **Client UI → bridge channel → `sudo -S` stdin** (never a tool param, never the LLM context / ACP wire) → the command runs.
- [ ] **Password not persisted** — after the reply is sent, the password is **removed from store state immediately** (inspect the bridge store / devtools — it is not retained in `params` after `markAnswered`).

## 3. Todo board

- [ ] **Main column** — the main agent calls `manage_todo_list` → the `TodoBoardPanel` (right rail) shows the **main** column (numbered ✓/◉/○: completed/in_progress/pending).
- [ ] **Subagent columns** — a subagent calls `manage_todo_list` → a **labeled subagent column** appears (fed by the `TODOS_UPDATE` bridge event via the existing `stream.ts` parent-bus relay — the bridge receives it for free).
- [ ] **Auto-clear** — complete all todos → the board **auto-collapses** when empty.
- [ ] **Child exit** — a subagent exits → its todo column is **cleared** (`TODOS_CLEAR` on child exit).
- [ ] **`rawInput` fallback** — for a **non-bridge** agent, the board seeds from the latest `manage_todo_list` `rawInput` (ACP `tool_call` frame, filtered by `title === "manage_todo_list"`).

## 4. Lifecycle / edge cases

- [ ] **Session close with a pending prompt** — trigger an `ask`/`sudo` prompt, leave it pending, then **close the session** → the pending prompt is **drained as cancelled** (the `close_tx` flag cancels the waiter; the `pending_bridge` entry is drained by the `"{session_id}/"` prefix). No leaked waiter, no crash.
- [ ] **Crash / restart mid-request** — kill the desktop (or the agent) mid-request, then **restart** the desktop → the **stale socket file is tolerated/cleaned** at startup (a fresh per-spawn randomized socket is used; a stale file does not block the new session) → the new session works.
- [ ] **Two sessions (frame isolation)** — given the one-live policy, run two `pi` sessions **back-to-back**: start session A, let it push frames (`seq` 1, 2, …), close it; start session B, let it push → session B's `seq` **starts at 1 again** (a fresh per-spawn listener), **not** continuing from A's `lastSeq`. Each session's `bridge-request`/`bridge-event` carries its **own** `sessionId`; no cross-talk.
- [ ] **Agent exit tears the listener down** — let a `pi` session end on its own → the bridge listener is **torn down and unlinked** (the socket file disappears), closing the pid-reuse window.

## 5. Downgrade combos (release safety)

The bridge is **env-gated and non-breaking**. Confirm both directions are safe:

- [ ] **Old desktop + new suite = safe.** Install the new pi-archimedes suite (with the bridge module) but keep the **old** desktop (which **never sets** the env vars) → the suite's `isBridgeMode` is **false** (no env vars) → the bridge is **inert** → a plain terminal run behaves **exactly as today** (`ask`/`sudo` use their existing TUI/headless paths). **No regression.**
- [ ] **New desktop + old suite = safe.** Install the **new** desktop (which sets the env vars) but keep the **old** suite (no bridge module) → the env vars are **ignored** (the old suite never reads them) → the old suite behaves **exactly as today**. **No regression.**
- [ ] **Inert without env (unit-verified, spot-check here).** A plain terminal `pi` run (no desktop, no env vars) → `getBridge().active === false` → `ask` throws `BridgeInactiveError`, `confirm` → `false`, `password` → `""` (the inactive API). The bridge is never visible in a plain terminal.

---

## Rollout (process)

1. **pi-archimedes:** the bridge module + tool changes land as a **non-breaking release** (inert without the env vars — a plain terminal run never sees the bridge; an old desktop never sets the env vars).
2. **archimedes-desktop:** the listener + spawn wiring + the three UI surfaces.
3. **Validate:** the unit tests (both sides) + the integration test (`src-tauri/tests/bridge_integration.rs`) + this manual E2E checklist all pass.
