---
status: approved
done-when: A tool-call card in the transcript shows a per-tool summary (command / file path) on its header line and, when expanded, the tool's output text — live while the tool runs and after a session reload; `pnpm test` + `pnpm build` green.
---

# Tool-call output display

## Problem

Tool-call cards in the transcript show only the tool name (e.g. `bash`, `read`) and, when
expanded, a `content`-derived diff that the RPC path never populates — so every card
expands to `"(no output)"`. The user cannot see which file was read/written, which
command ran, or what the tool returned.

## Root cause

The data already flows end-to-end; the frontend drops it:

1. **Backend (Rust, `src-tauri/src/agent/session.rs` `normalize()`)** emits
   `tool_call` / `tool_call_update` frames carrying:
   - `rawInput` — the tool's arguments (from `toolcall_start` / `toolcall_delta` /
     `toolcall_end`),
   - `rawOutput` — the tool's result, including **live partial results**
     (`tool_execution_update` → `rawOutput: partialResult`) and the final result
     (`tool_execution_end` → `rawOutput: result`, `status: completed|failed`).
   Both are **persisted** into the tool-call row by `persist_update`'s `merge_json`.
2. **Frontend type (`src/lib/tauri.ts`)** — `AcpSessionUpdate` has `rawInput` but no
   `rawOutput` field, so the reducer never reads it.
3. **Frontend store (`src/store/sessions.ts`)** — the `tool-call` `Message` carries
   `rawInput` (used only by the todo board) and `diff` (never set on the RPC path);
   no `rawOutput`.
4. **Frontend card (`src/components/ToolCallCard.tsx`)** — renders title + status icon
   only; expanded body shows the (never-present) diff, else `"(no output)"`.

## Design (frontend-only — no Rust changes)

### Data plumbing

- `src/lib/tauri.ts`: add `rawOutput?: unknown` to the `tool_call` and
  `tool_call_update` variants of `AcpSessionUpdate` (the wire already carries it).
- `src/store/sessions.ts`:
  - The `tool-call` `Message` gains `rawOutput?: unknown` alongside `rawInput`.
  - The reducer stores it with the same merge rule as `rawInput`
    (`update.rawOutput ?? prev.rawOutput`): a status-only update does not clobber the
    result; `tool_execution_update` frames overwrite it with newer partial results
    while the tool runs.
  - `rowToMessages` restores `rawOutput` from the persisted payload (the row already
    contains it — reloads work, no backend change).
  - `mergeDedupeKey` is unchanged (`tool-call|id|title|status`) so resume-dedupe
    matching is unaffected.

### Rendering — `ToolCallCard`

**Header line** (stays the current single `h-8` row): status icon + `title` + a muted,
truncated **summary** derived from `rawInput` by a small pure helper
(`summarizeToolCall(title, rawInput): string | undefined`):

| Tool(s) | Summary |
|---|---|
| `bash` / `sudo_exec` / `powershell` | the `command` |
| `read` | `path` (+ `L5–54` when `offset`/`limit` present) |
| `write` | `path` |
| `edit` | `path` (+ `· 3 edits` when `edits.length > 1`) |
| `grep` / `find` | `pattern` (+ ` in {path}` when present) |
| `ls` | `path` (or `.`) |
| `web_search` | `query` |
| `fetch_content` | `url` |
| `ask` | first question text (truncated) |
| `manage_todo_list` | `operation` |
| `subagent` | `task` (or `N tasks` when `tasks[]` present) |
| `mcp` | `tool` |
| fallback | compact `JSON.stringify(rawInput)` truncated (~80 chars); empty/missing input → title only |

**Expanded body** (replaces the always-`"(no output)"` path; existing diff rendering
keeps priority when a diff is present):

- Normalize `rawOutput` to display text:
  - pi's `AgentToolResult` shape (`{ content: (TextContent | ImageContent)[],
    details?, usage? }`): join text items, count image items as `(+2 images)`, show
    `details` only as a fallback when there is no text;
  - a bare string → shown as-is;
  - any other object → compact JSON.
- Render in a `<pre>` with `max-h-80 overflow-auto whitespace-pre-wrap`, capped at
  20k chars with a `… (truncated — N bytes total)` note.
- While `status` is pending, the same block shows the live partial result (no extra
  work — the partial frames already flow through the merge rule above).
- No output and no diff → `(no output)` (unchanged).

### Tests (TDD — failing first)

- `src/store/sessions.test.ts`:
  - `applySessionUpdate` stores `rawOutput` on `tool_call` create and on
    `tool_call_update`;
  - a later update without `rawOutput` preserves the stored value;
  - `rowToMessages` restores `rawOutput` from the persisted payload.
- `src/components/ToolCallCard.test.tsx` (new):
  - header summary for `bash` (command) and `read` (path + line range);
  - unknown tool with an object input → compact-JSON fallback; empty input → title only;
  - expanded body renders output text (content array and bare-string shapes) and
    `(no output)` when absent.

## Out of scope (YAGNI)

- Rendering image bytes inline (counted only).
- Showing the full raw input JSON in the expanded body (the summary covers it).
- Any Rust/backend changes (`rawOutput` is already emitted + persisted).
