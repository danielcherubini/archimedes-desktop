---
status: live
last-verified: 2026-10-02
verified-by: cargo test (the subagent.rs native-dispatch, throwaway-Db, and cancel suites) + pnpm test in archimedes-desktop
---

# Subagent sessions

A **Subagent session** is a native `AgentLoop` child the desktop spawns **in
process** — the SAME in-process runtime (the main tokio runtime) and the
same shared `SessionDriver` machinery as the parent session (ADR 0011/0022) —
the external `pi` embodiment is removed (ADR 0022), so a subagent session is
always an in-process native child. The `subagent` tool's dispatch
(`dispatch_native` on the `SubagentSessionManager`) `tokio::spawn`s a driver
task that builds the child, drives its single task turn, and resolves with
the final output + metrics. Subagent sessions are **ephemeral** (the driver's `db` is `None` —
the child's `native_messages` writes go to a throwaway `Db` discarded on
teardown) and are excluded from the one-live policy by definition. A
subagent cannot dispatch subagents (the `subagent` tool is stripped from the
child's tool set — the recursion guard).

## Config resolution (layered: explicit > frontmatter > parent defaults)

The main agent's `subagent` tool takes `task` (required) + optional
`agentName` / `model` / `systemPrompt` / `tools` / `thinking` (`launch`
params). `agentName` is resolved **case-insensitively against the discovered
Agent definitions** (ADR 0020 — flat `*.md` files, space-level
`.agents/agents` + `.pi/agents` walked up to the repo root, user-level
`~/.agents/agents` + `~/.pi/agent/agents`; advertised by the `list_agents`
tool). An **unknown or omitted `agentName` is a config-less label-only
dispatch** — never an error. The layers:

- **`model`**: the explicit param, else the frontmatter `model`, else the
  parent's model. Resolved **at dispatch time against the effective catalog**
  (`EffectiveCatalog::resolve` — the base catalog + the settings' providers
  + live discovery; `NativeDeps` carries a catalog *supplier*, NOT a startup
  snapshot — the base catalog is empty after the pi-config seeding removal,
  so a named agent's `model:` must resolve against user providers). An
  explicit unknown model **fails** the dispatch (`unknown model: …`); a
  frontmatter model that resolves to nothing **degrades** to the next layer
  (a stale user file must not break a dispatch). A trailing `:<level>`
  suffix is stripped for resolution and becomes a thinking-level candidate.
- **`thinking`**: the explicit param, else the model key's `:<level>` suffix,
  else the frontmatter `thinking` (a native extension of the pi agent
  format). Validated against the model's `thinking_levels` (an empty set
  soft-passes; a mismatch is DROPPED — never sent upstream as a bogus
  `reasoning_effort`).
- **`tools`**: the explicit param VERBATIM (a non-empty set that empties out
  after the minus yields NO tools — not re-expanded), else the frontmatter
  `tools` (unknown names dropped; a list that empties out is treated as
  absent), else the parent's `enabled_tools` (the `[]` = all harness
  convention) — **always minus `subagent` AND `list_agents`**.
- **system prompt**: the explicit `systemPrompt`, else the frontmatter body
  (an empty body means "no system prompt"), plus the todo-guidance line when
  the child has `manage_todo_list` (ADR 0017); `None` → no system message.

## The driver task (settle / cancel / teardown)

`dispatch_native` (and the test-only
`dispatch_native_force_temp_file`) call `dispatch_native_inner`, which
`tokio::spawn`s ONE driver task on the **main runtime** (in-process — the
oneshot is the observation point; the `JoinHandle` is detached). The task:

1. Resolves the effective catalog (step 0 — at dispatch time),
2. Opens the **throwaway child `Db`** — `:memory:` first, a temp-file
   fallback (`open_temp_db`), guarded by a `TempFileGuard` held for the
   WHOLE task (the temp file is removed on ANY exit — happy OR
   panic/early-return; the `Arc<Db>` is released FIRST so the SQLite
   connection closes before the file is removed — safe on Windows),
3. Records the child session in the throwaway `Db` (the `native_messages`
   FK — the child is ephemeral, never archived, ADR 0016),
4. Builds the child `AgentLoop` with FRESH channels / tokens (NOT the
   parent's), the `CapturingSink`, the manager's **SHARED `pending_*` maps**
   (the UI's `respond_*` searches the shared maps — fresh maps would hang
   the child until `settle_timeout`), and `trust_db` threaded from
   `NativeDeps` (below),
5. Seeds the system message, spawns the loop, emits
   `subagent-session-started` on the parent's REAL sink (NOT the
   `CapturingSink` — the resolved `model` / `thinkingLevel` /
   `enabledTools` make the child's config observable), and sends the task
   prompt (a `SendError` preflight failure = the child died before the turn
   started → `Failed`),
6. Races: the child's **settle** vs the **`settle_timeout`** (default 30
   min — a hung turn is torn down and reported `failed "timed out"` instead
   of lingering as a zombie) vs the **`SubagentCancel`** handle vs the child
   loop task **dying** (a `settle_tx` drop → `Failed`, NOT `Completed`),
