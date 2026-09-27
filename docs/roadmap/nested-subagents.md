---
status: committed
done-when: When the main agent delegates a task (the `subagent` tool), the "Delegating" card in the main chat view nests the subagents under it (one row per subagent: a status icon + the agent name + the task + a live one-line activity preview like `read: docs/foo.md · 12s`); clicking a row opens a right-side modal with the subagent's full transcript (closable via X / Esc, without dismissing the entry). The sidebar no longer has a Subagents panel (the `SidePane` keeps its Todos section + the subagent sudo modals). A subagent whose turn never settles is torn down after a 30-minute settle timeout (its `pi` process is killed + it is reported `failed`), so a hung subagent can no longer linger as a zombie. All validation gates are green (`pnpm test`, `pnpm build`, `cargo test`, `cargo clippy --all-targets` with 0 warnings, `cargo fmt --check`).
---

# Nested Subagents Plan

**Goal:** Move subagents out of the sidebar — nest them under the "Delegating" tool card in the main view (with a live activity preview + a dedicated transcript modal) — and fix the zombie-subagent bug (a hung `pi` process lingering forever because the settle wait is unbounded).

**Architecture:** The backend bounds the subagent's settle wait (a 30-min `settle_timeout` on the `SessionDriver`); a hung turn is torn down (the external close kills the `pi` process) + reported `failed` instead of lingering. The frontend special-cases the `subagent` tool in `MessageBubble` (a new `SubagentDelegatingCard` — the `ToolCallCard` header + a nested list of the parent's subagents); clicking a row opens a dedicated modal (the subagent's full transcript, reusing the extracted `SubagentTranscript`). The sidebar Subagents panel is removed (the `SidePane` keeps its Todos section + the subagent sudo modals). An always-mounted hidden host keeps every subagent's prompt cards rendered (the "always mounted" invariant — an unrendered request would hang until the bridge timeout).

**Tech Stack:** Rust (Tauri backend — `tokio` async, the `SessionDriver` / `SubagentSessionManager` / `RpcError`), React 19 + TypeScript (frontend — Zustand stores, the `MessageBubble` / `ToolCallCard` / `SubagentPanel` / `SidePane` components), the `fake_pi` test binary (the RPC wire simulation), `/proc`-scan process assertions.

**Pre-existing uncommitted work this builds on:** the working tree already has (a) `[subagent-dispatch]` diagnostic `eprintln!`s in `SubagentSessionManager::dispatch` (`SPAWNED` / `established` / `prompt send returned` / `settled`), (b) a `dispatch_streams_the_subagents_own_session_updates` test in `subagent_dispatch.rs`, (c) `summarizeSubagentFor` + `subagentActivityLine` + `summarizeSubagentDetails` in `toolOutput.ts`, and (d) the `useSubagents` store (`entries` keyed by subagent session id, `addSession` / `markClosed` / `dismiss`, the `MAX_CLOSED_ENTRIES` = 20 cap). This plan assumes that work stays in the tree; it does NOT revert it.

**The "always mounted" invariant (load-bearing — read before Task 5/6):** a subagent's interactive requests (`ask` / permission / sudo `confirm` / `password`) MUST be rendered by a mounted React component, or they hang until the bridge timeout (`DEFAULT_BRIDGE_TIMEOUT` = 330 s). Today `SubagentDirectory` (in `SidePane`) renders a `SubagentTranscript` per row, ALWAYS mounted (hidden via the `hidden` attribute, never unmounted). Removing `SubagentDirectory` (Task 6) WITHOUT a replacement would break this invariant. Task 5 introduces the replacement: `SubagentDetailHost` (an always-mounted component that renders a hidden `SubagentTranscript` for every subagent entry EXCEPT the selected one, + a visible modal `SubagentTranscript` for the selected one). Every subagent's `SubagentTranscript` is rendered exactly once (selected → modal, non-selected → hidden host), so the invariant holds.

---

### Task 1: Bounded subagent settle (the zombie fix)

**Context:**
The subagent dispatch (`SubagentSessionManager::dispatch` in `src-tauri/src/agent/subagent.rs`) awaits the subagent's turn settle via `SessionDriver::wait_for_settle`, which is currently UNBOUNDED. A subagent whose turn never settles (the observed zombie: a `pi` process alive 14 min, ~2 s CPU, no model calls, no tools) hangs the worker task forever, so the dispatch's `task_cancel.cancel()` (the teardown trigger) is never reached and the `pi` process lingers. This task bounds the settle wait (a 30-min `settle_timeout` on the `SessionDriver`); a hung turn is torn down (the external close kills the `pi` process) + reported `failed` instead of lingering.

Two things change in the dispatch's tail (the `let settle = …` block):
1. `wait_for_settle` becomes BOUNDED (the new `settle_timeout` field).
2. The `let cancelled = *close_probe.borrow();` read moves to BEFORE `task_cancel.cancel()`. Today it is read AFTER `task_cancel.cancel()`, and because `cancel()` unconditionally does `tx.send(true)`, the probe is ALWAYS `true` after the cancel — so the `(Ok(_), Err(e))` arm always reports `"cancelled"`, even for an agent death or a settle timeout. Reading the probe BEFORE the unconditional cancel makes it reflect whether a USER cancel won the race (the flag was already flipped), so a `SettleTimeout` / agent death is reported as its own error (not `"cancelled"`), while a real user cancel still reports `"cancelled"`. `task_cancel.cancel()` is still called (AFTER the read), so the teardown still happens in every case.

The dispatch's `match (prompt, settle)` is otherwise UNCHANGED: the `(Ok(_), Err(e))` arm already maps a `wait_for_settle` `Err` to `failed` + (with the moved probe) the correct error text. The existing `eprintln!("[subagent-dispatch] settled for {sid}: {settle:?}")` already shows the `SettleTimeout` (the `{settle:?}` debug) — no change needed. The teardown-on-completion (a completed subagent's `pi` process is killed) is ALREADY verified by the existing `dispatch_spawns_rpc_child_and_captures` test (the marker process count returns to 1) — re-run it to confirm the roadmap's "teardown on completion" concern.

**Files:**
- Modify: `src-tauri/src/agent/session.rs` (the `SessionDriver` struct + `SessionDriver::new` + `SessionDriver::wait_for_settle`)
- Modify: `src-tauri/src/agent/errors.rs` (the `RpcError` enum)
- Modify: `src-tauri/src/agent/subagent.rs` (the per-dispatch driver construction in `dispatch` + the `cancelled`-read move + a new test in the `#[cfg(test)] mod tests`)

**What to implement:**
1. `SessionDriver` struct (`session.rs`, after the `establish_timeout` field): add `pub(crate) settle_timeout: Duration`, documented as "How long a turn may run (the `wait_for_settle` bound) before it is reported a settle timeout. Default: 30 min."
2. `SessionDriver::new()` (`session.rs`): add `settle_timeout: Duration::from_secs(1800)` (30 min) to the struct literal (next to `establish_timeout: Duration::from_secs(30)`).
3. `RpcError` enum (`errors.rs`): add a `SettleTimeout { detail: String }` variant with `#[error("settle timeout: {detail}")]` (mirror the existing `EstablishTimeout { detail: String }` variant). The enum's existing derives (`Debug`, `thiserror::Error`, `Clone`, `Serialize`, `PartialEq`, `Eq` + `#[serde(tag = "kind", rename_all = "snake_case")]`) apply to the new variant automatically.
4. `SessionDriver::wait_for_settle` (`session.rs`): make it BOUNDED by `self.settle_timeout`. Keep the existing `borrow_and_update` + `if seq == 0` guard. Replace the unbounded `let _ = rx.changed().await;` with:
   ```rust
   match tokio::time::timeout(self.settle_timeout, rx.changed()).await {
       Ok(Ok(_)) => {} // a settle was recorded (fall through)
       Ok(Err(_)) => {
           // The sender was dropped without a settle (the agent died
           // mid-turn, or the session was torn down).
           return Err(RpcError::ProcessExited(None));
       }
       Err(_elapsed) => {
           // The settle timed out (a hung turn — the caller's cancel
           // tears the session down; see the dispatch).
           return Err(RpcError::SettleTimeout {
               detail: format!(
                   "the turn did not settle within {}s",
                   self.settle_timeout.as_secs()
               ),
           });
       }
   }
   ```
   The final `let (seq, reason) = *rx.borrow(); if seq == 0 { Err(ProcessExited(None)) } else { Ok(reason) }` is UNCHANGED. Update the method's doc comment: it is NO LONGER unbounded — it is bounded by the `settle_timeout` (a hung turn can't linger forever; the timeout resolves `Err(SettleTimeout)`, a teardown resolves `Err(ProcessExited)`, a settle resolves `Ok(reason)`).
