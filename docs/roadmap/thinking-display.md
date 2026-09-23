---
status: approved
done-when: In a live session with a thinking model: a collapsed "Thinking… · <live line>" block appears before the answer text, auto-collapses when the answer starts, and expands to the full thinking text; after an app restart, opening a stored/resumed session shows the thinking block (collapsed, "Thought · a few seconds"); the subagent panel shows subagent thinking the same way.
---

# Thinking display (show the agent's streamed reasoning, ZCode-style)

**Problem.** The agent streams its internal reasoning over ACP as `agent_thought_chunk` (pi-acp emits it for every `thinking_delta`). The Client receives these events (the Rust driver forwards all `session/update` notifications verbatim) but drops them in both layers — Rust `persist_update` (`_ => {}`) and the frontend `applySessionUpdate` reducer (`default`). Thinking is invisible. Additionally, resume re-streams text only (pi-acp `loadSession`), so unpersisted thinking is gone forever.

**Goal.** Show thinking the way ZCode does — a collapsible **Thinking block** per contiguous thinking run: collapsed by default, "Thinking…" (animated gradient) while streaming with a live one-line summary of the last non-empty line, "Thought · Ns" when done, expandable plain-text content (max-h-60, left rail, auto-follow-bottom). Thinking is persisted to the transcript DB and appears in history loads, resumes, and the subagent panel.

## 1. Rust backend (`src-tauri/`)

- **`persist_update`** (`acp/session.rs`): new `SessionUpdate::AgentThoughtChunk` arm mirroring the `AgentMessageChunk` arm — skip empty text; key = `chunk.message_id` or `"default"`; accumulate in a new `agent_thought_acc: StdMutex<HashMap<String, String>>` (separate from the agent-text accumulator); upsert `db.record_message(session_id, "agent-thought", Some(key), {"text": entry})`. (The v1 schema has no final `AgentThought` message — chunk-only.)
- **Forwarding:** no change — `agent_thought_chunk` frames already reach the frontend verbatim.
- **`text_capture` (subagents):** unchanged — captures final answer text only.
- **`fake_agent`** (dev/test double): emit a few `agent_thought_chunk`s before its text so the display is testable in dev mode.

## 2. Frontend data model

- **`src/lib/tauri.ts`**: `AcpSessionUpdate` gains `{ sessionUpdate: "agent_thought_chunk"; content?: ContentBlock; messageId?: string }`; `MessageRow.kind` gains `"agent-thought"`.
- **`src/store/sessions.ts`**:
  - `Message` union gains `{ kind: "agent-thought"; messageId: string; text: string; at: number }`.
  - `applySessionUpdate` gains an `agent_thought_chunk` case mirroring the `agent_message_chunk` rule: append to the trailing `agent-thought` message with the same `messageId`; a new `messageId` (or an intervening non-thought message) starts a new block. `think → text → think` therefore yields two blocks.
  - `rowToMessages` gains an `agent-thought` case (payload `text` → message; `messageId` = `row.messageKey ?? "default"`).
  - Untouched: `finalizeSessionMessages`, `discardSessionMessages`, subagents store (`SubagentPanel` reads `useSessions.messages`).
- **Duration:** not persisted — a loaded block renders ZCode's `duration === undefined` display ("Thought · a few seconds"); live duration is component-tracked.

## 3. UI (faithful port — ADR 0007)

- **New `src/lib/scrollMask.ts`** — verbatim port of ZCode `mentions/components/scrollMask.ts` (pure functions: scroll metrics → vertical mask style).
- **New `src/components/QueuedSummaryContent.tsx`** — verbatim port of ZCode `ToolCallBlocks/QueuedSummaryContent.tsx` (framer-motion roll for the summary line).
- **New `src/components/Reasoning.tsx`** — port of ZCode `components/ai-elements/reasoning.tsx` (`Reasoning`, `ReasoningTrigger`, `ReasoningContent`, `useReasoning`, and the pure helpers). Behavior verbatim: collapsed by default; auto-collapse on the streaming→done transition unless the user interacted; duration tracking (start on streaming begin, finalize on end, 1s tick while open); trigger = `BrainIcon` + "Thinking…" (`.animated-gradient-text` while streaming) / "Thought · a few seconds" / "Thought · Ns" + one-line live summary (last non-empty line, auto-scroll to end, gradient edge mask via `QueuedSummaryContent`); content = max-h-60 scroll box, plain text (no markdown), left border rail, auto-follow-bottom while streaming, scroll masks, 300ms delayed unmount.
  - Adaptations only: `Collapsible*` from the already-ported `@/components/ui/collapsible`; `cn` from `@/components/lib/utils`; `BrainIcon`/`ChevronRightIcon` from `lucide-react`; `useControllableState` from `@radix-ui/react-use-controllable-state` (new direct dep, the version radix-ui pins — 1.2.6); i18n lookups → literal English strings; TID constants → literal `data-testid` (`"reasoning-trigger"`, `"reasoning-content"`).
- **`src/index.css`**: add the `.animated-gradient-text` class (ZCode `styles.css` L823–843; the CSS vars are already ported).
- **New dependencies:** `motion`, `@radix-ui/react-use-controllable-state`.

## 4. Wiring

- **Streaming semantics:** a thought block is *streaming* when its session is `inTurn` and the block is the last message in the transcript (no text/tool call followed). First text chunk, first tool call, or `turnCompleted` flips it to done.
- **`MessageBubble`**: gains an optional `isStreaming` prop (default `false`); `ChatStream` computes it per thought message and passes it down. The `agent-thought` case renders `<Reasoning isStreaming autoCollapseKey={isStreaming ? null : "complete"}>` with `ReasoningTrigger streamingText={message.text}` + `ReasoningContent>{message.text}`.
- **Loaded history:** `isStreaming=false`, no duration → "Thought · a few seconds".
- **`SubagentPanel`**: gains an `agent-thought` case — same `Reasoning` wiring; `isStreaming` = the subagent session is still live and the block is its last message; collapsed by default.

## 5. Tests

- **Rust** (`persist_update`): `AgentThoughtChunk` accumulates into an `agent-thought` row; a new `messageId` → new row; empty chunks ignored (mirrors the existing agent-text test pattern).
- **Store** (`applySessionUpdate`): append to trailing thought with same id; new id → new block; intervening text/tool → new block; non-text content ignored. `rowToMessages`: `agent-thought` row → message.
- **Component** (`Reasoning`): collapsed by default; "Thinking…" while streaming; summary line = last non-empty line; expands on trigger click; auto-collapses on done-transition when the user didn't interact; "Thought · Ns" when done.
- **`MessageBubble`**: a thought message renders as a `Reasoning` block.

## Out of scope

- Thinking token *counts* (usage stats) — no usage display surface exists.
- Markdown rendering inside thinking (ZCode deliberately uses plain text — performance).
- `agent_thought` final-message handling (v2-only; the Client speaks v1).
