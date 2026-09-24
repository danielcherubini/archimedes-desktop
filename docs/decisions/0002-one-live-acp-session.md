---
status: superseded
date: 2026-09-16
superseded-by: 0009-rpc-replaces-acp.md
superseded-date: 2026-09-24
---

# Cap live ACP sessions at one app-wide (v1)

> **SUPERSEDED — LIFTED 2026-09-22.** The one-live cap is REMOVED: `start_session`
> / `resume_session` no longer close other live sessions (the `replaced`
> close-reason is gone; the `supersede_live_sessions` call sites are deleted).
> **Resolved-by: no longer reproduces on current versions.** The 2026-09-15
> hang does NOT reproduce on the current versions — the diagnostic repro
> (`src-tauri/tests/acp_concurrency_real.rs`, rebuilt for the 2026-09-15
> topology) PASSES: two concurrent REAL pi ACP sessions on ONE
> `new_multi_thread().worker_threads(2)` runtime both answer `send_prompt` in
> well under 1 s (runs: 511 ms / 597 ms / 489 ms; the two-runtime control
> passed as expected). Versions: pi-acp 0.0.33 + pi 0.87.0 +
> `agent-client-protocol` 2.1.0 / tokio 1.53.1 / async-io 2.6.0. **Honest
> caveat:** the root cause was never confirmed — the hang simply no longer
> reproduces (it was likely version/environment-specific). This is the ADR's
> option 3 (root-cause + fix + lift the cap) taken with the "no longer
> reproduces" evidence. The worker-runtime decision (0004) STANDS
> independently; only the cap's rationale is removed. Evidence: the experiment
> log at the top of `src-tauri/tests/acp_concurrency_real.rs`:
>
> ```
> 2026-09-22  repro   two_real_sessions_one_runtime_hang
>             pi-acp 0.0.33 + pi 0.87.0 (two runs, --test-threads=1)
>             → PASS (no hang). Run 1: session 1 -> EndTurn, session 2 ->
>             EndTurn, prompt phase 511ms. Run 2: ... 597ms.
> GATE VERDICT: the repro PASSES — the 2026-09-15 hang does NOT reproduce on
> the current versions (pi-acp 0.0.33 + pi 0.87.0, `agent-client-protocol`
> 2.1.0, tokio 1.53.1, async-io 2.6.0): two concurrent real ACP sessions on
> ONE multi-threaded runtime (2 workers) both answer `send_prompt` in well
> under 1 s. The hang is resolved — the orchestrator goes straight to Task 4
> (the "resolved" conclusion: lift the cap). Tasks 2-3 are NOT run.
> ```
>
> *Original decision (2026-09-16), retained for the record:*

On 2026-09-15 we reproduced (real-app topology, live) that two *concurrent* ACP sessions on one tokio runtime hang `send_prompt` for 60+ seconds, with the suspected cause at the SDK/async-io level (two long-lived per-connection transport tasks sharing one global async-io reactor). Root cause was never confirmed and the diagnostic repro was deleted. For the Spaces work we decided to cap the app at **one live Session at a time**: `start_session`/`resume_session` closes any live session first (new `replaced` close-reason) instead of running two sessions in parallel.

**Considered Options**

1. Allow concurrent live sessions and root-cause + fix the hang in the same body of work. Rejected: the suspected layer is a third-party crate stack; the fault-fix is unbounded effort and could block the entire Spaces feature behind it.
2. Allow concurrent sessions and accept the risk (bet the wave-1 lock redesign happens to have fixed it). Rejected: the 2026-09-15 repro is wall-clock evidence and it was never re-verified.
3. **Cap at one, enforced in the app (chosen)**: sidesteps the unverified bug with zero schema cost.

**Consequences**

- Opening a live session while another is live auto-suspends it; the affected Space shows a "Paused — a conversation started elsewhere. Resume to reconnect" banner. Offered as a data change: if the SDK hang is later fixed, lifting the cap is a one-token policy flip (the schema has been verified to be multi-session-safe).
- One extra `ClosedReason` variant (`replaced`) on the `session_closed` event.
- The root-cause fix for the two-session hang remains a tracked follow-up, out of scope for the Spaces work.
