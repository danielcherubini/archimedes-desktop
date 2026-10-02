---
status: approved
done-when: The desktop spawns no agent process and reads no pi config: `src-tauri/` and `src/` contain no `PiRpc`, `PI_ARCHIMEDES_BRIDGE`, `seed_from_pi_config`, or agent-registry references; sessions are native-only with the Settings providers list as the sole model source; the DB migration (delete external `sessions` rows, drop `messages`, drop `sessions.agent_id`) runs cleanly on open of a pre-rip-out database; existing `~/.pi/agent/skills` + `~/.pi/agent/agents` files still resolve; `pnpm test` + `pnpm build` (repo root) and `cargo test` + `cargo clippy --all-targets` (0 warnings) + `cargo fmt --check` (`src-tauri/`) all green; the 7 pi ADRs deleted, 0011/0014 updated, ADR 0022 + CONTEXT.md committed.
---

# Native harness only — rip out the pi harness integration

**Problem:** The desktop still carries its entire first-generation integration with the external pi harness — the spawned `pi --mode rpc` process, the env-gated bridge socket channel to the pi-archimedes suite, the agent registry (with `pi` as the *default* entry), and the pi-config seeding of the model catalog. The native harness (ADR 0011) has proven out; the external path was a bootstrap and testing vehicle. The desktop is now **only the native archimedes harness**: one harness, one model source, one config surface.

## Scope

**Removed**

