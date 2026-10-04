---
status: accepted
date: 2026-10-04
superseded-by:
---

# Sessions run in Worker processes — the desktop is a pure Supervisor

The native harness (ADR 0011/0022) ran every session — main and subagent — as `tokio::spawn` tasks in the single Tauri app process, with `panic = "abort"` in the release profile: a Rust panic in ANY task aborts the whole app (observed 2026-10-03: a panic during an 8-subagent fan-out killed the app, with zero logging to diagnose it). **Decision: every session runs in its own child OS process (a Worker process) — one binary, self-exec `archimedes --worker` — running the existing `AgentLoop` unchanged over a stdio JSONL protocol; the Tauri app process becomes the Supervisor: UI + worker lifecycle + sole SQLite writer (persisting from the event stream) + the response side of the permission/interactive gates.** A Worker crash degrades to a *stalled* (resumable) session or a `SubagentOutcome::Failed` tool error — never an app death. `panic = "abort"` is retired (→ `unwind`), and both processes get panic hooks writing `~/.local/share/archimedes/crash-<ts>-*.log`.

ADR 0022 stands: the Worker is the desktop's OWN binary running the OWN harness — no external pi process comes back (the pi-archimedes repo stays dead). ADR 0020 is unaffected (Agent definitions still resolve; the child just runs in a Worker).

## Considered Options

- **A — Task-level containment only (rejected as the answer, adopted as a complement).** `panic = "unwind"` + `catch_unwind` around session tasks + a crash-logging panic hook, keeping one process. Rejected as the *answer*: it contains the panic but not the memory — a leaky or wedged session still takes the UI down, and there is no per-session resource boundary. The containment pieces are still adopted (a Worker panic stays in the Worker; both processes log crashes).
- **B — Subagents out, main session stays in-process (rejected).** The main session is the interactive one — its panics would still kill the app, and the app would still die from a leak in the session the user is typing into. Rejected: the user chose full isolation (a harness's UI must be leak-proof).
- **C — Chosen: everything out — the desktop is a pure Supervisor.** Every session (main + subagent) in its own Worker process. Accepted costs: the session driver, resume, config options, and the interactive/permission round-trips all cross IPC (µs per line — not a practical latency risk); the in-process native driver is deleted (no dual-mode fallback — a Worker spawn failure is a visible session error with retry).

## Consequences

- **The throwaway-DB machinery is deleted** (`open_throwaway_db`, `TempFileGuard`, the FK dance) — the Worker has no DB at all (its `SessionStore` is a no-op; the conversation is in-memory). The Supervisor persists ALL transcripts from the event stream, including **subagent transcripts as hidden ephemeral session rows** (never archived, hidden from Space lists, inspectable in a debug view — previously discarded, which is why a 32-minute subagent's work was un-inspectable after the fact).
- **Resume re-hydrates via the Supervisor**: the Supervisor reads `native_messages` and sends the transcript in the `start` envelope (the re-read moves from harness to Supervisor; the resume *semantics* are unchanged).
- **The permission gate and interactive channel cross the boundary**: the Worker's request side sends `permission-request` / `interactive-request` IPC; the Supervisor relays to the existing Tauri events (the **frontend is unchanged**) and forwards the user's answer back. The trust flag travels in the envelope + `config` updates; `sudo_exec` executes in the Worker (the password lives in the Worker's memory for the session, replacing the desktop-side `CachedPassword` — same trust boundary: same machine, same user).
- **A new UI state: the Stalled session** — a session whose Worker died: transcript intact (persisted as events arrived), last turn incomplete (no `agent_settled` = incomplete), a banner with a Resume action (fresh Worker + re-hydrate).
- **The UI process is I/O-bound only** — a CPU-heavy subagent can no longer starve the UI; N parallel sessions are parallel at the OS level, not the thread level.
- **`panic = "abort"` is retired from the release profile** (→ `unwind`) — the release binary grows (unwind tables) and a Worker panic is contained to the Worker; both processes get crash-logging panic hooks (the app's first logging of any kind).
- **Cross-platform**: process-group kill = `setpgid` (unix) / job objects (Windows) — the same treatment the existing `RealSudoRunner` has.
- **Rollback is a git revert of the feature** — the in-process driver comes back; the crash-logging/`unwind` profile changes are kept either way (they are strictly an improvement).
