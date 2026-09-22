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

- **Metrics are zero-valued in v1 except `durationMs`.** The suite does NOT
  emit a `cost_update` for a process's OWN usage (the only bus `COST_UPDATE`
  emitter is the subagent tool reporting forked children's deltas, source
  `subagent:<name>`). The desktop's captured metrics for a subagent session are
  `{0, 0, 0, <real durationMs>}` — `durationMs` is real (wall clock); the
  token/cost fields are 0. The desktop's capture mechanism is in place for a
  future suite-side self-usage push (e.g. on `agent_settled`) — that push is a
  tracked follow-up; it would also change the main session's `cost_update`
  semantics. The tool's `usage`/`progressSummary.tokens` report zeros in bridge
  mode — documented, not silent.
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
- Suite-side self-usage `cost_update` push (fills the v1 metrics zeros above).
