---
status: approved
done-when: The `archimedes` UI process runs no `AgentLoop` — every session (main + subagent) runs in a self-exec `archimedes --worker` child process over the stdio JSONL protocol; a Worker crash degrades to a stalled (resumable) session or a `SubagentOutcome::Failed` tool error, never an app death; a crash writes `~/.local/share/archimedes/crash-<ts>-*.log`; all validations green (cargo test / clippy 0 warnings / fmt / pnpm test / pnpm build)
---

# Sessions run in Worker processes — the desktop is a pure Supervisor

## Context

The desktop's native harness (ADR 0011/0022) runs every session — main and subagent — as `tokio::spawn` tasks in the single Tauri app process. The release profile is `panic = "abort"`, so a Rust panic in ANY task aborts the whole app: on 2026-10-03 a panic during a 8-subagent fan-out (local Qwen3.8 inference, one subagent mid-scan) killed the app at 18:45 (core dump: `core.archimedes.1000.…2145078…`), and the app has no logging at all — the core dump was the only evidence.

A harness fans out parallel subagents and runs long work; one bug must not take the app down. Decision (2026-10-04): **process-level isolation for everything** — every session runs in its own child OS process (a **Worker process**); the Tauri app process becomes the **Supervisor** (UI + worker lifecycle + sole SQLite writer + the response side of the permission/interactive gates). The Worker is the desktop's own binary (`archimedes --worker` — self-exec) running the existing `AgentLoop` unchanged; ADR 0022 stands (no external pi harness — the pi-archimedes repo stays dead).

## 1. Process topology

