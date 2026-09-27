---
status: accepted
date: 2026-09-27
superseded-by:
---

# The desktop is the Agent harness — native sessions run in-process

ADR 0009 established the path to "tools live in the desktop" (Phases 1–3) and sketched an end-state where the bridge retires and pi becomes "a pure brain." This decision pulls that end-state forward and generalizes it: the desktop doesn't just own the *tools*, it owns the **entire Agent harness** — the model loop, session persistence, and provider integration — so that pi becomes an *optional external agent* rather than a required harness. A **Native session** is a Session whose harness is the desktop's own in-process Rust runtime; an **External session** is one whose harness is a spawned agent process (pi).

**Decision:** the desktop runs native agent sessions **in-process** — each native session is a tokio task in the Rust backend (the `AgentLoop`), and the desktop implements the model client **directly** (an OpenAI-compatible Rust provider). A native session therefore has **no dependency on pi at all** — no subprocess, no bridge, no pi.

**Considered Options**

- **A sidecar harness process** (a standalone Rust binary per native session, driven over a minimal control protocol): rejected. (a) *State co-location* — the harness needs the todo store, permission gate, sudo password cache, cost store, SQLite, and the subagent manager, all of which the desktop already owns in-process; a sidecar would need a second IPC channel to reach them (the "bridge problem" all over again). (b) *No second binary* — a sidecar adds a standalone binary to the Tauri bundle (cross-platform distribution complexity). (c) *Crash isolation is already free* — a panic in a tokio task aborts only that task, not the process, so per-session isolation is provided by the runtime. (d) The desktop already runs all surrounding machinery in-process (bridge listeners, subagent manager, permission gate, todo store); the harness is the same class of workload.
- **pi as the model client** (the desktop runs the loop, but each model turn round-trips to a spawned pi in an inference-only mode): rejected. pi has **no inference-only mode** — RPC mode is a full session with its own loop; `pi -p` is one-shot, non-streaming, and has no tools. Adding a mode requires **owning pi**, which the desktop does not: `pi-coding-agent` is a pinned third-party npm dependency (the desktop is a *consumer*, not an owner), and its RPC surface is fixed (33 commands, 9 extension-UI methods) — the desktop cannot extend it.

**Consequences**

- **The Tauri process gets heavier** — it now runs the agent loop. Mitigated by tokio's per-task isolation (a session's panic aborts only that task) + Rust's memory safety.
- **The desktop owns the model client** — a new dependency surface (an OpenAI-compatible Rust provider: reqwest + SSE streaming). v1 scope is OpenAI-compatible only, which covers the user's entire current model set (tama + OpenRouter are both OpenAI-compatible); Anthropic/Gemini/Bedrock are added later as needed.
- **The trust boundary tightens** — a native session has no cross-process boundary; the harness is the desktop's own code, tools are sandboxed to the Space's cwd (the existing `FsBackend`), and the permission gate is in-process. The boundary moves from "a peer-verified socket descendant" to "the desktop's own code."
- **ADR 0009's "the bridge retires in Phase 3" is corrected** — the bridge is the *only* generic agent→client channel (pi's RPC has no generic request/response: 9 fixed extension-UI methods, 4 of which are dialogs that cannot carry arbitrary tool results). So the bridge is **retained** as the tool round-trip channel for *External* sessions; it becomes dead code only once native sessions replace them, and is removed in a later cleanup — not in the tool-move phase.
- **The pi-archimedes suite retires** — all of its tools (`ask`/`sudo_exec`/`manage_todo_list`/`subagent`) are desktop-executed in the end-state; the suite is no longer needed.
- **`Agent` and `Session` generalize** (CONTEXT.md) — a Session is "backed by exactly one Agent harness," not "exactly one spawned subprocess"; a native session's Agent is embodied in the desktop's in-process runtime, not an external process.
