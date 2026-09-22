---
status: committed
done-when: In bridge mode, a subagent session's metrics are REAL (not `{0,0,0,duration}`): the suite self-emits a per-turn `COST_UPDATE` (source `"main"`, from pi's `turn_end` usage), the desktop ACCUMULATES a subagent's `cost_update` payloads (not last-payload), and `subagent-closed` / the `dispatch_subagent` response carry the accumulated tokens + cost + the real `durationMs`. The TUI footer's cost line shows the true session cost (own + subagent forks) — the documented, intentional semantic change.
---

# Subagent Metrics — Suite Self-Usage Cost Push Plan

**Goal:** Fill the v1 metrics zeros: the suite emits a subagent process's OWN usage (per turn) over the existing `cost_update` bridge event, and the desktop accumulates it into the subagent's metrics snapshot.

**Architecture:** The suite's extension API exposes `pi.on("turn_end")` with `message.usage` (the pi-ai `Usage` type: `input`/`output`/`cacheRead`/`cacheWrite`/`cost.{input,output,cacheRead,cacheWrite,total}` — per-turn, NOT cumulative). A new core module emits a `COST_UPDATE` bus event per turn (source `"main"`, the turn's usage as a delta — the footer's `CostAccumulator` already sums, so per-turn deltas are exactly right); the existing bridge wiring (events.ts:140) forwards it to the desktop unchanged. The desktop's `cost_capture` changes from LAST-PAYLOAD to ACCUMULATOR (sum across `cost_update` payloads — required: with per-turn deltas, last-payload semantics would capture only the final turn). The subagent's own pi process (spawned by the desktop with the suite loaded) self-emits through its OWN bridge listener (its `PI_ARCHIMEDES_BRIDGE_SOCKET` — the desktop set it for the spawn), so the desktop's subagent `cost_update` handler (the `cost_capture = Some` arm) accumulates the subagent's real usage.

**Tech Stack:** TypeScript (the pi-archimedes suite: `packages/core` bridge/bus, the pi extension API `@earendil-works/pi-coding-agent` 0.87.x) + Rust (the desktop: `src-tauri/src/acp/`) + Vitest / cargo test.