- **One Worker per session; Worker lifetime = session lifetime.** A main session's Worker is spawned at session start (or resume) and reaped at session end / app quit. A subagent's Worker is spawned per dispatch, reaped after `agent_settled` + `close`. No pool (v1).
- **One binary, self-exec:** `archimedes --worker` — the Tauri binary doubles as the Worker (the flag is parsed *before* Tauri init; the Tauri runtime never starts in Worker mode). No new binary, no new install surface.
- **The Supervisor** = the Tauri app process: React UI + WorkerManager (spawn / reap / crash-detect / abort-all) + sole SQLite writer + provider catalog + the response side of the permission/interactive gates.
- **App quit / Supervisor death:** the Supervisor sends `close` to all Workers (grace → process-group SIGKILL — a Worker's in-flight `bash` children die with it). A Worker that sees stdin EOF exits cleanly.
- **No dual-mode fallback:** a Worker spawn failure = a visible session error with retry. The in-process native driver is deleted in the same release.

## 2. The IPC protocol (stdio JSONL)

One line = one JSON message; the Supervisor holds the Worker's stdin/stdout. ~10 message types:

| Direction | Message | Purpose |
|---|---|---|
| W→S | `ready { version, session_id }` | Handshake (the Supervisor waits for it, with a timeout, before `start`) |
| W→S | `RpcEvent` (the existing ~40-variant vocabulary, verbatim) | The session's event stream |
| W→S | `permission-request { id, tool, args }` | The permission gate (non-trusted Space) |
| W→S | `interactive-request { id, kind }` | `ask` / `sudo_exec` (confirm + password) |
| W→S | `worker-error { code, message, backtrace? }` | Structured crash report before exit |
| S→W | `start { session_id, mode: fresh|resume, transcript?, model, thinking, providers, trusted, tools, … }` | Session-start envelope — `resume` carries the re-hydrated transcript; carries the resolved provider config (incl. API keys) — the Worker never reads `settings.json` |
| S→W | `prompt { text, attachments? }` | A user message / subagent task |
| S→W | `config { model?, thinking?, trusted? }` | Config-option changes (the existing `ControlCmd` path) |
| S→W | `abort` | Stop the current turn (the session stays alive) |
| S→W | `close` / `permission-response { id, ok }` / `interactive-response { id, … }` | Teardown / gate resolutions |

Semantics: round-trips carry a `reqId` (the Supervisor correlates); events are fire-and-forget; a single stream = total ordering (the Supervisor routes events to the session's Tauri sink in arrival order). Raw model SSE and tool execution never cross the wire. `ready` carries the binary version (skew is impossible — self-exec only); the `RpcEvent` vocabulary is already permissive of unknown event types.

## 3. Component inventory

- **In the Worker (unchanged harness):** `AgentLoop` (the model → tool → retry/compaction loop), the `Provider` clients (all 3 wires, ADR 0024 — direct HTTP from the Worker), the tool executors (`read`/`bash`/`edit`/`write`/`mcp`/`skills`/…), the permission gate + interactive channel (request side), prompt building + skill/agent-definition/MCP discovery from disk (the Worker has `cwd` + config dir from the envelope), the `RealSudoRunner` (the Worker executes the privileged command; the password lives in the Worker's memory for the session, replacing the desktop-side `CachedPassword`). The Worker's `SessionStore` is a **no-op** — it keeps the conversation in memory.
- **In the Supervisor:** the Tauri command surface (frontend **unchanged** — same commands, same events), SQLite (**sole writer** — persists `messages` + `native_messages` from the event stream; events carry full `Value` messages), the provider catalog + `GET /models` discovery (sends `config` on settings edits; trust toggles → `config { trusted }`), the WorkerManager, the event router, the permission/interactive response side (the existing `permission-request` / `interactive-request` events + `respond_*` commands — the Supervisor just relays answers back over IPC).
- **Subagent transcripts are persisted** to the main DB as hidden ephemeral session rows (never archived, hidden from Space lists, inspectable in a debug view) — the throwaway-DB machinery (`open_throwaway_db`, `TempFileGuard`, the FK dance) is **deleted**. Subagents remain ephemeral (no resume; the recursion guard is unchanged — a child gets no `subagent`/`list_agents`, so the process tree is ≤ 2 levels).

## 4. Session lifecycle semantics

- **Start/resume:** spawn → wait for `ready` (timeout → failed dispatch) → `start` (a `resume` envelope carries the Supervisor-rehydrated transcript from `native_messages` — the existing resume semantics, the re-read moved from harness to Supervisor).
- **Stop:** `abort` → the Worker cancels the current turn (the existing `turn_cancel`); in-flight `bash` children die via the existing process-group kill (inside the Worker). The session stays alive.
- **Close:** `close` → the Worker exits 0.
- **Crash:** the process exits without `close` (optionally after `worker-error`) → the session is marked **stalled** (a new UI state: a banner — *"Session stopped unexpectedly at <time> — last turn incomplete. [Resume]"* — with the crash-log path when one exists). A stalled session's last turn is incomplete (no `agent_settled` = incomplete — the existing settle semantics); the transcript is intact (persisted as events arrived). **Resume** = spawn a fresh Worker + re-hydrate. A crashed *subagent* Worker = `SubagentOutcome::Failed { error }` for the parent agent — the fan-out continues.
- **Durability:** events persist as they arrive → a crash loses at most the un-persisted tail of the in-flight turn.

## 5. Observability

- `panic = "abort"` is **retired** from the release profile (→ `unwind` — a Worker panic stays in the Worker).
- **Crash logs (both processes):** a panic hook writes `~/.local/share/archimedes/crash-<ts>-<session>.log` (panic message + location — survives `strip` — + a captured backtrace); the Worker additionally emits `worker-error` before exiting.
- **Lifecycle/protocol log (opt-in):** `ARCHIMEDES_DEBUG=1` → the Supervisor logs worker lifecycle (spawn/ready/crash/reap) + the IPC protocol to `~/.local/share/archimedes/supervisor.log` (rotated).
- **The stalled-session UI** (Section 4) is the user-visible half.

## 6. Naming, ADR, migration

- **ADR 0025** — "Sessions run in Worker processes; the desktop is a pure Supervisor." ADR 0022 stands (no external pi harness — the Worker is our own binary running our own harness; the pi-archimedes repo stays dead). ADR 0020 is unaffected (Agent definitions still resolve; the child runs in a Worker).
- **CONTEXT.md:** *Native session* / *Subagent session* / *Agent harness* / *Client* re-pointed at the Worker/Supervisor framing; new terms: **Worker process**, **Supervisor**, **Stalled session** (a session whose Worker died — transcript intact, resumable; distinct from archived/paused).
- **Migration:** none (schema unchanged; ephemeral subagent rows are a new row shape, backward-compatible). Cross-platform: process-group kill = `setpgid` (unix) / job objects (Windows) — the same treatment the existing `RealSudoRunner` has.
- **Known risk (accepted):** IPC latency on the interactive session — JSONL over stdio is µs per line; a token stream is at most a few hundred lines/s. Not a practical risk. Long-lived Worker memory: a future "recycle on idle" is the mitigation if a leak appears (the v2 pool option).

## 7. Test strategy

- The existing harness tests survive (the `AgentLoop` is unchanged).
- New: protocol round-trip tests (a mock Worker speaking JSONL; a mock Supervisor driving a real in-process `AgentLoop` — the existing `tauri::test` pattern); Supervisor lifecycle tests (spawn/ready-timeout/crash/resume/reap-all/app-quit); event→persistence tests (the Supervisor's persist path, incl. subagent ephemeral rows); one **real-binary integration test** (spawn the actual `archimedes --worker`, drive a canned provider via the existing wiremock pattern, assert the event stream + the persisted transcript).
- Validation per AGENTS.md: `cargo test` + `cargo clippy --all-targets` (0 warnings) + `cargo fmt --check` in `src-tauri/`; `pnpm test` + `pnpm build` at the repo root.

## 8. Related fix (separate, small — not part of this architecture)

The 2026-10-03 18:45 crash (core dump `core.archimedes.1000.…2145078…`) is a panic in the running harness; the exact location is being symbolized from the core (the forensics build is in its final LTO link). Whatever it is (leading suspects: the SSE/JSON parsing path under a long local-model stream), it is a **separate bug fix** on top of this architecture — which makes that class of bug non-fatal by construction.
