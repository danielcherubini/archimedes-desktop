---
status: accepted
date: 2026-09-16
superseded-by:
---

# Subagent sessions run on a dedicated worker runtime

The desktop is capped at one live Session (0002) because two *concurrent* ACP sessions on one tokio runtime hung `send_prompt` 60+ seconds (2026-09-15 repro; suspected cause at the SDK/async-io level — two long-lived per-connection transport tasks sharing one global async-io reactor; root cause never confirmed). Subagent sessions (desktop-spawned ACP sessions that run a task delegated by the main agent via the bridge) are a new concept — not user-facing Sessions — so the one-live policy excludes them by definition. But the *runtime* question is separate: where do their ACP session objects run? We decided: all subagent sessions run on a **dedicated tokio runtime on separate worker threads**; the main Session stays on the existing runtime.

**Considered Options**

1. **Dedicated worker runtime (chosen)**: bounded effort; the one-live policy code is untouched; a 2-session concurrency regression test on the worker runtime de-risks it before the rest of the feature is built on top.
2. Per-session OS thread + runtime: maximum isolation, guaranteed if the hang is per-runtime, but more lifecycle machinery for a handful of sessions — kept as the documented fallback if option 1's regression test fails.
3. Root-cause + fix the hang, lift the cap, single runtime: the right long-term end state, but unbounded third-party fault-fix effort (0002's rejection rationale); remains the tracked follow-up — lifting the cap is a one-token policy flip once it's fixed.
4. Run subagent sessions on the main runtime anyway (bet the lock redesign fixed the hang): rejected — 0002 rejected it on wall-clock evidence.

**Consequences**

- The desktop holds two tokio runtimes; subagent sessions must be created on the worker runtime (the ACP SDK session is bound to the runtime it was created on).
- If the regression test shows the hang persists on the worker runtime (i.e. the async-io reactor is process-global), the design falls back to per-session thread + runtime without re-deciding.
- The one-live policy (0002) governs user-facing Sessions only; subagent sessions never pass through the close-then-spawn path.
