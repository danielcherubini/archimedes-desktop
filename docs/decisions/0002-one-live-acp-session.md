---
status: accepted
date: 2026-09-16
superseded-by:
---

# Cap live ACP sessions at one app-wide (v1)

On 2026-09-15 we reproduced (real-app topology, live) that two *concurrent* ACP sessions on one tokio runtime hang `send_prompt` for 60+ seconds, with the suspected cause at the SDK/async-io level (two long-lived per-connection transport tasks sharing one global async-io reactor). Root cause was never confirmed and the diagnostic repro was deleted. For the Spaces work we decided to cap the app at **one live Session at a time**: `start_session`/`resume_session` closes any live session first (new `replaced` close-reason) instead of running two sessions in parallel.

**Considered Options**

1. Allow concurrent live sessions and root-cause + fix the hang in the same body of work. Rejected: the suspected layer is a third-party crate stack; the fault-fix is unbounded effort and could block the entire Spaces feature behind it.
2. Allow concurrent sessions and accept the risk (bet the wave-1 lock redesign happens to have fixed it). Rejected: the 2026-09-15 repro is wall-clock evidence and it was never re-verified.
3. **Cap at one, enforced in the app (chosen)**: sidesteps the unverified bug with zero schema cost.

**Consequences**

- Opening a live session while another is live auto-suspends it; the affected Space shows a "Paused — a conversation started elsewhere. Resume to reconnect" banner. Offered as a data change: if the SDK hang is later fixed, lifting the cap is a one-token policy flip (the schema has been verified to be multi-session-safe).
- One extra `ClosedReason` variant (`replaced`) on the `session_closed` event.
- The root-cause fix for the two-session hang remains a tracked follow-up, out of scope for the Spaces work.
