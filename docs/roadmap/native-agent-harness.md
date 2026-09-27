---
status: approved
done-when: A native session (the 'Archimedes' agent) runs a full conversation in-process — calling an OpenAI-compatible model, executing tools (bash/edit/write/read/find/grep/ls + the suite tools) in-process with the permission gate, persisting to SQLite, and resuming from SQLite, with cost + thinking + compaction working — AND external (pi) sessions execute their built-in tools in the desktop (Linux) over the bridge.
---

# Native Agent Harness — the desktop becomes the Agent harness

## Context & motivation

ADR 0009 established the path to "tools live in the desktop" (Phases 1–3) and sketched an end-state where the bridge retires and pi becomes "a pure brain." This design pulls that end-state **forward and generalizes it**: the desktop doesn't just own the *tools* — it owns the **entire Agent harness** (the model loop, session persistence, and provider integration), so that pi becomes an *optional external agent* rather than a required harness. The user's stated goal: *"bring everything we don't have into Archimedes, as we will eventually write the remaining part of the agent harness."*

Two verified constraints shape the design:

- **The desktop does not own pi** — `pi-coding-agent` is a pinned third-party npm dependency (the desktop is a *consumer*, not an owner). Its RPC surface is fixed (33 commands, 9 extension-UI methods, no generic request/response), so the harness **cannot extend it**. The only generic agent→client channel is the **bridge**.
- **The user's models are all OpenAI-compatible** (`tama` + `OpenRouter`), so a single Rust OpenAI-compatible provider client covers the entire current model set.

## Terminology (captured in `CONTEXT.md`)

- **Agent harness** (new): the machinery that runs an agent conversation end-to-end — the model loop (model call → tool dispatch → retry/compaction) + tool registry + session persistence + provider integration. pi is one harness (external Node process); the desktop's native runtime is another (in-process Rust).
- **Native session** (new): a Session whose harness is the desktop's in-process Rust runtime — no spawned process, no bridge, no pi.
- **External session** (new): a Session whose harness is a spawned external agent process (today: pi, `pi --mode rpc`).
- **Session** (generalized): one live conversation, backed by exactly one **Agent harness** (not "exactly one spawned subprocess").
- **Agent** (generalized): the conversation *partner*, embodied either as a spawned external process (pi) or in the desktop's in-process harness.

## End-state architecture

```
The desktop (Tauri 2)
├── Frontend (React 19/TS) — event contract UNCHANGED
└── Rust backend
    ├── SessionDriver (existing, generalized)
    │     ├── External session → PiRpc (stdio JSONL) + bridge (socket)  [existing]
    │     └── Native session   → AgentLoop (in-process tokio task)      [NEW, Phase 2]
    ├── Harness (NEW, Phase 2)
    │     ├── AgentLoop    — one per native session: prompt queue + model→tool→retry/compaction loop + event emission
    │     ├── Provider     — OpenAiCompatibleProvider (reqwest + serde + streaming SSE)
    │     ├── ToolRegistry — Rust executors: bash, edit, write, read, find, grep, ls, powershell  +  ask, sudo_exec, manage_todo_list, subagent
    │     ├── SessionStore — SQLite (the desktop's existing store): messages/entries; resume = replay from SQLite
    │     ├── Compactor    — context-usage tracking + a summary model call
    │     └── RetryPolicy  — exponential backoff on 429/5xx
    ├── Permission gate (existing) — trusted-space auto-approve; the harness calls it in-process
    ├── TodoStore / cost / sudo password cache (existing, in-process)
    └── SubagentSessionManager (existing, generalized) — spawns Native OR External subagent sessions
```

1. **The desktop is the harness** — a Native session is driven by the desktop's own AgentLoop (a tokio task); no subprocess, no bridge, no pi.
2. **Two session kinds, one driver, identical events** — the SessionDriver generalizes; both kinds emit the SAME normalized event shapes (frontend unchanged); a session's kind is fixed at creation (from the registry entry).
3. **The registry gains a kind** — `AgentEntry` gets `kind: external | native`; a new built-in 'Archimedes' native agent entry sits beside the pi entry.
4. **End-state inventory** — remains: pi (optional External agent), the bridge (tool round-trip for External sessions; dead code later), the permission gate, the todo/cost stores. Retires: the pi-archimedes suite, the gate extension for native sessions, ACP (already gone).
5. **Trust boundary** — External: bridge peer-verification + gate. Native: no cross-process boundary (the harness is the desktop's own code; tools sandboxed to the Space's cwd; permission gate in-process).