1. **External sessions** — the spawned `pi --mode rpc` process, the `PiRpc` JSONL client (`agent/rpc.rs`), the gate/tools extension install (`agent/gate.rs`, `agent/tools.rs`), the `fake_pi` test fixture, all external/bridge integration tests.
2. **The Bridge** — the env-gated peer-verified socket channel, the `PI_ARCHIMEDES_BRIDGE_*` env vars, bridge-mode subagents, and the ADR 0004 `WorkerRuntime` (external subagents only; native subagents already run on the main runtime). The in-process interactive cores the native harness reuses (`todo_apply`, `sudo_run_flow`, `dispatch_params`, `PendingBridge`/`PendingSudo`/`CachedPassword`/`SudoRun`/`RealSudoRunner`, `bridge_key`) **survive** in a new `agent/interactive.rs` module.
3. **The Agent registry** — `config/registry.rs`, `agents.json`, the `list_agents` command, the frontend agent picker. `HarnessConfig`'s fields move into `settings.json`: `defaultModel` (already there), plus new `defaultThinkingLevel` and `enabledTools` (empty = all). The native resolution chain drops the `HarnessConfig` layer (settings → catalog default).
4. **pi-config seeding** — `seed_from_pi_config` and the `~/.pi/agent/{settings,auth,models-store}.json` readers (ADR 0014's "scheduled for removal"). The Settings providers list (with live `GET /v1/models` discovery) becomes the **sole** model source.
5. **Stored external data** — DB migration: delete `sessions` rows of the retired pi entry (its `messages` rows cascade), drop the `messages` table, drop the `sessions.agent_id` column.

**Survives (unchanged in behavior):** the in-process `AgentLoop` (`agent/harness/`), native sessions + native-native subagents, the in-process permission gate (ADR 0010), skills + Agent-definition discovery — **both root sets kept** (`~/.agents/` + `~/.pi/agent/` user; `.agents/` + `.pi/` Space-walked — data locations, not harness coupling), MCP, providers, settings, trusted spaces, the `native_messages` table.

**Out of scope:** the pi-archimedes repo (a separate repo; dead code once this ships — noted in ADR 0022 for follow-up), the `~/.pi/` directory *convention* itself (kept as a discovery root), renumbering existing ADRs.

## Design

### Rust backend (`src-tauri/`)

**Deleted outright:** `agent/rpc.rs` (1k), `agent/gate.rs`, `agent/tools.rs`, `agent/worker_runtime.rs`, `bin/fake_pi.rs`, `tests/{bridge_integration, bridge_tool_exec, desktop_tools, pi_wire_smoke, rpc_flow, subagent_concurrency, subagent_dispatch}.rs`, `tools_*.test.mjs`, `config/registry.rs`.

**Split (native half survives, external half deleted):**

- `agent/session.rs` (7.2k) → `SessionManager` keeps `NativeHandle` + `drive_native_session` + native start/resume + the driver machinery; the `SessionBackend::External` arm, the external `SessionDriver` spawn/establish loop, and external resume go. The `SessionBackend` enum collapses to a newtype.
- `agent/bridge.rs` (4.7k) → the in-process cores move to `agent/interactive.rs`; the socket listener, peer verification, env-var handling, and extension-listener plumbing go.
- `agent/subagent.rs` (2.8k) → keeps `dispatch_native` + metrics/outcome machinery; the bridge-mode spawn path + `WorkerRuntime` handoff go.
- `agent/permission.rs` (713) → keeps `native_permission_gate` + `PendingPermissions` + `PermissionOutcome`; the `extension_ui_request` handler + its 300 s timeout machinery go.

**Renamed (harness-neutral — the "bridge" term dies with the suite):** wire events `bridge-request`/`bridge-event`/`bridge-request-close` → `interactive-request`/`interactive-event`/`interactive-request-close`; command `respond_bridge_request` → `respond_interactive_request`; `RpcError` → `SessionError`; `agent/mod.rs`'s "Pi-RPC session core" doc header rewritten.

**Commands/settings:** `start_session`/`resume_session` lose their `agent_id` param; `list_agents` goes; `settings.json` gains `defaultThinkingLevel` + `enabledTools`.

### Frontend (`src/`)

**Deleted:** `NewSpaceDialog`'s agent picker; `useStartNewConversation`'s `agentId` fallback logic (→ "start a native session in this Space"); `lib/tauri.ts`'s `listAgents`/`AgentEntryDto`; `store/sessions.ts`'s `agentId` field.

**Renamed:** `store/bridge.ts` → `store/interactive.ts`; the `bridge-*` Tauri bindings → `interactive-*` (behavior unchanged).

**Settings page:** gains a default thinking-level select (the per-model remembered level, ADR 0015, still wins when set) and an enabled-tools editor (default: all).

**Unchanged (verified shared, native-driven):** `PermissionPrompt`, `SessionConfigSelect`, `AskQuestionCard`, `SudoConfirmModal`, `SudoPasswordModal`, `TodoBoardPanel`, the `Subagent*` components.

### Storage

One forward migration in `storage/db.rs`: (1) `DELETE FROM sessions WHERE agent_id = 'pi'`, (2) `DROP TABLE messages`, (3) `ALTER TABLE sessions DROP COLUMN agent_id`. `native_messages`, `spaces`, and the existing migration-recovery machinery are untouched.

### Docs

**Deleted (7):** ADRs `0001`, `0002`, `0003`, `0004`, `0005`, `0009`, `0012` — the pi generation, retired wholesale (no renumbering; the gaps are the record).

**Touched (2):** `0014` — the "transitional seeding" framing becomes "the Settings list is the sole model source"; `0011` — decision stands, its "removed in a later cleanup" / "the pi-archimedes suite retires" consequences get a dated note marking both done.

**New ADR `0022-native-harness-only.md`:** the rip-out decision — hard to reverse (stored pi sessions + `messages` destroyed), surprising (why can't the app run external agents at all?), a real trade-off (keeping unresumable rows / seeding / the registry were all considered and rejected). Considered options: the three rejected in the design dialogue. Consequences: Settings as sole model source + config surface; `~/.pi/` discovery roots retained; the pi-archimedes repo is dead code (out of scope, noted). Motivation: the external path was a bootstrap/testing vehicle; the native harness has proven out.

**CONTEXT.md:** `Agent`/`Session`/`Native session`/`Subagent session`/`Permission prompt`/`Config option` rewritten (the external disjuncts and the `extension_ui_request`/`get_state` mechanisms go); `External session`/`Agent registry`/`Bridge`/`Suite tool` deleted; `Skill`/`Agent definition` lightly touched (the `~/.pi/` roots stay in "standard locations").

**Code-comment sweep:** every reference to the seven deleted ADRs in comments/doc-tests is re-pointed to 0022 or the surviving native ADR.

## Acceptance (done-when)

The desktop spawns no agent process and reads no pi config: `grep` finds no `PiRpc`, `PI_ARCHIMEDES_BRIDGE`, `seed_from_pi_config`, or `registry` references in `src-tauri/`/`src/`; sessions are native-only with the Settings providers list as the sole model source; the DB migration runs cleanly on open of a pre-rip-out database (external rows gone, `messages` gone, `agent_id` gone); existing `~/.pi/agent/skills` + `~/.pi/agent/agents` files still resolve; `pnpm test` + `pnpm build` (repo root) and `cargo test` + `cargo clippy --all-targets` (0 warnings) + `cargo fmt --check` (`src-tauri/`) all green; the 7 ADRs deleted, 0011/0014 updated, ADR 0022 + CONTEXT.md committed.
