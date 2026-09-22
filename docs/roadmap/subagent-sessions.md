---
status: approved
done-when: In `pnpm tauri dev`, a `subagent` tool call from a bridge-mode main session spawns a desktop-managed subagent session: the right-rail panel shows its live compact stream (state indicator, tool calls); its `ask` renders in the panel and the answer flows directly to the subagent (no main-agent relay); parallel dispatch shows multiple panels; the tool returns combined results to the main agent; the 2-session concurrency regression test passes on the worker runtime; non-bridge subagent behavior is unchanged; `cargo test` / `pnpm test` / `cargo clippy --all-targets` / `cargo fmt --check` are green.
---

# Subagent Sessions Spec

**Goal:** In bridge mode, move subagent execution from the agent-side suite (the `subagent` tool forking a child pi process) to the desktop (Client): the desktop spawns a full ACP session per delegated task, renders it in a side panel through the same pipeline as the main agent, and the subagent's interactive tools (`ask`, `sudo_exec`) talk to the desktop directly — no relay through the main agent.

**Terminology:** `CONTEXT.md` — **Subagent session** (new entry); "Agent" remains the ACP process; the one-live policy (ADR 0002) governs user-facing Sessions only.
**Decisions on record:** ADR 0004 (dedicated worker runtime; per-session-thread fallback), ADR 0005 (per-dispatch `PI_ACP_PI_COMMAND` wrapper for launch config).

## 1. Architecture & process topology

- Desktop topology: the main runtime hosts the main Session; a dedicated worker runtime hosts all N subagent sessions (each a `pi-acp` → `pi` chain, suite in bridge mode).
- Bridge mode: the `subagent` tool takes a dispatch branch — instead of forking, it sends a `dispatch_subagent` bridge request; the desktop's `SubagentSessionManager` spawns the session through the existing spawn path (registry `pi` entry + the four bridge env vars: per-spawn socket, its own session id, server pid).
- The subagent's suite activates the bridge (root-only gate satisfied — the desktop spawn sets no `PI_SUBAGENT_SOCKET`); its interactive tools go directly to the Client. The `subagent` tool is excluded from the spawned subagent (workers can't spawn workers).
- Sandbox: `cwd` (and fs sandbox root) = the parent session's Space folder, through the same spawn path as a regular Session; the tool's optional `cwd` parameter is ignored in bridge mode (documented).
- Non-bridge modes (TUI / headless / RPC-without-env): the existing fork path, ask relay, and todo-board subagent columns are untouched.
- Fallback: bridge unreachable (e.g. macOS, where the listener is not started — the documented v1 bridge limitation) → the tool falls back to the fork path; subagents still work, no side panel.
- Ephemeral: subagent sessions are not stored.

## 2. Dispatch protocol & launch config

- New bridge method `dispatch_subagent` (existing frame format): one request per task; parallel dispatch = N concurrent bridge connections (existing concurrent-connection support); the tool awaits all (existing blocking semantics).
- Request params carry the *resolved* agent definition — the suite resolves the agent file (name, system prompt, model, thinking level, tools) and keeps model pre-validation (0008); the desktop only launches.
- Method-aware timeouts: `dispatch_subagent` has no 5-min/330 s timeout; the connection lives until completion or cancellation. Cancellation = user stop (connection close = cancel, existing semantics), parent session close, app exit, session error/exit — always with an explicit terminal frame before close (existing pattern).
- Response: per task — the subagent's final output + metrics (tokens/cost/time), or an error (spawn failure, cancellation, model unavailable). The tool returns the combined result in the existing shape (final output beneath the metrics line).
- The subagent's own interactive requests (ask/confirm/password) ride its **own** bridge connection (its own per-spawn socket/listener, peer-verified — the subagent's pi is a grandchild of the desktop, the same topology as the main agent); a subagent blocked on an ask keeps the dispatch request pending (the main agent is blocked on the tool — consistent), and its `state` push shows `blocked`. Its asks arrive with `source: "main"` from its own session id; the panel provides the labeling. The `source: "subagent:<name>"` relayed shape stays for the non-bridge fork path.
- Launch config (ADR 0005): the desktop spawns `pi-acp` with `PI_ACP_PI_COMMAND=<per-dispatch wrapper>` (in the per-spawn 0700 dir); the wrapper execs `pi <flags> "$@"` (pi-acp passes `--mode rpc --no-themes` and `env: process.env`):
  - `--system-prompt <agent file body>` (named agents only; config-less dispatch: no override),
  - `--model <resolved model>` + `--thinking <level>` (when resolved/explicit),
  - `--tools <list>` (named agents with tools — the allowlist excludes the subagent tool) or `--exclude-tools subagent` (config-less),
  - `--no-session` (ephemeral — no pi session file).