**Repos, paths and commands used in every task below:**
- Desktop repo root: `/home/daniel/Coding/AI/archimedes-desktop` (Rust under `src-tauri/`).
- Suite repo root: `/home/daniel/Coding/Javascript/pi-archimedes` (pnpm workspace; this plan's suite-side tasks live in `packages/core/` — the suite repo has its own `docs/roadmap/` but the plan lives in the desktop repo, the feature's canonical home, mirroring how the subagent-sessions plan covered both repos from the desktop).
- Rust: `cd src-tauri && cargo test --test <name>` / `cargo test` (all), `cargo clippy --all-targets` (0 warnings), `cargo fmt --check`.
- Frontend: `pnpm test` (repo root), `pnpm build` (tsc + vite).
- Suite: `cd /home/daniel/Coding/Javascript/pi-archimedes && pnpm test`, `pnpm -r exec -- tsc --noEmit`.
- Wire case conventions (unchanged): ACP property keys camelCase, discriminator values snake_case; bridge frames `v: 1`; bridge event names snake_case; the `COST_UPDATE` bus payload / `cost_update` wire payload shape is `{ source: string, inputTokens?: number, outputTokens?: number, cacheReadTokens?: number, cacheWriteTokens?: number, cost?: number }` (the suite's `CostUpdatePayload`, `packages/core/src/bus.ts:11-18` — the bus payload types ARE the wire types, forwarded verbatim by `packages/core/src/bridge/events.ts:140-142`).

**Design decisions (decided — the executing agent implements these, not alternatives):**
1. **Push timing: per-turn, on `pi.on("turn_end")`** (NOT `agent_settled` — that fires only when the agent run has FULLY settled (no retries/queued continuations), i.e. once at the very end: no live metrics while the subagent works, and a cancel loses everything). `turn_end` fires once per LLM turn with `message: AgentMessage` — the turn's assistant message, whose `usage` (the pi-ai `Usage` type) is the FULL usage for that turn. Per-turn usage is already a DELTA (not cumulative), so emitting it verbatim needs no bookkeeping.
2. **Payload: the `CostUpdatePayload` shape, `source: "main"`** — `{ source: "main", inputTokens: u.input, outputTokens: u.output, cacheReadTokens: u.cacheRead, cacheWriteTokens: u.cacheWrite, cost: u.cost?.total ?? 0 }`. `Usage.reasoning` is EXCLUDED (it is a subset of `output` — the pi-ai type docs: "`output` already includes these tokens" — adding it would double-count; the wire shape has no field for it). `cost` is REAL (pi-ai computes per-message cost from its pricing table: `usage.cost.total`) — not a zero.
3. **NOT gated on bridge mode.** The emitter runs whenever the extension loads (TUI + RPC). Consequences, both intentional: (a) the TUI footer's `CostAccumulator` (`packages/footer/src/cost-accumulator.ts` — it accumulates ALL `COST_UPDATE` events, NO source filter) now sums the session's own usage + subagent fork deltas → the footer's cost line shows the TRUE session cost (today it shows only subagent-fork cost — the "main session `cost_update` semantics" change the v1 plan scoped out; this plan makes it deliberately). (b) In bridge mode the existing `events.ts:140` forwarder (started only when the bridge is active, `packages/core/src/bridge/index.ts:151-160`) carries it to the desktop; in TUI mode it simply doesn't (no bridge) — no new gating code.
4. **The desktop's `cost_capture` becomes an ACCUMULATOR** (sum across `cost_update` payloads), not last-payload. REQUIRED: with per-turn deltas, last-payload semantics would capture only the final turn's usage (the metrics snapshot would show one turn, not the session). The accumulator sums `inputTokens`/`outputTokens`/`cacheReadTokens`/`cacheWriteTokens`/`cost` (all optional — absent fields contribute 0). `durationMs` stays the desktop's wall-clock computation (unchanged). The wire metrics shape is UNCHANGED: `{ inputTokens, outputTokens, cost, durationMs }` (the accumulator also tracks the cache fields — they flow in the payload but are NOT in the wire metrics shape; the shape is not extended in v1 — YAGNI).
5. **The main session is unaffected in the desktop:** the main session's `cost_capture` is `None` (`session.rs:228`) — its `cost_update` events flow to the frontend's `useBridge.cost[sessionId]` (stored, NOT rendered in v1 — `src/store/bridge.ts:66-67`) with the session's own usage now included. **Precisely:** the store is LAST-PAYLOAD (`applyCost`, `src/store/bridge.ts:157-158`, overwrites — it does NOT accumulate), so the stored value becomes the session's own LATEST-TURN delta (not a session sum) — a future cost UI needs its own accumulator (the footer's `CostAccumulator` is the in-suite precedent). No v1 user-visible change (not rendered); the stored data is now at least the session's own usage instead of only subagent-fork deltas.
6. **No frontend changes.** The subagent panel's metrics line reads `useSubagents.entries[sessionId].metrics` (the `subagent-closed` snapshot — `src/components/SubagentPanel.tsx`), which is now real. The `subagent-closed` payload shape is unchanged.

**What NOT to change:**
- The fork path (`packages/subagent/src/execute.ts`'s `onProgress` delta emission, source `subagent:<name>`) — byte-identical.
- The `CostAccumulator` (it already sums; the new events flow through it by construction).
- The `cost_update` bridge forwarder (`events.ts:140`) — verbatim forwarding, unchanged.
- The `COST_UPDATE` bus event name / `CostUpdatePayload` type (the new emitter reuses them).
- The one-live policy, the worker runtime, the subagent lifecycle (ADR 0002/0004/0005).

---

### Task 1: Suite — the self-usage emitter (per-turn `COST_UPDATE`, source `"main"`)

**Context:** The suite's extension API (`@earendil-works/pi-coding-agent` 0.87.x, `dist/core/extensions/types.d.ts`) exposes `pi.on("turn_end", handler: ExtensionHandler<TurnEndEvent, TurnEndEventResult>)` where `TurnEndEvent = { type: "turn_end", turnIndex, message: AgentMessage, toolResults, messageEntryId, toolResultEntryIds }` (types.d.ts:649-656) and the turn's assistant message carries `usage: Usage` (the pi-ai type, `@earendil-works/pi-ai` 0.87.x `dist/types.d.ts:270-291`: `{ input, output, cacheRead, cacheWrite, cacheWrite1h?, reasoning?, totalTokens, cost: { input, output, cacheRead, cacheWrite, total } }`). The suite ALREADY hooks pi extension events in `registerBridge` (`packages/core/src/bridge/index.ts:151-167`: `session_start` / `agent_start` / `agent_settled`) and is called from the suite's extension entry (`packages/core/src/index.ts:153`). This task adds the emitter as a new module, wired from the same entry.

**Files:**
- Create: `packages/core/src/bridge/self-usage.ts` (the emitter)
- Create: `packages/core/src/bridge/self-usage.test.ts` (the tests)
- Modify: `packages/core/src/index.ts` (call `registerSelfUsage(pi)` next to `registerBridge(pi)` at line 153 — the call chain is real: `packages/core/package.json` `pi.extensions: ["./src/index.ts"]` → `export default` (:301) → `registerCore` (:146) → `registerBridge(pi)` at :153)
- **NO `package.json` change and NO re-export** (decision, resolving the earlier self-contradictory instruction): `registerSelfUsage` is called ONLY from core's own `index.ts` (relative import `./bridge/self-usage.js`) and the tests import relatively (`./self-usage.js`) — nothing external (the subagent package, the desktop) needs the module, so no `packages/core/package.json` `exports` entry is added and no `bridge/index.ts` re-export is written. (The `./bridge` + `./bridge/channel` subpath exports in `packages/core/package.json` exist for the subagent package's `@pi-archimedes/core/bridge` import — `dispatch.ts:33` — but that is NOT the pattern for this module: this module is core-internal.)

**What to implement:**

`packages/core/src/bridge/self-usage.ts`:
```ts
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { getBus, Events } from "../bus.js";

/**
 * Emit the session's OWN usage as a per-turn COST_UPDATE (source "main").
 * Per design decision 1-3: per-turn (turn_end), delta (the turn's usage is
 * not cumulative), NOT gated on bridge mode (the TUI footer's
 * CostAccumulator sums it — the footer's cost line becomes the true session
 * cost; in bridge mode the existing events.ts forwarder carries it to the
 * desktop). `reasoning` is excluded (a subset of `output` — double-count).
 */
export function registerSelfUsage(pi: ExtensionAPI): void {
  pi.on("turn_end", (event) => {
    const usage = (event.message as { usage?: UsageLike } | undefined)?.usage;
    if (!usage) return;
    getBus().emit(Events.COST_UPDATE, {
      source: "main",
      inputTokens: usage.input ?? 0,
      outputTokens: usage.output ?? 0,
      cacheReadTokens: usage.cacheRead ?? 0,
      cacheWriteTokens: usage.cacheWrite ?? 0,
      cost: usage.cost?.total ?? 0,
    });
  });
}
```
(with `UsageLike` a minimal local structural type `{ input?: number; output?: number; cacheRead?: number; cacheWrite?: number; cost?: { total?: number } }` — do NOT import the pi-ai `Usage` type into core if core doesn't already depend on `@earendil-works/pi-ai` — check core's `package.json` deps; a structural local type keeps the dependency surface unchanged. The `message` cast: `TurnEndEvent.message` is the `AgentMessage` union; the usage is on the assistant variant — the defensive `as` + `?.usage` + `if (!usage) return` handles the union at runtime without a type-predicate maze. If the extension API's `turn_end` handler signature makes `event.message` directly typed as the assistant message in 0.87.x, use the direct access — check the actual `types.d.ts:649-656` + the `AgentMessage` union definition before writing the cast.)

`packages/core/src/index.ts`: call `registerSelfUsage(pi)` immediately after `registerBridge(pi)` (line 153) — unconditionally (design decision 3: not gated on bridge mode; `registerBridge`'s own gating is internal to it).

`packages/core/src/bridge/index.ts`: **NO change** — the module is NOT re-exported (the decision above: core-internal relative import only — `index.ts` imports `./bridge/self-usage.js` directly; the `BridgeTransportError`/`BridgeCancelledError` re-export at line 17 is for the subagent package's subpath import, a different consumer, and is NOT the pattern for this module).

TESTS (`packages/core/src/bridge/self-usage.test.ts` — the suite's existing test pattern: `packages/core/src/bridge/index.test.ts` shows the mock-ExtensionAPI idiom — QUOTE IT VERBATIM, a plain object will NOT compile):

> **THE MOCK IDIOM (empirically verified — a plain object fails `tsc` with TS2740: `ExtensionAPI` has ~26 required members; the repo's strict tsconfig type-checks test files — `packages/core/tsconfig.json` `include: ["src"]`, gate `pnpm -r exec -- tsc --noEmit`):** the repo's actual idiom (`packages/core/src/bridge/index.test.ts:58-61`, `registerAndFireSessionStart`) is:
> ```ts
> const onSpy = vi.fn();
> registerSelfUsage({ on: onSpy } as unknown as ExtensionAPI);
> const calls = onSpy.mock.calls as Array<[string, (e: unknown, ctx: unknown) => void]>;
> const handler = calls.find(([event]) => event === "turn_end")![1];
> ```
> The `as unknown as ExtensionAPI` cast AND the `mock.calls` re-typing are BOTH required: the cast makes the plain object legal, and the re-typing (params as `unknown`) is what makes the synthetic `turn_end` event below legal without a full `TurnEndEvent` cast (the real type requires `BoundaryState`/`toolResults`/`messageEntryId`/`toolResultEntryIds` — the re-typed handler takes `unknown`). Both compile clean under the repo's exact tsconfig (verified).

**BUS DISCIPLINE (load-bearing — read before writing the tests):** the bus (`packages/core/src/bus.ts`) has **NO reset seam** (only `getBus`/`initBus` — `initBus()` is NOT a reset: it clears the queue then `emit`s, which RE-ENqueues ghosts if nobody is subscribed). Worse: `emit` with ZERO subscribers **queues the payload globally** (bus.ts:44-49) and `on()` **replays queued events to the new listener WITHOUT removing them from the queue** (bus.ts:58-72) — a ghost is redelivered to EVERY future subscriber of that event until a subscriber drains it (or `initBus()` runs *with* a subscriber attached). `packages/core/src/bridge/index.test.ts` resets only the channel/events (`resetChannel()`/`resetEvents()` at :109-124) — NEVER the bus. **The discipline (every test, no exceptions):** subscribe a PER-TEST listener FIRST, then invoke the handler; assert ONLY on that test's captured array; unsubscribe via the `on` return value (or `afterEach`). Do NOT add a bus reset seam (out of scope — the existing tests live with the discipline). Do NOT invoke the handler before subscribing (the queue's replay-on-subscribe would make a "no events" assertion pass vacuously for the wrong reason, and a ghost would break the "exactly one" counts).
1. **per-turn emit:** the mock `ExtensionAPI` (the verbatim idiom above — `vi.fn()` + `as unknown as ExtensionAPI` + the `mock.calls` re-typing) → `registerSelfUsage(mock)` → invoke the captured `turn_end` handler with a synthetic event `{ type: "turn_end", turnIndex: 0, message: { role: "assistant", usage: { input: 100, output: 50, cacheRead: 10, cacheWrite: 5, totalTokens: 165, cost: { input: 0.001, output: 0.002, cacheRead: 0.0001, cacheWrite: 0.0002, total: 0.0033 } } }` (the re-typed handler takes `unknown` — the synthetic needs no `TurnEndEvent` cast) → assert the `COST_UPDATE` bus event fired with EXACTLY `{ source: "main", inputTokens: 100, outputTokens: 50, cacheReadTokens: 10, cacheWriteTokens: 5, cost: 0.0033 }` (exact match — no `reasoning` key, no `totalTokens` key; the per-test listener subscribed FIRST per the discipline above).
2. **no usage → no emit:** the handler with `message: { role: "assistant" }` (no `usage`) → no `COST_UPDATE` emitted (assert the bus listener was not called).
3. **two turns → two events (delta semantics):** invoke the handler twice (turn 0: input 100; turn 1: input 200) → TWO `COST_UPDATE` events with `inputTokens` 100 then 200 (NOT 300 — the emitter never accumulates; the footer does).
4. **unaffected by bridge mode:** the emitter is registered unconditionally — the test does NOT `configure` the bridge channel at all (NONE of these tests do — the channel stays inactive by default; vitest's file isolation means no channel state leaks in from other test files) → the `COST_UPDATE` bus event STILL fires (the bus is in-process; only the BRIDGE forwarder is gated — `events.start()` runs only when the bridge is active, `packages/core/src/bridge/index.ts:152-158`). This pins design decision 3.

**Steps:**
- [ ] Read `packages/core/src/bridge/index.test.ts` (the mock-ExtensionAPI idiom at :58-61 — the verbatim `vi.fn()` + `as unknown as ExtensionAPI` + `mock.calls` re-typing quoted in the TESTS section above — a plain object fails `tsc` with TS2740; the bus discipline above — NO reset seam exists), `packages/core/src/bus.ts` (the `Events` const object — `export const Events = {…} as const`, bus.ts:101-108 — NOT an enum + the `CostUpdatePayload` type at :11-18 + the queue/replay behavior at :44-49 / :58-72 that the discipline addresses), `packages/core/src/index.ts:145-160` (the registration site — `registerBridge(pi)` at :153; the call chain: `packages/core/package.json` `pi.extensions: ["./src/index.ts"]` → `export default` (:301) → `registerCore` (:146) → :153), and the pi-ai `Usage` type (`node_modules/.pnpm/@earendil-works+pi-ai@*/node_modules/@earendil-works/pi-ai/dist/types.d.ts:270-291` — the field names: `input`/`output`/`cacheRead`/`cacheWrite`/`cacheWrite1h?`/`reasoning?`/`totalTokens`/`cost: { input, output, cacheRead, cacheWrite, total }` — the `cost` object closes at :290-291) + the extension `TurnEndEvent` (`node_modules/.pnpm/@earendil-works+pi-coding-agent@*/node_modules/@earendil-works/pi-coding-agent/dist/core/extensions/types.d.ts:649-656` — `message: AgentMessage`, the full union — the defensive `as { usage?: UsageLike }` cast in the sketch is BOTH necessary (direct `event.message.usage` is TS2339 on the union — `BashExecutionMessage`/`ToolResultMessage` variants lack `usage`) and legal (compiles clean under the repo's strict tsconfig — verified).
- [ ] Write the 4 failing tests in `packages/core/src/bridge/self-usage.test.ts`.
- [ ] Run `cd /home/daniel/Coding/Javascript/pi-archimedes && pnpm test` (the new tests FAIL — `registerSelfUsage` missing) — confirm.
- [ ] Implement `self-usage.ts` + the `index.ts` call wiring (no `bridge/index.ts` — the NO-re-export decision above).
- [ ] Run `pnpm test` (all green) + `pnpm -r exec -- tsc --noEmit` (clean).
- [ ] Commit (suite repo) with message: "feat(core): self-usage COST_UPDATE emitter (per-turn, source main — fills the v1 metrics zeros)"

**Acceptance criteria:**
- [ ] All 4 tests pass (exact payload shape, no-usage no-emit, per-turn delta semantics, bridge-independence).
- [ ] The full suite gauntlet green (`pnpm test` + `tsc --noEmit`) — the existing `COST_UPDATE` consumers (the footer's `CostAccumulator`, the bridge forwarder) are untouched and their tests pass verbatim.
- [ ] The `reasoning` field is NOT in the emitted payload (double-count guard — the test 1 exact-match asserts it).

---

### Task 2: Desktop — `cost_capture` accumulator + the E2E (fake-agent cost push)

**Context:** The desktop's `cost_update` handler (`src-tauri/src/acp/bridge.rs:610-619`) currently OVERWRITES `cost_capture` (a `StdMutex<Option<Value>>`, `session.rs:209`) with the last `cost_update` payload. With per-turn deltas (Task 1), last-payload semantics would capture only the final turn — the metrics snapshot must ACCUMULATE. This task changes the capture to a sum + adds the E2E proof (a fake agent that pushes `cost_update` frames mid-session).

**Files:**
- Modify: `src-tauri/src/acp/bridge.rs` (the `start_listener` `cost_capture` param type + the handler: accumulate instead of overwrite — the param is threaded at bridge.rs:148, 226)
- Modify: `src-tauri/src/acp/session.rs` (the `SessionDriver.cost_capture` field, session.rs:209 — the type change; the main manager's `None` at :228 is unchanged)
- Modify: `src-tauri/src/acp/subagent.rs` (the per-dispatch fresh capture at :167 — the new type's default; the `captures()` metrics construction at :547 — read the ACCUMULATED values instead of the last payload)
- Modify: `src-tauri/src/bin/fake_agent.rs` (a new env-gated behavior — see below)
- Test: `src-tauri/tests/subagent_dispatch.rs` (a new E2E test — the existing 5-test file, `subagent_dispatch.rs`, is the pattern: fake-agent registry entries + the `SubagentSessionManager` wiring)

**What to implement:**

1. **The accumulator type** (placement — DECIDED: put `CostAccumulator` in `session.rs`, next to `SessionDriver`, whose `cost_capture` field at :209 holds it. `SubagentMetrics` STAYS in `subagent.rs:585-591` — the two are different types with different homes; do not move `SubagentMetrics`. Note: `session.rs` has NO `#[cfg(test)]` module today — add one for the unit test (the `#[cfg(test)]` modules at `subagent.rs:606` and `bridge.rs:887` are the pattern to mirror):
```rust
/// Accumulated `cost_update` usage (the subagent metrics source). Sums the
/// optional numeric fields across `cost_update` payloads (per-turn deltas
/// from the suite's self-usage emitter — Task 1 of this plan); an absent
/// field contributes 0. `Default` = all zeros (a session that never pushed
/// usage — the pre-Task-1 v1 state).
#[derive(Debug, Clone, Default)]
pub struct CostAccumulator {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub cost: f64,
}
impl CostAccumulator {
    /// Fold one `cost_update` payload (the wire shape: `inputTokens` /
    /// `outputTokens` / `cacheReadTokens` / `cacheWriteTokens` / `cost`,
    /// all optional) into the accumulator (absent → 0).
    pub fn add_payload(&mut self, p: &Value) { /* the 5 field sums */ }
}
```
2. **The type swap (the ACTUAL sites — verified; the earlier "4 sites" list was wrong):** `cost_capture: Option<Arc<StdMutex<Option<Value>>>>` → `Option<Arc<StdMutex<CostAccumulator>>>` at **`session.rs:209`** (the `SessionDriver` field) + **`bridge.rs:148`** (the `start_listener` param) + **`bridge.rs:416`** (the `ConnCtx.cost_capture` struct field — the site the earlier list missed) + **`subagent.rs:167`** (the per-dispatch construction: `Some(Arc::new(StdMutex::new(None)))` → `Some(Arc::new(StdMutex::new(CostAccumulator::default())))`). **NO edits needed at** `bridge.rs:226` (`cost_capture: cost_capture.clone()` — a clone, no type annotation), `:475` / `:1084` (`None` / destructure — the compiler confirms). The `None` at `session.rs:228` (the main manager) is unchanged. The compiler enforces the :416 fix — but the list above is complete, so the first `cargo build` should be clean.
3. **The handler** (`bridge.rs:610-619`): `cc.lock().unwrap_or_else(|p| p.into_inner()).add_payload(p);` (the poison-tolerant lock pattern stays — the comment at bridge.rs:603-609 is kept, updated to say "accumulates" instead of "stores the last". NOTE: NO leading `*` — the old code assigned (`*guard = Some(…)`), but `add_payload` is a `&mut self` method returning `()` — the guard's `DerefMut` auto-derefs for the method call; a `*` before it would dereference `()` and fail to compile.)
4. **The metrics construction** (`subagent.rs:547` area — `captures()` / the `SubagentMetrics` build): read `cc.input_tokens` / `cc.output_tokens` / `cc.cost` (the accumulator's fields) instead of mapping the last payload's keys. `duration_ms` stays the wall-clock computation (unchanged). The `SubagentMetrics` struct + `metrics_json` (subagent.rs:570-589) are UNCHANGED (the wire shape is the same — design decision 4).
5. **The fake-agent cost push** (`src-tauri/src/bin/fake_agent.rs`): a new env-gated behavior — `FAKE_COST_PUSH=1` in a subagent-mode agent's registry `env`: after `session/new`, BEFORE answering `session/prompt`, the fake agent pushes TWO `cost_update` frames through its OWN `PI_ARCHIMEDES_BRIDGE_SOCKET`. **The seam (name it — the earlier attribution was wrong):** the PLAIN `subagent` variant (`fake_agent.rs:519-538`, `handle_prompt_subagent`'s `_` arm) only writes a chunk + `end_turn` — it NEVER touches the bridge (the module doc, :74-75: "the fake agent ignores them EXCEPT the `dispatch` / `dispatch-cancel` / `ask` modes"). The connect/write/read pattern the plan reuses lives in the **`FAKE_SUBAGENT_MODE=ask` variant's `ask_round_trip`** (`fake_agent.rs:547-599`) + the `connect_bridge_socket` helper (`fake_agent.rs:102`) — that is the pattern test 3 (`dispatch_subagent_own_bridge_ask_round_trip`, `subagent_dispatch.rs:759`) exercises. **Instruction:** extend the `subagent` variant's prompt handler with a `FAKE_COST_PUSH=1` branch that reuses `connect_bridge_socket` + `ask_round_trip`'s connect/write/read pattern. **The frame shape (the desktop's PARSER is the contract — verified: `handle_connection` dispatches on `frame.get("type")` and handles ONLY `Some("request")` (bridge.rs:478) and `Some("push")` (bridge.rs:588); a `"type": "event"` frame falls into the `_ =>` close arm (bridge.rs:630-633) and is SILENTLY DROPPED — the real wire shape is `type: "push"`: the suite's `sendEvent` builds `{ v: 1, type: "push", seq: ++seq, event, payload }` (channel.ts:149) and `bridge_integration.rs:102` constructs `{"v": 1, "type": "push", "seq": seq, "event": "state", …}`):** each push frame is `{ "v": 1, "type": "push", "event": "cost_update", "payload": { … } }` + `\n` — `seq` OMITTED (bridge.rs:589 `unwrap_or(0)` → `seq == 0` → always delivered, :600 — the simplest correct choice; if a `seq` IS supplied it must be strictly increasing across the two pushes (1, 2) or the `fetch_max` dedupe at :600 drops the second). **The PROTOCOL (three mechanical facts — the E2E can never pass without them):** (1) `handle_connection` reads EXACTLY ONE frame per connection (the one-frame read is at bridge.rs:438-453; the quote "the agent opens a connection per message and destroys it on the first data" is at :419-420) — so ONE `connect_bridge_socket` connection PER PUSH (two pushes on one socket lose frame 2); (2) the desktop acks (`ack\n`, bridge.rs:627) AFTER the capture (:610-616), and `captures()` runs at the session's `end_turn` (subagent.rs:384) — so the fake agent must READ THE `ack\n` LINE (blocking) after EACH push before writing the next, and only answer the prompt (`agent_message_chunk` + `end_turn`) after BOTH acks — otherwise the listener may not have processed the pushes before the session closes → the sum is 0/partial INTERMITTENTLY (the existing fake-agent modes all block on reads — `ask_round_trip`'s `read_line`, fake_agent.rs:585 — the pattern exists; use it); (3) **NOTE THE DIFFERENCE IN WHAT THE READ RETURNS:** `ask_round_trip` sends a REQUEST and reads a JSON **response frame** (the request arm writes `type: "response"` — it NEVER writes `ack\n`); a PUSH is **acked, not responded-to** — the read in the `FAKE_COST_PUSH` branch returns the BARE line `ack` (bridge.rs:627): read exactly ONE line and do NOT JSON-parse it (a `serde_json::from_str` on the ack line would fail at runtime in the fake agent — the line is not JSON). The two payloads: payload 1 `{ "source": "main", "inputTokens": 100, "outputTokens": 50, "cost": 0.001 }` (note: `cacheReadTokens`/`cacheWriteTokens` ABSENT — the accumulator must treat absent as 0), payload 2 `{ "source": "main", "inputTokens": 200, "outputTokens": 25, "cacheReadTokens": 10, "cost": 0.002 }`.
6. **The E2E test** (`src-tauri/tests/subagent_dispatch.rs`, a 6th test — the existing 5 tests are the pattern: the registry entry points at `fake_agent` with `bridge: true` + `env` carrying the test knobs): the existing tests' entry point — `SessionManager` (main) + `SubagentSessionManager` + the `set_subagent_manager` wiring, with the fake agent's `dispatch` mode sending the `dispatch_subagent` bridge frame (test 1's pattern) — here with the `FAKE_COST_PUSH=1` registry env → the subagent session closes → assert `subagent-closed`'s `metrics` = `{ inputTokens: 300, outputTokens: 75, cost: 0.003, durationMs: <real, > 0> }` — the SUM of both payloads (NOT payload 2 alone — the old last-payload semantics would have produced `{ 200, 25, 0.002 }`; the exact-match assertion on `inputTokens: 300` + `outputTokens: 75` + `cost: 0.003` proves the accumulator). Use exact `Value` comparisons on the three numeric fields (`cost` with an `abs_diff` tolerance of 1e-9 for the f64 sum).
7. **The stale-comment sweep (the accumulator changes `cost_capture`'s semantics — EIGHT comments become factually wrong and must be updated in the same commit):** `bridge.rs:133` (the `start_listener` doc: "`cost_capture` (subagents only — `Some`) stores the last `cost_update` push payload (the v1 metrics source)" → "`cost_capture` (subagents only — `Some`) accumulates the `cost_update` payloads (the v1 metrics source)"); `bridge.rs:415` (the `ConnCtx.cost_capture` field doc: "Last `cost_update` push payload (subagents only; `None` for main)." → "Accumulated `cost_update` usage (subagents only; `None` for main)."); `session.rs:208` (the `SessionDriver` field doc: same "Last `cost_update` push payload" wording → "Accumulated `cost_update` usage"); `subagent.rs:380-383` (the `captures()` caller comment: "the last `cost_update` payload, defaulting to 0" → "the accumulated `cost_update` usage, defaulting to 0"); `subagent.rs:533-534` (the `captures()` DOC comment — a SECOND copy of the retired statement: "the token/cost fields come from the last `cost_update` payload (defaulting to 0 — v1: the suite does not emit a `cost_update` for a process's OWN usage); `duration_ms` is the caller's wall clock" → "the token/cost fields come from the accumulated `cost_update` usage (defaulting to 0 — the `CostAccumulator` default when the session pushed none); `duration_ms` is the caller's wall clock"); `subagent.rs:580-584` (the `SubagentMetrics` doc — BOTH sentences: "The metrics captured for a subagent session (the last `cost_update` payload + the wall-clock duration). v1: the suite does not emit a `cost_update` for a process's OWN usage, so the token/cost fields are 0 and `duration_ms` is real" → "The metrics captured for a subagent session (the accumulated `cost_update` usage + the wall-clock duration). The suite self-emits a per-turn `cost_update` (source `main` — `subagent-metrics-cost-push` Task 1); the token/cost fields are real when the session pushed usage (0 when it didn't — the `CostAccumulator` default) and `duration_ms` is always the wall clock"); `subagent.rs:597` (the `SubagentOutcome::Completed` doc: "`metrics` is the captured `cost_update` payload + duration." → "`metrics` is the accumulated `cost_update` usage + duration."). (This plan is comment-conscious — it fixes the `bridge.rs:603-609` handler comment + `execute.ts:84-86` in Task 3; leaving these eight stale would be inconsistent.)

**Steps:**
- [ ] Read `src-tauri/src/acp/bridge.rs:603-619` (the handler), `src-tauri/src/acp/subagent.rs:540-590` (the `captures()` + `SubagentMetrics` + `metrics_json`), `src-tauri/src/acp/session.rs:205-230` (the `SessionDriver` field), `src-tauri/tests/subagent_dispatch.rs` (the FIVE-test pattern — :424, :620, :758, :903, :1025 — + the fake-agent registry entries), `src-tauri/src/bin/fake_agent.rs` (the mode dispatch + the `ask` variant's `ask_round_trip` bridge-connect pattern at :547-599 — the plain `subagent` variant at :519-538 never touches the bridge; the seam note in item 5 above).
- [ ] Write the failing E2E test (the 6th test, the exact-sum assertions) + a unit test for `CostAccumulator::add_payload` (two payloads with one absent field → the sums; an all-absent payload → no change) in a NEW `#[cfg(test)]` module in `session.rs` (session.rs has NO `#[cfg(test)]` module today — add one; the modules at `subagent.rs:606` + `bridge.rs:887` are the pattern). NOTE: the unit test lives in the LIB test target — `cargo test --test subagent_dispatch` does NOT compile or run it (integration target only; `#[cfg(test)]` is stripped there).
- [ ] Run `cd src-tauri && cargo test --lib` — confirm the unit test FAILS to compile (the `CostAccumulator` type doesn't exist yet — a compile error of the lib-test target, NOT a test failure) + `cargo test --test subagent_dispatch` — confirm the new E2E test FAILS (it fails with ZEROS — `{ 0, 0, 0, durationMs }` — the `FAKE_COST_PUSH` fake-agent behavior doesn't exist yet, so no pushes flow; the `{ 200, 25, 0.002 }` last-payload counterfactual applies once the pushes exist but before the accumulator).
- [ ] Implement the `CostAccumulator` + the type swap (the 3 annotated sites + the construction: session.rs:209, bridge.rs:148, bridge.rs:416, subagent.rs:167 — item 2 above) + the handler + the metrics construction + the fake-agent `FAKE_COST_PUSH` behavior (item 5 above — one connection per push, read the `ack` after each, `type: "push"` frames, seq omitted) + the stale-comment sweep (item 7 above).
- [ ] Run `cargo test --lib` (the unit test passes) + `cargo test --test subagent_dispatch` (ALL 6 pass — the 5 existing + the new one; the existing tests' `cost_update`-related assertions, if any, must still pass: the accumulator is a superset of the old behavior for a single payload) + `cargo test` (all) + `cargo clippy --all-targets` (0 warnings) + `cargo fmt --check`.
- [ ] Commit (desktop repo) with message: "feat: subagent metrics — cost_capture accumulator (per-turn deltas sum, not last-payload) + fake-agent cost-push E2E"

**Acceptance criteria:**
- [ ] The E2E test (the 6th) proves the SUM (both payloads' `inputTokens` 100+200=300, `outputTokens` 50+25=75, `cost` 0.001+0.002=0.003 — exact, `cost` with an `abs_diff` tolerance of 1e-9 for the f64 sum), with an absent `cacheReadTokens` in payload 1 handled as 0; it is DETERMINISTIC (the one-connection-per-push + read-the-ack protocol — item 5 — makes the capture complete before `captures()` at `end_turn`).
- [ ] The unit test (lib target) proves `add_payload`'s absent-field semantics.
- [ ] The wire metrics shape is UNCHANGED (`{ inputTokens, outputTokens, cost, durationMs }` — the existing 5 E2E tests' metrics assertions pass verbatim; a session with no `cost_update` still yields the zeros + real `durationMs` — the pre-Task-1 v1 state is preserved as the empty case).
- [ ] The full desktop gauntlet green (`cargo test` + clippy 0 + fmt + `pnpm test` + `pnpm build` — the frontend is untouched but the gauntlet is the gate).

---

### Task 3: Docs — the metrics constraint is now real

**Context:** `docs/features/subagent-sessions.md` (the desktop repo — the feature's canonical doc, written at the subagent-sessions ship) carries a "Metrics are zero-valued in v1 except `durationMs`" constraint + a "Suite-side self-usage `cost_update` push (fills the v1 metrics zeros above)" follow-up. Both are stale after Tasks 1-2.

**Files:**
- Modify: `docs/features/subagent-sessions.md`
- Modify: `packages/subagent/src/execute.ts` (the bridge-branch comment at :84-86 — see the last item below; docs-only, no behavior change)

**What to implement:**
- Replace the "Metrics are zero-valued in v1" constraint bullet with the NEW present-tense behavior: the suite self-emits a per-turn `COST_UPDATE` (source `"main"`, from pi's `turn_end` usage — per-turn deltas, `reasoning` excluded as an `output` subset); the desktop accumulates a subagent's `cost_update` payloads (sum, not last-payload) into the metrics snapshot; `cost` is real (pi-ai's pricing table); `durationMs` is the desktop's wall clock. Keep the note that the wire metrics shape is `{ inputTokens, outputTokens, cost, durationMs }` (the cache fields flow in the payload but are not in the wire shape — YAGNI).
- Replace the "The tool's `usage`/`progressSummary.tokens` report zeros in bridge mode — documented, not silent" line with the FACTUAL post-Task-2 behavior (verified against the code — write EXACTLY this, no hedge): in bridge mode, the `subagent` TOOL's `usage` (its report to the main agent) is built from the `dispatch_subagent` RESPONSE's `metrics` (the desktop's accumulator output — Task 2): `dispatchViaBridge`'s success path (`packages/subagent/src/dispatch.ts`) maps `res.metrics` → `usage` with REAL `input`/`output`/`cost` (the `cacheRead`/`cacheWrite` fields are hardcoded 0 — the wire metrics shape has no cache fields) and `progressSummary.tokens = inputTokens + outputTokens` (REAL). The SYNTHESIZED `progress.*` token fields (the in-flight progress updates from `execute.ts`'s bridge branch — the placeholder + the final-failure progress) remain ZEROS (the bridge branch synthesizes `progress` without token data — including the RESULT's own `progress` field, also synthesized zero-token by `dispatchViaBridge` (`packages/subagent/src/dispatch.ts:115-131`); only the RESULT's `usage`/`progressSummary` are real). Write that distinction precisely: result `usage`/`progressSummary` = real, in-flight `progress` = zeros.
- **Also update the stale bridge-branch comment in `packages/subagent/src/execute.ts:84-86`** (the `NO emitCostUpdate` comment currently reads "the suite does not push the subagent's own usage in v1; double-counting the main agent's cost would be wrong" — the "in v1" clause goes stale after Task 1, since the suite NOW pushes the subagent's own usage — from the SUBAGENT'S OWN process, to the subagent's OWN bridge). Rewrite it to distinguish the two (the PARENT-side policy stays, the subagent self-emit is new): "NO `emitCostUpdate` here — the PARENT never pushes the subagent's usage on the PARENT's bus (the fork-delta semantics: only the fork path reports a child's usage, source `subagent:<name>`). The subagent process self-emits its own usage to its OWN bridge (the core self-usage emitter, `subagent-metrics-cost-push` Task 1) — a different bus, a different session; no double-count." (Docs-only — the `NO emitCostUpdate` behavior itself is UNCHANGED; the fork-path no-change rule protects it.)
- Delete the "Suite-side self-usage `cost_update` push" follow-up bullet (done — this plan).
- Keep the other constraints (macOS/Windows fork fallback, the cancel/timeout taxonomy) verbatim.

**Steps:**
- [ ] Read `docs/features/subagent-sessions.md` (the current constraint + follow-up text) + `packages/subagent/src/dispatch.ts` (the success path — `res.metrics` → `usage` + `progressSummary.tokens` — confirm the factual answer above against the ACTUAL code) + `packages/subagent/src/execute.ts` (the bridge branch's `progress`/`usage` synthesis + the :84-86 comment).
- [ ] Rewrite the metrics constraint + the tool-usage line (the factual result/progress distinction above) + update the `execute.ts:84-86` comment (the rewrite above — comment-only, no behavior change) + delete the done follow-up.
- [ ] Run `cd /home/daniel/Coding/Javascript/pi-archimedes && pnpm test` + `pnpm -r exec -- tsc --noEmit` (the `execute.ts` comment change is in the SUITE repo — a comment, but the suite gauntlet is the gate) + `pnpm test` + `pnpm build` in the desktop repo (the docs change is in the DESKTOP repo — the gauntlet is the gate; a docs commit that breaks the build is a process failure).
- [ ] Commit (TWO commits — the change spans both repos): suite repo: "docs: subagent self-usage — refresh the NO-emitCostUpdate bridge-branch comment (the parent policy stays; the subagent self-emit is new)"; desktop repo: "docs: subagent metrics are real (suite self-usage push shipped — the v1 zeros constraint is retired)"

**Acceptance criteria:**
- [ ] The metrics constraint reads present-tense and matches the shipped behavior (per-turn `COST_UPDATE` source `"main"`, desktop accumulator, real `cost`, unchanged wire shape).
- [ ] The tool-usage line writes the FACTUAL result/progress distinction (result `usage`/`progressSummary` real from the response's `metrics`; the synthesized in-flight `progress.*` token fields zeros) — confirmed against `dispatch.ts`'s success path, not assumed.
- [ ] The `execute.ts:84-86` comment distinguishes the parent-side policy (kept) from the subagent self-emit (new); the `NO emitCostUpdate` behavior is unchanged (the suite gauntlet green).
- [ ] The done follow-up is gone; the other constraints are verbatim.

---

## Cross-cutting notes (for the executing agent)

- **The suite's fork path is untouched.** The `subagent` tool's fork emission (`execute.ts` `onProgress`, source `subagent:<name>`, deltas) stays byte-identical — the new emitter is ADDITIVE (a second `COST_UPDATE` source). The footer's `CostAccumulator` sums both (own + fork) → the true session cost — the intentional semantic change (design decision 3a).
- **The bus payload types ARE the wire types** (`events.ts:132-142` — verbatim forwarding). The new emitter's payload MUST match `CostUpdatePayload` (`bus.ts:11-18`) exactly — no new fields (YAGNI: no `totalTokens`, no `reasoning`, no `cacheWrite1h`).
- **Do NOT gate the emitter on bridge mode** (design decision 3) — the test 4 (bridge-inactive emit) pins it. If a future change wants bridge-only emission, it's a one-line `channelActive()` check — but that would regress the TUI footer, which is why it's out.
- **The desktop's main session is unaffected** (its `cost_capture` is `None` — `session.rs:228`): its `cost_update` events flow to `useBridge.cost[sessionId]` (stored, not rendered in v1) with the session's own usage now included — no v1 user-visible change; the stored data becomes the session's own LATEST-TURN delta (design decision 5 — the store is last-payload, NOT a sum; a future cost UI needs its own accumulator).
- **Every Rust commit** passes `cargo clippy --all-targets` (0 warnings) + `cargo fmt --check`; **every suite commit** `pnpm test` + `pnpm -r exec -- tsc --noEmit`; the frontend gate (`pnpm test` + `pnpm build`) runs on the desktop commits (unchanged code, but the gauntlet is the gate).
