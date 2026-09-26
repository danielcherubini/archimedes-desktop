---
status: accepted
date: 2026-09-24
superseded-by:
---

# The Rust core speaks pi's RPC mode natively; ACP is removed

The desktop's Rust core spawns `pi --mode rpc` (one process per session, the agent registry's `command` + the per-agent config flags) and speaks pi's **JSONL-over-stdio RPC protocol** directly — the third-party `pi-acp` adapter and the `agent-client-protocol` crate are removed from the dependency graph.

**Supersedes:** ADR 0001 (ACP is the only internal protocol), ADR 0002 (one live ACP session — the one-live policy was already lifted; recorded here for the record), ADR 0005 (the per-dispatch `PI_ACP_PI_COMMAND` wrapper — the config flags are now passed directly to `pi --mode rpc`). ADR 0004 (the subagent worker runtime) and ADR 0006–0008 are unaffected.

**Why:** the evidence in `docs/research/pi-rpc-replaces-acp.md` (verified against `@earendil-works/pi-coding-agent@0.87.1` = npm latest on 2026-09-24):

1. **The third-party adapter is lossy in 17 areas** — cost metrics, todos, subagent visibility, thinking on resume, masked stopReasons, and more are dropped or mangled by the adapter's ACP mapping. Speaking pi's own protocol natively recovers all of it (the `cost_update` / `thinking_*` / `tool_execution_*` frames the desktop already consumes).
2. **A third-party dependency is removed** — the adapter is a separate crate with its own release cadence; the RPC surface is owned by the same project as the agent, and the desktop can contract-test it on upgrades.
3. **The macOS interactive gap closes** — the permission dialog rides the RPC's `extension_ui_request` subprotocol over stdio (the bundled gate extension's `ctx.ui.confirm`), which works where the bridge is fail-closed (macOS has no `SO_PEERCRED`).
4. **The end-state is expressible in pi, not in ACP** — `registerTool` same-name override + `--no-builtin-tools` makes "tools live in the desktop, not the agent" natively achievable (Phase 2/3 below), which ACP cannot express for arbitrary agents.

**Consequences**

- **The RPC surface is stable-but-unversioned.** Pin the pi version; contract-test on upgrades — the `rpc_flow` integration tests (against `fake_pi`) + a real-`pi` manual wire smoke are the wire checks (the `fake_pi` test double is written from the same table as the client, so it cannot catch a systematic casing error; the `.d.ts` files are the authority).
- **`prompt` success ≠ completion.** A `prompt` response is the preflight (start-of-turn) ack; the turn resolves on the `agent_settled` event (a `cancel_requested` flag maps a late settle to `Cancelled`). The desktop's `send_prompt` awaits the settle, not the response.
- **The permission model is the bundled gate extension** — `gate.ts` (embedded via `include_str!`, self-gated on `PI_ARCHIMEDES_GATE=1`) hooks `tool_call` for `bash`/`edit`/`write`/`sudo_exec` and answers via `ctx.ui.confirm` → `extension_ui_request` → the desktop's `PermissionPrompt` (allow/reject; a rejection is a `block` with a reason). ADR 0003's double-prompt is now real for `sudo_exec` (gate confirm → bridge confirm modal → bridge password modal).
- **The bridge is UNCHANGED in Phase 1** — it is the suite's own channel (cost pushes, `ask`, subagent dispatch) and becomes redundant only in Phase 3.
- **`resume` reads the pi session file from the stored `capabilities_json`** (`piSessionFile` → `--session <path>`; `loadSession: true` when the file is present). Legacy ACP rows are history-only: the frontend's Resume button is gated on the normalized `loadSession: false`, and `resume_session` returns `NotResumable` for them.
- **Accepted Phase-1 parity losses:** tool-call diffs (no `content: ToolCallContent[]` frames — the desktop-side tool executor that would produce them is Phase 3) and thinking blocks on resume if `get_messages` doesn't return them (gap G2 in the research report).

**Phases**

- **Phase 1 (this decision):** the swap — the RPC client (`PiRpc`), the session driver, the gate extension, direct subagent spawns, ACP deletion.
- **Phase 2 (done):** the suite's `ask` / `sudo_exec` / `manage_todo_list` execute in the desktop (the agent's copies are thin delegates). The desktop ships a `tools.ts` override (embedded via `include_str!`, written to `config_dir/pi-tools/tools.ts`, injected with a SECOND `-e` arg) that re-registers the three tools with the SAME name + label + description + parameters schema but `execute()` = a desktop round-trip over the bridge. The override is **self-gated on Linux + the bridge env** (`process.platform !== "linux"` first — the bridge is available on Linux **and** Windows, but the listener is a no-op on Windows, so the platform check keeps the override inert on Windows **and** macOS; the suite's tools remain on both, no regression). `subagent` is NOT overridden (already desktop-executed; overriding it would break named-agent / parallel / async dispatch). `gate.ts`'s `GATED` set dropped `sudo_exec` (the desktop's `sudo_exec` handler is the single confirm — a gate confirm + a desktop confirm would double). The mechanism is **deferred `session_start` registration**: a load-time same-name registration is a `process.exit(1)` conflict (`DefaultResourceLoader.addExtensionConflictDiagnostics`); a deferred registration is absent at load-finalization → no conflict, and the override still wins (the CLI `-e` loads first + the tool merge is first-wins, `registerTool` auto-calls `runtime.refreshTools()`). The desktop's `sudo_exec` / `todo_update` bridge handlers own the confirm + password modals + the `TodoStore`; the `ask` handler is the generic `bridge-request` path (the override shapes the raw `AskResponsePayload`). The end-to-end contract is proven by the `desktop_tools` listener-level e2e (`fake_pi` `FAKE_PI_TOOLS` wire simulation) + the `tools_selfgate` vitest check (the self-gate with/without the env).
- **Phase 3:** the built-in tools move too (`--no-builtin-tools` + re-registered delegated tools — the Gondolin pattern; the `*Operations` seams, e.g. `dist/core/tools/bash.d.ts:24-27`) and the bridge retires (one channel, stdio, replaces two).