5. `subagent.rs` `dispatch`: add `settle_timeout: base.settle_timeout` to the per-dispatch `SessionDriver` struct literal (next to `establish_timeout: base.establish_timeout`).
6. `subagent.rs` `dispatch` (the `let settle = …` tail): move `let cancelled = *close_probe.borrow();` to BEFORE `task_cancel.cancel()` (see the Context). The new order:
   ```rust
   let settle = driver.wait_for_settle(&sid).await;
   eprintln!("[subagent-dispatch] settled for {sid}: {settle:?}");
   // Read the cancel probe BEFORE the unconditional teardown cancel: a
   // flipped flag here means a USER cancel won the race (the
   // `(Ok(_), Err(e))` arm reports "cancelled"); a SettleTimeout / agent
   // death has NO flipped flag yet (the arm reports the `wait_for_settle`
   // error, not "cancelled").
   let cancelled = *close_probe.borrow();
   // Ensure teardown on completion or failure.
   task_cancel.cancel();
   match (prompt, settle) { /* UNCHANGED */ }
   ```
   Do NOT change the `match` arms or the `eprintln!`. ALSO reword the existing `close_probe` doc comment (the one above the `let close_probe = ec.rx.clone();` line, `~lines 290–292`): it currently claims a failed-prompt cancel is reported `"cancelled"`, which mismatches the new probe semantics (the probe is read BEFORE the unconditional cancel and is only consulted by the `(Ok(_), Err(e))` arm). Reword it to: "A probe receiver (cloned BEFORE `ec` moves into `drive_session`): read BEFORE the unconditional teardown cancel, a flipped flag means a USER cancel won the race (the `(Ok(_), Err(e))` arm reports `\"cancelled\"`); a `SettleTimeout` / agent death has NO flipped flag yet (the arm reports the `wait_for_settle` error, not `\"cancelled\"`)."

