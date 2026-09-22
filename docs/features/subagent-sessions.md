---
status: live
last-verified: 2026-09-22
verified-by: cargo test (63 passed, incl. subagent_dispatch + subagent_concurrency) + pnpm test (102 passed) in archimedes-desktop
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

- Root-cause the 2026-09-15 hang (two concurrent ACP sessions on one tokio
  runtime; suspected SDK/async-io global reactor) — tracked in ADR 0004
  (option 3); lifting the one-live cap is a one-token policy flip once fixed.
