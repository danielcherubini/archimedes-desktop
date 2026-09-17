---
status: committed
done-when: In a desktop session with the bridge active, `ask` returns the user's actual answer via the desktop's AskQuestionCard (not "cancelled"); `sudo_exec` completes through the desktop's confirm + masked-password modals; the todo board updates live from bridge events (main + subagent columns). ADRs (pi-archimedes 0021/0022, desktop 0003) and glossary entries are recorded; the suite release is non-breaking (bridge inert without the env vars).
---

# Bridge Plan

**Goal:** when the Agent (pi, managed by the desktop "Client") uses the archimedes interactive tools, the Client renders the real UI — not a dead TUI path. Scope: bridge mechanism + `ask` + `sudo_exec` + todo board.

**Architecture:** An env-gated local channel ("the bridge") from the suite to the Client. The Client is the server (a 0600 Unix socket / Windows named pipe, peer-verified by process identity); the suite is the client (ephemeral `net.createConnection` per message, herdr-style). The bridge lives in `packages/core` (every package's peer dep) so it ships with any archimedes install and stays inert without the env vars. `ask` and `sudo_exec` each gain a one-line delegate to the bridge; the Client renders `AskQuestionCard` (inline), `SudoConfirmModal` + `SudoPasswordModal` (modals), and `TodoBoardPanel` (right rail). ACP is untouched — it stays the read-only transcript + generic permission gate.

**Tech Stack:** pi-archimedes (pnpm monorepo, TypeScript, vitest, `node:net`/`node:crypto`); archimedes-desktop (Tauri 2, Rust, tokio, `@tauri-apps/api`, React 19, zustand, vitest + @testing-library/react).

---

## Shared facts (read before any task)

**Repo paths:**
- pi-archimedes monorepo: `~/Coding/Javascript/pi-archimedes` (packages: `core`, `ask`, `sudo`, `subagent`, `todo`, …). Root `pnpm test` runs `vitest run` using `vitest.config.ts` (a `projects` list). Each package has its own `vitest.config.ts` + co-located `*.test.ts`.
- archimedes-desktop: `~/Coding/AI/archimedes-desktop` (Rust in `src-tauri/`, React in `src/`). `pnpm test` runs `vitest run` (frontend); `cargo test` runs the Rust tests.

**pi-archimedes key code (verified):**
- `packages/core/src/bus.ts` — global pub/sub via `globalThis` `Symbol.for("archimedes:bus")`. `getBus()` / `initBus()`. `Events` const: `COST_UPDATE`, `TODOS_UPDATE`, `TODOS_CLEAR`, `ASK_REQUEST` (`"archimedes:ask_request"`), `ASK_RESPONSE` (`"archimedes:ask_response"`). Payload types: `CostUpdatePayload {source, inputTokens?, …}`, `TodoUpdatePayload {source, todos}`, `TodoClearPayload {source}`, `AskRequestPayload {source, requestId, questions}`, `AskResponsePayload {requestId, cancelled, results}`. There is a pre-init event queue (events emitted before `initBus` are queued and flushed).
- `packages/core/src/index.ts` — `registerCore(pi)` is the extension entry. It subscribes `session_start` (captures `ctx`, calls `initBus()`) and `session_shutdown`. `registerCore` is called from the default export. **This is where `registerBridge(pi)` is wired in (Task 1).**
- `packages/core/package.json` — `exports` map (`.`, `./bus`, `./chrome`, …). **Add `./bridge` (Task 1).**
- `packages/ask/src/tool.ts` — `registerAskTool(pi)` registers the `ask` tool: `execute(_toolCallId, params, _signal, _onUpdate, ctx)`. The `!ctx.hasUI` branch (headless/subagent) connects to `PI_SUBAGENT_SOCKET` and exchanges `ask_request` (`{type, requestId, questions}` — **no `toolCallId` today**) / `ask_response` JSON lines (5-min timeout). The `ctx.hasUI` branch (TUI) emits `getBus().emit(Events.ASK_REQUEST, {source: "main", requestId: randomUUID(), questions})` (a notify hook — **it is never paired with an `ASK_RESPONSE` today**) then shows the TUI picker. The results are built inline in the `!ctx.hasUI` branch.
- `packages/ask/src/ipc-relay.ts` — `registerIpcRelay(pi, getCtx, unsubscribes)` subscribes `ASK_REQUEST`; the handler skips `source === "main"`, requires `ctx?.ui` (in `handleAskRequest`), shows the TUI picker, and emits `ASK_RESPONSE`. It is registered **unconditionally** at extension load (`packages/ask/src/index.ts:18`).
- `packages/subagent/src/spawn.ts` — `startAskSocketServer(agentName)` creates a per-child socket server. On `ask_request` it registers a write-back in `pending` (requestId → `socket.write`) and emits `ASK_REQUEST` (`{source: "subagent:<name>", requestId, questions}`). On `ASK_RESPONSE` it writes back to the child. `child.on("exit", cleanupSocket)` closes the server but **does not drain `pending` or emit any cancel** (the gap Task 2 fills).
- `packages/subagent/src/stream.ts` — `streamEvents` watches the child's `manage_todo_list` `tool_execution_end` and emits `TODOS_UPDATE` (`{source: "subagent:<name>:<childSessionId>", todos}`) on the parent bus; on `child.on("close")` it emits `TODOS_CLEAR`. **This subagent-todo path already exists** — the bridge receives it for free.
- `packages/sudo/src/prompt.ts` — `promptForPassword(ctx, command?, reason?)` is gated `if (ctx.mode !== "tui") return Promise.reject(…)` (the 0010 headless gate) then shows a `ctx.ui.custom` masked field; `confirmCommand(ctx, command, reason)` uses `ctx.ui.confirm`. `packages/sudo/src/tool.ts:426/440` call them unconditionally.
- **`vitest.config.ts` (root) `projects` list is MISSING `packages/sudo`** — sudo has tests + a `vitest.config.ts` but they don't run under `pnpm test`. Task 3 adds it.

**archimedes-desktop key code (verified):**
- `src-tauri/src/config/registry.rs` — `AgentEntry {id, name, command, args, env: BTreeMap<String,String>}`; `Registry::default_registry()` yields one `pi` entry (`command: "pi-acp"`, empty `env`). **Task 4 adds `bridge: bool` (default `false`) and sets `bridge: true` on the default `pi` entry.**
- `src-tauri/src/acp/session.rs` — `SessionManager {sessions, pending_permissions, registry, config_dir, db, establish_timeout}`. `start_session`/`resume_session` build `AcpAgent::new(AcpAgentConfig::new(entry.command).args(entry.args).envs(entry.env))`. `drive_session` spawns the driver task (the ACP client + `connect_with` closure) and, on close, drains `pending_permissions` by the `"{session_id}/"` prefix. `EventSink` trait (`emit(&str, Value)`) is how events reach the frontend; `TauriSink` (in `commands/sessions.rs`) implements it via `AppHandle::emit`.
- `src-tauri/src/acp/permission.rs` — the template for user-paced prompts: `handle_permission_request` emits a `permission-request` Tauri event, registers a oneshot in `pending_permissions` (keyed `"{session_id}/{request_id}"`), and `cx.spawn`s a waiter (300 s cap) that responds to the agent. `respond_permission` (in `commands/sessions.rs`) resolves the oneshot. **Task 4 mirrors this for `bridge-request` / `pending_bridge` / `respond_bridge_request`.**
- `src-tauri/src/lib.rs` — `setup_dirs` manages the `EventSink`, `SessionManager`, and `Db`; the `invoke_handler` lists the commands. **Task 4 registers `respond_bridge_request`.**
- `src/store/permissions.ts` — the zustand template (pending prompts keyed by session id; `addPrompt`/`removePrompt`/`dismissSessionPrompts`). **Task 5 mirrors it for the bridge store.**
- `src/lib/tauri.ts` — typed wrappers over the IPC (`respondPermission`, `listenPermissionRequest`, …). **Task 5 adds the bridge wrappers.**
- `src/components/PermissionPrompt.tsx` — the inline-card template (reads the store, `respondPermission`, removes on success). **Task 5 mirrors it for `AskQuestionCard`.**
- `src/components/ChatStream.tsx` — mounts `PermissionPrompt` inline in the message stream (after `messages.map`). **Task 5 mounts the bridge surfaces here.** `src/App.tsx` registers the event listeners once and dispatches into the stores.
- `src-tauri/Cargo.toml` — `tokio` features `["process","io-util","sync","macros","time"]`; `uuid` (v4); `serde`/`serde_json`. **Task 4 adds `tokio` `net` + `rt`, `libc` (Linux `SO_PEERCRED`), and the `windows` crate (Windows pipe pid APIs).**

**Protocol (the wire contract — full detail in the Reference section below):**
- Frames are line-delimited JSON, all carrying `v: 1`. Push `{v, type:"push", seq, event, payload}`; Request `{v, type:"request", id, method, source, toolCallId, params}`; Response `{v, type:"response", id, result|error}`; Ack = one line after a push.
- Methods: `ask` (params = ask schema; result = `AskResponsePayload` verbatim), `confirm` (`{command,reason}` → `{confirmed}`), `password` (`{command,reason}` → `{password}` or cancel). `toolCallId` rides the frame (nullable for `confirm`/`password`); `source` = `"main"` or `"subagent:<name>"`.
- Push events: `todos_update`/`todos_clear`/`cost_update` (bus payload verbatim), `state` (`working`/`idle`/`blocked`), `session` (pi session refs + echo of `PI_ARCHIMEDES_BRIDGE_SESSION`).
- Env contract (Client sets at spawn): `PI_ARCHIMEDES_BRIDGE=1`, `PI_ARCHIMEDES_BRIDGE_SOCKET=<path>`, `PI_ARCHIMEDES_BRIDGE_SESSION=<the desktop's own session id>`, `PI_ARCHIMEDES_BRIDGE_SERVER_PID=<the desktop's own pid>`. All present + `ctx.mode !== "tui"` + `PI_SUBAGENT_SOCKET` absent → bridge mode.
- `state` machine: `refcount > 0 → blocked`; `settled ∧ refcount == 0 → idle`; else `working`. `refcount`: +1 on `ASK_REQUEST` (any source), −1 on `ASK_RESPONSE`. `settled`: `agent_start` → false, `agent_settled` → true.
- Failure: push = best-effort (initial + 2 retries at 500/1500 ms, then drop); interactive = 5-min timeout → cancel; connection close = immediate cancel on both ends; the Client sends an explicit terminal frame (`error:"cancelled"`) before closing on timeout/drain/auth-failure; unreachable channel → fail fast; session close → drain pending as cancelled. Client waiter cap: 330 s.
- Inactive API: `ask` → throws `BridgeInactiveError`; `confirm` → `false`; `password` → `""`.

---

### Task 1: core bridge module

**Context:** The bridge is the env-gated local channel from the suite to the Client. It lives in `core` (every package's peer dep) so it ships with any archimedes package (including standalone installs) and the ask/sudo delegates import it exactly like they import the bus. This task creates the bridge module (`channel` + `events` + `index`), adds the `ASK_CANCEL` bus event + the `AskRequestPayload.toolCallId` field, wires `registerBridge` into core's extension entry, and adds the `./bridge` export. It is the foundation: Tasks 2–3 import `getBridge()` from here.

**Files:**
- Create: `packages/core/src/bridge/channel.ts`
- Create: `packages/core/src/bridge/events.ts`
- Create: `packages/core/src/bridge/index.ts`
- Create: `packages/core/src/bridge/index.test.ts`
- Modify: `packages/core/src/bus.ts`
- Modify: `packages/core/package.json`
- Modify: `packages/core/src/index.ts`

**What to implement:**

`packages/core/src/bus.ts`:
- Add to the `Events` const: `ASK_CANCEL: "archimedes:ask_cancel"`.
- Add + export a payload type: `interface AskCancelPayload { requestId: string; source: string }`.
- Extend the existing `AskRequestPayload` interface with an optional `toolCallId?: string` field (keep `source`, `requestId`, `questions`). Do not change the other payload types.

`packages/core/src/bridge/channel.ts` (the connection layer — herdr + subagent precedents):
- Module-level `let active = false`, `let socketPath: string | undefined`, `let seq = 0` (starts at 0; the first frame is `seq: 1`).
- `export function configure(opts: { active: boolean; socketPath?: string }): void` — sets `active` and `socketPath` (called from `registerBridge` at `session_start`).
- `function socketTarget(): string` — returns `process.platform === "win32" ? `\\\\.\\pipe\\${socketPath}` : socketPath` (the herdr win32 line).
- `export function sendEvent(event: string, payload: unknown): void` — best-effort push. Build the frame `{v: 1, type: "push", seq: ++seq, event, payload}`. `net.createConnection(socketTarget())`; on connect, write `JSON.stringify(frame) + "\n"`; on the first `data` (the ack line), `socket.destroy()`; on `error`/no-ack, retry — initial attempt + 2 retries at 500 ms then 1500 ms, then drop. Coalesce to at most one in-flight connection per event type (a `Map<event, inFlight>` guard; if one is in flight, queue the latest payload and send it when the current one settles — latest wins). A failed connect is a no-op (the `active` flag does not depend on a successful connection).
- `export function request<T>(method: string, params: unknown, opts?: { toolCallId?: string; source?: string }): { promise: Promise<T>; cancel: () => void }` — interactive. Build `{v: 1, type: "request", id: randomUUID(), method, source: opts?.source ?? "main", toolCallId: opts?.toolCallId, params}`. `net.createConnection(socketTarget())`; on connect, write the frame; buffer `data` lines; on a line whose `type === "response"` and `id` matches, resolve with `result` (or reject with `error`); on `error`/`close` before a response, reject with a cancel error; a 5-minute `setTimeout` (unref'd) rejects with a cancel error and `socket.end()`. **`cancel()` closes the socket (→ the promise rejects with a cancel error) and is idempotent** (the `ASK_CANCEL` path needs a cancellation surface — a bare `Promise` cannot be cancelled). Connection close = immediate cancel. Unreachable channel → fail fast (reject on connect error).

`packages/core/src/bridge/events.ts` (the bus subscriptions — the state machine + subagent forwarding + push events):
- Module-level `let refcount = 0`, `let settled = false`, `let started = false` (a `session_start` has occurred).
- `export function state(): "working" | "idle" | "blocked" { return refcount > 0 ? "blocked" : (settled ? "idle" : "working"); }`
- `function pushState(): void { sendEvent("state", { state: state() }); }`
- **Pi extension events (NOT bus events — nothing emits `agent_start`/`agent_settled`/`session_start` on the bus; they're dispatched by the extension runner via `pi.on(...)` and DO fire in RPC mode, since the RPC driver runs the same session machinery):** `export function onAgentStart(): void { settled = false; pushState(); }`, `export function onAgentSettled(): void { settled = true; pushState(); }`, `export function onSessionStart(): void { started = true; sendEvent("session", { ...sessionRefs(), bridgeSession: process.env.PI_ARCHIMEDES_BRIDGE_SESSION }); }` (called from `registerBridge`'s `pi.on` handlers — see `index.ts`). `sessionRefs()` reads `ctx.sessionManager` if available, else `{}`.
- `const pending = new Map<string, { source: string; toolCallId?: string; cancel: () => void }>();`
- `export function start(): void` — subscribe (via `getBus().on`) to the **six real bus events only** and store the unsubscribers in a module array (so a re-`start` on `/reload` doesn't double-subscribe — guard with a `let started` flag so `start` is idempotent). Bus event names map to wire names by **stripping the `archimedes:` prefix via a lookup table** (not string surgery): `COST_UPDATE` → `cost_update`, `TODOS_UPDATE` → `todos_update`, `TODOS_CLEAR` → `todos_clear`, `ASK_REQUEST`/`ASK_RESPONSE`/`ASK_CANCEL` → (no push; refcount / subagent forwarding / cancel):
  - `ASK_REQUEST`: `refcount++;` if `payload.source !== "main"` (a subagent ask) → forward to the Client: `const { cancel } = requestAskToClient(payload)`; `pending.set(payload.requestId, { source: payload.source, toolCallId: payload.toolCallId, cancel })`. (A `source === "main"` ask is NOT forwarded here — the root's Client request comes from `bridge.ask()` directly; the bus event is for the refcount only.)
  - `ASK_RESPONSE`: `refcount--; pushState();`
  - `ASK_CANCEL`: `const entry = pending.get(payload.requestId); if (entry) { entry.cancel(); }` (the `cancel` closes the socket → the `request` rejects → the `.catch` below emits the cancelled `ASK_RESPONSE` → `refcount--` + `pending.delete`; the `ASK_CANCEL` itself does not touch the refcount — no double-decrement).
  - `TODOS_UPDATE` / `TODOS_CLEAR` / `COST_UPDATE`: `sendEvent(<wire name>, payload)` (payload verbatim — the bus payload types ARE the wire types).
- `function requestAskToClient(payload): { promise: Promise<void>; cancel: () => void }` — send the forwarded ask to the Client and wire the response: `const handle = channel.request<{ cancelled: boolean; results: Array<{ id: string; selectedOptions: string[]; customInput?: string }> }>("ask", payload.questions, { toolCallId: payload.toolCallId, source: payload.source });` `handle.promise.then((resp) => { getBus().emit(Events.ASK_RESPONSE, { requestId: payload.requestId, cancelled: resp.cancelled, results: resp.results }); pending.delete(payload.requestId); }).catch(() => { getBus().emit(Events.ASK_RESPONSE, { requestId: payload.requestId, cancelled: true, results: payload.questions.map((q) => ({ id: q.id, selectedOptions: [] })) }); pending.delete(payload.requestId); });` return `{ promise: handle.promise, cancel: handle.cancel }`. (The bridge is the **sole emitter** of child-path `ASK_RESPONSE` — `spawn.ts` only signals via `ASK_CANCEL`.)

`packages/core/src/bridge/index.ts` (the public API):
- `import type { AskResponsePayload, AskRequestPayload } from "../bus.js";` (relative, matching `index.ts`'s existing `import { initBus } from "./bus.js"` — type-only, no cycle: `bus.ts` imports nothing from `bridge`).
- `export class BridgeInactiveError extends Error { constructor() { super("bridge is not active"); this.name = "BridgeInactiveError"; } }`
- `export function getBridge() { return { active: channelActive(), ask, confirm, password, state: eventsState }; }` where `channelActive()` reads `channel`'s `active`, `eventsState` reads `events`' `state()`.
- `export function ask(params: { questions: AskRequestPayload["questions"] }, toolCallId: string): Promise<AskResponsePayload>` — `if (!channelActive()) throw new BridgeInactiveError(); return channel.request<AskResponsePayload>("ask", params, { toolCallId, source: "main" }).promise;` (the param is typed **structurally from the bus payload** — `AskQuestion` lives in `packages/ask`, which core cannot import; the `AskRequestPayload["questions"]` shape already models id/question/description/options/multi/recommended).
- `export function confirm(params: { command: string; reason: string }): Promise<boolean>` — `if (!channelActive()) return Promise.resolve(false); return channel.request<{ confirmed: boolean }>("confirm", params, { source: "main" }).promise.then((r) => r.confirmed).catch(() => false);`
- `export function password(params: { command: string; reason: string }): Promise<string>` — `if (!channelActive()) return Promise.resolve(""); return channel.request<{ password?: string }>("password", params, { source: "main" }).promise.then((r) => r.password ?? "").catch(() => "");`
- `export function registerBridge(pi: ExtensionAPI): void` — subscribe the **pi extension events** (NOT the bus — nothing emits `agent_start`/`agent_settled`/`session_start` on the bus):
  - `pi.on("session_start", (_e, ctx) => { const active = isBridgeMode(ctx); channel.configure({ active, socketPath: process.env.PI_ARCHIMEDES_BRIDGE_SOCKET }); if (active) { events.start(); events.onSessionStart(); } });`
  - `pi.on("agent_start", () => { if (channelActive()) events.onAgentStart(); });`
  - `pi.on("agent_settled", () => { if (channelActive()) events.onAgentSettled(); });`
  where `isBridgeMode(ctx)` = `process.env.PI_ARCHIMEDES_BRIDGE === "1" && !!process.env.PI_ARCHIMEDES_BRIDGE_SOCKET && !!process.env.PI_ARCHIMEDES_BRIDGE_SESSION && !!process.env.PI_ARCHIMEDES_BRIDGE_SERVER_PID && ctx.mode !== "tui" && !process.env.PI_SUBAGENT_SOCKET`. (TUI always wins; root-only; the `active` flag does not depend on a successful connection. The extension events DO fire in RPC mode — the RPC driver runs the same session machinery.)
- **`PI_ARCHIMEDES_BRIDGE_SERVER_PID` is presence-checked only in v1** (part of `isBridgeMode`). The Windows agent-side `GetNamedPipeServerProcessId` verification is a **documented v1 limitation** (Node's `net` exposes no such API — it would need a native addon); the desktop-side descendant check is the real defense, and the same-user environ-reading attacker it would additionally deter is already inside the stated trust boundary. (ADR 0022 is amended to match.)

`packages/core/package.json`:
- Add `"./bridge": "./src/bridge/index.ts"` to the `exports` map.

`packages/core/src/index.ts`:
- `import { registerBridge } from "./bridge/index.js";`
- In `registerCore`, after `patchConsoleLog();`, add `registerBridge(pi);` (top-level, like `patchConsoleLog` — never inside a session handler, so it doesn't accumulate on `/reload`).

`packages/core/src/bridge/index.test.ts` (TDD — write these first):
- Env matrix (set `process.env` + a fake `ctx`): (a) no env → `getBridge().active === false`; (b) all env + `ctx.mode === "tui"` → `active === false` (TUI wins); (c) all env + `ctx.mode === "rpc"` + `PI_SUBAGENT_SOCKET` unset → `active === true`; (d) all env + `ctx.mode === "rpc"` + `PI_SUBAGENT_SOCKET` set → `active === false` (root-only).
- Inactive API: `ask` rejects with `BridgeInactiveError`; `confirm` resolves `false`; `password` resolves `""`.
- State machine (drive the **`events` forwarding functions** — `events.onAgentStart()`/`events.onAgentSettled()` — and the real bus events; do NOT emit `agent_start`/`agent_settled` on the bus, since nothing does): `onAgentSettled()` → `state() === "idle"`; `onAgentStart()` → `state() === "working"`; emit `ASK_REQUEST` on the bus → `state() === "blocked"` (and stays `blocked` through a subsequent `onAgentSettled()`); emit the pairing `ASK_RESPONSE` on the bus → back to `idle` (if settled) / `working` (if not).
- `ASK_CANCEL` flow: emit `ASK_REQUEST` (source `subagent:x`) on the bus with a fake socket server that never responds, then emit `ASK_CANCEL` on the bus for that `requestId` → the bridge emits a cancelled `ASK_RESPONSE` and `refcount` returns to 0 (assert via `state()`).
- `request` round-trip + cancel: a `net.createServer` on a temp socket path that echoes a `{type:"response", id, result}` frame for a matching `id` → `request(…).promise` resolves with the `result`; a second test calls `request(…).cancel()` before the server responds → the promise rejects with a cancel error.
- `seq` counter: two `sendEvent` calls produce `seq: 1` then `seq: 2` (assert via the frames the fake server receives).
- Reset `process.env` + the module singletons between tests (export a `__resetForTests()` from `channel`/`events` or re-import via `vi.resetModules`).

**Steps:**
- [ ] Write the failing tests in `packages/core/src/bridge/index.test.ts` (the cases above).
- [ ] Run `pnpm --filter @pi-archimedes/core test` (or `pnpm test` from the repo root) — confirm the new tests fail (module doesn't exist).
- [ ] Implement `bus.ts` changes (`ASK_CANCEL` + `AskCancelPayload` + `AskRequestPayload.toolCallId`).
- [ ] Implement `channel.ts`, `events.ts`, `index.ts`.
- [ ] Wire `registerBridge(pi)` into `core/src/index.ts`; add the `./bridge` export to `core/package.json`.
- [ ] Run `pnpm --filter @pi-archimedes/core test` — all bridge tests pass.
- [ ] Run `pnpm test` (full suite) — no regressions (the bus change is additive).
- [ ] Run `pnpm --filter @pi-archimedes/core exec tsc --noEmit` (or the package's typecheck) — types clean.
- [ ] Commit: `feat(core): bridge channel (env-gated, herdr-style) + ASK_CANCEL bus event`.

**Acceptance criteria:**
- [ ] `getBridge().active` is true iff (all 4 env vars present ∧ `ctx.mode !== "tui"` ∧ `PI_SUBAGENT_SOCKET` absent), evaluated at `session_start`.
- [ ] TUI mode is permanently inactive even with env present; a subagent child (env present) is inactive.
- [ ] `ask`/`confirm`/`password` hit the inactive API when `active` is false; `ask` throws `BridgeInactiveError`.
- [ ] The `state` machine matches the spec (blocked/idle/working) under the full event sequence, including a pending ask spanning `agent_settled`.
- [ ] Every `ASK_REQUEST` the bridge forwards is eventually paired with an `ASK_RESPONSE` (success, error, timeout, or `ASK_CANCEL`), so `refcount` never leaks.
- [ ] Push frames are `seq`-numbered from 1; the `session` push is re-emitted on every `session_start` and echoes `PI_ARCHIMEDES_BRIDGE_SESSION`.
- [ ] The full `pnpm test` suite passes (no regression from the `bus.ts` change).

---

### Task 2: ask tool + ipc-relay gate + subagent ASK_CANCEL wiring

**Context:** The `ask` tool gains a bridge branch (checked first), the `ask_request` socket payload + the bus `ASK_REQUEST` payload carry the tool-call id, the `ipc-relay` is gated on `!getBridge().active` (per-message), and the subagent `spawn.ts` gains the `ASK_CANCEL` wiring (a socket-close handler that signals a child exit so the bridge can cancel the relayed Client request). This makes the ask data flow work in bridge mode with exactly one consumer per source.

**Files:**
- Modify: `packages/ask/src/tool.ts`
- Modify: `packages/ask/src/ipc-relay.ts`
- Modify: `packages/subagent/src/spawn.ts`
- Modify: `packages/ask/src/tool.test.ts` (create if absent — the ask tool has no co-located test today)
- Modify: `packages/subagent/src/spawn.test.ts` (create if absent)

**What to implement:**

`packages/ask/src/tool.ts`:
- `import { getBridge } from "@pi-archimedes/core/bridge";`
- **Extract a shared `responseToResults(response, questions)` helper** (move the results-building logic currently inline in the `!ctx.hasUI` branch into a top-level function that takes the `AskResponsePayload`-shaped `{cancelled, results}` + `params.questions` and returns `QuestionResult[]`). Both the child branch and the new bridge branch call it.
- **Reorder the `execute` body to:**
  1. `if (getBridge().active) {` — the new bridge branch:
     - `const requestId = randomUUID();`
     - `getBus().emit(Events.ASK_REQUEST, { source: "main", requestId, questions: params.questions, toolCallId: _toolCallId });` (notify + `blocked` refcount — the root's Client request is sent by `bridge.ask()` directly, NOT by the relay/bridge bus subscription).
     - **try/catch so the pairing `ASK_RESPONSE` fires on ALL paths (success, error, timeout) — otherwise `refcount` leaks and the state machine is stuck at `blocked` forever:**
       ```
       let response: AskResponsePayload;
       try {
         response = await getBridge().ask(params, _toolCallId);
       } catch {
         response = { cancelled: true, results: params.questions.map((q) => ({ id: q.id, selectedOptions: [] })) };
       }
       getBus().emit(Events.ASK_RESPONSE, { requestId, cancelled: response.cancelled, results: response.results });
       ```
       (the `ASK_RESPONSE` emit is **new** — the existing TUI path never paired its notify-only `ASK_REQUEST (main)`; this is new code, not a mirror).
     - `const results = responseToResults(response, params.questions);` then return the same `{content, details}` shape the other branches return (reuse `buildAskSessionContent` + the cancel short-circuit).
     - `}`
  2. `if (!ctx.hasUI) {` — the existing child socket branch, **unchanged except**: the `ask_request` socket write gains `toolCallId: _toolCallId` (so the parent `spawn.ts` can propagate it into the bus `ASK_REQUEST`), and the inline results-building is replaced by the `responseToResults` helper.
  3. `else {` — the existing TUI branch, **unchanged except**: the `ASK_REQUEST` emit gains `toolCallId: _toolCallId` (and keep the existing `randomUUID()` `requestId`). No `ASK_RESPONSE` emission here (the TUI path settles the picker inline; the bridge is inactive in TUI mode).
- Preserve the existing empty-questions guard (it **stays in the TUI section, after the bridge/child branches** — the reorder puts the bridge branch first, but the guard's position relative to the TUI picker is unchanged) and all result-text behavior.

`packages/ask/src/ipc-relay.ts`:
- `import { getBridge } from "@pi-archimedes/core/bridge";`
- At the **top of the `ASK_REQUEST` subscription handler** (alongside the `if (data.source === "main") return;` skip), add `if (getBridge().active) return;`. This is a **per-message** gate (in the handler, not at registration — `registerIpcRelay` runs at extension load when `bridge.active` is false in every mode, and `ctx.mode` is only captured at the first `session_start`, so a registration-time gate would silently reintroduce the double-consumption bug). Result: TUI → bridge inactive → relay on; RPC-no-env → bridge inactive → relay on (unchanged); bridge mode → relay off, bridge on. Exactly one consumer per source.

`packages/subagent/src/spawn.ts`:
- In the `ask_request` handler, read `msg.toolCallId` (now present from the child) and include it in the `ASK_REQUEST` emit: `getBus().emit(Events.ASK_REQUEST, { source: \`subagent:${agentName}\`, requestId: msg.requestId, questions: msg.questions, toolCallId: msg.toolCallId });`
- **Add the `ASK_CANCEL` wiring:** track the requestIds per socket (`const socketRequestIds = new Set<string>()`; add `msg.requestId` in the `ask_request` handler). Add a `socket.on("close", () => { for (const rid of socketRequestIds) { if (pending.has(rid)) { getBus().emit(Events.ASK_CANCEL, { requestId: rid, source: \`subagent:${agentName}\` }); pending.delete(rid); } } });`. This covers both child exit (the socket closes) and the child's own 5-min timeout (the child `socket.end()`s → the socket closes). `spawn.ts` **signals** via `ASK_CANCEL`; it does **not** emit `ASK_RESPONSE` (the bridge is the sole emitter — no double-decrement).

`packages/ask/src/tool.test.ts` (TDD — create):
- Bridge branch: with a mocked `getBridge()` (`active: true`, `ask` resolves a canned `AskResponsePayload`), calling `execute` emits `ASK_REQUEST (main)` (with `toolCallId`) then `ASK_RESPONSE (main)` (with the matching `requestId`), and returns the built results. Assert via a spy on `getBus().emit`.
- Child branch unchanged: with `getBridge().active === false` + `ctx.hasUI === false`, the tool still writes an `ask_request` line to `PI_SUBAGENT_SOCKET` (now including `toolCallId`).
- TUI branch unchanged: with `getBridge().active === false` + `ctx.hasUI === true`, the tool emits `ASK_REQUEST (main)` (with `toolCallId`) and does NOT emit `ASK_RESPONSE (main)`.
- `ipc-relay`: with `getBridge().active === true`, an `ASK_REQUEST (subagent:x)` is NOT consumed by the relay (no `ASK_RESPONSE` emitted, no `ctx.ui` call); with `active === false`, it is consumed (the existing behavior).

`packages/subagent/src/spawn.test.ts` (TDD — create):
- `ASK_CANCEL` wiring: start the socket server, open a client socket, send an `ask_request`, then close the client socket → the bus receives an `ASK_CANCEL` with the matching `requestId` + `source`.
- `toolCallId` propagation: a client `ask_request` carrying `toolCallId` → the bus `ASK_REQUEST` carries the same `toolCallId`.

**Steps:**
- [ ] Write the failing tests in `packages/ask/src/tool.test.ts` + `packages/subagent/src/spawn.test.ts`.
- [ ] Run `pnpm --filter @pi-archimedes/ask test` and `pnpm --filter @pi-archimedes/subagent test` — confirm the new tests fail.
- [ ] Implement the `tool.ts` changes (bridge branch + `responseToResults` helper + `toolCallId` on both the `ask_request` socket payload and the `ASK_REQUEST` emits).
- [ ] Implement the `ipc-relay.ts` `!getBridge().active` per-message gate.
- [ ] Implement the `spawn.ts` `ASK_CANCEL` wiring + `toolCallId` propagation.
- [ ] Run both package test suites — all pass.
- [ ] Run `pnpm test` (full) — no regressions.
- [ ] Typecheck both packages.
- [ ] Commit: `feat(ask,subagent): bridge ask branch + ipc-relay gate + ASK_CANCEL wiring`.

**Acceptance criteria:**
- [ ] In bridge mode, a root ask is answered by `bridge.ask()` (the Client), and the tool emits exactly one `ASK_REQUEST (main)` + one `ASK_RESPONSE (main)` (paired).
- [ ] In bridge mode, a subagent ask is forwarded by the bridge (NOT the relay) and answered via the child socket; the relay is inert.
- [ ] In TUI and RPC-no-env, the relay behaves exactly as today (no regression).
- [ ] A child exit (socket close) emits `ASK_CANCEL` for any pending child ask; the bridge cancels the relayed Client request and emits the single pairing `ASK_RESPONSE` (refcount returns to 0).
- [ ] The `ask_request` socket payload and the bus `ASK_REQUEST` payload carry `toolCallId` end-to-end (child → `spawn.ts` → bridge → Client frame).

---

### Task 3: sudo `prompt.ts` bridge routing

**Context:** `sudo_exec` is broken in RPC/bridge mode for the same reason as `ask` (no TUI surface). This task routes `confirmCommand` and `promptForPassword` through the bridge in bridge mode (the single choke point — `prompt.ts`), with the 0010 headless gate **unchanged** (it sits after the bridge branch). The tool's call sites are untouched. It also fixes the missing `packages/sudo` entry in the root `vitest.config.ts` so the sudo tests actually run.

**Files:**
- Modify: `packages/sudo/src/prompt.ts`
- Modify: `packages/sudo/src/tool.ts` (the tool's OWN headless gate — see below)
- Modify: `packages/sudo/src/prompt.test.ts`
- Modify: `packages/sudo/src/tool.test.ts` (create if absent)
- Modify: `vitest.config.ts` (root — add `packages/sudo` to the `projects` list)

**What to implement:**

`packages/sudo/src/tool.ts` (the tool's OWN headless gate — **`prompt.ts` alone is not enough**):
- `sudo_exec`'s `execute` has its own `if (ctx.mode !== "tui") { return fail("sudo_exec requires an interactive session…", …); }` gate (line 413) that fails the tool in bridge mode **before** `confirmCommand`/`promptForPassword` are reached. Add `import { getBridge } from "@pi-archimedes/core/bridge";` and change the gate from `if (ctx.mode !== "tui") {` to `if (ctx.mode !== "tui" && !getBridge().active) {`. In bridge mode the gate is bypassed (the bridge `confirm`/`password` are user-paced and 0010-compliant); in TUI / RPC-no-env it behaves exactly as today.

`packages/sudo/src/prompt.ts`:
- `import { getBridge } from "@pi-archimedes/core/bridge";`
- `promptForPassword`: **prepend** `if (getBridge().active) return getBridge().password({ command, reason });` as the first statement, BEFORE the existing `if (ctx.mode !== "tui")` headless gate. The headless gate and the `ctx.ui.custom` masked field are otherwise **unchanged** (0010's guarantee preserved verbatim). The bridge `password` method returns the password string or `""` (cancel).
- `confirmCommand`: **prepend** `if (getBridge().active) return getBridge().confirm({ command, reason });` as the first statement, BEFORE the existing `ctx.ui.confirm` call. The `ctx.ui.confirm` path is otherwise unchanged. The bridge `confirm` method returns `true`/`false`.
- No call-site ternaries: `packages/sudo/src/tool.ts` still calls `confirmCommand`/`promptForPassword` unconditionally (lines 426/440) — `prompt.ts` is the single place where the bridge routing and the headless gate both live.

`packages/sudo/src/prompt.test.ts` (extend the existing file):
- `promptForPassword` bridge mode: with a mocked `getBridge()` (`active: true`, `password` resolves `"s3cret"`), `promptForPassword` resolves `"s3cret"` (does NOT reject, does NOT call `ctx.ui.custom`), for a `ctx` with `mode: "rpc"`.
- `promptForPassword` bridge cancel: `password` resolves `""` → `promptForPassword` resolves `""` (the caller treats `""` as cancellation — unchanged).
- `promptForPassword` headless unchanged: `getBridge().active === false` + `mode: "json"` → still rejects (the existing tests must still pass).
- `confirmCommand` bridge mode: `getBridge().active === true`, `confirm` resolves `true` → `confirmCommand` resolves `true` (does NOT call `ctx.ui.confirm`).
- `confirmCommand` bridge decline: `confirm` resolves `false` → `confirmCommand` resolves `false`.
- `confirmCommand` TUI unchanged: `getBridge().active === false` → still uses `ctx.ui.confirm` (existing behavior).

`packages/sudo/src/tool.test.ts` (TDD — create if absent):
- `sudo_exec` bridge mode: `getBridge().active === true` + `mode: "rpc"` → the tool's headless gate is bypassed (the command proceeds to `confirmCommand`/`promptForPassword`, which route to the bridge).
- `sudo_exec` headless unchanged: `getBridge().active === false` + `mode: "json"` → the tool's headless gate still rejects (the existing behavior).

`vitest.config.ts` (root):
- Add `"packages/sudo"` to the `test.projects` array (it is missing — the sudo tests exist but don't run under `pnpm test`).

**Steps:**
- [ ] Write the failing bridge-mode tests in `packages/sudo/src/prompt.test.ts`.
- [ ] Run `pnpm test` (root) — confirm the new sudo tests fail (and that `packages/sudo` is now in the projects list).
- [ ] Implement the `prompt.ts` bridge branches (prepend the `getBridge().active` routing in both functions) AND the `tool.ts` headless-gate change (`&& !getBridge().active`).
- [ ] Run `pnpm test` (root) — all sudo tests pass (bridge + the pre-existing headless/TUI tests).
- [ ] Run `pnpm --filter @pi-archimedes/sudo test` — confirm.
- [ ] Typecheck `packages/sudo`.
- [ ] Commit: `feat(sudo): bridge-mode confirm + password routing (headless gate unchanged)`.

**Acceptance criteria:**
- [ ] In bridge mode, `confirmCommand` and `promptForPassword` route through the bridge (the Client's modals); the headless gate is not reached.
- [ ] In non-bridge headless modes (json/print/RPC-no-env), `promptForPassword` still rejects and `confirmCommand` still uses `ctx.ui.confirm` (0010 unchanged).
- [ ] The password flows only Client UI → bridge channel → `sudo -S` stdin (never a tool param, never the LLM context / ACP wire).
- [ ] `packages/sudo` is in the root `vitest.config.ts` projects list and its tests run under `pnpm test`.

---

### Task 4: desktop Rust — registry flag + spawn env + bridge listener

**Context:** The desktop (the Client) is the bridge server. This task adds the registry `bridge` flag, the spawn-env wiring (the 4 bridge env vars), the `acp/bridge.rs` listener (peer-verified, per-spawn socket, `pending_bridge` map + 330 s waiter, `bridge-request`/`bridge-event` events, `respond_bridge_request` command), and the Cargo deps. It mirrors `permission.rs`'s user-paced-prompt pattern. The listener is started before the spawn call returns (the push retry window is only ~2 s).

**Files:**
- Modify: `src-tauri/src/config/registry.rs`
- Modify: `src-tauri/src/acp/session.rs`
- Create: `src-tauri/src/acp/bridge.rs`
- Modify: `src-tauri/src/acp/mod.rs`
- Modify: `src-tauri/src/commands/sessions.rs`
- Modify: `src-tauri/src/lib.rs`
- Modify: `src-tauri/Cargo.toml`

**What to implement:**

`src-tauri/src/config/registry.rs`:
- Add `#[serde(default)] pub bridge: bool` to `AgentEntry`.
- In `default_registry()`, set `bridge: true` on the `pi` entry (keep `env: BTreeMap::new()`).

`src-tauri/src/acp/session.rs`:
- Add a `pending_bridge: Arc<Mutex<HashMap<String, oneshot::Sender<serde_json::Value>>>>` field to `SessionManager` (the oneshot carries the **`result` `Value` verbatim** — no `BridgeResponse` wrapper; see `bridge.rs`); initialize it in `new()`.
- Add `use crate::acp::bridge;`.
- In `start_session` and `resume_session`, **before** building the `AcpAgent`: if `entry.bridge`, build a `BTreeMap` of the 4 bridge env vars and pass it via `.envs(…)`:
  - `PI_ARCHIMEDES_BRIDGE` = `"1"`.
  - `PI_ARCHIMEDES_BRIDGE_SESSION` = the desktop's own session id — for `start_session`, generate a new session id (the ACP `session_id` is agent-generated and doesn't exist yet, so use a client-side id: `uuid::Uuid::new_v4().to_string()`); for `resume_session`, use the stored `session_id`.
  - `PI_ARCHIMEDES_BRIDGE_SERVER_PID` = `std::process::id().to_string()`.
  - `PI_ARCHIMEDES_BRIDGE_SOCKET` = a per-spawn randomized **name** under a 0700 dir: on Unix, the full path `std::env::temp_dir().join("archimedes-bridge-<uid>").join(format!("bridge-{uuid}.sock"))` (create the 0700 dir if absent — **include the uid in the dir name** so a multi-user temp dir can't be pre-squatted by another user); on Windows, the **bare name** `bridge-{uuid}` (the listener prefixes `\\.\pipe\`). (The env value is the bare name on Windows, the full path on Unix — the agent's `socketTarget()` adds the `\\.\pipe\` prefix on Windows.)
  - Merge these into a clone of `entry.env` (so user-configured env is preserved) and pass the merged map to `.envs(…)`.
- **Start the bridge listener** for bridge agents: call `bridge::start_listener(placeholder_session_id, socket_path, std::process::id(), sink, pending_bridge, &close_tx, bridge::DEFAULT_BRIDGE_TIMEOUT)` (see below — the **anchor is the desktop's own pid**, `std::process::id()`, NOT the agent's pid) and store the returned handle so the driver-task cleanup can tear it down. The listener is started **before** the `connect_with` spawn returns (the push retry window is only ~2 s). On **macOS**, skip the bridge listener + the bridge env entirely (the fail-closed macOS policy — see `bridge.rs`).
- **Session-id identity (the frontend keys by the ACP `session_id`):** the listener is started pre-spawn with a **placeholder** id (the client-side UUID for `start_session`, the stored id for `resume_session`). When the ACP `session_id` becomes known (the driver task's `session_id_tx` delivers it), call `handle.set_session_id(&acp_id)`. After that, all `bridge-request`/`bridge-event` payloads carry the ACP id (bridge requests only occur mid-turn, after establish, so they always carry the ACP id). Pre-establish pushes (`session`/early `state`) carry the placeholder — they are wired-not-rendered in v1, so the mismatch is harmless.
- In the driver-task cleanup (where `pending_permissions` is drained by the `"{session_id}/"` prefix), **call `bridge::teardown(handle)` unconditionally** (close the listener + unlink the socket — do NOT nest it inside the `session_id_tx` guard, or a failed `connect_with`/`session/new` would leak the listener). **Drain `pending_bridge`** by the `"{session_id}/"` prefix inside the session-id guard (the `session_id` is only known there). The spawned child's exit tears the listener down (the `close_tx` / driver-task close path covers it).

`src-tauri/src/acp/bridge.rs` (the listener — mirrors `permission.rs`):
- **No `BridgeResponse` wrapper** — the oneshot + the `respond_bridge_request` command take the response **`result` `Value` verbatim** (the `result` of the response frame; for `password`, `{password}`; for `confirm`, `{confirmed}`; for `ask`, the `AskResponsePayload`). The frontend sends `result` directly; the desktop writes `{v:1, type:"response", id, result}` (the `result` verbatim — no double-nesting).
- `pub type PendingBridge = Arc<Mutex<HashMap<String, oneshot::Sender<serde_json::Value>>>>;`
- `pub fn bridge_key(session_id: &str, request_id: &str) -> String { format!("{session_id}/{request_id}") }` (reuse the `permission` key convention).
- `pub const DEFAULT_BRIDGE_TIMEOUT: Duration = Duration::from_secs(330);` (agent 5-min timeout + 30 s margin — the agent's cancel deterministically wins). **The timeout is a `Duration` parameter to `start_listener`/`handle_connection`** (defaulting to `DEFAULT_BRIDGE_TIMEOUT`) so Task 6's shrunk-timeout test can inject a short value.
- `pub struct BridgeHandle { listener: Listener, socket_path: PathBuf, session_id: Arc<Mutex<String>> }` where `Listener` is a `#[cfg]`-gated enum (`UnixListener` on Unix, `NamedPipeServer` on Windows — `tokio::net`'s `named_pipe::ServerOptions::first_pipe_instance` covers the listener + first-instance logic). **No `child` field** (the anchor is the desktop's own pid, always alive — see below). `pub fn teardown(h: Option<BridgeHandle>)` closes the listener + unlinks the socket (Unix; Windows pipes vanish when the last handle closes). `pub fn set_session_id(&self, id: &str)` updates `session_id` (called by the driver task when the ACP id is known).
- `pub async fn start_listener(placeholder_session_id: String, socket_path: &Path, anchor_pid: u32, sink: Arc<dyn EventSink>, pending_bridge: PendingBridge, close_tx: &watch::Sender<bool>, timeout: Duration) -> BridgeHandle` (the `close_tx` is the driver task's `watch::Sender<bool>` — the real field type in `session.rs`; the listener `subscribe()`s it for its waiters):
  - **Platform policy (macOS is a bundle target — the `#[cfg(unix)]` + `SO_PEERCRED` combo would break the macOS build, since macOS has no `SO_PEERCRED`/`ucred`):** the cred-based peer verification is gated `#[cfg(target_os = "linux")]`. On **macOS**, the bridge listener is **not started** (fail-closed — `start_listener` returns a no-op handle and `session.rs` skips the bridge env for the `pi` agent on macOS, documented). On **Windows**, the named-pipe `GetNamedPipeClientProcessId` path. (This is a documented v1 platform limitation — the bridge is unavailable on macOS.)
  - Bind a Unix listener (or a Windows named pipe — the **first** instance with `FILE_FLAG_FIRST_PIPE_INSTANCE` for pre-squat detection; subsequent instances without it). On bind failure (name taken by a squatter), use a fresh randomized path.
  - Spawn an accept loop (`tokio::spawn`): for each incoming connection, run `handle_connection` (below).
  - Return the `BridgeHandle` (the `anchor_pid` is the **desktop's own pid** — `std::process::id()` — passed in from `session.rs`; it is always alive, so no liveness check is needed).
- `async fn handle_connection(socket, session_id: &Arc<Mutex<String>>, anchor_pid: u32, sink, pending_bridge, timeout: Duration)`:
  - **Peer verification (fail-closed, `#[cfg(target_os = "linux")]`):** read the peer's pid — `SO_PEERCRED` via `libc::getsockopt` on the connected fd (returns `ucred {pid, uid, gid}`). Then walk the parent chain (Linux: `/proc/<pid>/status` `PPid`, bounded ≤8 hops) up to `anchor_pid` (the **desktop's own pid**). If the chain doesn't reach `anchor_pid`, **reject the connection** (close it). (The match trusts the desktop's entire process tree — the desktop → `pi-acp` (direct child) → `pi` (grandchild); the peer is the grandchild. The trust boundary is the desktop's process tree, slightly wider than the agent's, but the desktop only spawns the agent, so the practical widening is nil — ADR 0003 notes this. The guard is against non-agent same-user processes.)
  - Read frames (line-delimited JSON). For a **request** frame (`{id, method, source, toolCallId, params}`):
    - Build the `bridge-request` payload: `json!({ "sessionId": session_id, "requestId": id, "method": method, "source": source, "toolCallId": toolCallId, "params": params })` (the `session_id` is the **current** value of the `session_id` `Mutex` — the ACP id after `set_session_id`, the placeholder before); `sink.emit("bridge-request", payload)`.
    - Register a oneshot in `pending_bridge` keyed `bridge_key(session_id, id)`.
    - Spawn a waiter (the `timeout` cap): await the oneshot OR a timeout OR the connection closing. On a user answer, write the response frame `{v:1, type:"response", id, result}` to the socket (the `result` verbatim). On timeout/drain/auth-failure, write an **explicit terminal frame** `{v:1, type:"response", id, error:"cancelled"}` then close (connection close = immediate cancel). Remove the `pending_bridge` entry.
  - For a **push** frame (`{seq, event, payload}`): apply the `seq` drop (keep `last_seq` per listener; drop `seq <= last_seq`), `sink.emit("bridge-event", json!({ "sessionId": session_id, "seq": seq, "event": event, "payload": payload }))`, then write an ack line (`"ack\n"`) and close the connection (the agent destroys on first data).
  - **Agent→Client verification (Windows only, v1 limitation):** the agent's `GetNamedPipeServerProcessId` check is a **documented v1 limitation** (the agent-side check would need a native addon in Node — see Task 1); in v1 the agent presence-checks `PI_ARCHIMEDES_BRIDGE_SERVER_PID` only. The desktop-side descendant check (above) is the real defense.
- **Note on the anchor (B4 fix):** the ACP SDK spawns the agent internally, so the desktop does not hold a `std::process::Child` for it, and the SDK does not expose the child pid. The **anchor is therefore the desktop's own pid** (`std::process::id()`), which is always alive. The topology is desktop (D) → `pi-acp` (A, a direct child of D, spawned in-process by the SDK) → `pi` (P, a grandchild, spawned by `pi-acp`). The peer connecting to the desktop's socket is `pi` (P). The descendant walk reads P's pid, walks the parent chain (P → A → D), and accepts iff it reaches D. This is simpler than anchoring at the agent pid and doesn't need the SDK's child pid. (The trust boundary is the desktop's process tree — ADR 0003 notes the theoretical widening to the desktop's other children, which is nil in practice since the desktop only spawns the agent.)

`src-tauri/src/acp/mod.rs`:
- `pub mod bridge;` and re-export `pub use bridge::{PendingBridge};` (and `bridge_key` if used by `session.rs`). **No `BridgeResponse` re-export** — the wrapper does not exist (I7: the oneshot + command take the `result` `Value` verbatim).

`src-tauri/src/commands/sessions.rs`:
- Add `#[tauri::command] pub async fn respond_bridge_request(state: State<'_, Arc<SessionManager>>, session_id: String, request_id: String, result: serde_json::Value) -> Result<(), AcpError> { state.respond_bridge_request(&session_id, &request_id, result).await }` (the param is `result: serde_json::Value` — the `result` of the response frame verbatim; **no `BridgeResponse` wrapper**). The frontend invoke key must be `result` (Tauri's camelCase↔snake_case handles `sessionId`/`requestId` but will not alias `response`↔`result`) — see Task 5.
- Add `SessionManager::respond_bridge_request` (mirror `respond_permission`): look up `pending_bridge` by `bridge_key`, `sender.send(result)`, no-op if gone.

`src-tauri/src/lib.rs`:
- Register `commands::sessions::respond_bridge_request` in the `invoke_handler!` list.

`src-tauri/Cargo.toml` (**target-gate the platform deps** — `libc`/`windows` are only needed on their respective platforms, so a top-level dep would pull `windows` into the Linux build and `libc` into the Windows build):
- `tokio` features: add `"net"` and `"rt"` (→ `["process","io-util","sync","macros","time","net","rt"]`) (top-level — `net` covers both the Unix listener and the Windows named pipe).
- `[target.'cfg(target_os = "linux")'.dependencies]`: `libc = "0.2"` (Linux `SO_PEERCRED` / `ucred`).
- `[target.'cfg(windows)'.dependencies]`: `windows = { version = "0.58", features = ["Win32_Foundation", "Win32_System_Pipes", "Win32_System_Threading", "Win32_System_Diagnostics_ToolHelp"] }` (Windows pipe pid + Toolhelp).
- Gate the `windows`-specific code behind `#[cfg(windows)]`, the `libc`/`SO_PEERCRED` code behind `#[cfg(target_os = "linux")]`, and the macOS policy (listener not started) behind `#[cfg(target_os = "macos")]`.

`src-tauri/src/acp/bridge.rs` tests (`#[cfg(test)] mod tests`):
- `bridge_key` carries the trailing-slash prefix (mirror the `permission` test).
- `seq` drop: a `handle_connection` fed two push frames (`seq: 1`, then `seq: 1` again) → the second is dropped (assert via a mock `EventSink` that captures the `bridge-event` emissions — only one `bridge-event` for the duplicate).
- **`is_descendant` unit test (the real peer-verification test — a plain spawned stub is NOT a valid "foreign process" under the B4 fix, since any test-spawned process is a descendant of the test process → accepted):** unit-test the `is_descendant(peer_pid, anchor_pid)` helper directly with a **fabricated `/proc` layout via a `#[cfg(test)]` seam** (inject the `/proc` reader so the test can return a fabricated parent chain). Cases: (a) the chain reaches the anchor → accepted; (b) the chain does NOT reach the anchor (a fabricated orphan whose parent chain is a double-forked process reparented to init) → rejected; (c) a cycle in the parent chain → rejected (bounded ≤8 hops, no infinite loop). To make the "foreign" case real, **double-fork** a child (spawn a process that spawns a grandchild then exits, so the grandchild is reparented to init — its parent chain never reaches the test process) and use its pid as the `peer_pid`.
- `respond_bridge_request` round-trip: register a `pending_bridge` entry, call `respond_bridge_request`, assert the oneshot resolves with the `result` `Value` (no `BridgeResponse` wrapper).

**Steps:**
- [ ] Add the Cargo deps (`tokio` `net`+`rt` top-level; `libc` under `[target.'cfg(target_os = "linux")'.dependencies]`; `windows` under `[target.'cfg(windows)'.dependencies]`); run `cargo check` (deps resolve).
- [ ] Add `AgentEntry.bridge` + the default `bridge: true`.
- [ ] Implement `acp/bridge.rs` (the `PendingBridge`/`bridge_key`/`BridgeHandle` types first — no `BridgeResponse` wrapper — then the listener + `handle_connection` + the `#[cfg(target_os = "linux")]` peer verification + the `is_descendant` helper with the `#[cfg(test)]` `/proc` seam + the macOS no-op policy + the waiter), with the `#[cfg(test)]` tests.
- [ ] Wire `session.rs` (the `pending_bridge` field, the spawn-env merge, the listener start with the `std::process::id()` anchor, the `set_session_id` call, the unconditional `teardown` + the `pending_bridge` drain).
- [ ] Add `respond_bridge_request` (command + `SessionManager` method — takes the `result` `Value` verbatim) + register it in `lib.rs`.
- [ ] Run `cargo test` — all Rust tests pass (including the new `bridge` tests).
- [ ] Run `cargo clippy` + `cargo fmt` — clean.
- [ ] Commit: `feat(desktop): bridge listener (peer-verified, anchored at the desktop pid) + registry flag + spawn env`.

**Acceptance criteria:**
- [ ] A bridge agent is spawned with all 4 env vars set (the socket name is per-spawn + randomized; `SERVER_PID` = the desktop's pid; `SESSION` = the client-side session id; the temp dir includes the uid).
- [ ] A connection from a descendant of the desktop (the `pi` grandchild, via the `pi-acp` direct child) is accepted; a connection from a foreign same-user process (a double-forked orphan) is rejected (fail-closed).
- [ ] A request frame → `bridge-request` event (carrying the ACP `session_id` after `set_session_id`) + `pending_bridge` entry + 330 s waiter; `respond_bridge_request` writes the response frame (the `result` verbatim); timeout/drain writes the terminal `error:"cancelled"` frame then closes.
- [ ] Push frames are `seq`-deduped (drop `seq <= last_seq`) and acked; the `session` push is delivered.
- [ ] Session close (and a failed `connect_with`/`session/new`) tears the listener down unconditionally (unlink on Unix) and drains `pending_bridge`.
- [ ] On macOS, the bridge listener is not started (fail-closed, documented).
- [ ] `cargo test` + `cargo clippy` + `cargo fmt` are clean.

---

### Task 5: desktop UI — bridge store + the three surfaces

**Context:** The frontend renders the delegated UI. This task adds the bridge store (`store/bridge.ts`), the `lib/tauri.ts` wrappers, the three surfaces (`AskQuestionCard` inline, `SudoConfirmModal` + `SudoPasswordModal` modals, `TodoBoardPanel` right rail), and the `App.tsx` event wiring + `ChatStream` mounting. It mirrors the `permissions` store / `PermissionPrompt` component patterns.

**Files:**
- Create: `src/store/bridge.ts`
- Modify: `src/lib/tauri.ts`
- Create: `src/components/AskQuestionCard.tsx`
- Create: `src/components/SudoConfirmModal.tsx`
- Create: `src/components/SudoPasswordModal.tsx`
- Create: `src/components/TodoBoardPanel.tsx`
- Modify: `src/App.tsx`
- Modify: `src/components/ChatStream.tsx`
- Modify: `src/store/sessions.ts` (the `rawInput` fallback needs it — see below)
- Create: `src/store/bridge.test.ts`
- Create: `src/components/AskQuestionCard.test.tsx`

**What to implement:**

`src/lib/tauri.ts`:
- Add types: `BridgeRequestPayload { sessionId, requestId, method: "ask"|"confirm"|"password", source, toolCallId?: string, params }`; `BridgeEventPayload { sessionId, seq, event, payload }`; `BridgeResponseDto` (the `result` to send back — for `ask`, the `AskResponsePayload`; for `confirm`, `{confirmed}`; for `password`, `{password}`).
- `export async function respondBridgeRequest(sessionId, requestId, result): Promise<void>` → `invoke("respond_bridge_request", { sessionId, requestId, result })` (the invoke key is **`result`**, matching the Rust command's `result: serde_json::Value` param — Tauri's camelCase↔snake_case handles `sessionId`/`requestId` but will not alias `response`↔`result`).
- `export function listenBridgeRequest(cb): Promise<UnlistenFn>` → `listen<BridgeRequestPayload>("bridge-request", …)`.
- `export function listenBridgeEvent(cb): Promise<UnlistenFn>` → `listen<BridgeEventPayload>("bridge-event", …)`.

`src/store/bridge.ts` (zustand — mirror `permissions.ts`):
- **Session-id identity (B3):** the `sessionId` in every `bridge-request`/`bridge-event` payload is the **ACP `session_id`** (the desktop calls `handle.set_session_id(&acp_id)` once the ACP id is known; bridge requests only occur mid-turn, after establish, so they always carry the ACP id). The store keys by this ACP id, which is the same id the frontend's `activeSessionId` holds (from `startSession`'s return) — so the keys line up. (Pre-establish pushes carry the client-side placeholder UUID, but they are wired-not-rendered in v1.)
- State: `requests: Record<string, BridgeRequestData[]>` (keyed by the ACP session id; `BridgeRequestData { requestId, method, source, toolCallId, params, answered? }`); `todos: Record<string, TodoColumn>` (keyed by the ACP session id; `TodoColumn { main: TodoItem[], subagents: Record<string, TodoItem[]> }`); `agentState: Record<string, "working"|"idle"|"blocked">` (wired, not rendered in v1); `cost: Record<string, unknown>`; `session: Record<string, unknown>`.
- Actions: `addRequest(sessionId, data)` (dedup by `requestId`); `removeRequest(sessionId, requestId)`; `markAnswered(sessionId, requestId, response)` (**removes the `password` from the stored `params` immediately** — the password must not persist in store state after the reply is sent); `applyTodoUpdate(sessionId, {source, todos})` (if `source === "main"` set `main`, else `subagents[source]`); `applyTodoClear(sessionId, {source})` (delete the column); `applyState`/`applyCost`/`applySession` (store, don't render in v1); `dismissSession(sessionId)` (clear all for the session — called on `session-closed`).

`src/components/AskQuestionCard.tsx` (inline — mirror `PermissionPrompt.tsx`):
- Props `{ sessionId, requestId }`. Read the request from the store; if absent, `return null`.
- Render the ask UI ported from the TUI (accent separator, circular radio list — filled amber dot = selected; `Other (type your own)` free-text; checkboxes for `multi`; `(Recommended)` suffix on `recommended`; a per-option note field; footer hints; a final batch review for multi-question). **Correlation is keyed by `(source, toolCallId)` or the frame UUID `requestId`, never by bare `toolCallId`** (the store keys by `requestId`, which is the frame UUID — the `toolCallId` is carried for anchoring to the ACP `tool_call` frame when present). **Subagent asks** (`source` starts with `subagent:`) render a labeled card anchored by `source` (the desktop has no ACP `tool_call` frame for child tool calls). **If the request arrives before its ACP `tool_call` frame, the card renders queued (bounded ~1–2 s) then unanchored/labeled** (a `useEffect` timeout that flips a `queued` flag).
- Submit: build the `AskResponsePayload` (`{ cancelled: false, results: [{id, selectedOptions, customInput?}] }`), call `respondBridgeRequest(sessionId, requestId, payload)`, `markAnswered` + `removeRequest`. Cancel/timeout → `{ cancelled: true, results: [] }` + `removeRequest`. Card collapses on settle.
- **Stacking for concurrent asks:** the `ChatStream` renders one `AskQuestionCard` per pending request (they stack vertically in the stream, in arrival order).

`src/components/SudoConfirmModal.tsx` (modal):
- Props `{ sessionId, requestId }`. Read the request (`method === "confirm"`); render a modal (overlay) with `params.command` + `params.reason` + Run/Cancel. Run → `respondBridgeRequest(…, { confirmed: true })`; Cancel → `{ confirmed: false }`; then `markAnswered` + `removeRequest`.

`src/components/SudoPasswordModal.tsx` (modal):
- Props `{ sessionId, requestId }`. Read the request (`method === "password"`); render a modal with `params.command` + `params.reason` **verbatim** + a masked `•` text field (Enter confirm / Esc cancel / Backspace delete — port the TUI `maskLine` behavior: one `•` per char, the raw value never rendered). Confirm → `respondBridgeRequest(…, { password })`; Esc/empty → `{ password: "" }` (cancel); then `markAnswered` (**removes the password from the store**) + `removeRequest`.

`src/components/TodoBoardPanel.tsx` (collapsible right rail):
- Read `todos[sessionId]`. Render `Todo List — n/m completed` header; numbered ✓/◉/○ items (completed = ✓, in_progress = ◉, pending = ○) for the `main` column; **subagent columns** (`subagents[source]`) as labeled right-side columns (fed by the `TODOS_UPDATE`/`TODOS_CLEAR` bridge events — the existing `stream.ts` parent-bus relay). Auto-collapse when empty. **`rawInput` fallback for non-bridge agents:** if there are no bridge todos but the latest `manage_todo_list` `rawInput` (from the ACP `tool_call` frame, via the `sessions` store) exists, seed the board from it.

`src/App.tsx`:
- In the existing `useEffect` that registers event listeners, add: `listenBridgeRequest((p) => useBridge.getState().addRequest(p.sessionId, p))` and `listenBridgeEvent((p) => { const s = useBridge.getState(); if (p.event === "todos_update") s.applyTodoUpdate(p.sessionId, p.payload); else if (p.event === "todos_clear") s.applyTodoClear(p.sessionId, p.payload); else if (p.event === "state") s.applyState(p.sessionId, p.payload); else if (p.event === "cost_update") s.applyCost(p.sessionId, p.payload); else if (p.event === "session") s.applySession(p.sessionId, p.payload); })` (note the wire name is `cost_update` — the bus `COST_UPDATE` maps to `cost_update` by the `archimedes:`-prefix-strip lookup, NOT `cost`). Also wire `session-closed` → `useBridge.getState().dismissSession(payload.sessionId)` (alongside the existing `handleSessionClosed`).

`src/store/sessions.ts` (the `rawInput` fallback needs it):
- The reducer currently **drops `rawInput`** from tool-call messages. Keep `rawInput` on `tool-call` messages (so the `TodoBoardPanel`'s `rawInput` fallback can read the latest `manage_todo_list` `rawInput`). The fallback filters by `title === "manage_todo_list"` (the `name` field is UNSTABLE in ACP 1.7; `title` is the reliable discriminator).

`src/components/ChatStream.tsx`:
- **Restructure both return paths (M3):** the `TodoBoardPanel` right rail requires wrapping **both** of `ChatStream`'s return paths (the loading/empty state and the populated stream) in a flex row: `<div className="flex"> <main>…</main> <TodoBoardPanel …/> </div>`. A single-path change would leave the rail missing in one state.
- After the existing `prompts.map(…PermissionPrompt…)` block, render the bridge surfaces: the pending `ask` requests as `AskQuestionCard`s (one per request, stacked), and the pending `confirm`/`password` requests as `SudoConfirmModal`/`SudoPasswordModal` (modals — rendered at the `ChatStream` root, not inside the scroll region).
- **Resolve the request to the active session (B3):** read the pending requests for the **active ACP session id** (`requests[activeSessionId]`) — the store is keyed by the ACP id, which matches `activeSessionId`.
- **The `AskQuestionCard` replaces the pending `ask` `ToolCallCard`:** when an `ask` request is pending for a `toolCallId` that matches a `tool-call` message in the stream, render the `AskQuestionCard` in place of (or above) that `ToolCallCard` (correlated via `(source, toolCallId)`/`requestId`).

`src/store/bridge.test.ts` (TDD):
- `addRequest`/`removeRequest`/`markAnswered` (assert the `password` is removed from `params` after `markAnswered`).
- `applyTodoUpdate` (main vs subagent routing) / `applyTodoClear` / `dismissSession`.
- `applyState`/`applyCost`/`applySession` store the values (wired, not rendered).

`src/components/AskQuestionCard.test.tsx` (TDD, @testing-library/react + a mocked `respondBridgeRequest`):
- Render a single-question request → select an option → submit → `respondBridgeRequest` called with the right `AskResponsePayload`; the card collapses (`removeRequest`).
- Render a `multi` request → select multiple → submit.
- Render a subagent request (`source: "subagent:x"`) → the card is labeled by `source`.
- Cancel → `respondBridgeRequest` called with `{ cancelled: true }`.

**Steps:**
- [ ] Write the failing tests (`store/bridge.test.ts`, `AskQuestionCard.test.tsx`).
- [ ] Run `pnpm test` (frontend) — confirm the new tests fail.
- [ ] Implement `lib/tauri.ts` (types + `respondBridgeRequest` + `listenBridgeRequest` + `listenBridgeEvent`).
- [ ] Implement `store/bridge.ts`.
- [ ] Implement the four components (`AskQuestionCard`, `SudoConfirmModal`, `SudoPasswordModal`, `TodoBoardPanel`).
- [ ] Wire `App.tsx` (the bridge event listeners) + `ChatStream.tsx` (mount the surfaces + the `AskQuestionCard`-replaces-`ToolCallCard` correlation).
- [ ] Run `pnpm test` (frontend) — all pass.
- [ ] Run `pnpm build` (`tsc && vite build`) — types + build clean.
- [ ] Commit: `feat(desktop-ui): bridge store + AskQuestionCard + sudo modals + TodoBoardPanel`.

**Acceptance criteria:**
- [ ] A bridge `ask` request renders an `AskQuestionCard`; the user's answer flows back via `respondBridgeRequest` and the card collapses.
- [ ] A bridge `confirm`/`password` request renders the matching modal; the password is removed from store state immediately after `respondBridgeRequest` (never persisted).
- [ ] `todos_update`/`todos_clear` bridge events update the `TodoBoardPanel` (main + subagent columns); `rawInput` seeds the board for non-bridge agents.
- [ ] `state`/`cost`/`session` are stored (wired) but not rendered in v1.
- [ ] `pnpm test` + `pnpm build` are clean.

---

### Task 6: integration test + rollout validation

**Context:** The unit tests cover the pieces; this task adds one end-to-end integration test (a Node stub speaking the bridge protocol against the desktop listener, **including the peer-verification rejection path**) and documents the manual E2E checklist + the non-breaking release. It is the final validation gate before the feature is considered done.

**Files:**
- Create: `src-tauri/tests/bridge_integration.rs` (a Rust integration test that drives the real `acp::bridge` listener with a Node/Python stub agent, OR a Node test under `scripts/` if a Rust harness is impractical — prefer the Rust one for the peer-verification path).
- Create: `docs/roadmap/bridge-e2e-checklist.md` (the manual E2E checklist).

**What to implement:**

`src-tauri/tests/bridge_integration.rs`:
- Bring up the real `acp::bridge::start_listener` with a temp socket path + a mock `EventSink` (a channel) + a `pending_bridge` map.
- **Happy path:** spawn a stub agent process (a small Node/Python script that `net.connect`s to the socket, sends a `request` frame (`method: "ask"`), reads the `response` frame, and prints it). Assert the `EventSink` received a `bridge-request` with the right `method`/`params`; call `respond_bridge_request` with a canned `AskResponsePayload`; assert the stub received the matching `response` frame.
- **Peer-verification rejection (I8):** spawn a **foreign** process — a **double-forked orphan** (a process that spawns a grandchild then exits, so the grandchild is reparented to init and its parent chain never reaches the test process, which is the anchor = the desktop's own pid) — that connects to the socket → assert the connection is rejected (closed, no `bridge-request` emitted). (A plain spawned stub is NOT a valid "foreign process" here — it's a descendant of the test process → accepted. The double-fork makes it a true foreign process. This is the security-critical path.)
- **Push `seq` drop:** the stub sends two `push` frames with the same `seq` → assert only one `bridge-event` is emitted.
- **Terminal frame on timeout (M4):** the stub sends a `request` frame and the test does NOT call `respond_bridge_request` within the **injectable** timeout (the `Duration` parameter from Task 4, shrunk to e.g. 50 ms for the test) → assert the stub received the `error:"cancelled"` terminal frame.
- Clean up the temp socket after each test.

`docs/roadmap/bridge-e2e-checklist.md`:
- A manual E2E checklist (to run in `tauri dev` with a real `pi` agent): ask (multi / Other / cancel / timeout / **concurrent asks** — two `ask` tools in one turn), sudo (decline / cancel / warm cache / cold cache), todo (main + subagent columns / auto-clear / child exit), **session close with a pending prompt** (the prompt is drained as cancelled), **crash/restart mid-request** (the stale socket is tolerated/cleaned at startup), **two simultaneous sessions** (frame isolation — each session's `seq`/`lastSeq` is independent), and the **downgrade combos** (old desktop + new suite = safe — the old desktop never sets the env vars, so the suite stays inert; new desktop + old suite = the env vars are ignored, today's behavior).

**Rollout (process, not a file):**
1. pi-archimedes: the bridge module + tool changes land as a **non-breaking release** (inert without the env vars — a plain terminal run never sees the bridge; an old desktop never sets the env vars).
2. archimedes-desktop: the listener + spawn wiring + the three UI surfaces.
3. Validate: the unit tests (both sides) + the integration test + the manual E2E checklist all pass.

**Steps:**
- [ ] Write the failing integration test in `src-tauri/tests/bridge_integration.rs` (the 4 scenarios).
- [ ] Run `cargo test --test bridge_integration` — confirm the happy path passes and the peer-verification rejection path passes (the foreign process is rejected).
- [ ] Write `docs/roadmap/bridge-e2e-checklist.md`.
- [ ] Run the full validation: `cargo test` (Rust) + `pnpm test` (frontend) + `pnpm test` (pi-archimedes, from the monorepo) — all green.
- [ ] Commit: `test(desktop): bridge integration test + E2E checklist`.

**Acceptance criteria:**
- [ ] The integration test's happy path (request → response round-trip) passes against the real listener.
- [ ] The peer-verification rejection path passes (a foreign same-user process is rejected; a descendant is accepted).
- [ ] The `seq`-drop and terminal-frame-on-timeout paths pass.
- [ ] The E2E checklist is documented and the downgrade combos are confirmed safe.
- [ ] All three test suites (Rust, desktop-frontend, pi-archimedes) are green.

---

## Reference: approved design (v5)

The full approved spec (post-4-review-rounds, verdict: Pass). Tasks 1–6 implement this verbatim; where a task refines a detail, the task wins. **Plan-review refinements (round 1):** the peer-verification **anchor is the desktop's own pid** (`std::process::id()`, always alive) — NOT the spawned agent's pid (the ACP SDK does not expose the child pid); the **agent→Client `GetNamedPipeServerProcessId` check is a v1 limitation** (presence-check only — Node's `net` has no such API); the **macOS bridge is unavailable in v1** (the listener is not started — fail-closed, since macOS has no `SO_PEERCRED`/`ucred`); `request` returns a **cancellable handle** (`{promise, cancel}`); the state machine's `settled` flip comes from **`pi.on("agent_start")`/`pi.on("agent_settled")`** (extension events, NOT the bus — nothing emits them on the bus); the sudo `tool.ts` has its **own headless gate** (gated on `!getBridge().active`); the `session_id` in bridge payloads is the **ACP id** (set via `handle.set_session_id` once known); the `respond_bridge_request` command takes the **`result` `Value` verbatim** (no `BridgeResponse` wrapper).

### 1. Bridge contract
- **Env contract** (Client sets at spawn, via the existing registry `env` passthrough): `PI_ARCHIMEDES_BRIDGE=1`, `PI_ARCHIMEDES_BRIDGE_SOCKET=<path>`, `PI_ARCHIMEDES_BRIDGE_SESSION=<the desktop's own session id>`, `PI_ARCHIMEDES_BRIDGE_SERVER_PID=<the desktop's own pid>` — **not the ACP `sessionId`**, which is agent-generated in the `session/new` response and doesn't exist at spawn time. All present → bridge mode. (`SERVER_PID` lets the agent verify the server it connects to is the real desktop.)
- **Channel:** Client is the server (0600 Unix socket under a 0700 dir; named pipe on Windows). **Peer-verified (both directions where the platform allows):**
  - **Client→agent (server-side):** a connection is accepted only if the connecting process is **the spawned pid itself or a living descendant** of the agent process the Client spawned (covers both the `pi-acp`→`pi` child topology and a registry entry that spawns `pi` directly). The Client records the spawned pid from its spawn handle; on each connection it reads the peer's pid (`SO_PEERCRED` on Linux, `GetNamedPipeClientProcessId` on Windows) and walks the parent chain up to the spawned pid (Linux: `/proc/<pid>/status` `PPid`; Windows: a Toolhelp process snapshot), with the **anchor pid's** liveness checked via the Client's retained spawn handle (kept for the session lifetime — not just recorded as a number), and intermediate ancestors covered by the live-chain walk (a dead intermediate reparents the peer, breaking the chain) — a dead or broken chain fails closed. The spawned child's exit **tears the listener down and unlinks immediately** (closes the pid-reuse window deterministically; consistent with session teardown).
  - **Agent→Client (client-side, Windows):** the bridge client calls `GetNamedPipeServerProcessId` on its connected handle and verifies the server pid equals `PI_ARCHIMEDES_BRIDGE_SERVER_PID` (the desktop's pid); a mismatch fails the connection. This is the real defense against the Windows **second-instance** vector (a different process creating a competing instance of the same pipe name — the flag does *not* prevent that). On Windows the Client creates the **first** pipe instance with `FILE_FLAG_FIRST_PIPE_INSTANCE` (pre-squat detection: if a squatter already took the name, the create fails and the Client uses a fresh path) and subsequent listener instances **without** the flag (so the Client's own concurrent connections work). (On Unix there is no client-side server-pid API, so only the Client→agent direction is verifiable there.)
  - **Trust boundary (stated honestly):** the descendant match trusts the **entire spawned process tree** — including any command the agent executes (bash-tool children, MCP servers, subagents), all of which inherit the env and pass verification. So the verification's guarantee is only against **non-agent** same-user processes; an agent-executed process can issue bridge requests directly (a direct `password` call skips the confirm gate — mitigated by the `SudoPasswordModal` displaying `command`+`reason`).
  - (A 0600 file alone is only a user boundary — a same-user process can read the agent's environ, so a token in env would not stop the phishing vector. A same-user process can also unlink the Client's socket and bind its own to become a fake Client, but gains nothing beyond existing same-user capabilities — the protected direction is password exfiltration. Responses are sent only on the originating, peer-verified connection.)
- **Frames** (all carry `v: 1`):
  - Push: `{v, type:"push", seq, event, payload}` — `seq` = per-process monotonic (herdr-style, **starts at 1**); the Client drops `seq <= lastSeq` (covers reordering **and** duplicate delivery); `lastSeq` is scoped **per bridge listener (per spawn)** — a respawn resets it by construction; **all push payloads are absolute-state and idempotent**; coalesced to at most one in-flight connection per event type (latest wins — also bounds `cost_update` cadence).
  - Request: `{v, type:"request", id, method, source, toolCallId, params}` — `id` = UUID; **`source`** = `"main"` (root ask) or `"subagent:<name>"` (relayed child ask); **`toolCallId`** = the tool-call id of the process that invoked the tool (for a root ask, the same id pi-acp emits in `tool_call` frames → Client-side correlation; for a subagent ask, the child's own tool-call id, which pi-acp never frames — the Client anchors by `source` instead). `params` = the method's params (no duplicated `toolCallId`). (`toolCallId` is nullable/absent for `confirm`/`password` — invoked from `prompt.ts`'s choke point with no tool-call id in scope; `source` is `"main"` for them, since sudo is root-only.)
  - Response: `{v, type:"response", id, result | error}`.
  - Ack: one line after processing a push; sender destroys on first data (herdr-style).
- **Methods:** `ask` (params = ask schema; result = `AskResponsePayload` verbatim — the `toolCallId` rides the frame, not the params), `confirm` (`{command,reason}` → `{confirmed}`), `password` (`{command,reason}` → `{password}` or cancel — password flows Client UI → channel → `sudo -S` stdin **and the pre-existing in-memory credential cache, nothing else**).
- **Push events:** `todos_update`/`todos_clear`/`cost_update` (bus payload types verbatim), `state` (`working`/`idle`/`blocked`; refcount: +1 on `ASK_REQUEST` any source, −1 on `ASK_RESPONSE`; **the machine: `refcount > 0` → `blocked`; `settled ∧ refcount == 0` → `idle`; else `working`** — so `agent_start` → `working` only when `refcount == 0`, and a pending ask keeps `blocked` through `agent_settled`), `session` (pi session refs from `ctx.sessionManager`, **re-emitted on every `session_start`** so a lost frame is recoverable, and the payload **echoes `PI_ARCHIMEDES_BRIDGE_SESSION`** so the Client can assert the connection↔session mapping rather than infer it from the socket path — the desktop maps desktop-session ↔ ACP-session ↔ pi-session).
- **Gating:** env present + `ctx.mode !== "tui"` (captured once at first `session_start`) + `PI_SUBAGENT_SOCKET` absent (root-only — children inherit env). TUI always wins. RPC without env → headless, unchanged. The `active` flag does not depend on a successful connection (lazy per-message connect).
- **Failure:** push = best-effort (initial + 2 retries at 500/1500 ms, then drop); interactive = 5-min timeout → cancel (the bridge's own 5-min timeout is the backstop for a child that neither answers nor exits); **connection close = immediate cancel on both ends**; the Client sends an **explicit terminal frame** (`error:"cancelled"`) before closing on timeout/drain/auth-failure; unreachable channel → fail fast; session close → drain pending as cancelled. **Refcount invariant: every `ASK_REQUEST` is eventually paired with an `ASK_RESPONSE`** — on success, error, timeout, cancel, child exit, and session close — so the `state` refcount never leaks; a **child exit cancels the relayed Client request immediately** (not left to the 5-min timeout). **Child-exit wiring:** `spawn.ts` gains a socket-close/child-exit handler that emits a new `ASK_CANCEL` bus event (requestId + source); the bridge subscribes, cancels its Client request (terminal frame / connection close → the Client's connection-close=cancel handles the UI), and emits the **single** pairing `ASK_RESPONSE` — the bridge is the **sole emitter** of child-path `ASK_RESPONSE` (`spawn.ts` signals, it does not emit — no double-decrement). **Root-path emitter:** the root path's `ASK_RESPONSE (main)` is a **new emission added to the tool's bridge branch**, emitted exactly once when `bridge.ask` settles (success, error, or timeout) — the tool's `ASK_REQUEST (main)` emission is unchanged (a notify hook), and the existing TUI path never paired it, so this response emission is new code, not a mirror of existing behavior.
- **Client waiter cap: 330 s** (agent timeout + 30 s margin — the agent's cancel deterministically wins).
- **Forward compatibility:** unknown push events ignored (still acked); unknown methods → `error` response.
- **API when inactive:** `ask` → throws `BridgeInactiveError` (loud); `confirm` → `false`; `password` → `""` (cancel).

### 2. Bridge module in `core`
`core/src/bridge/{index,channel,events}.ts` — `getBridge()` singleton, `registerBridge(pi)` from core's extension entry; no manifest entry, no settings.
- **Ask data flow (one path, no double-handling):** root asks in bridge mode → the tool's new branch calls `bridge.ask(params, toolCallId)` directly (passing the tool's `_toolCallId` argument; the bridge puts it in the frame, not in `params`); the tool's bus `ASK_REQUEST` emission (source `main`) is unchanged (notify + `blocked` refcount). Child asks: child socket → parent `spawn.ts` → bus `ASK_REQUEST` (source `subagent:<name>`) → **the bridge consumes it in bridge mode, mirroring `ipc-relay`'s TUI behavior — including skipping `source === "main"`** — forwards to the Client, and emits `ASK_RESPONSE` so `spawn.ts` writes it to the child. **The `ask_request` socket payload and the bus `ASK_REQUEST` payload are extended to carry the tool-call id** (from the tool's `_toolCallId` argument); `spawn.ts` propagates it, and the bridge uses it for the request frame's `toolCallId` (and sets `source`). **Child exit:** `spawn.ts`'s socket-close/child-exit handler emits a new `ASK_CANCEL` bus event (requestId + source); the bridge subscribes and cancels the Client request (see §1). **The `ipc-relay` is gated on `!getBridge().active`** (in addition to skipping `source === "main"` and requiring `ctx.ui`) — the gate is checked **per-message in the subscription handler, not at registration** (`registerIpcRelay` runs at extension load when `bridge.active` is false in every mode; the mode is only captured at the first `session_start`, so a registration-time gate would silently reintroduce the double-consumption bug). Gating on `!getBridge().active` (rather than `ctx.mode === "tui"`) preserves today's behavior in **both** TUI and RPC-no-env (relay on) and diverts only in bridge mode (relay off, bridge on) → exactly one consumer per source.
- **Subagent todo path (already exists, cited):** parent-side `subagent/src/stream.ts` watches the child's `manage_todo_list` executions and emits `TODOS_UPDATE`/`TODOS_CLEAR` on the parent bus with a unique `source` per child (cleared on child exit) — the bridge's bus subscription receives subagent columns for free.
- The response→results helper serves **all three** sources (subagent socket, TUI picker, bridge).

### 3. Tool changes
- **ask:** the branch order becomes `if (bridge.active)` → `else if (!ctx.hasUI)` (child socket path) → `else` (TUI picker) — the bridge is checked **first**, so its branch is reachable regardless of `hasUI` (in the actual deployment `hasUI` is true in RPC/ACP, so this is belt-and-suspenders). The bridge branch calls `bridge.ask(params, toolCallId)` (passing the tool's `_toolCallId` argument; the bridge puts it in the frame, not in `params`). **The `ask_request` socket payload is extended to carry the tool-call id** (from `_toolCallId`), so the parent `spawn.ts` can propagate it into the bus `ASK_REQUEST` payload. Shared response→results helper (three sources). Bus emission + result text unchanged.
- **sudo: one change in `prompt.ts` (the single choke point); no call-site ternaries.** `confirmCommand` and `promptForPassword` each **prepend** `if (bridge.active) return bridge.confirm/password(...)`; the existing `ctx.mode !== "tui"` headless gate in `promptForPassword` is **unchanged** (0010's guarantee preserved verbatim) and sits after the bridge branch. The tool calls `confirmCommand`/`promptForPassword` unconditionally — `prompt.ts` is the single choke point where the bridge routing and the headless gate both live, so every caller (current and future) stays safe.
- Unchanged: bash `tool_call` veto, credential cache lifecycle, fail-streak/ticket-probe, `runSudo`. Decline/cancel error text preserved.
- **ADR 0021** (written, updated): bridge mode is a promptable session; the bridge routing lives in `prompt.ts` (the single choke point) with the headless gate unchanged; 0010's guarantees preserved; the 0600 channel is the trust boundary **plus two-direction process-identity verification** (Client→agent descendant match; agent→Client `GetNamedPipeServerProcessId` on Windows); subagent children remain blocked.

### 4. Desktop — Rust
- Registry `bridge: bool` (built-in `pi` entry: `true`); spawn env wiring in `session.rs`.
- `acp/bridge.rs` (mirrors `permission.rs`): listener **started before the spawn call returns** (the push retry window is only ~2 s); **per-spawn randomized socket path** under the 0700 dir; **sets `PI_ARCHIMEDES_BRIDGE_SERVER_PID` to its own pid** (for the agent's `GetNamedPipeServerProcessId` check); peer verification (**descendant match**: record the spawned pid, read the peer's pid via `SO_PEERCRED`/`GetNamedPipeClientProcessId`, walk the parent chain to the spawned pid, liveness via the **retained spawn handle** (kept for the session lifetime); the **first** Windows pipe instance created with `FILE_FLAG_FIRST_PIPE_INSTANCE` (pre-squat detection), subsequent instances **without** it; the spawned child's exit tears the listener down and unlinks immediately (Windows: the pipe instance vanishes when the last handle closes — no unlink API); `bridge-request`/`bridge-event` Tauri events; `pending_bridge` map + waiter tasks (330 s cap; session-close drain; terminal frames; connection-close = cancel); `respond_bridge_request` command; **unlink on session close; tolerate/clean stale files at startup**.
- Invariants: password never in ACP frames, tool results, SQLite history, **logs, or `pending_bridge`/store state after the reply is sent** (removed immediately); the **bridge-channel copy** is dropped after the `sudo -S` stdin write (the pre-existing **in-memory credential cache** retention is unchanged — ADR 0010's scope); listener local-only and peer-verified.

### 5. Desktop — UI (ported from the TUI screenshots)
- **`AskQuestionCard`** (inline, **correlated via the request frame's `toolCallId`** — the tool-call id, i.e. the same id pi-acp emits in `tool_call` frames): accent separator, circular radio list (filled amber dot = selected), `Other (type your own)`, checkboxes for multi, `(Recommended)` suffix, per-option note field, footer hints, final batch review; **stacking defined for concurrent asks**. **Correlation is keyed by `(source, toolCallId)` or the frame's UUID `id`, never by bare `toolCallId`** (cross-process tool-call ids can collide). **Subagent asks anchor by `source` (`subagent:<name>`) with a labeled card** — the desktop has no ACP `tool_call` frame for child tool calls. (Two same-named concurrent children share a `source` and produce identically-labeled cards; response routing is still safe via `(source, toolCallId)`/UUID, so this is cosmetic.) **If the request arrives before its ACP `tool_call` frame, the card renders queued (bounded ~1–2 s) then unanchored/labeled.** Submit/cancel/timeout → card collapses.
- **`SudoConfirmModal`** (modal): command + reason + Run/Cancel. **`SudoPasswordModal`** (modal): **displays `command` + `reason` verbatim from the `password` params** + a masked `•` field + Enter/Esc/Backspace — so even a direct `password` call (which skips the confirm gate) shows the user what the password is for.
- **`TodoBoardPanel`** (collapsible right rail): `Todo List — n/m completed` header; numbered ✓/◉/○ items; **subagent columns fed by the existing `stream.ts` parent-bus relay** (unique `source` per child, cleared on child exit); auto-collapse when empty; `rawInput` fallback for non-bridge agents.
- **The double prompt is by design**: vanilla ACP permission prompt (generic gate) → tool-specific bridge modal — the user's explicit choice.
- **Store** `src/store/bridge.ts`: pending requests (password removed after send), todo state, `state`/`cost`/`session` (wired, not rendered in v1).

### 6. Artifacts & rollout
- **ADRs:** pi-archimedes **0021** (sudo bridge mode — unchanged headless gate + bridge routing in `prompt.ts` as the single choke point, whole-process-tree trust boundary) + **0022** (bridge channel mechanism — env contract incl. `SERVER_PID`, core placement, root-only gate, frame schema with `source`, ask data flow with tool-call-id propagation + `ipc-relay` `!bridge.active` gate, lifecycle, two-direction peer verification); desktop **0003** (bridge client — registry flag, listener, descendant + `GetNamedPipeServerProcessId` peer verification, 330 s cap, terminal frames, invariants). **Glossary:** Bridge terms in both CONTEXT.md files.
- **Rollout:** (1) pi-archimedes: bridge module + tool changes → non-breaking release (inert without env). (2) Desktop: listener + spawn wiring + the three UI surfaces. (3) Validation: unit tests both sides (env matrix, fake socket server, Tauri command round-trip, store updates, `seq`-drop behavior) + one desktop integration test (node stub speaking the protocol, **including the peer-verification rejection path**) + manual E2E checklist: ask (multi/Other/cancel/timeout/**concurrent asks**), sudo (decline/cancel/warm/cold cache), todo (main+subagent/auto-clear/child exit), **session close with a pending prompt, crash/restart mid-request, two simultaneous sessions (frame isolation), downgrade combos** (old desktop + new suite = safe — old desktop never sets env; new desktop + old suite = env ignored, today's behavior).

### Review history
- 2026-08-14 — reviewer subagent (1st): ❌ Fail (2 blocking, 8 important, 9 minor). All 19 findings fixed and folded into v2; ADRs 0021/0022/0003 updated to match.
- 2026-08-14 — reviewer subagent (2nd, re-review of v2): ❌ Fail (2 critical, 3 major, 8 minor). The criticals were in the peer-verification fix: (1) the pid-match assumed the connecting pid equals the spawned pid, but the Client spawns `pi-acp` which spawns `pi --mode rpc` — the bridge runs inside `pi`, so the peer pid is a child, not equal (verified against the installed `pi-acp` binary); (2) the Windows "pipe DACL by process SID" mechanism doesn't exist. All 13 findings fixed and folded into v3; ADRs 0021/0022/0003 updated to match.
- 2026-08-14 — reviewer subagent (3rd, re-review of v3): ❌ Fail (2 blocking, 2 important, 5 minor). The blocking: (1) the Windows `FILE_FLAG_FIRST_PIPE_INSTANCE` claim is wrong (it's a creator-side first-instance assertion, not name ownership — a foreign second instance isn't denied, and the flag on every instance breaks the Client's own concurrency) → fixed with first-instance-with-flag + agent-side `GetNamedPipeServerProcessId` verification via a new `PI_ARCHIMEDES_BRIDGE_SERVER_PID` env var; (2) the `ipc-relay` `ctx.mode === "tui"` gate would disable the relay in RPC-no-env (contradicting the "unchanged"/"inert"/downgrade-safe claims) → fixed by gating on `!getBridge().active`. All 9 findings fixed and folded into v4; ADRs 0021/0022/0003 updated to match.
- 2026-08-14 — reviewer subagent (4th, re-review of v4): ⚠️ **Pass with Issues** (0 blocking, 2 important, 5 minor, 3 nits). All 9 v4 fixes verified sound (several against the actual code: `ipc-relay.ts`, `tool.ts`, `spawn.ts`, `bus.ts`). The 2 important (both folded in as v5): (1) the root-path `ASK_RESPONSE (main)` was described as "unchanged… mirroring the existing TUI path" but the code shows the TUI path never emits it (the only emitter is `ipc-relay.ts`, child path) — reworded as a **new emission** in the tool's bridge branch; (2) the child-exit cancellation had no owner/mechanism — added a new `ASK_CANCEL` bus event (spawn.ts signals, bridge is the sole `ASK_RESPONSE` emitter, no double-decrement). Minors: `bridge.ask(params, toolCallId)` signature; `toolCallId` nullable for `confirm`/`password`; relay gate is per-message (not at registration); anchor-pid liveness via the retained handle; ADR 0003 correlation keying. Nits: state machine written as a total function; subagent same-name card collision noted as cosmetic; Windows unlink parenthetical. ADRs 0021/0022/0003 updated to match. **Verdict: the spec passes — ready for implementation.**
- 2026-08-14 — reviewer subagent (5th, **implementation-plan review**, not the spec): ❌ Fail (4 blocking, 9 important, 8 minor). The 4 blocking: (1) the state machine subscribed `agent_start`/`agent_settled`/`session_start` as **bus events**, but those are **pi extension events** (`pi.on(...)`) — nothing emits them on the bus, so `settled` never flipped → fixed by subscribing `pi.on(...)` in `registerBridge` and having `events.start()` subscribe only the six real bus events; (2) `sudo/src/tool.ts:413` has its **own** `ctx.mode !== "tui"` gate that rejects before `prompt.ts` is reached → fixed by gating it on `!getBridge().active`; (3) **session-id identity mismatch** — the listener starts pre-spawn with a client-side UUID but the frontend keys by the ACP `session_id` → fixed with `handle.set_session_id(&acp_id)` (bridge requests only occur mid-turn, after establish, so they always carry the ACP id); (4) the ACP SDK does **not** expose the spawned child's pid, so "fail closed if unavailable" would reject every connection forever → fixed by **anchoring the descendant walk at the desktop's own pid** (`std::process::id()`, always alive). The 9 important: `request` returns a cancellable handle (`{promise, cancel}`); the `ask` param typed structurally from the bus payload (not the unresolvable `AskQuestion`); the Windows agent-side `GetNamedPipeServerProcessId` check scoped out as a v1 limitation (presence-check only); the `SO_PEERCRED` code gated `#[cfg(target_os = "linux")]` + an explicit macOS fail-closed policy (the `#[cfg(unix)]` combo would break the macOS build); listener teardown called unconditionally after `connect_with` (not nested in the `session_id_tx` guard); the `BridgeResponse` wrapper dropped (the command takes the `result` `Value` verbatim); the foreign-process rejection test uses a double-fork orphan (a plain spawned stub is a descendant → accepted); the `rawInput` fallback needs `sessions.ts` to keep `rawInput` on tool-call messages. All 21 findings fixed and folded into the plan; ADRs 0003/0021/0022 updated to match.
