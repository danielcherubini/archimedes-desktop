---
status: live
last-verified: 2026-09-27
verified-by: cargo test (143 passed, incl. subagent_dispatch + subagent_concurrency) + pnpm test (505 passed) in archimedes-desktop
---

# Subagent sessions

In bridge mode, the desktop manages subagent sessions: the suite's `subagent`
tool sends a `dispatch_subagent` bridge request (method-aware — no 5-min/330 s
timeout) and the desktop spawns a full ACP session per delegated task on the
dedicated worker runtime (ADR 0004), driven through the shared session-driver
machinery (per-spawn peer-verified bridge listener, per-dispatch
`PI_ACP_PI_COMMAND` launch wrapper — ADR 0005). The request resolves with the
final output + metrics. Subagent sessions are ephemeral (not stored,
`--no-session`) and excluded from the one-live policy by definition (ADR 0002).

## Bounded settle (the zombie fix, 2026-09-27)

A subagent's turn settle wait is BOUNDED (a 30-min `settle_timeout` on the
`SessionDriver`; `RpcError::SettleTimeout`): a hung turn is torn down (the
external close kills the `pi` process) and reported `failed` instead of
lingering as a zombie. The dispatch's cancel probe is read BEFORE the
unconditional teardown cancel, so a user cancel still reports
`error "cancelled"` while a settle timeout / agent death reports its own
error (a `SettleTimeout` / agent death has no flipped flag yet). Teardown on
completion is preserved (a completed subagent's `pi` process is still
reaped — `dispatch_spawns_rpc_child_and_captures`).

## Presentation (2026-09-27)

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
by a mounted React component, or they hang until the bridge timeout (330 s).
`SubagentDetailHost` (always mounted at the `App` root) renders every
subagent's `SubagentTranscript` exactly once — the selected one in the
visible modal, the rest hidden (the `hidden` attribute keeps the component
mounted) — so an unrendered request can never hang. Closing the modal (X / Esc)
only clears the selection; the entry stays in `useSubagents`. The modal's
`onKeyDown` Esc handler is a React handler on the sheet (NOT a window-level
capture handler) so it stops the native event before `ChatStream`'s
window-level Escape-cancel fires, and a nested consumer's own
`stopPropagation` (e.g. `AskQuestionCard`'s Esc-dismiss) still wins.

## Constraints

- **Metrics are real.** The suite self-emits a per-turn `COST_UPDATE` (source
  `"main"`, from pi's `turn_end` usage — per-turn deltas, `reasoning` excluded
  as an `output` subset); the desktop accumulates a subagent's `cost_update`
  payloads (sum, not last-payload) into the metrics snapshot. `cost` is real
  (pi-ai's pricing table); `durationMs` is the desktop's wall clock. The wire
  metrics shape is unchanged: `{ inputTokens, outputTokens, cost, durationMs }`
  (the cache fields flow in the payload but are not in the wire shape — YAGNI).
  In bridge mode, the `subagent` tool's `usage` (its report to the main agent)
  is built from the `dispatch_subagent` RESPONSE's `metrics` (the desktop's
  accumulator output): `dispatchViaBridge`'s success path
  (`packages/subagent/src/dispatch.ts`) maps `res.metrics` → `usage` with REAL
  `input`/`output`/`cost` (the `cacheRead`/`cacheWrite` fields are hardcoded 0 —
  the wire metrics shape has no cache fields) and
  `progressSummary.tokens = inputTokens + outputTokens` (REAL). The SYNTHESIZED
  `progress.*` token fields (the in-flight progress updates from
  `packages/subagent/src/execute.ts`'s bridge branch — the placeholder + the
  final-failure progress) remain ZEROS (the bridge branch synthesizes
  `progress` without token data — including the RESULT's own `progress` field,
  also synthesized zero-token by `dispatchViaBridge`; only the RESULT's
  `usage`/`progressSummary` are real): result `usage`/`progressSummary` = real,
  in-flight `progress` = zeros.
- **macOS/Windows: the bridge listeners are no-ops there (ADR 0003).** The
  suite's dispatch branch sees a `BridgeTransportError` (no response frame —
  the connect fails) and falls back to the fork. The wrapper's `.cmd` variant
  exists for compilation completeness; it is not exercised in v1.
- **A deliberate cancel never forks.** A bridge request aborted by the desktop
  settles deterministically as a `BridgeCancelledError` (core `channel.cancel()` —
  a typed class, matched by `instanceof`, NOT the message string), which the
  subagent package maps to a FAILED result (`error: "cancelled"`) — never
  `{fallback: true}`. Only a transport failure (no response frame) falls back
  to the fork. A pre-aborted signal short-circuits the bridge dispatch before
  it starts.
- **Timeouts are ambiguous-liveness, never fallback triggers.** A
  `dispatch_subagent` timeout is a plain `Error` (the peer may still be alive
  and deliver late); only `BridgeTransportError` triggers the fork fallback.

## Follow-ups

- 2026-09-15 hang (two concurrent ACP sessions on one tokio
  runtime): **no longer reproduces** on current versions (ADR 0002,
  resolved 2026-09-22 — root cause never confirmed, likely
  version/environment-specific); the one-live cap is lifted. If it
  reappears, root-cause per ADR 0004 (option 3).
