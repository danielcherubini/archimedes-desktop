---
status: live
last-verified: 2026-09-23
verified-by: PR #3 (squash-merged to main as 8d57012) — pnpm test (264) + pnpm build; Greptile + CI green
---

# Thinking display

The desktop shows the agent's streamed internal reasoning as a collapsible **Thinking block** (ZCode-style, ADR 0007), persisted in the transcript DB and visible in history loads and the subagent panel. The reasoning is streamed IN-PROCESS by the native `AgentLoop` (ADR 0011 / 0022 — the desktop is the harness; there is no external agent): the `agent_thought_chunk` wire vocabulary survives from the first-generation ACP era, but it is now emitted natively.

## How it works

- **Harness (in-process, `src-tauri/src/agent/harness/loop.rs`)**: the model's thinking content streams in as `ProviderEvent::ThinkingDelta` (the provider parses the model's reasoning output — `reasoning_effort` is set from the session's thinking level, ADR 0015), the loop accumulates it into the turn accumulator, and emits `RpcEvent::message_update { thinking_delta }` (the `thinking_start` / `thinking_end` / `text_start` / `text_end` events are bookkeeping — no frame; re-emitting the already-streamed content would double the text).
- **Normalizer (`src-tauri/src/agent/session.rs` `normalize()`)** maps `thinking_delta` to an `agent_thought_chunk` frame with a `messageId` — the current assistant message id (`m1`, `m2`, … — advanced by the assistant `message_start`, which the loop emits ONCE per assistant message BEFORE consuming the stream; a mid-stream retry is the SAME message, a new model call after tool results is a new one). The loop's `emit` runs every event through the SAME `normalize` + `persist_update` pipeline (the driver / tests consume the events channel as a liveness signal only).
- **Persistence** is in `persist_update` (`src-tauri/src/agent/session.rs`): a per-session `ThoughtState` accumulates the OPEN segment's text and upserts an `agent-thought` row keyed by `<messageKey>#<segment>` (payload `{"text": <accumulated>}` — `record_message` upserts on `(session_id, kind, message_key)`, so re-sent/replayed chunks collapse to one row).
- **Frontend** carries an `agent-thought` `Message` kind: the reducer appends to the TRAILING `agent-thought` message with the same `messageId` (a new `messageId` or an intervening non-thought message starts a new block); `rowToMessages` maps persisted rows back so history loads show the same N blocks as live.
- **UI** is the ZCode `Reasoning` port (ADR 0007): collapsed by default, "Thinking · <live line>" while streaming, auto-collapse when the answer starts, "Thought · N seconds" when done.

## Segmentation rules (do not re-derive)

The Rust `ThoughtState` deliberately **mirrors the frontend reducer's segmentation**, so a stored session shows the same N thinking blocks as live. A NEW segment starts when:

- the `messageId` changes (the native loop keys thought chunks to the CURRENT assistant message id — `m1`, `m2`, … — so a new assistant message is a new thinking segment; the first-generation ACP embodiment, removed in ADR 0022, sent NO `messageId`, so all of a session's runs keyed to `"default"` and would have merged without the other rules);
- a **non-empty** `agent_message_chunk` intervenes (the clear sits AFTER the empty-text guard, INSIDE the `if let ContentBlock::Text` — an empty or non-text chunk does NOT segment);
- a `tool_call` intervenes (a new tool call always adds a message);
- a `tool_call_update` **is for a tool call the `tool_call_state` map has never seen** (the reducer's "never saw it → treat as new" case adds the tool-call message). A plain in-place update (known id) does NOT segment. (The first-generation ACP `ToolCallContent[]` diff channel — "a `tool_call_update` carries a diff also segments" — has no native RPC input, so `persist_update`'s `has_diff` branch is gone with the crate types, ADR 0022.)
- a **prompt boundary** intervenes. This is INVISIBLE to `persist_update` (user rows are recorded by the `send_prompt` path, not the event pipeline) — so `SessionManager::begin_user_turn` (called in `send_prompt_with_images`, the SOLE write path — `send_prompt` delegates to it) resets `open_key = None`. Without it, a turn that ends mid-thinking (user hits Stop) followed by a next turn starting with thinking would append to the SAME row.
- `config_option_update` and all other unknown types are ignored (the reducer has no arm for `user_message_chunk` either — deliberately).

Segment numbering is monotonic per session (`#1`, `#2`, …); a closed segment is never re-appended.

## Known limitations (documented, not fixed)

- **Thinking on resume: PRESERVED** (the first-generation ACP embodiment, removed in ADR 0022, LOST it — the external resume called `db.clear_messages_for(sid)` before the `session/load` replay, and the external replay contained only user/assistant text and tool calls, never thinking). The native resume reconstructs the session from the stored rows and must NEVER clear the display rows (`src-tauri/src/agent/harness/store.rs` — `clear_messages_for` is deleted; the `SessionStore`'s `clear_messages` clears `native_messages` ONLY): the provider transcript is restored from the `native_messages` table (`SessionStore::load_messages` — the assistant messages' `thinking` content blocks re-enter the model's context) BEFORE the first model call, and the display transcript is reloaded by the frontend's `resumeSession` from the persisted `messages` rows (`load_history` → `messages_for` — the `agent-thought` rows included), so the transcript shows the same thinking blocks after resume.
- **Agent-text segmentation (first-generation limitation — RESOLVED natively):** with the first-generation ACP embodiment (removed, ADR 0022) the agent sent no `messageId` on text chunks, so the agent-text accumulator (keyed by `messageId ?? "default"`, no segmentation) landed ALL of a session's answer text in ONE row at the position of the first text chunk. The native `messageId` advances per assistant message (each new model call after tool results is a new assistant message), so agent text is segmented per assistant message: one row per assistant message, at the position of its first text chunk. History interleaving is now accurate for thinking, agent text, and tool calls alike.
