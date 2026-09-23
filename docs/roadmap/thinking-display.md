---
status: committed
done-when: In a live session with a thinking model: a collapsed "Thinking · <live line>" block appears before the answer text, auto-collapses when the answer starts, and expands to the full thinking text; after an app restart, opening a stored session shows the thinking blocks (collapsed, "Thought · a few seconds", one per contiguous thinking run — the same segmentation as live); the subagent panel shows subagent thinking the same way.
---

# Thinking display Plan

**Goal:** Show the agent's streamed internal reasoning (ACP `agent_thought_chunk`) as a ZCode-style collapsible **Thinking block**, persisted in the transcript DB and visible in history loads and the subagent panel.

**Architecture:** The agent's thinking already flows to the frontend verbatim (the Rust driver forwards every `session/update` notification); the gap is two layers dropping it. Rust `persist_update` gains an `AgentThoughtChunk` arm that persists thinking as `agent-thought` rows **segmented the same way the frontend reducer segments live blocks** (a new segment starts on a new `messageId`, on any message-adding update, or on a prompt boundary via `begin_user_turn`), so a stored session shows the same N thinking blocks as live. The frontend gains an `agent-thought` `Message` kind (reducer + history mapping); and a faithful port of ZCode's `Reasoning` component (ADR 0007) renders it — collapsed by default, live one-line summary while streaming, plain-text content when expanded.

**Tech Stack:** Rust (Tauri 2, `agent_client_protocol` v1 schema, rusqlite), React 19 + TypeScript (zustand, `radix-ui`, `lucide-react`, `motion`/framer-motion, Tailwind v4 with the ZCode token layer).

**Design references:**
- Spec sections: this file's git history (the approved spec was committed as `docs: add thinking-display spec`).
- ADR 0007 (`docs/decisions/0007-reasoning-port.md`): faithful port, including `QueuedSummaryContent` + the `motion` dependency.
- ZCode sources (sibling repo, READ-ONLY — never modify ZCode):
  - `/home/daniel/Coding/AI/ZCode/packages/ui/src/components/ai-elements/reasoning.tsx`
  - `/home/daniel/Coding/AI/ZCode/packages/ui/src/ToolCallBlocks/QueuedSummaryContent.tsx`
  - `/home/daniel/Coding/AI/ZCode/packages/ui/src/mentions/components/scrollMask.ts`
  - `/home/daniel/Coding/AI/ZCode/packages/ui/src/styles.css` (L823–842 `.animated-gradient-text` inside `@layer utilities`, L885–897 `@keyframes gradient-flow` inside the same layer)

**Validation commands (from AGENTS.md):**
- Frontend unit tests: `pnpm test` (repo root); single file: `pnpm vitest run <path>`
- Frontend type-check + build: `pnpm build`
- Rust tests: `cargo test` (from `src-tauri/`); single: `cargo test <name>`
- Rust lint (must be 0 warnings): `cargo clippy --all-targets` (from `src-tauri/`)
- Rust formatting: `cargo fmt` (from `src-tauri/`); check: `cargo fmt --check`

**Conventions:** TDD — write the failing test first, confirm it fails, then make it pass. One commit per task.

