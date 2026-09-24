---
status: current
last-verified: 2026-09-24
verified-by: web + local research — 5 angles, 2026-09-24 (pi v0.87.1 source-verified; ACP spec via agentclientprotocol.com)
---

# Pi RPC as the replacement for ACP in Archimedes Desktop

Research question: *Pi has a well-scoped RPC mode. How can the desktop swap ACP for it — and what does it take for the end-state where the tools live in the desktop app?*

Decision context: the user decided ACP is out of the frame. This report documents the evidence for the replacement and the end-state architecture. The Phase 1 implementation plan lives in `docs/roadmap/pi-rpc-swap.md`.

## Executive Summary

1. **Pi's RPC is a complete, well-scoped single-session protocol** (`pi --mode rpc --no-session`, strict JSONL over stdio, verified in `@earendil-works/pi-coding-agent@0.87.1`): 33 client→agent commands, ~21 agent→client events, a 9-method **extension-UI subprotocol** (4 blocking dialogs + 5 fire-and-forget), in-band cumulative usage/cost, and an authoritative `agent_settled` completion signal. It has **no permission protocol, no todo events, no multi-agent surface** — but the extension-UI subprotocol is precisely the agent→client input channel that ACP lacks and that the bridge (ADR 0003) was built to work around.
2. **The desktop's irreducible ACP surface maps cleanly onto RPC** for everything it actually uses (`initialize`/`session/new`/`session/load`/`session/prompt`/`session/cancel`/`session/set_config_option` + the 5 consumed `session/update` types). The desktop's `SessionDriver` is already structured as "generic session machinery + protocol-specific establisher closure + protocol-specific event normalization" — the swap is a replacement of the protocol layer, not a rewrite of the session model.
3. **The current pi-acp adapter (a third-party package, `svkozak/pi-acp@0.0.33` — not in the pi-archimedes monorepo) loses 17 distinct things** (cost, todos, subagent visibility, thinking on resume, real stopReasons, structured retry/compaction, native prompt blocks, diffs). The swap removes the third-party dependency and recovers all of them.
4. **The "double prompt" (ADR 0003) is effectively a single prompt for pi today**: in bridge mode the suite bypasses `ctx.ui`, and pi's built-in tools never ask permission — so the ACP generic gate never fires for pi. The swap loses a mostly-dead gate; a small bundled pi extension (`tool_call` hook → `ctx.ui.confirm`) restores a generic gate on **all platforms, fixing the macOS gap** (the bridge is fail-closed on macOS; the extension-UI subprotocol rides stdio and works everywhere).
5. **The "tools live in the desktop" end-state is natively expressible in pi**: `registerTool` same-name override + `--no-builtin-tools` is the delegation mechanism (pi's own `gondolin`/`ssh.ts` examples route all built-in tools to a micro-VM / remote machine via `*Operations` seams — we route to the desktop instead). The `tool_call` hook is gate-only (cannot supply a result); **pi has no native MCP client** (the "desktop as MCP server" variant is a dead end). The bridge becomes redundant in the end-state and retires — one channel (stdio) replaces two.
6. **Live docs verified**: pi.dev "latest" = npm latest = `0.87.1` = the installed version analyzed. Zero drift; the RPC surface is stable-but-unversioned (pin the pi version).

## Findings

### F1 — Pi's RPC surface (code-verified in v0.87.1)

**Transport** (`docs/rpc.md:15,52`; `dist/modes/rpc/jsonl.js:48`; `dist/modes/rpc/rpc-mode.js:17-24`):
- Invocation: `pi --mode rpc --no-session` (normal CLI flags apply; `@file` args rejected).
- Strict JSONL over stdio, one JSON object per record, LF-terminated. Bidirectional: commands + `extension_ui_response` on stdin; `response` + session events on stdout. Stderr = diagnostics only.
- Correlation: optional `id` on every command, echoed in the matching `response`; handling is **asynchronous** (correlate by ID, not order). `bash_execution_update` events repeat the originating `bash` command's `id`; all other events have no command ID.
- Shutdown: close stdin → `onInputEnd` → `shutdown()` disposes the runtime and exits. Failed command → one `response` with `success:false, error`.

**Client→agent commands (33)** — verified: the `RpcCommand` union (`dist/modes/rpc/rpc-types.d.ts:13-124`) and the `handleCommand` switch (`dist/modes/rpc/rpc-mode.js:295-543`) contain exactly the same set:

| Command | Params | Response `data` |
|---|---|---|
| `prompt` | `message`, `images?`, `streamingBehavior?` (`"steer"`/`"followUp"`) | none — success = accepted/queued only |
| `steer` / `follow_up` | `message`, `images?` | none |
| `abort` | — | none — waits for idle |
| `clear_queue` | — | `{steering: string[], followUp: string[]}` |
| `new_session` | `parentSession?` | `{cancelled: boolean}` |
| `get_state` | — | `RpcSessionState` (`model?`, `thinkingLevel`, `isStreaming`, `isCompacting`, `steeringMode`, `followUpMode`, `sessionFile?`, `sessionId`, `sessionName?`, `autoCompactionEnabled`, `messageCount`, `pendingMessageCount`) |
| `get_messages` | — | `{messages: AgentMessage[]}` |
| `set_model` | `provider`, `modelId` | full `Model` object |
| `cycle_model` / `get_available_models` | — | `{model, thinkingLevel, isScoped} \| null` / `{models: Model[]}` |
| `set_thinking_level` | `level` (`off`…`max`) | none |
| `cycle_thinking_level` / `get_available_thinking_levels` | — | `{level} \| null` / `{levels: ThinkingLevel[]}` |
| `set_steering_mode` / `set_follow_up_mode` | `mode` (`"all"`/`"one-at-a-time"`) | none |
| `compact` | `customInstructions?` | `CompactionResult` |
| `set_auto_compaction` / `set_auto_retry` | `enabled` | none |
| `abort_retry` | — | none |
| `bash` | `command`, `excludeFromContext?` | `BashResult` (`output`, `exitCode?`, `cancelled`, `truncated`, `fullOutputPath?`) — output streams as `bash_execution_update` |
| `abort_bash` | — | none |
| `get_session_stats` | — | `SessionStats` (message counts, token totals, `cost`, `contextUsage`) |
| `export_html` | `outputPath?` | `{path}` |
| `switch_session` | `sessionPath` | `{cancelled: boolean}` |
| `fork` | `entryId` | `{text, cancelled}` |
| `clone` | — | `{cancelled: boolean}` |
| `get_fork_messages` | — | `{messages: {entryId, text}[]}` |
| `get_entries` | `since?` (durable cursor) | `{entries: SessionEntry[], leafId: string \| null}` |
| `get_tree` | — | `{tree: SessionTreeNode[], leafId: string \| null}` |
| `get_last_assistant_text` | — | `{text: string \| null}` |
| `set_session_name` | `name` | none |
| `get_commands` | — | `{commands: RpcSlashCommand[]}` |

Key semantics (verified): `prompt`'s response means *accepted/queued/handled*, **not** that model work finished — `agent_settled` is the authoritative done signal. `new_session`/`switch_session`/`fork`/`clone` rebind the single active session (cancellable by extension `session_before_switch`/`session_before_fork` handlers). `bash` first emits a `user_bash` extension event — a handler returning `result` short-circuits local execution; otherwise `session.executeBash` runs it, and the result is injected as a user message on the next prompt (unless `excludeFromContext`).

**Agent→client events (~21)** — `AgentSessionEvent` (`dist/core/agent-session.d.ts:41-103`) + core `AgentEvent` (`@earendil-works/pi-agent-core/dist/types.d.ts:422-460`): `agent_start` / `agent_end` (`messages`, `willRetry`) / **`agent_settled`** / `turn_start` / `turn_end` / `message_start` / `message_end` / `message_update` (delta-only `assistantMessageEvent`: `text_delta`, `thinking_delta`, `toolcall_start/delta/end`, `done`, `error`; cumulative `usage` on every update) / `tool_execution_start` / `tool_execution_update` (`partialResult`) / `tool_execution_end` (`result`, `isError`) — correlated by `toolCallId` / `queue_update` / `entry_appended` / `session_info_changed` / `thinking_level_changed` / `compaction_start` / `compaction_end` / `auto_retry_start` / `auto_retry_end` / `summarization_retry_*` / `bash_execution_update` (RPC-only) / `extension_error` (RPC-only).

**Extension-UI subprotocol** (`dist/modes/rpc/rpc-types.d.ts:193-248`; `dist/modes/rpc/rpc-mode.js:52-190`; live-verified at https://pi.dev/docs/latest/rpc-extension-ui, 2026-09-24):
- **Dialogs** (request on stdout, blocks until the matching `extension_ui_response` on stdin): `select(title, options[], opts?)` → `value` | `undefined`; `confirm(title, message, opts?)` → `confirmed: true/false` | `false`; `input(title, placeholder?, opts?)` → `value`; `editor(title, prefill?)` → `value`. Optional `timeout` (ms) auto-resolves to the default — **omit `timeout` = wait indefinitely**; the client need not track timeouts.
- **Fire-and-forget**: `notify(message, notifyType)`, `setStatus(statusKey, statusText?)`, `setWidget(widgetKey, widgetLines?, widgetPlacement)`, `setTitle(title)`, `set_editor_text(text)`.
- `ctx.mode === "rpc"`, `ctx.hasUI === true`; `custom()`, `onTerminalInput()`, themes, editor components, autocomplete are no-ops/degraded (full list in the docs' Limitations section).
- Reference client + demo extension: `examples/rpc-extension-ui.ts`, `examples/extensions/rpc-demo.ts` in the pi repo.
- TS signatures (verified, `dist/core/extensions/types.d.ts`): `select(title: string, options: string[], opts?: ExtensionUIDialogOptions): Promise<string | undefined>`; `confirm(title: string, message: string, opts?: ExtensionUIDialogOptions): Promise<boolean>`; `input(title: string, placeholder?: string, opts?): Promise<string | undefined>`.

**Session model**: one process = **one active session** (rebind on switch/fork/clone). Sessions persist as JSONL at `~/.pi/agent/sessions/--<path>--/<timestamp>_<session-id>.jsonl` (v3 format, append-only entry tree, stable entry ids usable as durable cursors). Resume = spawn with `--session <path>` (no explicit resume command). `--no-session` = in-memory.

**No permission model** (verified by absence): no `request_permission`, no approval verbs, no auto-approve knobs anywhere in `dist/modes/rpc/` or `dist/core/`; built-in tools (`bash`, `edit`, `write`, `read`, `find`, `grep`, `ls`, `powershell`) execute unconditionally (`docs/security.md:3`). Gating levers: the `tool_call` extension hook (block + reason, or mutate `event.input` in place — **cannot supply a result**: agent-core consumes only `beforeResult?.block`/`.terminate`/`.reason`, `@earendil-works/pi-agent-core/dist/agent-loop.js:483-525`), the `user_bash` hook (`{result}` short-circuits; `{operations}` replaces the execution backend — but only for user `!`/RPC `bash`, **not** the LLM's `bash` tool; `dist/modes/rpc/rpc-mode.js:448-457`), tool selection at startup, and `pi.registerTool` (below).

**Stability**: no wire-protocol version field. CLI flags are version-specific (`docs/rpc.md:18`); the session file format is versioned (v1→v3, auto-migrated). Treat the RPC surface as **stable-but-unversioned**: pin the pi version, compile against the exported types, expect additive evolution.

### F2 — What the desktop actually uses from ACP (`archimedes-desktop`)

Crate: `agent-client-protocol = "2"` (`src-tauri/Cargo.toml:29`). Two distinct channels exist; only one is ACP: the ACP protocol (what the swap targets) and the **bridge** (ADR 0003 — the suite's own env-gated local socket channel, which the swap does not touch directly).

**Methods invoked** (`src-tauri/src/acp/session.rs`): `initialize` (:889, :992; `subagent.rs:282`), `session/new` (:902, `subagent.rs:295`), `session/load` (:1023-1025, gated by `agent_capabilities.load_session` at :1004), `session/prompt` (:1097; **awaits the response to resolve with `StopReason`** — the turn-completion signal), `session/cancel` (:1114-1117), `session/set_config_option` (:1149-1152). **Served**: `fs/read_text_file` + `fs/write_text_file` (cwd-sandboxed `FsBackend`, `fs_backend.rs`), `session/request_permission` (`permission.rs:63`: oneshot in `pending_permissions` keyed `"{session_id}/{request_id}"`, 300 s waiter, option-driven UI — one button per option + Cancel; the desktop is fully driven by whatever `options` arrive). `terminal/*` advertised `false`; `plan` ignored (test `sessions.test.ts:404`).

**Updates consumed** (all re-emitted as `session-update` Tauri events + persisted via `persist_update`, `session.rs:1273`): `agent_message_chunk` (accumulated per `messageId` → `kind="agent-text"` row), `agent_thought_chunk` (segmented `{messageId}#{segment}` → `kind="agent-thought"`), `tool_call` (full snapshot per `toolCallId` → `kind="tool-call"`), `tool_call_update` (shallow-merged, `merge_json` :1427), `config_option_update` (not persisted; drives the model/thinking `SessionConfigSelect` state, `sessions.ts:759,806`). `StopReason` = `end_turn | max_tokens | refusal | max_turn_requests | cancelled`.

**Session model**: ADR 0002 **superseded** (concurrent sessions OK; a session dies only on explicit close, subagent cancel, or process exit). Start = canonicalize cwd → registry lookup → bridge env (if `bridge: true`) → spawn → establisher closure. Resume = fresh spawn → `initialize` → `load_session` capability check → **clear stored rows** (`db.clear_messages_for`, :1019) → replay is authoritative. Persistence = client-owns-history in SQLite (`storage/db.rs`: `sessions` (id, agent_id, cwd, created_at, title, **capabilities_json**), `messages` (session_id, kind, message_key, payload_json, upserted as updates stream in — normalized rows, **not** raw frames), `spaces` (a Space = a folder/cwd)).

**Subagents (ADR 0004/0005)**: separate ACP sessions; `dispatch_subagent` **bridge** frame (not ACP) → `SubagentSessionManager::dispatch` (`subagent.rs:155`) spawns a fresh agent (same registry entry) + per-dispatch **launch-wrapper script** (`launch_wrapper.rs:81-101`: `exec pi <resolved flags> "$@"` via `PI_ACP_PI_COMMAND`, because pi-acp has no CLI surface for per-dispatch config) + `initialize`/`session/new`/first `session/prompt` (unbounded)/close. Dedicated `WorkerRuntime` (`worker_runtime.rs`). Ephemeral (db: `None`); capture = last-`messageId` text + accumulated `cost_update` + wall clock; no recursion; `SubagentPanel` consumes `subagent-session-started`/`subagent-closed` events.

**Registry** (`config/registry.rs:63`): default = single entry `{ id: "pi", name: "Pi", command: "pi-acp", args: [], env: {}, bridge: true }`. Env passthrough is the only mechanism for the bridge handshake and the subagent wrapper. **The desktop's irreducible ACP surface**: initialize / session/new / session/load / session/prompt (→StopReason) / session/cancel / set_config_option + config_options / request_permission (the generic gate) / fs/* (client-side backends) / the five session-update types + config_option_update.

### F3 — pi-acp fidelity: what is lost today, what changes with direct RPC

**The adapter is third-party**: `pi-acp@0.0.33` (svkozak, `/usr/local/lib/node_modules/pi-acp/dist/index.js`, 3044-line tsup bundle). **Not** in the pi-archimedes monorepo. Topology: desktop → `pi-acp` (ACP over stdio; `PiRpcProcess.spawn` at `:133-141` runs `pi --mode rpc --no-themes [--session <file>]` with `env: process.env`) → `pi`; the bridge dials desktop↔pi directly (pi = grandchild; peer verification walks ≤8 hops to the desktop pid).

**17 lossy areas** (cited in `pi-acp@0.0.33:dist/index.js`): (1) no cost frames — usage only via `get_session_stats`/`/session` text; (2) todos = `rawInput` fallback only (no `todos_clear`, no subagent columns); (3) subagent visibility collapsed to one progress blob (child internals ride a `PI_SUBAGENT_SOCKET` bus the adapter never sees); (4) `input`/`editor` UI **dropped** (`:1253-1260`); (5) `authenticate` no-op (`:2063`); (6) MCP not bridged; (7) retry/compaction de-structured to hardcoded English text (`:1166-1191`); (8) `turn_end`/`agent_end`/`agent_start` unframed; (9) **`error` stopReason masked as `end_turn`** (`:2435`); (10) resume loses thinking + tool inputs (text-only replay, `rawInput: null`); (11) slash commands round-trip as fake assistant text; (12) diffs adapter-computed (pre/post `readFileSync` snapshot; comment at `:798-801` flags it as an approximation); (13) bash "terminal" is a `_meta` hack (Zed-style); (14) tool-status monotonicity repaired in-adapter (out-of-order pi deltas); (15) audio dropped, resource blobs → byte counts, links → text; (16) one session per adapter process (`closeAllExcept`, `:2020`); (17) extension-sourced commands excluded from `available_commands_update`.

**Pivotal finding**: in bridge mode the suite **bypasses `ctx.ui` entirely** (bridge branch first — `pi-archimedes/packages/ask/src/tool.ts:236-275`, `packages/sudo/src/prompt.ts:33-36,107-109`), and pi's built-in tools never ask permission — so **the ACP generic gate of the "double prompt" effectively never fires for pi today**; the bridge modal *is* the gate.

**What the swap changes**: `pi-acp` leaves the topology (desktop → `pi` direct; bridge peer-verification shortens to 1 hop); the launch-wrapper hack disappears (desktop passes `pi` CLI flags directly); session-id anchoring collapses 3-way (desktop ↔ ACP ↔ pi) → 2-way (desktop ↔ pi, via `get_state()` + the bridge `session` push, which already carries pi's `sessionManager` refs); the `src-tauri/src/acp/` ACP layer gets an RPC driver; **the suite is protocol-agnostic and breaks nothing** (its `isBridgeMode` gate — `packages/core/src/bridge/index.ts:129-140`: 4 env vars ∧ `ctx.mode !== "tui"` ∧ no `PI_SUBAGENT_SOCKET` — is satisfied by RPC mode; the only ACP coupling is a comment, `packages/subagent/src/execute.ts:57-60`).

**What the swap gains**: in-band cost/usage; real todo semantics; subagent visibility; structured retry/compaction; honest stopReasons; native prompt blocks (no lossy translation); `set_model`/`set_thinking_level`/`compact`/`fork`/`switch_session` directly; the third-party dependency is gone. **macOS upside**: the bridge is fail-closed on macOS (no `SO_PEERCRED`), so macOS pi has degraded interactivity today; the extension-UI subprotocol rides stdio — a bundled gate extension restores interactive UX **on macOS too**.

**Risks**: the bridge env-inheritance edge case (ADR 0022 Consequences) becomes more important since the desktop now controls all agent spawns; `authenticate` onboarding heuristics (`maybeAuthRequiredError`, `:316-343`) are adapter-side and must be reimplemented from raw RPC errors; macOS loses the adapter's `extension_ui_request`→ACP permission synthesis (worse than today until the gate extension lands — it is in the Phase 1 plan, so net-positive).

### F4 — The ACP counterfactual (spec, for the record)

- **v1 = stable "Latest"** (integer `protocolVersion`, 15+ non-breaking RFDs); **v2 = Draft** (published 2026-07-20). (agentclientprotocol.com/protocol/v1/schema, /announcements/acp-v2-draft)
- v1 surface: 13 client→agent methods (`initialize`, `authenticate`, `logout`, `session/new|load|resume|list|delete|close`, `session/prompt` (held open until turn end → `stopReason`), `session/cancel`, `session/set_mode`, `session/set_config_option`); agent→client: `session/request_permission`, `elicitation/create` (form/url input), `fs/*`, `terminal/*`; 11 `session/update` variants (incl. `usage_update {size, used, cost?}`); 5 stopReasons; capabilities both directions; JSON-RPC 2.0 over stdio, one connection = many sessions.
- **v2 is converging on pi's RPC model**: `session/prompt` returns `messageId` (no turn ownership) + idle `state_update`; uniform patch semantics; `messageId` required; structured diffs + `git_patch`; extensible permission `subject`; display-only terminal chunks (v2 moves *away* from client-side tool execution).
- Design constraints: JSON-RPC 2.0, newline-delimited stdio; `initialize` once per connection; everything negotiated via capabilities.

### F5 — The "tools live in the desktop" mechanism (code-verified verdicts)

| Mechanism | Verdict | Basis |
|---|---|---|
| `registerTool` same-name override + tool selection | ✅ **WORKS — THE mechanism** | `agent-session.js:2510-2545` (`_refreshToolRegistry`: definition registry starts from built-ins, then `definitionRegistry.set(name, …)` for every extension/SDK tool — **same name = replace**); `--tools`/`--exclude-tools`/`--no-builtin-tools`/`--no-tools` (`docs/cli.md:118-127`); `defaultTools: []` disables all built-ins (`docs/settings.md:40`); runtime `getActiveToolNames()`/`setActiveTools()` (`agent-session.js:914-924`); LLM sees a tool only when registered AND active (`agent-session.js:330-345`); override without renderers reuses the built-in renderer (`dist/core/tools/renderers/index.js:43-44`) |
| `tool_call` hook | ⚠️ **WORKS-WITH-CAVEATS — gate-only** | `agent-loop.js:483-525`: only `beforeResult?.block` consumed; block → LLM receives an **error** tool result (`createErrorToolResult(reason)`); `event.input` mutable in place (no re-validation); **no `result` field exists** — cannot supply a result. A separate post-execution `tool_result` event *can* replace content/details/isError (`agent-loop.js:573-604`) |
| `user_bash` hook | ✅ **WORKS — full short-circuit, narrow scope** | `runner.js:868-889` + `rpc-mode.js:448-457`: `{result}` recorded, local execution skipped; `{operations}` replaces the backend (exactly one allowed); handler failure → blocked; **scope = user `!`/RPC `bash` only, not the LLM's `bash` tool** |
| MCP client | ❌ **DOES-NOT-WORK** | No MCP client anywhere in `dist/` (the only MCP code is vendored provider-API SDK: `mcpServerToVertex`/`mcpToGeminiTool`, Anthropic `mcp-tunnels` beta header). "Desktop as MCP server" is not implementable via pi features |
| Bridge as tool transport | ✅ **WORKS** | `pi-archimedes/packages/core/src/bridge/channel.ts`: request frame `{v:1, type:"request", id, method, source, toolCallId, params}` → exactly one response frame; `timeoutMs: undefined` → 5 min default, `null` → no timeout; `cancel()` → `BridgeCancelledError` + socket destroy (desktop sees EOF → must abort in-flight execution); no mid-flight streaming to the agent (sufficient: the LLM blocks, the desktop renders progress locally) |

**First-party precedent for the exact end-state**: `examples/extensions/gondolin/index.ts` (all built-in tools routed into a micro-VM via `createXTool(cwd, { operations: vmOps })` + same-name `registerTool` override + `user_bash` `{operations}`) and `examples/extensions/ssh.ts` (remote machine); `dist/core/tools/bash.d.ts:24-27` documents the `*Operations` interfaces as the official delegation seams; `docs/containerization.md:16` documents host-pi + delegated tools as a supported isolation method.

**Delegation transparency** (code-verified): the LLM sees only `content` of the tool result (`agent-loop.js:610-622`); `details` is UI-only; session persistence and compaction are shape-based (no local/remote distinction); a delegated `execute()` that throws produces the same error tool result a local failure would. Caveats: built-in renderers read tool-specific `details` (a delegated result with `details: undefined` renders fine, loses niceties); `usage` from desktop-side nested model calls can't flow back into session token totals.

`execute()` signature (`dist/core/extensions/types.d.ts:343-377`): `execute(toolCallId, params, signal: AbortSignal | undefined, onUpdate, ctx: ExtensionContext): Promise<AgentToolResult<TDetails>>` — gets abort signal, context, and streaming; `AgentToolResult = { content: (Text|Image)[], details, usage?, terminate? }`.

**End-state shape** (generalized Gondolin): desktop spawns `pi --mode rpc --no-themes --no-builtin-tools -e <gate/delegation extension>`; the extension re-registers `read/write/edit/bash/grep/find/ls` (same names, same schemas) with `execute = () => request the desktop, await, return verbatim` (wire the tool's `AbortSignal` to `cancel()`; `timeoutMs: null` for unbounded); the desktop gates (permission), executes (cwd-sandboxed), and returns `{content, details}`. The suite's tools move the same way. The bridge retires. Pi becomes a pure brain — and can later run remoted (server/container) with tools still executing locally.

### F6 — The replacement design (synthesis)

```
Desktop (the harness)
  │ JSONL over stdio (RPC: 33 cmds, ~21 events, extension-UI subprotocol)
  ▼
pi (the brain: LLM loop, sessions, compaction, retries — zero tool execution in the end-state)

Desktop owns:
  • RPC driver (Rust): spawn, codec, command senders, event normalizer →
    existing Tauri events + SQLite row shapes
  • Permission gate: bundled extension (tool_call hook → ctx.ui.confirm) →
    existing PermissionPrompt UI; works on ALL platforms (fixes macOS)
  • [Phase 2] Suite tools: ask, sudo_exec, subagent, manage_todo_list
  • [Phase 3] Tool executor: bash, read, write, edit, find, grep, ls
    (cwd-sandboxed; *Operations semantics ported to Rust)
```

**Phase 1 — the swap** (planned in `docs/roadmap/pi-rpc-swap.md`): `SessionDriver`'s protocol layer becomes a `PiRpc` child (spawn `pi --mode rpc --no-themes`); establisher = `get_state` (+ `get_messages` replay for resume, `get_available_models`/`get_available_thinking_levels` → config options); `send_prompt` sends `prompt` (steer when streaming) and resolves on `agent_settled`/`abort`/error (→ `end_turn`/`cancelled`/`refusal`); the bundled gate extension restores the generic gate; subagents spawn `pi --mode rpc` directly (launch wrapper dies); ACP artifacts deleted. Bridge + worker runtime + Tauri command surface + frontend event contract: **unchanged**.

**Phase 2 — suite tools move to the desktop**: re-registered via the bundled extension (same names/schemas) with `execute()` = desktop round-trip. `sudo_exec` runs `sudo -S` **in the desktop** (the password no longer touches the pi process — improves ADR 0010 hygiene). Subagent children = `pi --mode rpc` direct. The pi-archimedes suite starts dying.

**Phase 3 — built-in tools live in the desktop** (the "eventually"): `--no-builtin-tools` + extension re-registers the built-ins as desktop-delegated (Gondolin pattern). The bridge retires entirely (every frame redundant over stdio); one channel replaces two; the trust boundary tightens from "0600 socket + descendant walk" to "the desktop spawned this child."

**Key risks**: RPC is unversioned (pin pi version; contract-test on upgrade); `prompt` success ≠ completion (resolve on `agent_settled`); coarser stopReason taxonomy (no `max_tokens`/`refusal` distinction — map error→`refusal`); concurrent `extension_ui_request` dialogs unverified (parallel tool batches may open overlapping dialogs — the runtime matches by `id`, but verify); desktop-side `usage` won't flow back into pi session totals (fine — cost accounting lives in the desktop).

## Evidence

| Source | Credibility | Role |
|---|---|---|
| `@earendil-works/pi-coding-agent@0.87.1` `dist/` (JS + `.d.ts`), live-verified = npm latest = pi.dev latest (2026-09-24) | 1 (official source code) | F1, F5 — every RPC claim code-verified |
| `docs/` in the pi package (rpc, rpc-commands, rpc-extension-ui, json, session-format, extensions, cli, settings, security) | 1 (official docs, in-repo) | F1, F5 — in exact agreement with code |
| https://pi.dev/docs/latest/rpc-extension-ui (fetched 2026-09-24) | 1 (official docs, live) | F1 — live-doc verification (zero drift) |
| `archimedes-desktop` source (`src-tauri/src/`, `src/`, `docs/decisions/0001-0005`) | 1 (the codebase itself) | F2 |
| `pi-archimedes` monorepo (`packages/*/src/`, `docs/decisions/0020-0022`) | 1 (first-party, the suite) | F3, F5 |
| `pi-acp@0.0.33` `dist/index.js` (installed build) | 2 (third-party, the adapter under test) | F3 |
| agentclientprotocol.com v1 schema + v2 draft announcement | 1-2 (official spec; v2 = draft) | F4 |

## Unresolved Contradictions

1. **ADR 0003 (2026-08-14) says "ACP has no agent→client input channel" — but ACP v1 now has `elicitation/create` (form/url) and `usage_update` (cost).** Resolution: both are post-ADR RFDs; the desktop's Rust crate and pi-acp v0.0.33 predate/ignore them (verified: neither is consumed or emitted). The ADR's *practical* claim (no input channel *in our stack*) still held; the *spec* claim is stale.
2. **pi-acp claims "no ACP update type for cost" — the v1 spec has `usage_update {cost?}`.** Same root cause (generation gap). Not a real contradiction; it means a first-party ACP adapter could have recovered cost frames — the strongest counter-argument to the swap, moot now that the decision is made.

## Gaps (open items for the implementation plan)

- **G1 (resolved):** live docs vs installed — identical (0.87.1).
- **G2 (open):** does `get_messages`/`get_entries` return thinking blocks? (Resume fidelity — worst case = today's parity, since the adapter's replay is text-only anyway.)
- **G3 (resolved):** extension loading — `-e/--extension <path>` (repeatable) + `settings.json` `extensions[]`; extensions load in RPC mode (`docs/extensions.md:189`); `--no-extensions` disables discovered extensions but explicit `-e` paths still load.
- **G4 (resolved):** `extension_ui_request` with no `timeout` = no auto-resolve = unbounded wait (docs: "If a dialog method includes a `timeout` field, the agent-side will auto-resolve…").
- **G5 (open):** does the `agent-client-protocol` Rust crate v2 include `usage_update`/`elicitation`? (Moot for the swap; relevant only if ACP ever returns.)
- **G6 (open):** exact `ImageContent` shape from `@earendil-works/pi-ai` (the `prompt` command's `images` field) — verify during Task 3.
- **G7 (open):** does the RPC runtime tolerate concurrent in-flight `extension_ui_request` dialogs (parallel tool batches)? Verify during Task 4.
- **G8 (open):** does pi-acp v0.0.33 ever call client `fs/*`/`terminal/*`? (Determines whether the ACP fs-sandbox loss is real — likely a non-event; moot after the swap.)