**Steps:**
- [ ] Write the failing test `wait_for_settle_times_out_on_a_hung_turn` in `src-tauri/src/agent/subagent.rs` (the existing `#[cfg(test)] mod tests`, next to `external_close_tears_down_during_establish`), annotated `#[tokio::test(flavor = "multi_thread", worker_threads = 4)]` (the SAME attribute as the two sibling `fake_pi`-spawning tests — required for the multi-thread async). It: (a) `temp_config_dir()` + `write_agents_json_pi(&config_dir, &[("FAKE_PI_HANG_PROMPT", "1")])`; (b) a `TestSink`; (c) `Registry::load` + `registry.get("fake")`; (d) `let mut driver = SessionDriver::new(); driver.settle_timeout = Duration::from_millis(500); let driver = Arc::new(driver);`; (e) `crate::test_support::run_with_retry(|| …)` driving `driver.drive_session(…)` (the same establisher closure as `text_capture_and_last_message_id_track_distinct_messages` — `get_state` → `SessionInfo` with `capabilities: Value::Null`); (f) grab the session's `handle` from `driver.sessions` and `handle.send(serde_json::json!({ "type": "prompt", "content": "hi" })).await.expect("prompt should succeed");` (the `FAKE_PI_HANG_PROMPT` fake acknowledges the prompt WITHOUT settling — the `.await.expect(...)` is REQUIRED: an unused `Result` trips `unused_must_use` and fails the 0-warning clippy gate); (g) `let r = tokio::time::timeout(Duration::from_secs(5), driver.wait_for_settle(&sid)).await.expect("wait_for_settle should resolve (the timeout, not a test hang)").unwrap_err();` then `assert!(matches!(r, RpcError::SettleTimeout { .. }), "expected SettleTimeout, got {r:?}")`; (h) `close_session_internal(&driver, &info.session_id).await;` + `let _ = std::fs::remove_dir_all(&config_dir);` (the `let _ =` is REQUIRED — `remove_dir_all` returns a `#[must_use]` `Result`; an unhandled one trips `unused_must_use` and fails the 0-warning clippy gate, the same class of issue the `.await.expect(...)` in step (f) addresses). Add `use crate::agent::errors::RpcError;` to the test module imports (it is not currently imported there).
- [ ] Run `cargo test wait_for_settle_times_out` (from `src-tauri/` — NOTE: plain `cargo test`, NOT `--lib`: the `fake_pi` binary fixture is a `src/bin/` target and is NOT in `--lib`'s build graph, so `--lib` would fail with `Spawn("No such file or directory")` for the wrong reason. Plain `cargo test` builds `fake_pi` + runs the lib tests.)
  - Did it fail (the `SettleTimeout` variant does not exist yet / `wait_for_settle` is unbounded / the `settle_timeout` field is missing)? If it passed unexpectedly, stop and investigate why.
- [ ] Implement items 1–6 above (the `settle_timeout` field + `new` + bounded `wait_for_settle` + the `RpcError::SettleTimeout` variant + the per-dispatch driver threading + the `cancelled`-read move + the reworded `close_probe` comment).
- [ ] Run `cargo test` (from `src-tauri/` — plain, NOT `--lib`)
  - Did all tests pass (including the new test + the existing `text_capture_and_last_message_id_track_distinct_messages`, which calls `wait_for_settle` and expects `Ok` — the `FAKE_PI_TWO_MSGS` fake settles well within the 30-min default)? If not, fix the failures and re-run before continuing.
- [ ] Run `cargo test --test subagent_dispatch` (from `src-tauri/` — this integration test target builds `fake_pi` itself)
  - Did the existing `dispatch_spawns_rpc_child_and_captures` test pass (the marker process count returns to 1 — the teardown-on-completion is preserved) AND `dispatch_cancellation_tears_down_subagent` (a user cancel still reports `error "cancelled"` — the moved probe still detects a real cancel)? If not, fix and re-run.
- [ ] Run `cargo clippy --all-targets` (from `src-tauri/`)
  - 0 warnings? If not, fix and re-run.
- [ ] Run `cargo fmt` (from `src-tauri/`)
  - Did it succeed? If not, fix and re-run.
- [ ] Commit with message: `fix(agent): bound the subagent settle wait — a hung subagent is torn down + reported failed (the zombie fix)`

**Acceptance criteria:**
- [ ] `wait_for_settle` is bounded by the `settle_timeout` (default 30 min); a hung turn returns `Err(RpcError::SettleTimeout)` within the timeout instead of hanging forever.
- [ ] The dispatch maps the `SettleTimeout` to `failed` (the `(Ok(_), Err(e))` arm) + `task_cancel.cancel()` (the teardown kills the `pi` process); a real user cancel still reports `error "cancelled"` (the moved probe still detects it).
- [ ] A completed subagent's `pi` process is still reaped (the existing `dispatch_spawns_rpc_child_and_captures` test passes — the marker count returns to 1).
- [ ] `cargo test`, `cargo clippy --all-targets` (0 warnings), `cargo fmt --check` are all green.

---

### Task 2: Extract the shared subagent components (refactors)

**Context:**
`SubagentPanel.tsx` currently defines three things: `SubagentDirectory` (the sidebar directory — DELETED in Task 6), `SubagentTranscript` (the read-only transcript — REUSED by the dedicated view in Task 5), and `SubagentModals` (the sudo modals — KEPT, moved in Task 6). Plus two small helpers `StatusIcon` + `STATUS_CHIP_STYLES` (the status icon, reused by `SubagentDelegatingCard` in Task 3). `ToolCallCard.tsx` defines a self-contained header (the `<button>`: icon + verb + summary + stat + failure tooltip + chevron) that `SubagentDelegatingCard` (Task 3) must reuse. This task extracts the reusable pieces into their own files so the later tasks can import them. It is a PURE refactor (no behavior change) — the existing tests must pass unchanged.

**Files:**
- Create: `src/components/SubagentTranscript.tsx`
- Create: `src/components/SubagentStatusIcon.tsx`
- Create: `src/components/ToolCallCardHeader.tsx`
- Create: `src/components/SubagentTranscript.test.tsx` (moved from `SubagentPanel.test.tsx`)
- Modify: `src/components/SubagentPanel.tsx` (delete the moved definitions; import them back)
- Modify: `src/components/ToolCallCard.tsx` (use `ToolCallCardHeader`)
- Modify: `src/components/SubagentPanel.test.tsx` (delete the `SubagentTranscript` describe block — it moves to `SubagentTranscript.test.tsx`)

**What to implement:**
1. `SubagentTranscript.tsx`: move the `SubagentTranscript` component VERBATIM from `SubagentPanel.tsx` (its full doc comment + the `SubagentTranscript` function). ALSO move the module-level `const EMPTY: never[] = []` (with its doc comment, `~lines 32–37`) into `SubagentTranscript.tsx` — the `SubagentTranscript` function uses `EMPTY` at three sites (the stable-selector fallback: `requests ?? EMPTY`, `prompts ?? EMPTY`, `messages ?? EMPTY`); moving the function without it is an undefined reference, and leaving it behind in `SubagentPanel.tsx` trips `noUnusedLocals` (tsconfig `strict` + `noUnusedLocals: true`). It imports from `../store/subagents`, `../store/bridge`, `../store/permissions`, `../store/sessions`, `../lib/toolGroups`, `../lib/toolOutput`, `./Reasoning`, `./MessageBubble`, `./ChangesGroupCard`, `./ToolCallCard`, `./DiffBlock`, `./PermissionPrompt`, `./AskQuestionCard`. Export it as a named export `export function SubagentTranscript`.
2. `SubagentStatusIcon.tsx`: move `StatusIcon` + `STATUS_CHIP_STYLES` VERBATIM from `SubagentPanel.tsx` (the `StatusIcon` function + the `STATUS_CHIP_STYLES` const + their doc comments). Import `CircleAlert`, `CheckCircle2`, `LoaderCircle` from `lucide-react` + `type SubagentEntry` from `../store/subagents`. Export both (`export function StatusIcon`, `export const STATUS_CHIP_STYLES`).
3. `ToolCallCardHeader.tsx`: extract the `ToolCallCard` header (the `<button type="button" onClick={() => setOpen((o) => !o)} className="group/tool-summary …">…</button>` block, `ToolCallCard.tsx:114–191`) into a new `export function ToolCallCardHeader`. Its props: `{ title: string; status: ToolCallUiStatus; rawInput?: unknown; rawOutput?: unknown; open: boolean; onToggle: () => void; files: ToolFileSummary[]; stat: { added: number; removed: number } | undefined; range: string | undefined; isShell: boolean; command: string | undefined }` (NOTE: `diff` is NOT a prop — the header JSX never uses `diff`; the body's `diff` stays a `ToolCallCard` prop. `files`/`stat`/`range`/`isShell`/`command` ARE props — they are computed ONCE in `ToolCallCard` (the body uses them too) and passed in, so the header does NOT recompute them and there is no `SHELL_TOOLS` circular import). The header computes `verb` (`toolVerb(title, status)`), `Icon` (`toolIcon(title)`), `summary` (`summarizeToolCall(title, rawInput)`), and `failure` (`status === "failed" ? failureText(rawOutput) : undefined`) INTERNALLY (move those four lines from `ToolCallCard`), and OWNS the `copied` state + `resetRef` + `handleCopy` + the cleanup `useEffect` (move them from `ToolCallCard`). The `<button>`'s `onClick` becomes `onToggle` (NOT `setOpen` directly). The header JSX is otherwise UNCHANGED (it uses `verb`/`Icon`/`files`/`range`/`stat`/`isShell`/`command`/`summary`/`failure`/`copied`/`open`). The header's COMPLETE import list (the "UNCHANGED" JSX references all of these — a missing one is a build failure): `import { useEffect, useRef, useState } from "react";` (the moved `copied` state + `resetRef` + cleanup `useEffect`), `import { CheckIcon, ChevronRightIcon, CopyIcon } from "lucide-react";` (the failure-tooltip copy button + the chevron), `import FileChip from "./FileChip";` (the file chip), `import DiffCount from "./DiffCount";` (the change stat), `import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "./ui/tooltip";` (the failure tooltip), `import { failureText, summarizeToolCall, toolIcon, toolVerb, type ToolFileSummary } from "../lib/toolOutput";` (the four computed values + the `files` prop type), `import type { ToolCallUiStatus } from "../store/sessions";` (the `status` prop type — import ONLY `ToolCallUiStatus`, NOT `DiffRef`: `diff` is not a prop, so `DiffRef` is unused in the header and would trip `noUnusedLocals`).
4. `ToolCallCard.tsx`: replace the inline header `<button>` with `<ToolCallCardHeader title={title} status={status} rawInput={rawInput} rawOutput={rawOutput} open={open} onToggle={() => setOpen((o) => !o)} files={files} stat={stat} range={range} isShell={isShell} command={command} />`. KEEP in `ToolCallCard` (the body uses them): the `open` state, the `autoOpen` logic (`prevStatusRef` + `hasAutoOpenedRef` + the `useEffect`), the `output` computation (`normalizeToolOutput`), the body (the `{open && (…)}` block), AND the `files`/`stat`/`range`/`isShell`/`command` computations + the `SHELL_TOOLS` definition + the `fileSummaries`/`editChangeStat`/`readLineRange` imports (they are computed ONCE here and passed to the header as props — do NOT delete them; the body's `$`-command block + file-chip block need them). DELETE from `ToolCallCard` ONLY the now-moved lines: the `copied` state, `resetRef`, `handleCopy`, the cleanup `useEffect`, and the `verb`/`Icon`/`summary`/`failure` computations (they live in `ToolCallCardHeader` now). Remove the imports that are no longer used in `ToolCallCard`: `CheckIcon`, `CopyIcon`, `ChevronRightIcon` (all three from the `lucide-react` import — the header that used them moved out; after the move `ToolCallCard` uses NO `lucide-react` icons, so remove the `lucide-react` import line entirely), `Tooltip`/`TooltipContent`/`TooltipProvider`/`TooltipTrigger` (the failure tooltip moved to the header), `failureText`, `toolIcon`, `toolVerb`, `summarizeToolCall`. KEEP the imports the body + the kept computations still use: `normalizeToolOutput`, `fileSummaries`, `editChangeStat`, `readLineRange`, `DiffBlock`, `FileChip`, `DiffCount`, `type DiffRef`, `type ToolCallUiStatus`.
5. `SubagentPanel.tsx`: DELETE the `SubagentTranscript` + `StatusIcon` + `STATUS_CHIP_STYLES` + `EMPTY` definitions (all moved out in items 1–2; `EMPTY` moves to `SubagentTranscript.tsx`, `StatusIcon`/`STATUS_CHIP_STYLES` to `SubagentStatusIcon.tsx`). Add imports: `import { SubagentTranscript } from "./SubagentTranscript";` + `import { StatusIcon, STATUS_CHIP_STYLES } from "./SubagentStatusIcon";`. PRUNE the imports that ONLY `SubagentTranscript` used (moving it out orphans them → `noUnusedLocals` build failure): remove `usePermissions` from `../store/permissions`, `useSessions` from `../store/sessions`, `groupConsecutiveFileWrites` from `../lib/toolGroups`, `summarizeSubagentFor` from `../lib/toolOutput`, `Reasoning`/`ReasoningTrigger`/`ReasoningContent` from `./Reasoning`, `MessageBubble` from `./MessageBubble`, `ChangesGroupCard` from `./ChangesGroupCard`, `ToolCallCard` from `./ToolCallCard`, `DiffBlock` from `./DiffBlock`, `PermissionPrompt` from `./PermissionPrompt`, `AskQuestionCard` from `./AskQuestionCard` (KEEP the imports `SubagentDirectory` + `SubagentModals` still use — `X` from `lucide-react` (the `DirectoryRow` close button; the status icons `CircleAlert`/`CheckCircle2`/`LoaderCircle` move WITH `StatusIcon` to `SubagentStatusIcon.tsx` in item 2 — do NOT keep them here or they trip `noUnusedLocals`), `useEffect`/`useState` from `react`, `useShallow` from `zustand/react/shallow`, `useSubagents`/`SubagentEntry` from `../store/subagents`, `useBridge`/`BridgeRequestData` from `../store/bridge`, `SudoConfirmModal`/`SudoPasswordModal` — verify the final set with `pnpm build`). `SubagentDirectory` + `SubagentModals` stay in `SubagentPanel.tsx` (they are deleted/moved in Task 6). `SubagentDirectory`'s `DirectoryRow` uses `StatusIcon` (now imported) — UNCHANGED.
6. `SubagentTranscript.test.tsx`: move the `describe("SubagentTranscript …")` block VERBATIM from `SubagentPanel.test.tsx` (all its `it`s + the test-setup helpers it uses). Update its imports to `import { SubagentTranscript } from "./SubagentTranscript";` (instead of from `./SubagentPanel`). Keep the store imports (`useSubagents`, `useBridge`, `usePermissions`, `useSessions`).
7. `SubagentPanel.test.tsx`: DELETE the `describe("SubagentTranscript …")` block (moved to `SubagentTranscript.test.tsx`). DROP `SubagentTranscript` from the `import { SubagentDirectory, SubagentModals, SubagentTranscript } from "./SubagentPanel";` line (it no longer lives there → `noUnusedLocals` build failure; the file keeps importing `SubagentDirectory` + `SubagentModals`, which still live in `SubagentPanel.tsx` until Task 6). KEEP the `describe("SubagentDirectory …")` + `describe("SubagentModals …")` blocks (they stay — `SubagentDirectory` + `SubagentModals` are still in `SubagentPanel.tsx` until Task 6).

**Steps:**
- [ ] Create `src/components/SubagentTranscript.tsx` (move `SubagentTranscript` verbatim) + `src/components/SubagentStatusIcon.tsx` (move `StatusIcon` + `STATUS_CHIP_STYLES` verbatim) + `src/components/ToolCallCardHeader.tsx` (extract the header).
- [ ] Modify `SubagentPanel.tsx` (delete the moved definitions; import them back) + `ToolCallCard.tsx` (use `ToolCallCardHeader`; delete the moved lines; prune imports).
- [ ] Create `src/components/SubagentTranscript.test.tsx` (move the `SubagentTranscript` describe block) + modify `SubagentPanel.test.tsx` (delete the moved block).
- [ ] Run `pnpm test` (repo root)
  - Did ALL tests pass (the moved `SubagentTranscript` tests in the new file + the remaining `SubagentDirectory` / `SubagentModals` tests + the `ToolCallCard` tests — the refactors are no-ops)? If not, fix the failures (likely a missed import or a dangling reference) and re-run before continuing.
- [ ] Run `pnpm build` (repo root)
  - Did the type-check + build succeed (no unused-import / missing-import errors)? If not, fix and re-run.
- [ ] Commit with message: `refactor(subagent): extract SubagentTranscript, SubagentStatusIcon, and the ToolCallCard header into shared components`

**Acceptance criteria:**
- [ ] `SubagentTranscript`, `StatusIcon` + `STATUS_CHIP_STYLES`, and the `ToolCallCard` header are in their own files; `SubagentPanel.tsx` + `ToolCallCard.tsx` import them.
- [ ] `pnpm test` + `pnpm build` are green (the refactors are behavior-preserving).
- [ ] `SubagentPanel.tsx` still defines `SubagentDirectory` + `SubagentModals` (they are removed/moved in Task 6).

---

### Task 3: Create `SubagentDelegatingCard` + the selection store + the one-line activity

**Context:**
The "Delegating" (subagent) tool card must nest the subagents under it (the design: agent name + task + status + a one-line live activity preview). This task creates three things: (a) a `useSubagentSelection` store (the shared "which subagent is open in the modal" state — the card's rows write it, the `SubagentDetailHost` in Task 5 reads it), (b) a `subagentActivityFor` function in `toolOutput.ts` (the ONE-LINE activity for a single subagent — the roadmap's `summarizeSubagentFor` returns a multi-line `<agent>: <task>` + activity block, but the nested-list preview is ONE line, e.g. `read: docs/foo.md · 12s`, so this new function returns just the `subagentActivityLine` of the matching entry), and (c) the `SubagentDelegatingCard` component (the `ToolCallCardHeader` + the nested list). The card filters `useSubagents` entries by `parentSessionId === sessionId` (the `sessionId` is the parent's ACP id, passed by `MessageBubble` in Task 4).

**Files:**
- Create: `src/store/subagentSelection.ts`
- Create: `src/store/subagentSelection.test.ts`
- Create: `src/components/SubagentDelegatingCard.tsx`
- Create: `src/components/SubagentDelegatingCard.test.tsx`
- Modify: `src/lib/toolOutput.ts` (add `subagentActivityFor`)
- Modify: `src/lib/toolOutput.test.ts` (add `subagentActivityFor` tests)

**What to implement:**
1. `src/store/subagentSelection.ts`:
   ```ts
   import { create } from "zustand";
   interface SubagentSelectionState {
     /** The subagent session id open in the dedicated modal (`null` = closed). */
     selectedSessionId: string | null;
     /** Open (or close, with `null`) the dedicated modal for a subagent. */
     select: (sessionId: string | null) => void;
   }
   export const useSubagentSelection = create<SubagentSelectionState>((set) => ({
     selectedSessionId: null,
     select: (sessionId) => set({ selectedSessionId: sessionId }),
   }));
   ```
2. `src/lib/toolOutput.ts`: add an `export function subagentActivityFor(details: unknown, sessionId: string, task: string): string | undefined`. **Filter FIRST, then prefer** (mirror `summarizeSubagentFor`'s order — NOT prefer-then-filter): (a) return `undefined` when `details` is not an object OR is `null` (mirror `summarizeSubagentFor`'s exact guard `typeof details !== "object" || details === null` — `typeof null === "object"`, so the `null` check is required or `d[key]` throws); (b) build the FILTERED `progress` + `results` arrays exactly like `summarizeSubagentFor`'s `pick` (each entry matches if `typeof childSessionId === "string" ? childSessionId === sessionId : typeof task === "string" && task === <the passed task>`); (c) apply `summarizeSubagentDetails`'s preference ON THE FILTERED arrays (prefer the live `progress` while any filtered **`progress`** entry is `running` — i.e. `filteredProgress.some((p) => p.status === "running")`, the SAME predicate as `summarizeSubagentDetails` which checks only the `progress` entries — else the filtered `results` if non-empty, else the filtered `progress`); (d) return `subagentActivityLine(entry)` for the FIRST entry of the chosen filtered array (the ONE-LINE activity), or `undefined` when the chosen array is empty. (Filtering first matters for the mixed multi-subagent case — one `subagent` tool call's `details` can carry multiple subagents: with sub A finished while sub B runs, prefer-then-filter would pick `progress` (B running) for A's row → a stale preview; filter-first picks A's finished entry from `results`.) Do NOT modify `summarizeSubagentFor` / `summarizeSubagentDetails` / `subagentActivityLine` (they are used by `SubagentTranscript`'s progress fallback; `subagentActivityLine` is module-private but `subagentActivityFor` lives in the same file, so it can call it directly).
3. `src/components/SubagentDelegatingCard.tsx` (a DEFAULT export — `export default function SubagentDelegatingCard(…)`; Task 4 imports it as a default `import SubagentDelegatingCard from "./SubagentDelegatingCard"`):
   ```tsx
   import { useState } from "react";
   import { useSubagents } from "../store/subagents";
   import { useSubagentSelection } from "../store/subagentSelection";
   import { subagentActivityFor } from "../lib/toolOutput";
   import { StatusIcon, STATUS_CHIP_STYLES } from "./SubagentStatusIcon";
   import { ToolCallCardHeader } from "./ToolCallCardHeader";
   import type { ToolCallUiStatus } from "../store/sessions";
   ```
   - Props: `{ title: string; status: ToolCallUiStatus; rawInput?: unknown; rawOutput?: unknown; sessionId: string }` (NO `diff` prop — the `subagent` tool never produces a diff, and the `ToolCallCardHeader` does not take `diff`; an unused `diff` prop would trip `noUnusedLocals`).
   - `const entries = useSubagents((s) => s.entries);` + `const childEntries = Object.values(entries).filter((e) => e.parentSessionId === sessionId);`
   - `const select = useSubagentSelection((s) => s.select);`
   - **`open` defaults to `true`** (the card is OPEN by default — the subagents are the "delegating" detail the user wants to see; this is a deliberate deviation from the `ToolCallCard`'s collapsed-by-default, and it is what makes the empty state render in the no-entries tests): `const [open, setOpen] = useState(true);`. There is NO auto-open `useEffect` (the card is always open by default; the user closes it via the chevron and the close sticks — it is a plain `useState` toggle). Do NOT import `useEffect`/`useRef` (they are unused).
   - **Session-level grouping (documented behavior, not a bug):** the filter is `parentSessionId === sessionId` (the parent's ACP id). In a session with MULTIPLE `subagent` tool calls, EVERY `SubagentDelegatingCard` lists ALL of that session's subagent rows (the `subagent-session-started` payload carries no `toolCallId`, so per-card correlation is impossible with today's payloads). The activity `preview` resolves ONLY from each card's OWN `rawOutput.details` (a sibling card's row gets `preview === undefined` and renders no activity line). This is accepted as the intended session-level grouping.
   - Render: `<div className="w-full">` + `<ToolCallCardHeader title={title} status={status} rawInput={rawInput} rawOutput={rawOutput} open={open} onToggle={() => setOpen((o) => !o)} files={[]} stat={undefined} range={undefined} isShell={false} command={undefined} />` (the `files`/`stat`/`range`/`isShell`/`command` are the CORRECT defaults for the `subagent` tool — it is never a file/shell tool, so `fileSummaries`/`editChangeStat`/`readLineRange` would return `[]`/`undefined`/`undefined`/`false`/`undefined` anyway; passing the defaults avoids recomputing them) + the nested list `{open && childEntries.length > 0 && (…body…)}`.
   - The nested-list body: `<div className="mt-1 rounded-xl border border-border bg-panel px-4 py-3">` containing one row per `childEntries`:
     ```tsx
     {childEntries.map((entry) => {
       const preview = subagentActivityFor(
         (rawOutput as { details?: unknown } | undefined)?.details,
         entry.sessionId,
         entry.task,
       );
       return (
         <button
           key={entry.sessionId}
           type="button"
           onClick={() => select(entry.sessionId)}
           aria-label={`Open ${entry.agentName} transcript`}
           className="flex w-full items-start gap-2 rounded-lg px-2 py-1.5 text-left hover:bg-surface-hover focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
         >
           <span className="mt-0.5"><StatusIcon status={entry.status} /></span>
           <div className="min-w-0 flex-1">
             <div className="flex items-center gap-2">
               <span className="min-w-0 truncate text-ui-base font-medium">{entry.agentName}</span>
               <span className={`shrink-0 text-ui-xs ${STATUS_CHIP_STYLES[entry.status]}`}>{entry.status}</span>
             </div>
             {entry.task && <p className="truncate text-ui-sm text-foreground-subtle">{entry.task}</p>}
             {preview && <p className="truncate text-ui-xs text-foreground-subtlest">{preview}</p>}
           </div>
         </button>
       );
     })}
     ```
     (the `preview` is computed per row via `subagentActivityFor` — the ONE-LINE activity; `undefined` when no `details` entry matches, in which case the row shows no activity line).
   - Empty state: render `<p className="font-mono text-ui-base text-foreground-subtle">No subagents yet.</p>` as a **sibling of the nested list, directly under the header** (NOT inside the `{open && childEntries.length > 0 && (…body…)}` region, which is gated on `childEntries.length > 0` and would never show an empty state) — the condition is `open && childEntries.length === 0` (this renders because `open` defaults to `true`). The render structure: `<div className="w-full">` + `<ToolCallCardHeader … />` + `{open && childEntries.length > 0 && (<div className="mt-1 …">…rows…</div>)}` + `{open && childEntries.length === 0 && (<p className="…">No subagents yet.</p>)}`.

**Steps:**
- [ ] Write the failing `subagentActivityFor` tests in `src/lib/toolOutput.test.ts`: (a) a `progress` entry matching by `task` returns its one-line activity (e.g. a `running` entry with `currentTool: "read"` + `currentToolArgs: "docs/foo.md"` + `currentToolStartedAt` → `read: docs/foo.md · …s`); (b) a `results` entry matching by `childSessionId` returns its one-line activity (e.g. a final `results` entry with `exitCode: 0` + `finalOutput` → the last line, or `Done`); (c) `undefined` when no entry matches the `sessionId`/`task`; (d) `undefined` when `details` is `undefined`/non-object.
- [ ] Run `pnpm test toolOutput` (repo root — NOTE: vitest's positional argument filters TEST FILE PATHS, not test names; `pnpm test subagentActivityFor` matches NO file and exits with code 1 "No test files found" for the wrong reason. `pnpm test toolOutput` runs `src/lib/toolOutput.test.ts`, which now contains the new `subagentActivityFor` tests.)
  - Did it fail (the new `subagentActivityFor` tests fail because the function does not exist yet)? If it passed unexpectedly, stop and investigate why.
- [ ] Implement `subagentActivityFor` in `src/lib/toolOutput.ts`.
- [ ] Run `pnpm test toolOutput` (repo root)
  - Did all the `subagentActivityFor` tests pass (alongside the pre-existing `toolOutput` tests)? If not, fix and re-run.
- [ ] Write the failing `SubagentDelegatingCard` tests in `src/components/SubagentDelegatingCard.test.tsx`: (a) renders one nested row per subagent entry for the parent (seed `useSubagents` with 2 entries whose `parentSessionId` matches the card's `sessionId`; assert both agent names + tasks render); (b) renders NO rows for entries whose `parentSessionId` does NOT match (seed an entry with a different `parentSessionId`; assert it does not render); (c) the empty state (`No subagents yet.`) when there are no matching entries; (d) the activity preview updates live (seed a `running` entry + set the matching `subagent` tool call's `rawOutput.details.progress`; assert the activity line renders; then update `details` and assert the preview re-renders); (e) clicking a row calls `useSubagentSelection.select` with the entry's `sessionId` (assert the store's `selectedSessionId` updates).
- [ ] Run `pnpm test SubagentDelegatingCard` (repo root)
  - Did it fail (the component does not exist yet)? If it passed unexpectedly, stop and investigate why.
- [ ] Implement `SubagentDelegatingCard` + `useSubagentSelection` (items 1 + 3 above).
- [ ] Run `pnpm test` (repo root)
  - Did all tests pass? If not, fix the failures and re-run before continuing.
- [ ] Run `pnpm build` (repo root)
  - Did the type-check + build succeed? If not, fix and re-run.
- [ ] Commit with message: `feat(subagent): SubagentDelegatingCard — nest subagents under the Delegating tool card (name + task + status + live activity)`

**Acceptance criteria:**
- [ ] `SubagentDelegatingCard` renders one nested row per subagent entry for the parent (name + task + status icon + one-line activity preview); an empty state when there are no subagents; the preview updates live on a `details` change.
- [ ] Clicking a row sets `useSubagentSelection.selectedSessionId` to the entry's `sessionId`.
- [ ] `subagentActivityFor` returns the one-line activity for a matching entry; `undefined` when nothing matches.
- [ ] `pnpm test` + `pnpm build` are green.

---

### Task 4: Wire `SubagentDelegatingCard` into `MessageBubble`

**Context:**
The `subagent` tool call must render `SubagentDelegatingCard` (not the plain `ToolCallCard`) in the main chat. `MessageBubble`'s `tool-call` branch currently renders a plain `ToolCallCard` for every tool. This task special-cases `title === "subagent"`: it renders `SubagentDelegatingCard` (passing the session id, so the card can filter `useSubagents` by `parentSessionId`). `MessageBubble` learns the session id via a new OPTIONAL `sessionId` prop (the `tool-call` branch only uses it for the `subagent` case; every other kind ignores it). `ChatStream` (the only `MessageBubble` caller in the main chat) passes `activeSessionId`. `SubagentTranscript` ALSO renders `MessageBubble`, but ONLY for the subagent's `agent-text` messages (its `agent-thought` messages go through `Reasoning` and its `tool-call` messages go through `ToolCallCard` DIRECTLY — not through `MessageBubble`); it does NOT pass `sessionId`. Because a subagent cannot dispatch subagents (`subagent: None`), a subagent session never has a `subagent` tool call, so the `subagent` branch is never taken in the `SubagentTranscript` context — the optional prop defaulting to `undefined` is safe there.

**Files:**
- Modify: `src/components/MessageBubble.tsx`
- Modify: `src/components/ChatStream.tsx`
- Modify: `src/components/MessageBubble.test.tsx`

**What to implement:**
1. `MessageBubble.tsx`: add an OPTIONAL `sessionId?: string` prop to the component signature: `function MessageBubble({ message, isStreaming = false, sessionId }: { message: Message; isStreaming?: boolean; sessionId?: string })`. In the `case "tool-call":` branch, special-case `subagent`:
   ```tsx
   case "tool-call":
     return (
       <div className="w-full">
         {message.title === "subagent" ? (
           <SubagentDelegatingCard
             title={message.title}
             status={message.status}
             rawInput={message.rawInput}
             rawOutput={message.rawOutput}
             sessionId={sessionId ?? ""}
           />
         ) : (
           <ToolCallCard
             title={message.title}
             status={message.status}
             diff={message.diff}
             rawInput={message.rawInput}
             rawOutput={message.rawOutput}
           />
         )}
       </div>
     );
   ```
   Add `import SubagentDelegatingCard from "./SubagentDelegatingCard";`. (The `SubagentDelegatingCard` does NOT take a `diff` prop — the `subagent` tool never produces a diff, so `message.diff` is not passed to it; the plain `ToolCallCard` DOES take `diff` (unchanged). Pass `sessionId ?? ""` so the card always gets a string; when `sessionId` is `undefined` — the `SubagentTranscript` case — no subagent entry can match `parentSessionId === ""`, so the card renders its empty state, which is correct because a subagent never has a `subagent` tool call.)
2. `ChatStream.tsx`: at the `MessageBubble` render site (the `units.map` body, `~line 845`), pass `sessionId={activeSessionId}`:
   ```tsx
   <MessageBubble
     key={`${activeSessionId}:${i}`}
     message={message}
     isStreaming={…}
     sessionId={activeSessionId}
   />
   ```
   (`activeSessionId` is already in scope — it is the `useSessions((s) => s.activeSessionId)` value used throughout `ChatStream`.)
3. `MessageBubble.test.tsx`: add tests: (a) a `tool-call` message with `title: "subagent"` renders `SubagentDelegatingCard` (assert a marker unique to `SubagentDelegatingCard` — e.g. the `No subagents yet.` empty state, since the test seeds no subagent entries — and assert the plain `ToolCallCard`'s output body is NOT rendered); (b) a `tool-call` message with a NON-`subagent` title (e.g. `bash`) renders the plain `ToolCallCard` (UNCHANGED — assert its existing behavior, e.g. the verb + command); (c) a `subagent` `tool-call` with a seeded `useSubagents` entry matching the `sessionId` renders the nested row (assert the agent name).

**Steps:**
- [ ] Write the failing `MessageBubble` tests (item 3 above) in `src/components/MessageBubble.test.tsx`.
- [ ] Run `pnpm test MessageBubble` (repo root)
  - Did it fail (the `subagent` case still renders the plain `ToolCallCard` / the `sessionId` prop does not exist yet)? If it passed unexpectedly, stop and investigate why.
- [ ] Implement the `MessageBubble` `sessionId` prop + the `subagent` special-case (item 1) + the `ChatStream` `sessionId={activeSessionId}` (item 2).
- [ ] Run `pnpm test` (repo root)
  - Did all tests pass (the new `MessageBubble` tests + the existing `MessageBubble` / `ChatStream` tests — the non-`subagent` tools are unchanged)? If not, fix the failures and re-run before continuing.
- [ ] Run `pnpm build` (repo root)
  - Did the type-check + build succeed? If not, fix and re-run.
- [ ] Commit with message: `feat(subagent): MessageBubble renders SubagentDelegatingCard for the subagent tool (the nested card in the main view)`

**Acceptance criteria:**
- [ ] A `tool-call` message with `title === "subagent"` renders `SubagentDelegatingCard` (not the plain `ToolCallCard`); every other tool renders the plain `ToolCallCard` (unchanged).
- [ ] `ChatStream` passes `activeSessionId` to `MessageBubble`; `SubagentTranscript`'s `MessageBubble` uses do not pass it (the optional prop defaults to `undefined`).
- [ ] `pnpm test` + `pnpm build` are green.

---

### Task 5: Create the dedicated transcript view (the modal + the always-mounted host)

**Context:**
Clicking a nested subagent row (Task 3) must open a dedicated view showing the subagent's full transcript. This task creates `SubagentDetailHost` — a single ALWAYS-MOUNTED component (rendered at the `App` root) that: (a) renders a visible modal (a `fixed` right-side sheet) for the SELECTED subagent (from `useSubagentSelection.selectedSessionId`), and (b) renders a HIDDEN `SubagentTranscript` for every NON-selected subagent entry. This is the replacement for the "always mounted" role that `SubagentDirectory` played (Task 6 removes it): every subagent's `SubagentTranscript` is rendered exactly once (selected → modal, non-selected → hidden host), so the "always mounted" invariant holds (a subagent's prompt cards are always rendered, so they never hang until the bridge timeout). The modal is closable (X / Esc) and does NOT dismiss the subagent entry (closing only sets `selectedSessionId` to `null` — the entry stays in `useSubagents`).

**Files:**
- Create: `src/components/SubagentDetailHost.tsx`
- Create: `src/components/SubagentDetailHost.test.tsx`
- Modify: `src/App.tsx`

**What to implement:**
1. `src/components/SubagentDetailHost.tsx` (a DEFAULT export — `export default function SubagentDetailHost()`; Task 5's `App.tsx` wiring imports it as a default `import SubagentDetailHost from "./components/SubagentDetailHost"`):
   - `const entries = useSubagents((s) => s.entries);` + `const entryList = Object.values(entries);`
   - `const selectedId = useSubagentSelection((s) => s.selectedSessionId);` + `const select = useSubagentSelection((s) => s.select);`
   - `const selected = entryList.find((e) => e.sessionId === selectedId);` (the selected entry, or `undefined`).
   - **Esc-to-close (React `onKeyDown` on the sheet — NOT a window-level handler):** put `onKeyDown={handleEsc}` on the modal's OUTER `div` (the `fixed` sheet), where `handleEsc = (e: React.KeyboardEvent) => { if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); select(null); } }`. **Why a sheet `onKeyDown` and NOT a window-level capture handler:** `ChatStream.tsx` (`~lines 249–263`) registers a WINDOW-LEVEL `keydown` listener (active while `inTurn`) that calls `cancelSession(activeSessionId)` on Escape. A window-level CAPTURE handler would run first and (a) close the modal on EVERY Escape (even ones consumed by a nested consumer) and (b) `stopPropagation()` the event so the nested consumer's own Esc handler never runs — a REGRESSION for `AskQuestionCard`'s documented Esc-dismiss (`AskQuestionCard.tsx:225–231`) and the stacked sudo modals (`SudoPasswordModal`/`SudoConfirmModal`, rendered at the `SidePane` root by `SubagentModals`). A React `onKeyDown` on the sheet avoids this: the sheet's `e.stopPropagation()` (which React forwards to `nativeEvent.stopPropagation()`) stops the native event at the React root BEFORE it reaches `window`, so `ChatStream`'s window listener does NOT fire (the turn is NOT cancelled); AND because the sheet handler is a PARENT React handler, a nested consumer's own `stopPropagation()` (e.g. `AskQuestionCard`'s Esc-dismiss) stops the event before it reaches the sheet, so the modal does NOT close when a nested consumer consumes the Esc. (The sudo modals are `fixed` overlays at the `SidePane` root — NOT in the sheet's subtree — so their Esc handlers run independently and the sheet handler does not fire for them.) **Known limitation (accepted):** Esc only closes the modal when focus is INSIDE the sheet (the user clicked a row to open it, so focus is in it); an Esc with focus OUTSIDE the sheet does not close it (the user uses the `X` or re-clicks a row). Add a test: "Esc inside a nested `AskQuestionCard` does NOT close the modal" (seed an `ask` request for the selected subagent; `fireEvent.keyDown` the `AskQuestionCard`'s root with `Escape`; assert the modal header is STILL present).
   - **The hidden host:** `entryList.filter((e) => e.sessionId !== selectedId).map((e) => (<div key={e.sessionId} hidden><SubagentTranscript sessionId={e.sessionId} /></div>))` — a hidden `SubagentTranscript` for every NON-selected entry (the `hidden` attribute = CSS `display: none`; the component stays MOUNTED, so the prompt cards are rendered — the invariant).
   - **The modal:** a `fixed` right-side sheet, always mounted, hidden when `selected` is `undefined`:
     ```tsx
     <div
       className={
         "fixed top-0 right-0 z-50 flex h-full w-[480px] max-w-[90vw] flex-col border-l border-border bg-background-alt shadow-xl " +
         (selected ? "" : "hidden")
       }
     >
       {selected && (
         <>
           <div className="flex items-center justify-between gap-2 border-b border-border/50 p-3">
             <div className="flex min-w-0 items-center gap-2">
               <StatusIcon status={selected.status} />
               <span className="min-w-0 truncate text-ui-base font-medium">{selected.agentName}</span>
               <span className={`shrink-0 text-ui-xs ${STATUS_CHIP_STYLES[selected.status]}`}>{selected.status}</span>
             </div>
             <button
               type="button"
               onClick={() => select(null)}
               aria-label="Close subagent transcript"
               className="flex size-6 shrink-0 items-center justify-center text-foreground-subtle hover:text-foreground"
             >
               <X className="size-4" />
             </button>
           </div>
           <div className="flex-1 overflow-y-auto p-3">
             <SubagentTranscript sessionId={selected.sessionId} />
           </div>
         </>
       )}
     </div>
     ```
     (Import `X` from `lucide-react`, `StatusIcon` + `STATUS_CHIP_STYLES` from `./SubagentStatusIcon`, `SubagentTranscript` from `./SubagentTranscript`, `useSubagents` from `../store/subagents`, `useSubagentSelection` from `../store/subagentSelection`.)
   - The modal's OUTER `div` is ALWAYS mounted (the `hidden` class toggles `display: none`); the INNER content (`{selected && …}`) is conditionally mounted. When the user closes the modal (`select(null)`), the selected entry's `SubagentTranscript` moves from the modal (unmounted) to the hidden host (mounted) in the SAME render (it is no longer `selected`, so the hidden host now includes it) — so the prompt cards stay rendered (no gap). **Known behavior (accepted, not a bug):** the modal's `SubagentTranscript` and the hidden host's `SubagentTranscript` are DIFFERENT component instances, so any LOCAL state in the transcript's children (e.g. text typed into an `AskQuestionCard` reply) RESETS when the modal opens or closes (the instance is remounted on the swap). The PROMPT CARDS themselves stay rendered (the invariant holds) — only their local UI state resets. Do NOT treat this as a bug or try to "fix" it by lifting the instance (that would break the exactly-once rendering).

2. `src/App.tsx`: add `import SubagentDetailHost from "./components/SubagentDetailHost";` and render `<SubagentDetailHost />` at the root (next to `<SpacesList />` / `<ChatStream />` / `<SidePane />` — it is a `fixed` overlay, so its position in the flex row does not affect layout).

**Steps:**
- [ ] Write the failing `SubagentDetailHost` tests in `src/components/SubagentDetailHost.test.tsx`: (a) the modal is HIDDEN (no visible header) when `selectedSessionId` is `null`; (b) the modal OPENS (the header with the agent name renders) when `useSubagentSelection.select("sub1")` is called AND a `sub1` entry exists; (c) the modal shows the transcript (seed `useSessions.messages["sub1"]` with an `agent-text` message; assert it renders in the modal); (d) the modal CLOSES on the X button (`fireEvent.click` the close button; assert the header is gone + `selectedSessionId` is `null`); (e) the modal CLOSES on `Esc` (`fireEvent.keyDown(screen.getByRole("dialog", { name: /transcript/ }), { key: "Escape" })`; assert the header is gone) — **fire on the SHEET (an element inside the modal), NOT `window`**, since the implementation is a React `onKeyDown` on the sheet (a `window`-targeted keydown would never reach it and would contradict the known-limitation text); (e2) optionally add a companion assertion that a sheet-scoped Esc does NOT reach `ChatStream`'s window listener (mock `useSessions`'s `cancelSession`; assert it is NOT called) to lock the "Esc closing the modal must not cancel the turn" requirement; (f) closing does NOT dismiss the entry (after closing, the `useSubagents` entry still exists — assert `useSubagents.getState().entries["sub1"]` is defined); (g) a NON-selected entry's `SubagentTranscript` is rendered (hidden) — seed a `sub2` entry + a `sub1` selection; assert `sub2`'s transcript content is in the DOM (the hidden host).
- [ ] Run `pnpm test SubagentDetailHost` (repo root)
  - Did it fail (the component does not exist yet)? If it passed unexpectedly, stop and investigate why.
- [ ] Implement `SubagentDetailHost` (item 1) + wire it into `App.tsx` (item 2).
- [ ] Run `pnpm test` (repo root)
  - Did all tests pass? If not, fix the failures and re-run before continuing.
- [ ] Run `pnpm build` (repo root)
  - Did the type-check + build succeed? If not, fix and re-run.
- [ ] Commit with message: `feat(subagent): dedicated transcript modal (SubagentDetailHost) — the always-mounted host keeps every subagent's prompt cards rendered`

**Acceptance criteria:**
- [ ] Clicking a nested subagent row (Task 3's `select`) opens the modal showing the subagent's full transcript (the `SubagentTranscript`); the modal is closable via X / Esc and does NOT dismiss the entry.
- [ ] Every subagent's `SubagentTranscript` is rendered exactly once (selected → modal, non-selected → hidden host) — the "always mounted" invariant holds (a subagent's prompt cards are always rendered).
- [ ] `pnpm test` + `pnpm build` are green.

---

### Task 6: Remove the sidebar Subagents panel

**Context:**
The sidebar Subagents panel (`SubagentDirectory`, rendered by `SidePane`) is removed — all subagents now live under the "Delegating" card (Task 3/4) + the dedicated modal (Task 5). `SidePane` KEEPS its Todos section (`TodoBoardPanel`) + the subagent sudo modals (`SubagentModals` — the subagent sessions still exist; their sudo `confirm` / `password` requests still need rendering). `SubagentModals` is moved to its own file (it is the only survivor of `SubagentPanel.tsx` alongside the now-deleted `SubagentDirectory`). The `SidePane` auto open/close logic drops the `runningSubagents` term (the pane is driven by todos only — a subagent no longer opens the pane). `SubagentPanel.tsx` + `SubagentPanel.test.tsx` are DELETED (the `SubagentDirectory` tests are dropped — the component is gone; the `SubagentTranscript` tests already moved to `SubagentTranscript.test.tsx` in Task 2; the `SubagentModals` tests move to `SubagentModals.test.tsx`).

**Files:**
- Create: `src/components/SubagentModals.tsx`
- Create: `src/components/SubagentModals.test.tsx` (moved from `SubagentPanel.test.tsx`)
- Modify: `src/components/SidePane.tsx`
- Modify: `src/components/SidePane.test.tsx`
- Delete: `src/components/SubagentPanel.tsx`
- Delete: `src/components/SubagentPanel.test.tsx`

**What to implement:**
1. `SubagentModals.tsx`: move the `SubagentModals` component VERBATIM from `SubagentPanel.tsx` (its full doc comment + the function). Import `SudoConfirmModal` + `SudoPasswordModal` from `./SudoConfirmModal` / `./SudoPasswordModal`, `useSubagents` from `../store/subagents`, `useBridge` + `type BridgeRequestData` from `../store/bridge`, `useShallow` from `zustand/react/shallow`. Export it as a named export.
2. `SubagentModals.test.tsx`: move the `describe("SubagentModals …")` block VERBATIM from `SubagentPanel.test.tsx`. Update its import to `import { SubagentModals } from "./SubagentModals";`.
3. `SidePane.tsx`:
   - Change the import from `import { SubagentDirectory, SubagentModals } from "./SubagentPanel";` to `import { SubagentModals } from "./SubagentModals";` (drop `SubagentDirectory`).
   - DELETE the Subagents section from the render (the `{subagentCount > 0 && (… <SubagentDirectory onDismiss={dismissEntry} /> …)}` block, `~lines 250–275`). KEEP the `TodoBoardPanel` section + the `<SubagentModals />` at the frame root.
   - DELETE the now-dead state: `const subagentEntries = useSubagents((s) => s.entries);` + `const subagentCount = …` + `const runningSubagents = …` + `const pendingSubagentRequests = usePendingSubagentRequests();` + `const subagentWaiting = …` + `const dismissEntry = …`. (If `useSubagents` / `usePendingSubagentRequests` are no longer used, remove their imports — but KEEP the `usePendingSubagentRequests` import ONLY if it is still used elsewhere in `SidePane`; verify with the build.)
   - Update the auto open/close: `const visible = mainTodoCount > 0;` (drop `|| runningSubagents`). The `prevVisibleRef` edge-triggered logic is UNCHANGED.
   - Update the component's top doc comment: the Subagents section is GONE (the pane is now Todos-only + the subagent sudo modals at the root).
4. `SidePane.test.tsx`: apply these SPECIFIC changes (the `visible = mainTodoCount > 0` change inverts several tests' semantics — do NOT just "update" them):
   - DELETE the Subagents-section tests (`~lines 91–157` — the `SubagentDirectory` / Subagents-header / `SubagentModals`-rendered-in-`SidePane` assertions; the `SubagentModals` behavior is now covered by `SubagentModals.test.tsx` (item 2)).
   - DELETE the four subagent-driven auto open/close tests whose semantics INVERT under `visible = mainTodoCount > 0`: "auto-expands the frame when a subagent session starts" (`~:220`), "does NOT auto-expand on mount when a subagent is already running" (`~:234`), "does NOT auto-collapse when a subagent is still running" (`~:243` — now it MUST auto-collapse), "auto-collapses when the last running subagent ends" (`~:266`). (There is NO fifth — the next test, "respects a MANUAL collapse while work is in flight" (`~:296`), is TODOS-driven and must be KEPT; do NOT delete it.) Optionally add ONE test asserting "a subagent does NOT open the pane" (seed a running subagent + no open todos; assert the frame stays collapsed) to lock in the new behavior.
   - KEEP the `TodoBoardPanel` tests (unchanged) + the frame-collapse test "collapses the frame to width 0 (a pending modal stays mounted) and re-opens" (`~:387` — it covers the `SubagentModals`-at-root invariant; KEEP `useSubagents` in the test file's imports for it).
   - KEEP the todos-driven auto open/close tests (a subagent no longer opens the pane, but todos still do).
5. DELETE `src/components/SubagentPanel.tsx` + `src/components/SubagentPanel.test.tsx` (after the moves above). Verify nothing else imports from `./SubagentPanel` (grep — the only importer was `SidePane.tsx`, now fixed).

**Steps:**
- [ ] Create `src/components/SubagentModals.tsx` (move `SubagentModals` verbatim) + `src/components/SubagentModals.test.tsx` (move the `SubagentModals` describe block; update the import).
- [ ] Modify `SidePane.tsx` (the import + delete the Subagents section + delete the dead state + `visible = mainTodoCount > 0` + update the doc comment).
- [ ] Modify `SidePane.test.tsx` (update the Subagents-section tests; move the `SubagentModals` tests out; update the auto open/close tests to todos-only).
- [ ] Delete `src/components/SubagentPanel.tsx` + `src/components/SubagentPanel.test.tsx`.
- [ ] Run `pnpm test` (repo root)
  - Did all tests pass (the moved `SubagentModals` tests + the updated `SidePane` tests — the Todos section + the todos-only auto open/close)? If not, fix the failures and re-run before continuing.
- [ ] Run `pnpm build` (repo root)
  - Did the type-check + build succeed (no dangling `./SubagentPanel` imports)? If not, fix and re-run.
- [ ] Run `rg -n "SubagentPanel|SubagentDirectory" src/` (repo root)
  - No references remain (the only importer was `SidePane.tsx`, now fixed)? If any remain, fix them and re-run the build.
- [ ] Commit with message: `feat(subagent): remove the sidebar Subagents panel — subagents live under the Delegating card + the dedicated modal`

**Acceptance criteria:**
- [ ] The sidebar no longer has a Subagents panel (`SidePane` renders only the Todos section + the subagent sudo modals at the frame root).
- [ ] `SubagentModals` is in its own file (the subagent sudo `confirm` / `password` requests are still rendered — the invariant for sudo modals).
- [ ] The `SidePane` auto open/close is driven by `mainTodoCount` only (a subagent no longer opens the pane).
- [ ] `SubagentPanel.tsx` + `SubagentPanel.test.tsx` are deleted; no dangling imports.
- [ ] `pnpm test` + `pnpm build` are green.

---

## Validation gates (all green before merge)

- `pnpm test`, `pnpm build` (repo root).
- `cargo test`, `cargo clippy --all-targets` (0 warnings), `cargo fmt --check` (`src-tauri/`).

## Out of scope

- Subagents dispatching subagents (excluded by design — `subagent: None`).
- Persisting subagent transcripts (subagents stay ephemeral — `db: None`).
- The `[subagent-dispatch]` diagnostic `eprintln!`s (they stay — they are the diagnostic the user re-ran; a follow-up may move them to `tracing`).