**Known limitations (documented, NOT fixed here):**
- **Thinking on resume:** on resume, thinking is lost. `resume_session` calls `db.clear_messages_for(sid)` before `session/load` (the agent's replay is treated as the authoritative history), and pi-acp's `loadSession` replay contains only user/assistant text and tool calls — never thinking. So a resumed session's transcript has no `agent-thought` rows. Stored (non-resumed) sessions keep theirs. A future fix would preserve `agent-thought` rows across the wipe — with the ordering caveat that kept rows retain their old `id`s and would sort BEFORE the replayed text in `messages_for` (`ORDER BY id`).
- **Stored agent text is not segmented** (pre-existing behavior): the agent-text accumulator keys by `messageId ?? "default"` with no segmentation, so with pi-acp (which sends no `messageId` on text chunks) all of a session's answer text lands in ONE row at the position of the first text chunk. This plan segments thinking only; history interleaving is accurate for thinking and tool calls, not for text. A follow-up could apply the same `ThoughtState`-style segmentation to agent text.

---

### Task 1: Rust — persist `agent_thought_chunk` as segmented `agent-thought` rows

**Context:**
The Rust session driver already forwards every `session/update` notification to the frontend verbatim, but `persist_update` (`src-tauri/src/acp/session.rs`, L1199–1253) only handles `AgentMessageChunk`, `ToolCall`, and `ToolCallUpdate` — `AgentThoughtChunk` falls through `_ => {}` and is lost. This task persists thinking the same way the Client persists agent text, so thinking blocks survive history loads.

**Why segmentation:** pi-acp emits `agent_thought_chunk` with NO `messageId`, so a naive per-`messageId` accumulator (like the agent-text one) would merge ALL of a session's thinking runs into one DB row — while the live frontend reducer segments blocks on "the trailing message is not a thought with the same messageId". To make a stored session show the same N thinking blocks as live, the Rust side mirrors the reducer's segmentation. A new thinking segment starts when:
- the `messageId` changes (the reducer starts a new block on a new `messageId`);
- a **non-empty** `agent_message_chunk` intervenes (the reducer returns unchanged for empty/non-text chunks — so the clear goes AFTER the empty-text guard, inside the `if let ContentBlock::Text` — and when a thought segment is open the reducer's trailing message IS the thought block, so any non-empty text chunk starts a new text message and segments the run);
- a `tool_call` intervenes (a new tool call always adds a message);
- a `tool_call_update` **carries a diff** (`ToolCallUpdateFields.content` contains a `ToolCallContent::Diff` — the reducer appends a standalone diff message to the transcript, which breaks the thinking chain) **or** is for a tool call the `tool_call_state` map has never seen (the reducer's "never saw it → treat as new" case adds the tool-call message). A plain in-place `tool_call_update` (no diff, known id) does NOT segment.
- a **prompt boundary** intervenes. This one is INVISIBLE to `persist_update`: the user row is recorded by the `send_prompt` paths (`SessionManager::send_prompt` at L1035, `commands/sessions.rs::send_prompt` at L64), not the notification handler — but the frontend's `addUserMessage` appends a `user` message, which starts a new thinking block. Without the reset, a turn that ends mid-thinking (user hits Stop during a long think — common) followed by a next turn that starts with thinking would append to the SAME row (pi-acp's chunks all key to `"default"`), and the merged block would appear before the second user message in history. So `begin_user_turn` (below) resets the open segment at every prompt boundary.
- `config_option_update` and other unknown types are ignored by the reducer, so they don't segment. (The agent's `user_message_chunk` is ALSO ignored by the reducer — there is deliberately no arm for it.)

**Files:**
- Modify: `src-tauri/src/acp/session.rs`
- Modify: `src-tauri/src/commands/sessions.rs`
- Modify: `src-tauri/src/storage/db.rs` (doc comment only)

**What to implement:**
1. Near `persist_update` (in the same module), add:
   ```rust
   /// Per-session thinking-persistence state: the accumulated text of the
   /// OPEN segment, the segment's message key (or `None` when the last
   /// update was not a continuing thought chunk), and the next segment
   /// number. `current` is reset when a new segment starts (a closed
   /// segment is never re-appended — see the segmentation rule above).
   #[derive(Debug, Default)]
   pub(crate) struct ThoughtState {
       current: String,
       open_key: Option<String>,
       next: u32,
   }
   ```
2. In the driver setup, BEFORE the `tokio::spawn` (next to the existing accumulators at L370–373, which are created in the outer scope and moved into the spawned task), add:
   ```rust
   // Shared with `begin_user_turn` (the prompt-boundary reset) — the
   // driver task gets a clone.
   let thought_state: Arc<StdMutex<ThoughtState>> =
       Arc::new(StdMutex::new(ThoughtState::default()));
   let thought_state_task = thought_state.clone();
   ```
   and inside the driver task, pass `&thought_state_task` into the `persist_update` call at L425–430 (new parameter position: after `tool_call_state`).
3. `LiveSession` (L128–146): add the field
   `pub(crate) thought_state: Arc<StdMutex<ThoughtState>>,`
   and pass `thought_state` in the construction at L660–667.
4. In `SessionManager` (impl at L718), add:
   ```rust
   /// Reset the session's open thinking segment at a prompt boundary. The
   /// frontend's `addUserMessage` starts a new thinking block on a user
   /// message, but `persist_update` never sees user messages (they are
   /// recorded by the `send_prompt` paths, not the notification handler) —
   /// so the Rust accumulator must be reset here, not in `persist_update`.
   pub async fn begin_user_turn(&self, session_id: &str) {
       let sid = SessionId::new(session_id);
       if let Some(live) = self.driver.sessions.lock().await.get(&sid) {
           live.thought_state
               .lock()
               .expect("thought state poisoned")
               .open_key = None;
       }
   }
   ```
5. Call `begin_user_turn` at BOTH prompt boundaries, BEFORE the user-row `record_message`:
   - `SessionManager::send_prompt` (L1014–1048): add `self.begin_user_turn(session_id).await;` before the `if let Some(db) = &self.driver.db` block.
   - `commands/sessions.rs::send_prompt` (L50–83): add `state.begin_user_turn(&session_id).await;` before `let _ = db.record_message(&session_id, "user", ...)`.
6. In `persist_update` (L1199): add the parameter
   `thought_state: &StdMutex<ThoughtState>,`
   (after `tool_call_state`) and these changes:
   - New arm **before** the `_ => {}` arm:
     ```rust
     SessionUpdate::AgentThoughtChunk(chunk) => {
         if let ContentBlock::Text(text) = &chunk.content {
             if text.text.is_empty() {
                 return;
             }
             let key = chunk
                 .message_id
                 .as_ref()
                 .map(|m| m.to_string())
                 .unwrap_or_else(|| "default".to_string());
             let mut state = thought_state.lock().expect("thought state poisoned");
             if state.open_key.as_deref() != Some(key.as_str()) {
                 // New thinking segment: a new messageId, or an intervening
                 // segmenting update (see the rule above) cleared `open_key`.
                 state.next += 1;
                 state.open_key = Some(key);
                 state.current = String::new();
             }
             state.current.push_str(&text.text);
             let row_key = format!("{}#{}", state.open_key.as_ref().unwrap(), state.next);
             let payload = serde_json::json!({ "text": state.current });
             let _ = db.record_message(session_id, "agent-thought", Some(&row_key), &payload.to_string());
         }
     }
     ```
     (Note: `record_message` upserts on `(session_id, kind, message_key)`, so re-sent/replayed chunks collapse to one row — same as agent-text.)
   - In the existing `AgentMessageChunk` arm: add the segment clear AFTER the `if text.text.is_empty() { return; }` guard, INSIDE the `if let ContentBlock::Text` block (an empty or non-text chunk does NOT segment — the reducer returns unchanged for it):
     ```rust
     thought_state.lock().expect("thought state poisoned").open_key = None;
     ```
     (Keep the existing text accumulation exactly as it is — only the clear line is added.)
   - In the existing `ToolCall` arm: at the TOP of the arm, add the same `open_key = None` clear (a new tool call always adds a message).
   - In the existing `ToolCallUpdate` arm: a plain in-place update does NOT clear, EXCEPT when the tool call id is not yet in `tool_call_state` (the never-seen case adds a message) OR the update carries a diff (the reducer appends standalone diff messages). Add `ToolCallContent` to the top-level `agent_client_protocol::schema::v1` import list (L38–44), then:
     ```rust
     let is_new = !state.contains_key(&key);
     let has_diff = tool_call_update
         .fields
         .content
         .as_ref()
         .is_some_and(|items| items.iter().any(|item| matches!(item, ToolCallContent::Diff(_))));
     // ...existing `patch` / `entry` / `merge_json` / `record_message` lines unchanged...
     drop(state);
     if is_new || has_diff {
         thought_state.lock().expect("thought state poisoned").open_key = None;
     }
     ```
     (Drop `state` BEFORE locking `thought_state` — the two guards are never held at once, so there is no lock-ordering concern.)
   - `config_option_update` and all other types: no clear. There is deliberately NO `UserMessageChunk` arm (the reducer ignores it).
7. In the `MessageRow` doc comment (`src-tauri/src/storage/db.rs` L53) update the kind list to `'user' | 'agent-text' | 'agent-thought' | 'tool-call' | 'diff'`.
8. Do NOT change: the notification forwarding (L419–423), the subagent `text_capture` (L444–460 — it captures final answer text only), the `AgentMessageChunk` text accumulation itself, `merge_json`.

**Steps:**
- [ ] Add a test to the existing `#[cfg(test)] mod session_tests` in `src-tauri/src/acp/session.rs` (add `use agent_client_protocol::schema::v1::ContentChunk;` to the test module's imports — it is not in the top-level import list; `TextContent`, `SessionUpdate`, `SessionInfo`, `AgentCapabilities`, `StdMutex` are already in scope via `use super::*` (including `HashMap`):
  ```rust
  #[test]
  fn persist_update_records_agent_thought_chunks() {
      let dir = temp_config_dir();
      let db = Db::open(&dir.join("archimedes.db")).expect("db should open");
      // FK: `messages.session_id REFERENCES sessions(id)` and `Db::open`
      // enables `PRAGMA foreign_keys` — record the session FIRST, or every
      // `record_message` fails (and `persist_update` swallows the error).
      db.record_session(&SessionInfo {
          session_id: "sess-t1".into(),
          agent_id: "fake".into(),
          cwd: dir.clone(),
          capabilities: AgentCapabilities::default(),
          config_options: None,
      })
      .expect("record_session should succeed");
      let text_acc = StdMutex::new(HashMap::new());
      let tool_state = StdMutex::new(HashMap::new());
      let thought = StdMutex::new(ThoughtState::default());

      // Two chunks sharing one (absent) messageId accumulate into ONE row.
      persist_update(
          &db, "sess-t1",
          &SessionUpdate::AgentThoughtChunk(ContentChunk::new(
              ContentBlock::Text(TextContent::new("thinking ")),
          )),
          &text_acc, &tool_state, &thought,
      );
      persist_update(
          &db, "sess-t1",
          &SessionUpdate::AgentThoughtChunk(ContentChunk::new(
              ContentBlock::Text(TextContent::new("happens")),
          )),
          &text_acc, &tool_state, &thought,
      );
      let rows = db.messages_for("sess-t1").expect("messages_for should work");
      assert_eq!(rows.len(), 1, "two chunks, one messageId → one row");
      assert_eq!(rows[0].kind, "agent-thought");
      assert_eq!(rows[0].message_key.as_deref(), Some("default#1"));
      assert_eq!(rows[0].payload_json, r#"{"text":"thinking happens"}"#);

      // A non-empty text chunk interrupts the run: a later thought chunk
      // starts a NEW segment.
      persist_update(
          &db, "sess-t1",
          &SessionUpdate::AgentMessageChunk(ContentChunk::new(
              ContentBlock::Text(TextContent::new("answer")),
          )),
          &text_acc, &tool_state, &thought,
      );
      persist_update(
          &db, "sess-t1",
          &SessionUpdate::AgentThoughtChunk(ContentChunk::new(
              ContentBlock::Text(TextContent::new("second run")),
          )),
          &text_acc, &tool_state, &thought,
      );
      let rows = db.messages_for("sess-t1").expect("messages_for should work");
      assert_eq!(rows.len(), 3, "think → text → think → two thought rows + one text row");
      assert_eq!(rows.iter().filter(|r| r.kind == "agent-thought").count(), 2);

      // A prompt boundary (the `begin_user_turn` operation — a direct
      // `open_key = None` reset, since `begin_user_turn` needs a live
      // SessionManager entry) also starts a new segment.
      thought.lock().expect("poisoned").open_key = None;
      persist_update(
          &db, "sess-t1",
          &SessionUpdate::AgentThoughtChunk(ContentChunk::new(
              ContentBlock::Text(TextContent::new("third turn")),
          )),
          &text_acc, &tool_state, &thought,
      );
      let rows = db.messages_for("sess-t1").expect("messages_for should work");
      assert_eq!(rows.len(), 4, "prompt boundary → third thought row");

      // A DISTINCT messageId starts a new segment back-to-back; an EMPTY
      // chunk (and an empty text chunk) is ignored and does NOT segment.
      persist_update(
          &db, "sess-t1",
          &SessionUpdate::AgentThoughtChunk(
              ContentChunk::new(ContentBlock::Text(TextContent::new("m2 run"))).message_id("m2"),
          ),
          &text_acc, &tool_state, &thought,
      );
      persist_update(
          &db, "sess-t1",
          &SessionUpdate::AgentThoughtChunk(ContentChunk::new(
              ContentBlock::Text(TextContent::new("")),
          )),
          &text_acc, &tool_state, &thought,
      );
      persist_update(
          &db, "sess-t1",
          &SessionUpdate::AgentMessageChunk(ContentChunk::new(
              ContentBlock::Text(TextContent::new("")),
          )),
          &text_acc, &tool_state, &thought,
      );
      persist_update(
          &db, "sess-t1",
          &SessionUpdate::AgentThoughtChunk(
              ContentChunk::new(ContentBlock::Text(TextContent::new("still m2"))).message_id("m2"),
          ),
          &text_acc, &tool_state, &thought,
      );
      let rows = db.messages_for("sess-t1").expect("messages_for should work");
      // The empty text chunk did NOT segment: the last thought chunk
      // (same "m2" key, open segment) appended to the m2 row.
      assert_eq!(rows.len(), 5, "new messageId → fourth thought row; empty chunks → no rows, no segmentation");
      let m2: Vec<_> = rows.iter().filter(|r| r.message_key.as_deref() == Some("m2#4")).collect();
      assert_eq!(m2.len(), 1);
      assert_eq!(m2[0].payload_json, r#"{"text":"m2 runstill m2"}"#);
  }
  ```
  (Segment numbering check: `default#1` (run 1) → `default#2` (run 2) → `default#3` (run 3, after the boundary) → `m2#4` (run 4, new messageId). The empty text chunk in between does not bump `next`, so the final thought chunk continues `m2#4`.)
- [ ] Run `cargo test persist_update_records_agent_thought_chunks` (from `src-tauri/`)
  - It fails (compile error — `ThoughtState` / the new arm / parameter don't exist yet). Confirm the failure is the missing implementation, not a typo in the test.
- [ ] Implement the change (items 1–8 above).
- [ ] Run `cargo test` (from `src-tauri/`)
  - All tests pass? If not, fix and re-run before continuing.
- [ ] Run `cargo clippy --all-targets` (from `src-tauri/`)
  - 0 warnings? If not, fix and re-run before continuing.
- [ ] Run `cargo fmt` (from `src-tauri/`)
  - Then `cargo fmt --check` — clean? If not, fix and re-run.
- [ ] Commit with message: `feat: persist agent_thought_chunk as segmented agent-thought rows`

**Acceptance criteria:**
- [ ] `persist_update` has an `AgentThoughtChunk` arm that upserts an `agent-thought` row keyed by `(session_id, "agent-thought", "<messageKey>#<segment>")` with payload `{"text": <accumulated>}`.
- [ ] Consecutive chunks with the same `messageId` collapse to one row (accumulated text); a new `messageId`, a non-empty text chunk, a new tool call, a diff-carrying or never-seen `tool_call_update`, or a prompt boundary (`begin_user_turn`) starts a new segment; empty chunks and `config_option_update` do not segment.
- [ ] Both `send_prompt` paths call `begin_user_turn` before recording the user row.
- [ ] The agent-text and tool-call paths are unchanged (existing tests still pass).

---

### Task 2: Frontend data model — `agent-thought` message kind

**Context:**
The frontend receives `agent_thought_chunk` frames (the Rust driver forwards them verbatim — no Rust change needed here), but the `AcpSessionUpdate` type has no variant for it and the `applySessionUpdate` reducer (`src/store/sessions.ts`) drops it in the `default` branch. This task adds the `agent-thought` message kind end-to-end in the data layer: the wire type, the `Message` union, the reducer rule, and the history mapping. The reducer rule mirrors `agent_message_chunk` exactly: append to the TRAILING `agent-thought` message with the same `messageId`; a new `messageId` (or an intervening non-thought message) starts a new block — so `think → text → think` yields two blocks. (This is the same segmentation Task 1 mirrors on the Rust side.)

**Files:**
- Modify: `src/lib/tauri.ts`
- Modify: `src/store/sessions.ts`
- Test: `src/store/sessions.test.ts`

**What to implement:**
1. `src/lib/tauri.ts`:
   - In `AcpSessionUpdate` (L141–165), add a variant (order doesn't matter):
     ```ts
     | {
         sessionUpdate: "agent_thought_chunk";
         content?: ContentBlock;
         messageId?: string;
       }
     ```
   - In `MessageRow.kind` (L292): `"user" | "agent-text" | "agent-thought" | "tool-call" | "diff" | (string & {})`.
   - Update the `AcpSessionUpdate` doc comment (L134–139): it currently says "The three update types the desktop renders are modelled" — after this change there are FIVE variants (`agent_message_chunk`, `agent_thought_chunk`, `tool_call`, `tool_call_update`, `config_option_update`); rewrite it to say the five update types the desktop handles are modelled, with the same forward-compat note (unknown types fall through to the reducer's `default` and are ignored).
2. `src/store/sessions.ts`:
   - `Message` union (L32–45): add
     ```ts
     | { kind: "agent-thought"; messageId: string; text: string; at: number }
     ```
   - `applySessionUpdate` (L96–131): add a case mirroring `agent_message_chunk`, with the kind `"agent-thought"`:
     ```ts
     case "agent_thought_chunk": {
       const content = update.content;
       if (!content || content.type !== "text" || typeof content.text !== "string") {
         return messages;
       }
       const text = content.text;
       if (text === "") return messages;
       const messageId = update.messageId ?? "default";
       const last = messages[messages.length - 1];
       if (last && last.kind === "agent-thought" && last.messageId === messageId) {
         return [...messages.slice(0, -1), { ...last, text: last.text + text }];
       }
       return [...messages, { kind: "agent-thought", messageId, text, at }];
     }
     ```
   - `rowToMessages` (L214–254): add a case mirroring `agent-text`:
     ```ts
     case "agent-thought":
       return typeof payload.text === "string"
         ? [
             {
               kind: "agent-thought",
               messageId: row.messageKey ?? "default",
               text: payload.text,
               at: row.createdAt,
             },
           ]
         : [];
     ```
   - Update the `applySessionUpdate` doc comment (L83–95) to mention the `agent_thought_chunk` rule.
   - Do NOT change: `finalizeSessionMessages`, `discardSessionMessages`, the `user`/`agent-text`/`tool-call`/`diff` cases.

**Steps:**
- [ ] Add tests to `src/store/sessions.test.ts`:
  - `applySessionUpdate` with `agent_thought_chunk`:
    - two chunks, same (absent) `messageId` → ONE `agent-thought` message with accumulated text;
    - a chunk with a distinct `messageId` after a text chunk → a NEW `agent-thought` message (the trailing message is `agent-text`, so no append);
    - `agent_thought_chunk → agent_message_chunk → agent_thought_chunk` (all default ids) → two `agent-thought` messages (the text message breaks the chain);
    - a non-text `content` (e.g. `{ type: "image" }`) and an empty-text chunk → `messages` unchanged (same reference or equal array).
  - `rowToMessages` with a row `{ kind: "agent-thought", messageKey: "m9#1", payloadJson: '{"text":"recalled"}', createdAt: 123 }` → `[{ kind: "agent-thought", messageId: "m9#1", text: "recalled", at: 123 }]`; a row with a non-string `payload.text` → `[]`.
- [ ] Run `pnpm vitest run src/store/sessions.test.ts`
  - The new tests fail (the reducer returns the input unchanged / the type doesn't exist)? Confirm the failures are the missing case, not typos.
- [ ] Implement the change (items 1–2 above).
- [ ] Run `pnpm vitest run src/store/sessions.test.ts`
  - All pass? If not, fix and re-run.
- [ ] Run `pnpm build`
  - Type-check + build succeed? If not, fix and re-run.
- [ ] Commit with message: `feat: agent-thought message kind (wire type, reducer, history mapping)`

**Acceptance criteria:**
- [ ] `AcpSessionUpdate` has the `agent_thought_chunk` variant and `MessageRow.kind` includes `"agent-thought"`.
- [ ] The reducer groups a contiguous thinking run into one `agent-thought` message and starts a new one on a new `messageId` or an intervening non-thought message.
- [ ] `rowToMessages` maps a persisted `agent-thought` row back to a message (so history loads show thinking).

---

### Task 3: UI primitives — scrollMask, QueuedSummaryContent, CSS, dependencies

**Context:**
The `Reasoning` component (Task 4) depends on three things the Client doesn't have yet: the `scrollMask` util (vertical scroll-edge masks), the `QueuedSummaryContent` component (the framer-motion "roll" for the one-line live summary — ADR 0007 chose the faithful port, including this), and the `.animated-gradient-text` CSS class (the `--animated-gradient-text-*` CSS vars were already ported by the ZCode design-system work; the class itself was not). This task lands those, plus the two new dependencies, so Task 4 can compile.

**Files:**
- Create: `src/lib/scrollMask.ts`
- Create: `src/components/QueuedSummaryContent.tsx`
- Modify: `src/index.css`
- Modify: `package.json` (+ `pnpm-lock.yaml` via `pnpm install`)
- Test: `src/lib/scrollMask.test.ts`

**What to implement:**
1. **`src/lib/scrollMask.ts`** — copy `/home/daniel/Coding/AI/ZCode/packages/ui/src/mentions/components/scrollMask.ts` VERBATIM (it is pure: one `import type { CSSProperties } from "react"` and the five exports `ScrollMetrics`, `ScrollMaskState`, `EMPTY_SCROLL_MASK_STATE`, `resolveVerticalScrollMaskState`, `getVerticalScrollMaskStyle` — no other imports, no changes needed).
2. **`src/components/QueuedSummaryContent.tsx`** — copy `/home/daniel/Coding/AI/ZCode/packages/ui/src/ToolCallBlocks/QueuedSummaryContent.tsx` VERBATIM (its only imports are `react` and `motion/react`; both are available after the dependency step — no changes needed). Keep the file's header comment.
3. **`package.json`** — add to `dependencies` (alphabetical position):
   - `"@radix-ui/react-use-controllable-state": "^1.2.6"` — the exact version the installed `radix-ui@1.6.7` pins (check `pnpm-lock.yaml`). Rationale: `radix-ui`'s meta package does not re-export it at the ROOT (it is reachable via `radix-ui/internal`), and a direct import keeps the port's import line identical to ZCode's.
   - `"motion": "^12.38.0"` (the version ZCode uses)
   Then run `pnpm install` (it rewrites `pnpm-lock.yaml`).
4. **`src/index.css`** — append at the END of the file (after the `*::-webkit-scrollbar-corner` block, L693–696), wrapped in `@layer utilities` (as in ZCode — the Client's Tailwind v4 setup defines that cascade layer, and unlayered CSS would outrank Tailwind utilities, e.g. the class's `display: inline-block` beating a `hidden`/`flex` on the same element):
   ```css
   @layer utilities {
     .animated-gradient-text {
       display: inline-block;
       background: linear-gradient(
         90deg,
         var(--animated-gradient-text-strong) 0%,
         var(--animated-gradient-text-strong) 34%,
         var(--animated-gradient-text-soft) 50%,
         var(--animated-gradient-text-strong) 66%,
         var(--animated-gradient-text-strong) 100%
       );
       background-size: 300% 100%;
       background-clip: text;
       -webkit-background-clip: text;
       color: transparent;
       -webkit-text-fill-color: transparent;
       animation: gradient-flow 4s linear infinite;
       will-change: background-position;
       transform: translateZ(0);
       backface-visibility: hidden;
     }

     @keyframes gradient-flow {
       0% {
         background-position: 100% 0;
       }

       50% {
         background-position: 0% 0;
       }

       100% {
         background-position: 0% 0;
       }
     }
   }
   ```
   (This is ZCode `styles.css` L823–842 + L885–897 verbatim, keeping the `@layer utilities` wrapper.)

**Steps:**
- [ ] Write `src/lib/scrollMask.test.ts`:
  - `resolveVerticalScrollMaskState`: `scrollHeight <= clientHeight` (±1px threshold) → `EMPTY_SCROLL_MASK_STATE`; `scrollTop = 0`, scrollable → `{ showTop: false, showBottom: true }`; `scrollTop` past the top threshold (1px) and short of the bottom edge → `{ showTop: true, showBottom: true }`; `scrollTop` at the bottom edge → `{ showTop: true, showBottom: false }`.
  - `getVerticalScrollMaskStyle`: empty state → `undefined`; a state with `showBottom` only → an object with `maskRepeat: "no-repeat"`, `maskSize: "100% 100%"`, and `maskImage` EXACTLY `linear-gradient(to bottom, black 0px, black 24px, black calc(100% - 24px), transparent 100%)` (the function always emits FOUR stops — do NOT assert `startsWith("black")`, and do NOT invent a three-stop string); a state with `showTop` only → `maskImage` EXACTLY `linear-gradient(to bottom, transparent 0px, black 24px, black calc(100% - 24px), black 100%)` (the top edge fades in, the bottom edge stays opaque); a state with BOTH → `maskImage` EXACTLY `linear-gradient(to bottom, transparent 0px, black 24px, black calc(100% - 24px), transparent 100%)`.
- [ ] Run `pnpm vitest run src/lib/scrollMask.test.ts`
  - Fails (the module doesn't exist yet)? Confirm.
- [ ] Implement items 1–4 (create the two files, add the deps + `pnpm install`, append the CSS).
- [ ] Run `pnpm vitest run src/lib/scrollMask.test.ts`
  - All pass? If not, fix the TEST's expected strings against the real function output (NOT the verbatim port) and re-run.
- [ ] Run `pnpm build`
  - Type-check + build succeed (the new files compile, `motion` + `@radix-ui/react-use-controllable-state` resolve)? If not, fix and re-run.
- [ ] Commit with message: `feat: port scrollMask + QueuedSummaryContent, add motion dep and animated-gradient-text class` — stage `package.json` AND `pnpm-lock.yaml` together.

**Acceptance criteria:**
- [ ] `src/lib/scrollMask.ts` and `src/components/QueuedSummaryContent.tsx` exist and are verbatim ports of the ZCode sources.
- [ ] `pnpm build` passes with the two new dependencies installed (lockfile updated).
- [ ] `.animated-gradient-text` + `@keyframes gradient-flow` are in `src/index.css` inside `@layer utilities`.

---

### Task 4: Port the Reasoning component

**Context:**
The core of the feature: a faithful port of ZCode's `Reasoning` component (ADR 0007). It renders one **Thinking block**: a Radix Collapsible, collapsed by default, that auto-collapses on the streaming→done transition (unless the user interacted); a trigger with a brain icon + "Thinking" (animated gradient while streaming) / "Thought · a few seconds" / "Thought · N seconds" + a one-line live summary of the last non-empty line of the thinking text (auto-scrolls to the end, gradient edge mask, `QueuedSummaryContent` roll); and a max-h-60 scrollable content box (plain text — NO markdown, a deliberate ZCode performance choice; left border rail; auto-follows the bottom while streaming; scroll masks; 300ms delayed unmount so the collapse animation can read the content height).

**Files:**
- Create: `src/components/Reasoning.tsx`
- Test: `src/components/Reasoning.test.tsx`

**What to implement:**
Create `src/components/Reasoning.tsx` as a port of `/home/daniel/Coding/AI/ZCode/packages/ui/src/components/ai-elements/reasoning.tsx` — the entire file (all exports: `useReasoning`, `Reasoning`, `ReasoningTrigger`, `ReasoningContent`, `shouldAutoCollapseReasoning`, `getReasoningBottomDistance`, `isReasoningScrollAtBottom`, `scrollReasoningSummaryToEnd`, `isReasoningSummaryOverflowing`, `resolveReasoningStreamingSummary`, `getReasoningSummaryMaskStyle`, and the constants). Behavior is VERBATIM; only the following changes:

1. **Imports:**
   - `import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";` (replaces `../ui/collapsible.js`)
   - `import { cn } from "@/components/lib/utils";` (replaces `../lib/utils.js`)
   - `import { useControllableState } from "@radix-ui/react-use-controllable-state";` (same as ZCode)
   - `import { QueuedSummaryContent } from "./QueuedSummaryContent";` (replaces `@/ToolCallBlocks/QueuedSummaryContent.js`)
   - `import { EMPTY_SCROLL_MASK_STATE, getVerticalScrollMaskStyle, resolveVerticalScrollMaskState, type ScrollMetrics, type ScrollMaskState } from "@/lib/scrollMask";` (replaces `@/mentions/components/scrollMask.js`)
   - `import { BrainIcon, ChevronRightIcon } from "lucide-react";` (same as ZCode)
   - `import { createContext, memo, useCallback, useContext, useEffect, useMemo, useRef, useState } from "react";` and `import type { ComponentProps, CSSProperties, ReactNode } from "react";` (same as ZCode)
   - DELETE: `import { TID_CHAT_REASONING_CONTENT, TID_CHAT_REASONING_TRIGGER } from "@zcode/shared";` and `import { useZCodeIntl } from "@/i18n/IntlProvider.js";`
   - DROP the leading `"use client";` directive (the Client is a Vite SPA — a module-level directive triggers a Rollup "module level directives cause errors when bundled" warning and has no meaning here). This is the ONE allowed structural deletion.
   - Keep the provenance header comment, appending one line: `// Ported to the Client 2026-09-23 (ADR 0007): imports, i18n, test-ids adapted; "use client" dropped (Vite SPA); behavior verbatim.`
2. **i18n → literals** (the exact ZCode en-US strings, `packages/ui/src/i18n/locales/en-US.ts` L4346–4349):
   - `intl.formatMessage({ id: "chat.reasoning.thinking" })` → `"Thinking"`
   - `intl.formatMessage({ id: "chat.reasoning.thought" })` → `"Thought"`
   - `intl.formatMessage({ id: "chat.reasoning.durationFewSeconds" })` → `"a few seconds"`
   - `intl.formatMessage({ id: "chat.reasoning.durationSeconds" }, { seconds: String(duration) })` → `` `${duration} seconds` `` (the real locale value is `"{seconds} seconds"` — NOT `"{seconds}s"`)
   - Remove the `const { intl } = useZCodeIntl();` line.
3. **Test-ids → literals:** `data-testid={TID_CHAT_REASONING_TRIGGER}` → `data-testid="reasoning-trigger"`; `data-testid={TID_CHAT_REASONING_CONTENT}` → `data-testid="reasoning-content"`.
4. **Everything else stays verbatim** — do NOT "improve" the behavior: the `useControllableState` wiring, the `startTimeRef`/duration interval logic, the `autoCollapseKey` effect (including the `isOpenControlled` guard), the `shouldRenderContent` 300ms delayed-unmount effect, the trigger's `ResizeObserver` auto-scroll-to-end + mask logic, and the content's bottom-follow + scroll-mask logic. Keep all `data-reasoning-*` attributes and all inline comments (they document non-obvious behavior).

**Test-assertion rule (applies to ALL tests below):** the trigger's done-state label is THREE spans (`<span>Thought</span><span>·</span><span>N seconds</span>`) with no whitespace between them — there is NO text node reading "Thought · N seconds", and the project has no jest-dom `toHaveTextContent`. So: assert the pieces individually — `screen.getByText("Thought")`, `screen.getByText("a few seconds")` / `screen.getByText("2 seconds")` — and never `getByText("Thought · …")`. In fake-timer tests use ONLY synchronous `getBy*`/`queryBy*` + `act(() => vi.advanceTimersByTime(N))` — never `findBy*`/`waitFor` (they poll on timers and stall).

**Steps:**
- [ ] Write `src/components/Reasoning.test.tsx` (jsdom + `@testing-library/react`, following the conventions in `src/components/SessionConfigSelect.test.tsx` — `beforeAll` stubs for `matchMedia`/pointer capture as needed; `afterEach(() => vi.useRealTimers())`). Call `vi.useFakeTimers()` BEFORE `render` in every test that uses fake timers, so `Date.now()` is mocked from the start, and wrap every timer advance in `act(() => vi.advanceTimersByTime(N))`:
  - **Collapsed by default:** render `<Reasoning isStreaming={false}><ReasoningTrigger /><ReasoningContent>body</ReasoningContent></Reasoning>`; the trigger shows "Thought" AND "a few seconds" (duration undefined — two separate `getByText` assertions); the content body is NOT in the document.
  - **Streaming label:** with `isStreaming` and collapsed → the trigger shows "Thinking" in a `.animated-gradient-text` span (query by text + class).
  - **Live summary:** `<ReasoningTrigger streamingText={"line one\n\nline two"}` while streaming+collapsed → the summary span (`[data-reasoning-streaming-line]`) shows "line two" (the last NON-EMPTY line); with `streamingText="  "` (blank) → no summary line rendered.
  - **Expand:** `fireEvent.click` the trigger → the content body appears; the trigger chevron is `rotate-90`.
  - **Delayed content unmount (via context, NOT the DOM):** in jsdom no CSS is loaded, so Radix `Presence` reads `animationName === "none"` and unmounts `CollapsibleContent` immediately — the 300ms `shouldRenderContent` delay is NOT observable in the DOM here. Instead, render a probe child inside `<Reasoning>`: `function Probe() { const { shouldRenderContent } = useReasoning(); return <span data-testid="probe">{String(shouldRenderContent)}</span>; }` (add it alongside `ReasoningTrigger`/`ReasoningContent` in the test render). With fake timers on before render: open the block (click), assert probe = "true"; close it (click); assert probe is STILL "true" immediately after the click (the content is held for the delay); `act(() => vi.advanceTimersByTime(300))` → probe = "false".
  - **Auto-collapse — pure unit tests of `shouldAutoCollapseReasoning`:** `{ autoCollapseKey: "b", previousAutoCollapseKey: "a", userInteracted: false }` → `true` (a new key, no interaction → collapse); `{ autoCollapseKey: "b", previousAutoCollapseKey: "a", userInteracted: true }` → `false` (user interaction wins); `{ autoCollapseKey: null, previousAutoCollapseKey: "a", userInteracted: false }` → `false` (null key → no auto-collapse).
  - **Auto-collapse — integration (the real path, no user click):** render `<Reasoning defaultOpen isStreaming autoCollapseKey={null}>…</Reasoning>` (open via `defaultOpen` — this does NOT set `userInteracted` and does NOT make the component controlled), then re-render the same tree with `isStreaming={false}` + `autoCollapseKey="complete"` → the block collapses (assert via the probe from the previous test, or the chevron state; use fake timers + `act` advance as needed).
  - **Duration:** with fake timers on before render, `isStreaming` and the block opened (click to expand): the label's duration span shows "1 seconds" (`getByText("1 seconds")` — the initial `updateDuration` clamps `max(1, ceil(0/1000))` to 1); `act(() => vi.advanceTimersByTime(2000))` → `getByText("2 seconds")` (`ceil(2000/1000) = 2` — NOT 3); re-render with `isStreaming={false}` → the final duration is frozen at 2 (a further `act(() => vi.advanceTimersByTime(5000))` leaves `getByText("2 seconds")` in place and `queryByText("3 seconds")` is `null`).
- [ ] Run `pnpm vitest run src/components/Reasoning.test.tsx`
  - Fails (the component doesn't exist)? Confirm.
- [ ] Implement `src/components/Reasoning.tsx` (the port with the adaptations above).
- [ ] Run `pnpm vitest run src/components/Reasoning.test.tsx`
  - All pass? If not, fix the component (NOT the tests' expectations — the behavior is ZCode's; if a test's expected string is wrong per the test-assertion rule, fix the string, not the port) and re-run.
- [ ] Run `pnpm build`
  - Succeeds? If not, fix and re-run.
- [ ] Commit with message: `feat: port the ZCode Reasoning component (thinking block)`

**Acceptance criteria:**
- [ ] `src/components/Reasoning.tsx` exports `Reasoning`, `ReasoningTrigger`, `ReasoningContent`, `useReasoning` (+ the pure helpers) with ZCode-verbatim behavior.
- [ ] The only differences from the ZCode source are the import paths, the i18n literals, the test-id literals, the dropped `"use client"` directive, and the provenance comment line.
- [ ] All the tests above pass.

---

### Task 5: Wiring — MessageBubble, ChatStream, SubagentPanel

**Context:**
The `Reasoning` component exists but nothing renders it. This task wires the `agent-thought` message kind into the two transcript renderers. Streaming semantics (the spec's rule): a thought block is *streaming* when its session is `inTurn` and the block is the LAST message in the transcript (no text/tool call followed). The first text chunk, the first tool call, or `turnCompleted` all flip it to done — which the `autoCollapseKey={isStreaming ? null : "complete"}` prop turns into ZCode's auto-collapse-on-done behavior.

**Files:**
- Modify: `src/components/MessageBubble.tsx`
- Modify: `src/components/ChatStream.tsx`
- Modify: `src/components/SubagentPanel.tsx`
- Test: `src/components/MessageBubble.test.tsx` (new)
- Test: `src/components/ChatStream.test.tsx` (extend)
- Test: `src/components/SubagentPanel.test.tsx` (extend)

**What to implement:**
1. **`src/components/MessageBubble.tsx`**:
   - Change the signature to `export default function MessageBubble({ message, isStreaming = false }: { message: Message; isStreaming?: boolean })`.
   - Import `Reasoning`, `ReasoningTrigger`, `ReasoningContent` from `./Reasoning`.
   - Add a case to the `switch (message.kind)` (the switch currently covers `user`, `agent-text`, `tool-call`, `diff` — there is no `default` arm and `noImplicitReturns` is not on, so adding a case compiles; add it after the `diff` case):
     ```tsx
     case "agent-thought":
       return (
         <Reasoning
           isStreaming={isStreaming}
           autoCollapseKey={isStreaming ? null : "complete"}
         >
           <ReasoningTrigger streamingText={message.text} />
           <ReasoningContent>{message.text}</ReasoningContent>
         </Reasoning>
       );
     ```
   - Do NOT change the other cases.
2. **`src/components/ChatStream.tsx`** (the `messages.map` at L494–514; the final return is L513: `return <MessageBubble key={i} message={message} />;`): change that final return to
   ```tsx
   return (
     <MessageBubble
       key={i}
       message={message}
       isStreaming={
         message.kind === "agent-thought" &&
         inTurn &&
         i === messages.length - 1
       }
     />
   );
   ```
   (`inTurn` is already in scope — L75: `useSessions((s) => (s.activeSessionId ? !!s.inTurn[s.activeSessionId] : false))`.)
3. **`src/components/SubagentPanel.tsx`** (the `messageList.map` at L98–117): add a case for `agent-thought` — the same `Reasoning` wiring, where "live" is `entry.status === "running"` (the `SubagentEntry.status` from the subagents store; `entry` is in scope as the `SubagentSection` prop):
   ```tsx
   if (m.kind === "agent-thought") {
     const isStreaming = entry.status === "running" && i === messageList.length - 1;
     return (
       <Reasoning
         key={i}
         isStreaming={isStreaming}
         autoCollapseKey={isStreaming ? null : "complete"}
       >
         <ReasoningTrigger streamingText={m.text} />
         <ReasoningContent>{m.text}</ReasoningContent>
       </Reasoning>
     );
   }
   ```
   Add the `Reasoning`/`ReasoningTrigger`/`ReasoningContent` import. Update the compact-stream comment (L92–96) to mention the thinking block.
4. **Do NOT change:** the `AskQuestionCard` anchoring logic, the `PermissionPrompt`/`AskQuestionCard`/`SudoConfirmModal` rendering, the store.

**Steps:**
- [ ] Create `src/components/MessageBubble.test.tsx`:
  - an `agent-thought` message `{ kind: "agent-thought", messageId: "m1", text: "pondering", at: 1 }` renders a collapsed `Reasoning` block: the trigger shows "Thought" AND "a few seconds" (two separate `getByText` assertions); the content body is absent until the trigger is clicked.
  - with `isStreaming` → the trigger shows "Thinking" (`.animated-gradient-text`) + the live summary line "pondering"; the content is absent until clicked.
  - the existing kinds are unaffected: a `user` message renders its text; an `agent-text` message renders markdown (smoke: a `**bold**` message renders a `<strong>`).
- [ ] Extend `src/components/ChatStream.test.tsx`:
  - seed the store with a session whose transcript ends in an `agent-thought` message and `inTurn[sessionId] = true` → the rendered block shows "Thinking"; then `turnCompleted` (inside `act()`) → the block shows the "Thought" label (assert ONLY the "Thought" word, NOT a duration — the done-transition duration is `ceil(elapsed/1000)`, i.e. 0 or 1 here, since `startTimeRef` is set whenever `isStreaming` is true even while collapsed; only a block that NEVER streamed shows "a few seconds". Asserting the duration would be flaky).
  - a transcript whose LAST message is `agent-text` (with an earlier `agent-thought`) while `inTurn` → the thought block shows the done label ("Thought"), NOT "Thinking" (its duration is genuinely `undefined` → "a few seconds" — but assert only "Thought").
  (Use the store's actions — `addSession`/`applySessionUpdate`/`turnCompleted` — to seed state, following the existing test's patterns; do store updates made after `render` inside `act()`.)
- [ ] Extend `src/components/SubagentPanel.test.tsx`:
  - a `running` subagent entry whose last transcript message is `agent-thought` → the block shows "Thinking" (streaming: `entry.status === "running"` + last message);
  - the same entry with `status: "completed"` (via the store's `markClosed`) → the block shows "Thought" (not "Thinking").
- [ ] Run `pnpm vitest run src/components/MessageBubble.test.tsx src/components/ChatStream.test.tsx src/components/SubagentPanel.test.tsx`
  - The new tests fail (no `agent-thought` case / no `isStreaming` prop)? Confirm.
- [ ] Implement items 1–3.
- [ ] Run `pnpm vitest run src/components/MessageBubble.test.tsx src/components/ChatStream.test.tsx src/components/SubagentPanel.test.tsx`
  - All pass? If not, fix and re-run.
- [ ] Run `pnpm test` (the whole frontend suite)
  - All pass? If not, fix and re-run.
- [ ] Run `pnpm build`
  - Succeeds? If not, fix and re-run.
- [ ] Commit with message: `feat: render thinking blocks in the chat stream and subagent panel`

**Acceptance criteria:**
- [ ] A live turn's thinking renders as a collapsed "Thinking · <live line>" block before the answer text; when the answer text (or a tool call) starts, the block auto-collapses and shows "Thought · N seconds".
- [ ] Loaded history shows the persisted thinking blocks (collapsed, "Thought · a few seconds" — no duration was persisted), one per contiguous thinking run (the Task 1 segmentation).
- [ ] The subagent panel shows subagent thinking blocks (collapsed by default, streaming while the subagent is `running`).
- [ ] The full frontend suite (`pnpm test`) and `pnpm build` pass.

---

## Out of scope (from the approved spec)

- Thinking token *counts* (usage stats) — no usage display surface exists.
- Markdown rendering inside thinking (ZCode deliberately uses plain text — performance).
- `agent_thought` final-message handling (v2-only; the Client speaks v1).
- Duration persistence (loaded blocks show "a few seconds" — ZCode's undefined-duration display).
- **Thinking on resume** (known limitation, see the top of this plan): `resume_session` wipes the transcript rows before `session/load` and pi-acp's replay never contains thinking — a resumed session loses its thinking blocks. Stored (non-resumed) sessions keep them.
- **Agent-text segmentation** (pre-existing behavior, see the top of this plan): stored answer text is not segmented; a follow-up could apply the same `ThoughtState`-style segmentation to agent text.
