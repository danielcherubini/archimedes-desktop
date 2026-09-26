---
status: committed
done-when: A tool-call card in the transcript shows a per-tool summary (command / file path) on its header line and, when expanded, the tool's output text — live while the tool runs and after a session reload; `pnpm test` + `pnpm build` green.
---

# Tool-Call Output Display Plan

**Goal:** Tool-call cards show what happened: a one-line summary (command / file path) and the tool's output on expand.
**Architecture:** Frontend-only. The Rust backend already emits `rawInput` + `rawOutput` in `tool_call` / `tool_call_update` frames (`normalize()` in `src-tauri/src/agent/session.rs`) and persists both into the tool-call row (`persist_update`'s `merge_json`). The frontend drops `rawOutput`: `AcpSessionUpdate` has no such field, the `tool-call` `Message` has no such field, and `ToolCallCard` renders only the title. This plan plumbs `rawOutput` through type → store → card, and adds a `rawInput`-derived summary line.
**Tech Stack:** React 19 + TypeScript, Zustand (`src/store/sessions.ts`), Vitest + `@testing-library/react`. Validation from the repo root: `pnpm test` (vitest run), `pnpm build` (tsc + vite build). There is NO frontend lint/format script.

Background for the executing agent: pi's tool result (`tool_execution_end.result`, the RPC `AgentToolResult`) has the shape `{ content: (TextContent | ImageContent)[], details?: T, usage?: Usage, terminate?: boolean }` — e.g. bash returns `{ content: [{ type: "text", text: "stdout…" }] }`. `tool_execution_update` carries the same shape as a live partial result. The desktop's own suite tools (ask / sudo_exec / manage_todo_list / subagent / mcp, executed in `src-tauri/src/agent/bridge.rs`) return the same shape. The `status` of a tool-call frame is `pending` | `in_progress` | `completed` | `failed`; the store maps both `pending` and `in_progress` to the UI `pending` via `mapStatus`.

---

### Task 1: Plumb `rawOutput` through the tool-call message

**Context:**
The wire frames already carry `rawOutput` (final result on `tool_execution_end`, live partial on `tool_execution_update`) and the persisted tool-call row already contains it (Rust `merge_json` shallow-merges every frame into the row payload). But the frontend type `AcpSessionUpdate` (`src/lib/tauri.ts`) does not declare `rawOutput`, the `tool-call` `Message` variant (`src/store/sessions.ts`) has no `rawOutput` field, the reducer `applySessionUpdate` never reads it, and `rowToMessages` never restores it. This task makes the store carry `rawOutput` exactly like it already carries `rawInput` (which exists only to feed the todo board and is NOT rendered — do not touch its usages). No Rust changes: `src-tauri` is untouched in this feature.

**Files:**
- Modify: `src/lib/tauri.ts`
- Modify: `src/store/sessions.ts`
- Test: `src/store/sessions.test.ts`

**What to implement:**

1. `src/lib/tauri.ts` — in the `AcpSessionUpdate` type (around lines 142–170), add `rawOutput?: unknown;` immediately after the existing `rawInput?: unknown;` in BOTH variants:
   - the `tool_call` variant (`{ sessionUpdate: "tool_call"; toolCallId: string; title?; status?; rawInput?; content? }`),
   - the `tool_call_update` variant (same field set).
   Also update the type's doc comment: it currently says nothing about `rawOutput`; add one line: `rawOutput` is the tool's result (the RPC `AgentToolResult`), live-partial while the tool runs, final on `tool_execution_end`.

2. `src/store/sessions.ts`:
   - The `tool-call` `Message` variant (around lines 33–47): add `rawOutput?: unknown;` after the `rawInput?: unknown;` field, with a doc comment: `The ACP rawOutput (the tool's result — the RPC AgentToolResult, live-partial while running)`. Do NOT change the existing `rawInput` doc comment.
   - `applySessionUpdate`, `case "tool_call"` (around line 136): in the created `Message` literal, add `rawOutput: update.rawOutput,` after the `rawInput: update.rawInput,` line.
   - `applySessionUpdate`, `case "tool_call_update"` — the "update for a tool call we never saw" branch (the second `Message` literal, around line 158): add `rawOutput: update.rawOutput,` after the `rawInput` line.
   - `applySessionUpdate`, `case "tool_call_update"` — the existing-message branch (the `updated` literal, around line 169): add `rawOutput: update.rawOutput ?? prev.rawOutput,` after the `rawInput: update.rawInput ?? prev.rawInput,` line. (Same merge rule as `rawInput`: a status-only update must not clobber the result; a newer partial overwrites.)
   - `rowToMessages`, `case "tool-call"` (around line 317): in the restored `Message` literal, add `rawOutput: payload.rawOutput,` after the `rawInput: payload.rawInput,` line.
   - `mergeDedupeKey` (around line 249): **do NOT change** — it stays `tool-call|${m.id}|${m.title}|${m.status}` so resume-dedupe matching is unaffected by the new field.

**Steps:**
- [ ] Add these FIVE failing tests to `src/store/sessions.test.ts`, following the EXACT style of the existing `rawInput` tests (the three in `describe("applySessionUpdate — tool calls")` at lines ~282–346 and the one in `describe("rowToMessages (history replay from the database)")` at line ~805). Insert the first FOUR after the "preserves an earlier rawInput when a later update carries none" test (line ~346) — they all belong in `describe("applySessionUpdate — tool calls")` — and the FIFTH after the "maps a tool-call row's rawInput onto the message" test (line ~805), in `describe("rowToMessages (history replay from the database)")`:

  ```ts
  it("keeps rawOutput on the tool-call message", () => {
    const messages = applySessionUpdate(
      [],
      {
        sessionUpdate: "tool_call",
        toolCallId: "tc1",
        title: "bash",
        rawOutput: { content: [{ type: "text", text: "hello" }] },
      },
      1,
    );
    const toolCall = messages.find((m) => m.kind === "tool-call");
    if (toolCall?.kind !== "tool-call") throw new Error("no tool-call message");
    expect(toolCall.rawOutput).toEqual({ content: [{ type: "text", text: "hello" }] });
  });

  it("keeps the rawOutput from a tool_call_update", () => {
    let messages = applySessionUpdate(
      [],
      { sessionUpdate: "tool_call", toolCallId: "tc1", title: "bash" },
      1,
    );
    messages = applySessionUpdate(
      messages,
      {
        sessionUpdate: "tool_call_update",
        toolCallId: "tc1",
        rawOutput: { content: [{ type: "text", text: "world" }] },
      },
      2,
    );
    const toolCall = messages.find((m) => m.kind === "tool-call");
    if (toolCall?.kind !== "tool-call") throw new Error("no tool-call message");
    expect(toolCall.rawOutput).toEqual({ content: [{ type: "text", text: "world" }] });
  });

  it("preserves an earlier rawOutput when a later update carries none", () => {
    let messages = applySessionUpdate(
      [],
      {
        sessionUpdate: "tool_call",
        toolCallId: "tc1",
        title: "bash",
        rawOutput: { content: [{ type: "text", text: "hello" }] },
      },
      1,
    );
    messages = applySessionUpdate(
      messages,
      { sessionUpdate: "tool_call_update", toolCallId: "tc1", status: "completed" },
      2,
    );
    const toolCall = messages.find((m) => m.kind === "tool-call");
    if (toolCall?.kind !== "tool-call") throw new Error("no tool-call message");
    expect(toolCall.rawOutput).toEqual({ content: [{ type: "text", text: "hello" }] });
  });

  it("stores rawOutput when a tool_call_update arrives for a tool call we never saw", () => {
    const messages = applySessionUpdate(
      [],
      {
        sessionUpdate: "tool_call_update",
        toolCallId: "tc1",
        rawOutput: { content: [{ type: "text", text: "late" }] },
      },
      1,
    );
    const toolCall = messages.find((m) => m.kind === "tool-call");
    if (toolCall?.kind !== "tool-call") throw new Error("no tool-call message");
    expect(toolCall.rawOutput).toEqual({ content: [{ type: "text", text: "late" }] });
  });
  ```

  and in the `rowToMessages` describe:

  ```ts
  it("maps a tool-call row's rawOutput onto the message", () => {
    const [msg] = rowToMessages(
      row({
        kind: "tool-call",
        messageKey: "tc1",
        payloadJson: JSON.stringify({
          toolCallId: "tc1",
          title: "bash",
          status: "completed",
          rawOutput: { content: [{ type: "text", text: "done" }] },
        }),
      }),
    );
    if (msg?.kind !== "tool-call") throw new Error("no tool-call message");
    expect(msg.rawOutput).toEqual({ content: [{ type: "text", text: "done" }] });
  });
  ```

- [ ] Run `pnpm vitest run src/store/sessions.test.ts`
  - Did it fail with runtime `expect(undefined).toEqual(...)` failures (vitest transpiles with esbuild and does NOT type-check — the `Message`-type error would only surface in `pnpm build`)? If it passed unexpectedly, stop and investigate why.
- [ ] Implement the `src/lib/tauri.ts` + `src/store/sessions.ts` changes above.
- [ ] Run `pnpm vitest run src/store/sessions.test.ts`
  - Did all tests pass? If not, fix the failures and re-run before continuing.
- [ ] Run `pnpm build`
  - Did it succeed (tsc + vite build, 0 errors)? If not, fix and re-run before continuing.
- [ ] Commit with message: "feat(store): carry rawOutput on tool-call messages"

**Acceptance criteria:**
- [ ] `AcpSessionUpdate`'s `tool_call` and `tool_call_update` variants declare `rawOutput?: unknown`.
- [ ] A `tool_call` frame with `rawOutput` stores it on the message; a `tool_call_update` with `rawOutput` overwrites it; a status-only update preserves it.
- [ ] `rowToMessages` restores `rawOutput` from a persisted tool-call row payload.
- [ ] `pnpm test` (full suite) and `pnpm build` are green.

---

### Task 2: Render the summary line + output in `ToolCallCard`

**Context:**
After Task 1 the `tool-call` `Message` carries both `rawInput` (the tool's args — e.g. bash `{ command }`, read `{ path, offset?, limit? }`, edit `{ path, edits[] }`) and `rawOutput` (the result). The card (`src/components/ToolCallCard.tsx`) currently renders only the title + status icon and, on expand, a `content`-derived diff that the RPC path never populates — so it always shows `"(no output)"`. This task: (a) a muted, truncated one-line **summary** on the header derived from `rawInput` via a pure exported helper (per the approved design table), and (b) an expanded **output** block that normalizes `rawOutput` (pi's `AgentToolResult` shape, or a bare string) to display text — scrollable, capped at 20k chars with a truncation note, live-updating while the tool runs for free (the partial frames flow through Task 1's merge rule). The existing diff rendering keeps priority. Both call sites (`src/components/MessageBubble.tsx` ~line 201 and `src/components/SubagentPanel.tsx` ~line 118) must pass the two new props.

**Files:**
- Modify: `src/components/ToolCallCard.tsx`
- Modify: `src/components/MessageBubble.tsx`
- Modify: `src/components/SubagentPanel.tsx`
- Test: `src/components/ToolCallCard.test.tsx` (new file)

**What to implement:**

1. `src/components/ToolCallCard.tsx` — add THREE helpers (export `summarizeToolCall` and `normalizeToolOutput` — the tests call them directly; keep `truncate` private). Exact code:

   ```ts
   /** Truncate to `max` chars, appending an ellipsis when cut. */
   function truncate(s: string, max: number): string {
     return s.length > max ? s.slice(0, max) + "…" : s;
   }

   /**
    * A one-line summary of a tool call's input (the header line): the
    * command / file path / pattern, per the approved design table.
    * Unknown tools fall back to a compact JSON dump of the input;
    * `undefined` when there is nothing worth showing.
    */
   export function summarizeToolCall(title: string, rawInput: unknown): string | undefined {
     if (typeof rawInput !== "object" || rawInput === null) return undefined;
     const input = rawInput as Record<string, unknown>;
     const str = (k: string): string | undefined =>
       typeof input[k] === "string" ? (input[k] as string) : undefined;
     switch (title) {
       case "bash":
       case "sudo_exec":
       case "powershell":
         return str("command");
       case "read": {
         const path = str("path");
         if (!path) return undefined;
         const offset = typeof input.offset === "number" ? input.offset : undefined;
         const limit = typeof input.limit === "number" ? input.limit : undefined;
         // pi's `read` offset is 1-based; the end line is inclusive.
         if (offset !== undefined && limit !== undefined)
           return `${path} (L${offset}–${offset + limit - 1})`;
         return path;
       }
       case "write":
         return str("path");
       case "edit": {
         const path = str("path");
         if (!path) return undefined;
         const n = Array.isArray(input.edits) ? (input.edits as unknown[]).length : 0;
         return n > 1 ? `${path} (${n} edits)` : path;
       }
       case "grep":
       case "find": {
         const pattern = str("pattern");
         if (!pattern) return undefined;
         const path = str("path");
         return path ? `${pattern} in ${path}` : pattern;
       }
       case "ls":
         return str("path") ?? ".";
       case "web_search":
         return str("query");
       case "fetch_content":
         return str("url");
       case "ask": {
         const questions = input.questions;
         const first = Array.isArray(questions) && questions.length > 0 ? questions[0] : undefined;
         const q =
           first && typeof first === "object"
             ? (first as Record<string, unknown>).question
             : undefined;
         return typeof q === "string" ? truncate(q, 60) : undefined;
       }
       case "manage_todo_list":
         return str("operation");
       case "subagent": {
         const task = str("task");
         if (task) return truncate(task, 60);
         const tasks = input.tasks;
         return Array.isArray(tasks) ? `${tasks.length} tasks` : undefined;
       }
       case "mcp":
         return str("tool");
       default: {
         const text = JSON.stringify(rawInput);
         return text === "{}" || text === "null" ? undefined : truncate(text, 80);
       }
     }
   }

   /**
    * Normalize a tool result to display text. Accepts pi's `AgentToolResult`
    * shape (`{ content: (TextContent | ImageContent)[], details? }` — text
    * items joined, images counted, `details` as a no-text fallback), a bare
    * string (as-is), or any other object (compact JSON). `undefined` when
    * there is nothing to show.
    */
   export function normalizeToolOutput(rawOutput: unknown): string | undefined {
     if (typeof rawOutput === "string") return rawOutput === "" ? undefined : rawOutput;
     if (typeof rawOutput !== "object" || rawOutput === null) return undefined;
     const result = rawOutput as Record<string, unknown>;
     if (Array.isArray(result.content)) {
       const parts: string[] = [];
       let images = 0;
       for (const item of result.content as Array<Record<string, unknown>>) {
         if (item && item.type === "text" && typeof item.text === "string")
           parts.push(item.text);
         else if (item && item.type === "image") images += 1;
       }
       let text = parts.join("\n");
       if (images > 0)
         text = (text ? text + "\n" : "") + `(+${images} image${images > 1 ? "s" : ""})`;
       if (text !== "") return text;
     }
     if (result.details !== undefined) {
       const d = JSON.stringify(result.details);
       return d === "null" || d === "{}" ? undefined : d;
     }
     const whole = JSON.stringify(rawOutput);
     return whole === "{}" || whole === "null" ? undefined : whole;
   }
   ```

2. `src/components/ToolCallCard.tsx` — the component:
   - Props: add `rawInput?: unknown` and `rawOutput?: unknown` to the prop type (alongside `title`, `status`, `diff`).
   - Compute `const summary = summarizeToolCall(title, rawInput);` and `const output = normalizeToolOutput(rawOutput);` at the top of the component body.
   - Header row (the existing `button`, keep `flex h-8 w-full items-center gap-2 rounded-lg px-2 text-left hover:bg-surface-hover`): keep the status icon as-is; the existing title `span` becomes `min-w-0 truncate text-ui-base …` (keep its existing status-based color classes); then add, after the title span:
     ```tsx
     {summary && (
       <span className="min-w-0 truncate text-ui-sm text-foreground-subtlest">{summary}</span>
     )}
     ```
     (The row is `flex`; the two spans truncate independently. Do not change the icon's `shrink-0`.)
   - Expanded body (the existing `open &&` block, keep `mt-1 rounded-md bg-surface p-2`): keep the `diff` branch first, EXACTLY as today. Replace the `else` branch (currently the `"(no output)"` paragraph) with:
     ```tsx
     ) : output !== undefined ? (
       <div>
         <pre className="font-mono max-h-80 overflow-auto whitespace-pre-wrap text-ui-sm">
           {output.length > 20000 ? output.slice(0, 20000) + "…" : output}
         </pre>
         {output.length > 20000 && (
           <p className="text-ui-sm text-foreground-subtlest">
             … (truncated — {output.length} chars total)
           </p>
         )}
       </div>
     ) : (
       <p className="text-ui-sm text-foreground-subtlest">(no output)</p>
     )}
     ```
     (While `status` is `pending` the same block shows the live partial — no extra code: the partial frames update `rawOutput` through the store.)
   - Update the component's top doc comment: it currently claims "the `tool-call` message has a `rawInput` field but NO raw output field, so there is no output rendering to invent" — that is now false. Rewrite it: one `h-8` tool row (status icon + title + a muted one-line summary derived from `rawInput`); expandable body shows the diff (when present) else the normalized `rawOutput` text (scrollable, capped at 20k chars) else `"(no output)"`.

3. Call sites — pass the two new props:
   - `src/components/MessageBubble.tsx` (~line 201, the `case "tool-call"` `ToolCallCard`): add `rawInput={message.rawInput}` and `rawOutput={message.rawOutput}`.
   - `src/components/SubagentPanel.tsx` (~line 118, the `m.kind === "tool-call"` `ToolCallCard`): add `rawInput={m.rawInput}` and `rawOutput={m.rawOutput}`.

4. `src/components/ToolCallCard.test.tsx` (NEW file) — follow the style of the existing `src/components/*.test.tsx` tests: `fireEvent`/`render`/`screen` from `@testing-library/react` (the repo has NO `@testing-library/user-event` and NO `@testing-library/jest-dom` setup — click with `fireEvent.click`, assert with `expect(...).toBeTruthy()`), `describe`/`it`/`expect` from `vitest`. The `matchMedia` `beforeAll` stub is NOT needed for this component (it uses no hooks beyond `useState`). Tests to write (runnable as-is):

   ```tsx
   import { fireEvent, render, screen } from "@testing-library/react";
   import { describe, it, expect } from "vitest";
   import ToolCallCard, { summarizeToolCall, normalizeToolOutput } from "./ToolCallCard";

   describe("summarizeToolCall", () => {
     it("summarizes bash by its command", () => {
       expect(summarizeToolCall("bash", { command: "ls -la" })).toBe("ls -la");
     });
     it("summarizes read by path + line range when offset/limit present", () => {
       expect(
         summarizeToolCall("read", { path: "src/a.ts", offset: 5, limit: 50 }),
       ).toBe("src/a.ts (L5–54)");
     });
     it("summarizes read by path alone when no offset/limit", () => {
       expect(summarizeToolCall("read", { path: "src/a.ts" })).toBe("src/a.ts");
     });
     it("falls back to compact JSON for an unknown tool", () => {
       expect(summarizeToolCall("mystery_tool", { a: 1 })).toBe('{"a":1}');
     });
     it("returns undefined for empty or missing input", () => {
       expect(summarizeToolCall("mystery_tool", {})).toBeUndefined();
       expect(summarizeToolCall("mystery_tool", undefined)).toBeUndefined();
     });
   });

   describe("normalizeToolOutput", () => {
     it("joins text items of an AgentToolResult content array", () => {
       expect(
         normalizeToolOutput({
           content: [
             { type: "text", text: "line1" },
             { type: "text", text: "line2" },
           ],
         }),
       ).toBe("line1\nline2");
     });
     it("counts image items", () => {
       expect(
         normalizeToolOutput({
           content: [
             { type: "text", text: "t" },
             { type: "image" },
             { type: "image" },
           ],
         }),
       ).toBe("t\n(+2 images)");
     });
     it("shows a bare string as-is", () => {
       expect(normalizeToolOutput("world")).toBe("world");
     });
     it("falls back to details JSON when there is no text", () => {
       expect(normalizeToolOutput({ content: [], details: { todos: [] } })).toBe(
         '{"todos":[]}',
       );
     });
     it("returns undefined for nothing showable", () => {
       expect(normalizeToolOutput(undefined)).toBeUndefined();
       expect(normalizeToolOutput({})).toBeUndefined();
     });
   });

   describe("ToolCallCard (rendering)", () => {
     it("renders the summary next to the title", () => {
       render(
         <ToolCallCard
           title="bash"
           status="completed"
           rawInput={{ command: "ls -la" }}
         />,
       );
       expect(screen.getByText("bash")).toBeTruthy();
       expect(screen.getByText("ls -la")).toBeTruthy();
     });
     it("renders the output text when expanded", () => {
       render(
         <ToolCallCard
           title="bash"
           status="completed"
           rawInput={{ command: "ls" }}
           rawOutput={{ content: [{ type: "text", text: "fileA\nfileB" }] }}
         />,
       );
       fireEvent.click(screen.getByRole("button"));
       expect(screen.getByText(/fileA/)).toBeTruthy();
     });
     it("renders a bare-string rawOutput when expanded", () => {
       render(
         <ToolCallCard title="read" status="completed" rawOutput="file contents" />,
       );
       fireEvent.click(screen.getByRole("button"));
       expect(screen.getByText("file contents")).toBeTruthy();
     });
     it("caps output at 20k chars with a truncation note", () => {
       render(
         <ToolCallCard
           title="bash"
           status="completed"
           rawOutput={"x".repeat(20001)}
         />,
       );
       fireEvent.click(screen.getByRole("button"));
       expect(screen.getByText(/truncated — 20001 chars total/)).toBeTruthy();
     });
     it("renders (no output) when there is no rawOutput", () => {
       render(<ToolCallCard title="bash" status="completed" />);
       fireEvent.click(screen.getByRole("button"));
       expect(screen.getByText("(no output)")).toBeTruthy();
     });
   });
   ```

**Steps:**
- [ ] Write `src/components/ToolCallCard.test.tsx` with the tests above (runnable as-is — `fireEvent`/`toBeTruthy` style, no extra dependencies).
- [ ] Run `pnpm vitest run src/components/ToolCallCard.test.tsx`
  - Did it fail (helpers do not exist / props not rendered)? If it passed unexpectedly, stop and investigate why.
- [ ] Implement the `ToolCallCard` helpers + component changes + the two call-site prop additions.
- [ ] Run `pnpm vitest run src/components/ToolCallCard.test.tsx`
  - Did all tests pass? If not, fix the failures and re-run before continuing.
- [ ] Run `pnpm test`
  - Did the FULL frontend suite pass (including `sessions.test.ts` from Task 1)? If not, fix and re-run.
- [ ] Run `pnpm build`
  - Did it succeed (tsc + vite build, 0 errors)? If not, fix and re-run before continuing.
- [ ] Commit with message: "feat(ui): tool-call card shows input summary + output"

**Acceptance criteria:**
- [ ] The header line shows `title` + a muted truncated summary (bash → command; read → path + `L5–54` line range; unknown tool → compact JSON; empty input → title only, EXCEPT `ls` which defaults to `.` when `path` is absent).
- [ ] Expanding shows the normalized output (content-array text joined, images counted, bare string as-is, details/JSON fallback) in a scrollable `max-h-80` block, capped at 20k chars with a `… (truncated — N chars total)` note; `"(no output)"` only when there is genuinely nothing; the diff branch keeps priority.
- [ ] `pnpm test` and `pnpm build` are green.

---

## Verification (whole feature)

- `pnpm test` — full frontend suite green (repo root).
- `pnpm build` — tsc + vite build green (repo root).
- Rust is untouched: `cargo test` / `cargo clippy` in `src-tauri/` are NOT required for this feature (no Rust changes).
- Out-of-scope follow-up (do NOT do in this feature): the comment at `src-tauri/src/agent/session.rs:2009–2011` ("rawOutput is persisted-only … the frontend's `AcpSessionUpdate` has no `rawOutput` field") becomes stale after Task 1 — update it in a separate one-line docs commit.
- Manual check (optional, `pnpm dev` or `pnpm tauri dev`): start a session, ask the agent to run a bash command and read a file — the transcript tool cards show `bash <command>` / `read <path>` on the header and the output on expand; reload the session (resume) and the cards still show the summary + output.
