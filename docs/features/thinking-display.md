---
status: live
last-verified: 2026-09-23
verified-by: PR #3 (squash-merged to main as 8d57012) — pnpm test (264) + pnpm build; Greptile + CI green
---

# Thinking display

The desktop shows the agent's streamed internal reasoning (ACP `agent_thought_chunk`) as a collapsible **Thinking block** (ZCode-style, ADR 0007), persisted in the transcript DB and visible in history loads and the subagent panel.

## How it works

- **Rust (Client)** persists thinking in `persist_update` (`src-tauri/src/acp/session.rs`): a per-session `ThoughtState` accumulates the OPEN segment's text and upserts an `agent-thought` row keyed by `<messageKey>#<segment>` (payload `{"text": <accumulated>}` — `record_message` upserts on `(session_id, kind, message_key)`, so re-sent/replayed chunks collapse to one row).
- **Frontend** carries an `agent-thought` `Message` kind: the reducer appends to the TRAILING `agent-thought` message with the same `messageId` (a new `messageId` or an intervening non-thought message starts a new block); `rowToMessages` maps persisted rows back so history loads show the same N blocks as live.
- **UI** is the ZCode `Reasoning` port (ADR 0007): collapsed by default, "Thinking · <live line>" while streaming, auto-collapse when the answer starts, "Thought · N seconds" when done.

## Segmentation rules (do not re-derive)

The Rust `ThoughtState` deliberately **mirrors the frontend reducer's segmentation**, so a stored session shows the same N thinking blocks as live. A NEW segment starts when:

- the `messageId` changes (pi-acp sends NO `messageId` on thought chunks, so all of a session's runs key to `"default"` and would merge without the other rules);
- a **non-empty** `agent_message_chunk` intervenes (the clear sits AFTER the empty-text guard, INSIDE the `if let ContentBlock::Text` — an empty or non-text chunk does NOT segment);
- a `tool_call` intervenes (a new tool call always adds a message);
- a `tool_call_update` **carries a diff** (`ToolCallContent::Diff` — the reducer appends a standalone diff message) **or** is for a tool call the `tool_call_state` map has never seen (the reducer's "never saw it → treat as new" case adds the tool-call message). A plain in-place update (no diff, known id) does NOT segment.
- a **prompt boundary** intervenes. This is INVISIBLE to `persist_update` (user rows are recorded by the `send_prompt` paths, not the notification handler) — so `SessionManager::begin_user_turn` (called at BOTH `send_prompt` call sites, before the user-row `record_message`) resets `open_key = None`. Without it, a turn that ends mid-thinking (user hits Stop) followed by a next turn starting with thinking would append to the SAME row.
- `config_option_update` and all other unknown types are ignored (the reducer has no arm for `user_message_chunk` either — deliberately).

Segment numbering is monotonic per session (`#1`, `#2`, …); a closed segment is never re-appended.

## Known limitations (documented, not fixed)

- **Thinking on resume:** on resume, thinking is lost. `resume_session` calls `db.clear_messages_for(sid)` before `session/load` (the agent's replay is treated as the authoritative history), and pi-acp's `loadSession` replay contains only user/assistant text and tool calls — never thinking. Stored (non-resumed) sessions keep their `agent-thought` rows. A future fix would preserve `agent-thought` rows across the wipe — with the ordering caveat that kept rows retain their old `id`s and would sort BEFORE the replayed text in `messages_for` (`ORDER BY id`).
- **Stored agent text is not segmented** (pre-existing behavior): the agent-text accumulator keys by `messageId ?? "default"` with no segmentation, so with pi-acp (which sends no `messageId` on text chunks) all of a session's answer text lands in ONE row at the position of the first text chunk. Thinking is segmented; history interleaving is accurate for thinking and tool calls, not for text. A follow-up could apply the same `ThoughtState`-style segmentation to agent text.