**Decisions recorded:** ADR 0011 (desktop is the harness, in-process — over sidecar and over pi-as-model-client), ADR 0012 (v1 config seeds from pi's existing config, no new config surface).

## Phase 1 — the tool move (built-in tools → desktop)  *[= ADR 0009's Phase 3]*

- **Mechanism (Gondolin pattern):** spawn pi with `--no-builtin-tools`; extend the existing `tools.ts` override to re-register the built-ins (same name/label/description/parameter schema) with `execute()` = a bridge round-trip to the desktop.
- **Rust tool executors** (built here, **reused in Phase 2**): `bash` (tokio::process, stream, kill-on-abort, timeout), `read`, `write`, `edit`, `find`/`grep` (shell out to `fd`/`ripgrep`), `ls`. (`powershell` deferred to Phase 2 — Windows-only + bridge is Linux-only.)
- **Gating model (gate = permission, override = execution — orthogonal):** `bash`/`edit`/`write` gated (Permission prompt / Trusted Space auto-approve, ADR 0010) then executed in the desktop; `read`/`find`/`grep`/`ls` executed in the desktop directly. `gate.ts` unchanged.
- **Bridge handler:** a new `tool_exec` method (`{tool, params}` → dispatch → result); wires the tool's AbortSignal → `cancel()` (EOF) → abort in-flight execution.
- **Result transparency:** the LLM sees only `content`; `details` is UI-only (the desktop populates it to match pi's renderers).
- **Platform constraint:** the bridge is **Linux-only** → the external tool move is Linux-only (consistent with the Phase 2 suite tools); on Windows/macOS the override is inert (pi's built-ins remain, no regression). The native harness (Phase 2) is platform-agnostic.

## Phase 2 — the native harness (in-process loop)  *[= ADR 0011]*

- **AgentLoop** (a tokio task per native session): owns the control flow; runs model→tool→retry/compaction; prompt queue (steering/followUp); abort; emits the normalized events; persists to the SessionStore.
- **Provider** (`OpenAiCompatibleProvider`): `complete(messages, tools, options) → stream`; an OpenAI-compatible chat-completions client (reqwest + SSE): streaming chunks, tool calls (accumulate `tool_calls` deltas), `usage` (best-effort), a reasoning field (→ `thinking_delta`, per-model, best-effort), auth (API key) + base-URL (per model); 429/5xx → RetryPolicy.
- **Model catalog:** the desktop's model list (id, provider, context window, cost, capabilities, thinking support); sources: the provider's `/models` endpoint + a user-pinned list; **seeded from pi's config** (ADR 0012: `settings.json`, `auth.json`, `models-store.json` — degrades gracefully if absent).
- **ToolRegistry (native):** the Phase 1 Rust executors + the suite tools + `powershell` (Windows); dispatch is **in-process** (no bridge); the harness gates (permission) before a mutating tool; `subagent` spawns a native/external subagent session.
- **SessionStore (SQLite):** the native session's messages/entries persist to the desktop's existing SQLite; **resume = replay from SQLite** (no pi session file).
- **Compactor:** track context usage; when over a threshold (pi's `reserveTokens`/`keepRecentTokens` heuristic), summarize older messages via a model call, keep recent; emits `compaction_*`.
- **RetryPolicy:** exponential backoff on 429/5xx/timeout (bounded); emits `auto_retry_*`.
- **Event normalization + config + cost:** the AgentLoop emits the SAME normalized event shapes as external sessions (frontend unchanged); config options (model, thinking level) are **desktop-sourced**; cost is **desktop-computed** (model price × usage).

## Retirement + ADR 0009 correction

- **The bridge (retained, then removed later):** it is the **only generic agent→client channel** (pi's RPC has no generic request/response; the 4 stdio dialogs can't carry arbitrary tool results). **This corrects ADR 0009's Phase-3 claim "the bridge retires":** the bridge is **kept** as the tool round-trip channel for External sessions; it becomes **dead code** only once native sessions replace them, and is **removed in a later cleanup** (not the tool-move phase). The redundant ambient traffic (cost/todo pushes, agent state) **retires now** (already on stdio).
- **The pi-archimedes suite (retires):** all its tools are desktop-executed; it is no longer loaded/shipped; its packages are deprecated.
- **The gate extension (external-only):** a native session has no pi process → the harness gates **in-process**; an External session still loads `gate.ts` (unchanged).
- **Net effect:** the desktop's hard dependency on pi **narrows to External sessions only**; a native session depends on **no** pi artifact; pi becomes an **optional External agent**.

## Frontend, registry, default agent

- **Registry:** `AgentEntry` gains `kind: external | native`; the pi entry becomes `kind: external`; a new built-in **'Archimedes' native agent** entry (`kind: native`) carries a harness config (provider + default model + enabled tools + default thinking level), not a spawn spec.
- **Frontend:** a native session renders **identically** to an external session (same event shapes); the ONLY new UI element is the native agent in the picker; the model picker is catalog-driven; cost is desktop-computed; thinking level is desktop-sourced. **No new UI surface.**
- **Default agent (v1):** **pi remains the default; the native agent is opt-in** (a selectable entry). Flip the default to native once proven (a later decision).

## Phasing + risks

- **Phase 1 (this roadmap) = ADR 0009's Phase 3 (the tool move):** Linux-only; independently shippable.
- **Phase 2 (this roadmap) = ADR 0011 (the native harness):** platform-agnostic; independently shippable (reuses the Phase 1 executors).
- **Phase 3 (later cleanup):** remove the bridge + deprecate the pi-archimedes suite.
- **Risks:** (1) provider API drift (contract-test against a known server); (2) tama's tool-call support (spike; if unsupported, tama models can't use tools in v1); (3) in-process crash (mitigated by tokio task isolation + Rust memory safety); (4) usage/cost accuracy (best-effort); (5) thinking mapping (per-model, best-effort); (6) compaction quality (heuristic may need tuning); (7) config-seeding fragility (degrades gracefully); (8) scope (tight phasing avoids big-bang).

## References

- ADR 0009 (RPC replaces ACP; the tool-move mechanism + the corrected bridge claim)
- ADR 0010 (Trusted Space — desktop-side permission enforcement)
- ADR 0011 (the desktop is the Agent harness, in-process)
- ADR 0012 (native harness v1 config seeds from pi's config)
- `docs/research/pi-rpc-replaces-acp.md` (F5: the Gondolin delegation pattern + the `*Operations` seams; F1: the fixed RPC surface)