- ACP `session/set_config_option` (pi-acp supports model/mode config options) remains available for mid-session model changes; the initial model is set in the wrapper.
- Wrapper edge cases (flag ordering, quoting; pi-acp's best-effort `pi --version` probe uses the literal `pi` on PATH and is unaffected) are a verification item.

## 3. Runtime topology & de-risking

- One dedicated tokio runtime on a dedicated OS thread, lazily created at first dispatch, shared by all subagent sessions; the main Session stays on the existing runtime.
- Cross-runtime handoff is channel-based (oneshot), never `block_on` across runtimes.
- **Build gate:** a 2-session concurrency regression test — a main-runtime session plus worker-runtime session(s) sending concurrent prompts, asserting `send_prompt` latency stays sane (no 60 s stall; p95 well under a few seconds) — runs before the rest is built on top. If the hang reproduces on the worker runtime (i.e. the async-io reactor is process-global), fall back to **per-session thread + runtime** (ADR 0004's documented fallback) and re-run the test.
- One-live policy code untouched; subagent sessions never pass through `SessionManager`'s close-then-spawn path.
- Failure isolation: if the worker runtime dies, all subagent sessions cancel (terminal frames to their pending dispatch requests); the main session is unaffected.

## 4. Subagent session lifecycle

- Creation order: desktop session id (UUID before spawn — the existing pattern) → peer-verified bridge listener → launch wrapper (ADR 0005) → spawn `pi-acp` on the worker runtime → ACP `initialize` + `session/new` → the task text as the first `session/prompt`.
- Completion: the subagent's turn settles (`end_turn`) → collect final output + metrics → reply to the dispatch request → teardown (process exit, bridge listener down, socket unlinked). The panel entry stays (collapsed) until the parent session closes or the user dismisses it.
- Error: spawn failure / model unavailable / session error → immediate error reply + teardown.
- Cancellation: parent connection close / parent session close / app exit → kill the subagent session (process kill, bridge teardown, terminal frame to the pending dispatch request); all three reuse the existing "connection close = immediate cancel" semantics.
- Permission prompts: the subagent's tool calls raise ACP `session/request_permission` exactly as a regular Session's do — rendered as inline cards in the side panel's stream (existing permission-card machinery, keyed by the subagent session id).
- Concurrency cap: none in v1 — a pathological main agent can dispatch many subagents, the same exposure the fork path has today. Documented, not solved.

## 5. UI: the subagent side panel

- A new collapsible panel in the right rail (alongside `TodoBoardPanel`): one entry per active/recent subagent session, each a **compact view of the same ACP stream** — condensed text chunks, collapsed tool-call cards, a `working`/`idle`/`blocked` indicator (the `state` push finally rendered), and a metrics line (tokens/cost/time) on completion. Auto-expands on dispatch; a pending permission prompt badges it (and auto-expands) when collapsed.
- Ask cards: the subagent's `ask` renders in that session's compact stream, labeled with the subagent name (existing `AskQuestionCard` subagent-labeling machinery; the `source: "subagent:<name>"` relayed shape stays for the non-bridge fork path).
- Main stream: the `subagent` tool-call card in the main session shows a live "running" state (link/badge into the panel) while the dispatch is pending, and the result on completion — replacing today's silent block.
- Todo board: in bridge mode, the main session's todo-board subagent columns are no longer fed (no fork → no relay); the subagent's own todos appear in its compact stream. Non-bridge mode is unchanged.
- State/cost: the `state` push is rendered (panel indicator); the metrics line on completion comes from the dispatch response's metrics, with the `cost` push as the live source while running.

## Compatibility & limitations

- macOS: bridge dispatch unavailable (the listener is not started — the documented v1 bridge limitation) → fork fallback (subagents work, no panel).
- Windows: per-subagent-session named pipes (distinct names); the existing `FILE_FLAG_FIRST_PIPE_INSTANCE` first-pipe logic is unchanged.
- Subagent sessions are ephemeral (`--no-session` at pi level; not stored).
- The `subagent` tool is excluded from spawned subagents (`--tools` allowlist for named agents; `--exclude-tools subagent` for config-less) — workers can't spawn workers, as today.

## Out of scope

- Root-causing the 2-session hang (tracked follow-up; ADR 0002 / 0004).
- Storing subagent session history (rejected in design — ephemerality).
- Tabbed full-session view and inline-card placements (side panel chosen in design).
- Subagents of subagents (excluded by tool exclusion, as today).

## Verification highlights

- The 2-session concurrency regression test on the worker runtime (build gate; per-session-thread fallback if it fails).
- End-to-end: dispatch → panel stream → ask → answer → result, single and parallel; all three cancellation paths (user stop, parent close, app exit); fork fallback on unreachable bridge.
- `cargo test` / `pnpm test` / `cargo clippy --all-targets` / `cargo fmt --check` green.