7. Tears down **unconditionally on EVERY exit** (session + turn tokens,
   `abort()` — the belt-and-braces for a child hung inside a provider HTTP
   read that ignores cancellation, the `events_rx` drain, the throwaway
   `Db` / temp file via the `TempFileGuard`),
8. Resolves the `SubagentOutcome` (`Completed { output, metrics }` /
   `Failed { error }` — a cancel is `Failed "cancelled"`) and emits
   `subagent-closed` on the parent's real sink.

**Cancel — the `SubagentCancel` / `ExternalClose` pair:** despite the
`ExternalClose` type name (a harness-neutral rename survivor, ADR 0022),
this is the **native** cancel plumbing: `SubagentCancel::new_external_close`
builds one `watch` flag + one first-set-wins `CloseKind` (kind `User` set
before the flag flips); `cancel()` (idempotent) is the caller's path — a
parent turn cancel, a parent session close, or an app exit cancels the
in-flight dispatch, and the driver task's select arm sees the flipped flag
and tears the child down. A pre-aborted signal short-circuits the dispatch
before it starts.

**Metrics are real.** The child self-emits a per-turn `cost_update` (source
`"main"`, from the model's `turn_end` usage — per-turn deltas, `reasoning`
excluded as an `output` subset); the driver accumulates the subagent's
`cost_update` payloads (sum, not last-payload) into `SubagentMetrics` (token
/ cost fields are real when the session pushed usage, 0 otherwise;
`durationMs` is the desktop's wall clock). The wire metrics shape is
unchanged: `{ output, inputTokens, outputTokens, cost, durationMs }` (the
cache fields flow in the payload but are not in the wire shape — YAGNI). The
`subagent` tool's result to the main agent is the `Completed` output (or
`subagent failed: {error}` / `cancelled`). A failed child TURN (an
exhausted-retry model call) settles "successfully" via a bare
`agent_settled` and reports `Completed { output: "" }` — a documented
follow-up, not a bug to fix here.

## Trust inheritance

`NativeDeps.trust_db` is the TRUST lookup source threaded onto the child
(ADR 0010): a subagent in a **Trusted Space** inherits the parent's trust —
the in-process permission gate auto-approves the gated tools
(`bash`/`edit`/`write`), so the child does not prompt; `sudo_exec` (its own
confirm + password modal) and `ask` (a `select` request, not a confirm) are
unaffected. `None` = fail-closed (the gate prompts on every `bash` /
`edit` / `write` in the child). It is SEPARATE from the driver's `db`
(transcript persistence — always `None` for subagents): threading the full
db here would make subagent updates attempt transcript inserts into a
`sessions` row that does not exist (a FK failure) — the lookup and the
persistence are deliberately decoupled.

## Interactive tools (ask, sudo_exec)

The subagent's interactive tools run in the desktop's **in-process
interactive channel** (`agent/interactive.rs`): their prompts
(`interactive-request` / permission / sudo `:confirm` / `:password`
sub-prompts) go **directly to the Client** — without relaying through the
main agent. `SubagentSessionManager::respond_interactive_request` /
`respond_permission` resolve them from the driver's shared `pending_bridge`
/ `pending_sudo` / `pending_permissions` maps (keyed
`"{session_id}/{request_id}"`, `:confirm` / `:password` suffixes for the
sudo sub-prompts); awaiting the answer is bounded (330 s
`DEFAULT_INTERACTIVE_TIMEOUT`; a session close / teardown cancels the
waiter).

## Presentation

Subagents live under the "Delegating" tool card in the main chat view
(`SubagentDelegatingCard` — one row per subagent: status icon + agent name +
task + a live one-line activity preview; open by default); clicking a row
opens a dedicated right-side transcript modal. The sidebar Subagents panel
is removed (`SidePane` is todos-only + the subagent sudo modals at the frame
root; its auto open/close is driven by todos only — a subagent no longer
opens the pane). Grouping is session-level: every `SubagentDelegatingCard`
in a session lists all of that session's subagent rows (the
`subagent-session-started` payload carries no `toolCallId`), while the
activity preview resolves only from each card's own `rawOutput.details`.

**The "always mounted" invariant (load-bearing):** a subagent's interactive
requests (`ask` / permission / sudo `confirm` / `password`) MUST be rendered
by a mounted React component, or they hang until the 330 s interactive cap
(or the dispatch's `settle_timeout`). `SubagentDetailHost` (always mounted
at the `App` root) renders every subagent's `SubagentTranscript` exactly
once — the selected one in the visible modal, the rest hidden (the `hidden`
attribute keeps the component mounted) — so an unrendered request can never
hang. Closing the modal (X / Esc) only clears the selection; the entry stays
in `useSubagents`. The modal's `onKeyDown` Esc handler is a React handler on
the sheet (NOT a window-level capture handler) so it stops the native event
before `ChatStream`'s window-level Escape-cancel fires, and a nested
consumer's own `stopPropagation` (e.g. `AskQuestionCard`'s Esc-dismiss)
still wins.

## Follow-ups

- 2026-09-15 hang (two concurrent sessions on one tokio runtime):
  **no longer reproduces** on current versions (resolved 2026-09-22 — root
  cause never confirmed, likely version/environment-specific); the one-live
  cap is lifted (subagents run on the main runtime alongside the parent). If
  it reappears, root-cause before re-tightening.
