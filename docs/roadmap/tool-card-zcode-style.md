---
status: committed
done-when: Tool cards in the transcript and subagent streams render in the ZCode style — per-tool icon + past-tense verb (shimmer while running; a red `Failed` word with a dotted-underline tooltip, error text and copy button on failure), file chips with language icons, `+N -M` edit stats with a one-shot flip, and a `rounded-xl border bg-panel` expanded body with a `$` prompt for shell commands; consecutive `write`/`edit` runs fold into a `Changes` card with responsive `+N` chip overflow; a single file edit auto-opens once on completion; a subagent section auto-collapses on completion. `pnpm test` + `pnpm build` green.
---

# ZCode-Style Tool Cards Plan

**Goal:** Port ZCode's tool-card visual language (verbs, file chips, diff stats, shimmer, failure tooltip, panel body, `Changes` grouping, auto behaviors) onto the existing Archimedes tool cards.
**Architecture:** Frontend-only. The `tool-call` `Message` already carries `rawInput` / `rawOutput` (live partials included, from the `tool-call-output` feature). New pure display functions go in `src/lib/`, new small components (`FileChip`, `DiffCount`, `FlipMetricValue`, `ChangesGroupCard`) in `src/components/`, and the `ToolCallCard` header/body is redesigned. `ChatStream` and `SubagentPanel` apply a pure grouping function over the message array. The store and Rust are untouched.
**Tech Stack:** React 19 + TypeScript, lucide-react (all icon names below are verified present in the installed version), Radix `ui/tooltip.tsx`, Vitest + `@testing-library/react` (repo has NO `@testing-library/user-event` / `jest-dom` — click with `fireEvent.click`, assert with `expect(...).toBeTruthy()`). Validation from the repo root: `pnpm test` (vitest run), `pnpm build` (tsc + vite build). There is NO frontend lint/format script.

Existing code facts the plan relies on (verified):
- `src/lib/toolOutput.ts` holds `truncate` (private), `summarizeToolCall(title, rawInput)`, and `normalizeToolOutput(rawOutput, failed = false)` — all UNCHANGED by this feature.
- `src/components/ToolCallCard.tsx` renders a `<button>` header (status icon + title + summary) and a `bg-surface` expanded body; it defers `normalizeToolOutput` until `open` (a spy test in `ToolCallCard.test.tsx` asserts this — keep it passing).
- `src/components/ToolCallCard.test.tsx` holds the `summarizeToolCall` / `normalizeToolOutput` unit tests AND the card render tests (20 tests total: 5 + 7 + 8).
- `src/lib/paths.ts` exports `basenameOfPath(p: string): string` (returns `""` for empty/bare roots).
- `src/components/ui/tooltip.tsx` exports `TooltipProvider`, `Tooltip`, `TooltipTrigger` (supports `asChild`), `TooltipContent` (merges a `className`).
- `src/index.css` already defines the `animated-gradient-text` class (tokens `--animated-gradient-text-strong/soft` in all four theme blocks, `@keyframes gradient-flow`, `prefers-reduced-motion` guard) — NO new CSS is needed for the shimmer.
- `src/components/ChatStream.tsx` renders `messages.map((message, i) => …)` at ~line 806: a `tool-call` message with a matching `main` `ask` request renders `AskQuestionCard` IN PLACE (anchored by `toolCallId`); everything else renders `MessageBubble` with `isStreaming` = `message.kind === "agent-thought" && inTurn && i === messages.length - 1`.
- `src/components/SubagentPanel.tsx`: `SubagentSection` renders a header row + an always-visible `max-h-48` stream (`messageList.map`: `agent-text` → `MessageBubble`, `agent-thought` → `Reasoning`, `tool-call` → `ToolCallCard`, `diff` → `DiffBlock`) + a metrics line when `entry.status !== "running"`.
- The `tool-call` `Message` variant (`src/store/sessions.ts`) has `id: string`, `title: string`, `status: ToolCallUiStatus` (`"pending" | "completed" | "failed"`), `diff?`, `rawInput?`, `rawOutput?`, `at: number`.

---

### Task 1: Data layer — verbs, icons, file summaries, edit stats, failure text

**Context:**
The card redesign (Task 2), the `Changes` card (Task 3) and the auto behaviors (Task 4) all derive their display data from the `tool-call` `Message`'s `title` / `rawInput` / `rawOutput`. This task adds the pure functions that do that derivation, plus the file-language icon map, so the later tasks are pure composition. Nothing in this task changes rendering. No Rust changes. `summarizeToolCall` / `normalizeToolOutput` in `src/lib/toolOutput.ts` stay byte-identical.

**Files:**
- Modify: `src/lib/toolOutput.ts`
- Create: `src/lib/fileIcons.ts`
- Test: `src/lib/toolOutput.test.ts` (new file)
- Test: `src/lib/fileIcons.test.ts` (new file)

**What to implement:**

1. `src/lib/toolOutput.ts` — add these exports (append below the existing functions; import `basenameOfPath` from `./paths` and `type ToolCallUiStatus` from `../store/sessions` at the top of the file):

   ```ts
   import { basenameOfPath } from "./paths";
   import type { ToolCallUiStatus } from "../store/sessions";

   const VERBS: Record<string, { completed: string; running: string }> = {
     bash: { completed: "Ran", running: "Running" },
     powershell: { completed: "Ran", running: "Running" },
     sudo_exec: { completed: "Ran", running: "Running" },
     read: { completed: "Read", running: "Reading" },
     write: { completed: "Wrote", running: "Writing" },
     edit: { completed: "Edited", running: "Editing" },
     grep: { completed: "Searched", running: "Searching" },
     find: { completed: "Searched", running: "Searching" },
     web_search: { completed: "Searched", running: "Searching" },
     ls: { completed: "Listed", running: "Listing" },
     fetch_content: { completed: "Fetched", running: "Fetching" },
     ask: { completed: "Asked", running: "Asking" },
     manage_todo_list: { completed: "Todos", running: "Updating todos" },
     subagent: { completed: "Delegated", running: "Delegating" },
     mcp: { completed: "MCP", running: "MCP" },
   };

   /**
    * The header verb for a tool call. Past tense when finished (the verb
    * itself is the "done" signal — no check icon), present tense while
    * `pending` (the caller adds the `animated-gradient-text` shimmer).
    * A `failed` call takes the past tense too — the red `Failed` word is
    * the failure signal, so do not "fix" this. `undefined` for unknown
    * tools (the caller falls back to the raw title).
    */
   export function toolVerb(title: string, status: ToolCallUiStatus): string | undefined {
     const v = VERBS[title];
     if (!v) return undefined;
     return status === "pending" ? v.running : v.completed;
   }

   import type { LucideIcon } from "lucide-react";
   import {
     SquareTerminalIcon, FileTextIcon, FilePenIcon, PencilIcon, SearchIcon,
     ListIcon, GlobeIcon, DownloadIcon, MessageCircleQuestionIcon,
     ListTodoIcon, BotIcon, PlugIcon, WrenchIcon,
   } from "lucide-react";

   const TOOL_ICONS: Record<string, LucideIcon> = {
     bash: SquareTerminalIcon,
     powershell: SquareTerminalIcon,
     sudo_exec: SquareTerminalIcon,
     read: FileTextIcon,
     write: FilePenIcon,
     edit: PencilIcon,
     grep: SearchIcon,
     find: SearchIcon,
     web_search: GlobeIcon,
     ls: ListIcon,
     fetch_content: DownloadIcon,
     ask: MessageCircleQuestionIcon,
     manage_todo_list: ListTodoIcon,
     subagent: BotIcon,
     mcp: PlugIcon,
   };

   /** The header icon for a tool call (always static — no spinner). */
   export function toolIcon(title: string): LucideIcon {
     return TOOL_ICONS[title] ?? WrenchIcon;
   }

   export interface ToolFileSummary {
     path: string;
     fileName: string;
   }

   /**
    * The file a tool call touches (the header chip). `read` / `write` /
    * `edit` carry `path`; every other tool → `[]`. `fileName` is the
    * basename (the full path is the chip's `title` tooltip).
    */
   export function fileSummaries(title: string, rawInput: unknown): ToolFileSummary[] {
     if (title !== "read" && title !== "write" && title !== "edit") return [];
     if (typeof rawInput !== "object" || rawInput === null) return [];
     const path = (rawInput as Record<string, unknown>).path;
     if (typeof path !== "string" || path === "") return [];
     const fileName = basenameOfPath(path);
     return [{ path, fileName: fileName !== "" ? fileName : path }];
   }

   /**
    * Line-count change stat — `edit` ONLY (ZCode's `getChangeStat`
    * semantics): sum over `edits[]` of `lineCount(newText)` /
    * `lineCount(oldText)`. `undefined` for other tools, missing/empty
    * `edits`, or when both totals are 0.
    */
   export function editChangeStat(
     title: string,
     rawInput: unknown,
   ): { added: number; removed: number } | undefined {
     if (title !== "edit") return undefined;
     if (typeof rawInput !== "object" || rawInput === null) return undefined;
     const edits = (rawInput as Record<string, unknown>).edits;
     if (!Array.isArray(edits)) return undefined;
     const countLines = (s: string): number => (s === "" ? 0 : s.split("\n").length);
     let added = 0;
     let removed = 0;
     for (const e of edits as Array<Record<string, unknown>>) {
       if (typeof e.newText === "string") added += countLines(e.newText);
       if (typeof e.oldText === "string") removed += countLines(e.oldText);
     }
     if (added === 0 && removed === 0) return undefined;
     return { added, removed };
   }

   /**
    * The failure reason for a failed tool call (the failure tooltip):
    * `details.error` wins, then the first non-empty `content` text item,
    * then a bare-string result as-is. `undefined` when nothing usable.
    */
   export function failureText(rawOutput: unknown): string | undefined {
     if (typeof rawOutput === "string")
       return rawOutput.trim() === "" ? undefined : rawOutput;
     if (typeof rawOutput !== "object" || rawOutput === null) return undefined;
     const result = rawOutput as Record<string, unknown>;
     const details = result.details;
     if (typeof details === "object" && details !== null) {
       const err = (details as Record<string, unknown>).error;
       if (typeof err === "string" && err.trim() !== "") return err;
     }
     if (Array.isArray(result.content)) {
       for (const item of result.content as Array<Record<string, unknown>>) {
         if (
           item &&
           item.type === "text" &&
           typeof item.text === "string" &&
           item.text.trim() !== ""
         )
           return item.text;
       }
     }
     return undefined;
   }
   ```

   (Note: the `import` statements above are shown inline for clarity — in the file they go at the TOP with the other imports. `lucide-react` and `./paths` / `../store/sessions` are already used elsewhere in the codebase; add the new imports to the existing import block. All lucide names are verified present in the installed `lucide-react`.)

2. `src/lib/fileIcons.ts` (NEW file) — the file-language icon map:

   ```ts
   import type { LucideIcon } from "lucide-react";
   import {
     FileIcon, FileCodeIcon, FileJsonIcon, FileCogIcon, FileLockIcon,
     FileImageIcon, FileTextIcon, TerminalIcon,
   } from "lucide-react";
   import { basenameOfPath } from "./paths";

   export interface FileIconSpec {
     icon: LucideIcon;
     className: string;
   }

   const DEFAULT_SPEC: FileIconSpec = {
     icon: FileIcon,
     className: "text-foreground-subtlest",
   };

   const BY_EXTENSION: Record<string, FileIconSpec> = {
     js: { icon: FileCodeIcon, className: "text-warning" },
     jsx: { icon: FileCodeIcon, className: "text-warning" },
     ts: { icon: FileCodeIcon, className: "text-brand" },
     tsx: { icon: FileCodeIcon, className: "text-brand" },
     json: { icon: FileJsonIcon, className: "text-warning" },
     html: { icon: FileCodeIcon, className: "text-destructive" },
     htm: { icon: FileCodeIcon, className: "text-destructive" },
     css: { icon: FileCodeIcon, className: "text-foreground-subtle" },
     md: { icon: FileTextIcon, className: "text-foreground-subtle" },
     markdown: { icon: FileTextIcon, className: "text-foreground-subtle" },
     rs: { icon: FileCodeIcon, className: "text-destructive" },
     py: { icon: FileCodeIcon, className: "text-success" },
     toml: { icon: FileCogIcon, className: "text-foreground-subtlest" },
     ini: { icon: FileCogIcon, className: "text-foreground-subtlest" },
     yaml: { icon: FileCogIcon, className: "text-foreground-subtlest" },
     yml: { icon: FileCogIcon, className: "text-foreground-subtlest" },
     lock: { icon: FileLockIcon, className: "text-foreground-subtlest" },
     png: { icon: FileImageIcon, className: "text-foreground-subtle" },
     jpg: { icon: FileImageIcon, className: "text-foreground-subtle" },
     jpeg: { icon: FileImageIcon, className: "text-foreground-subtle" },
     gif: { icon: FileImageIcon, className: "text-foreground-subtle" },
     svg: { icon: FileImageIcon, className: "text-foreground-subtle" },
     webp: { icon: FileImageIcon, className: "text-foreground-subtle" },
     sh: { icon: TerminalIcon, className: "text-foreground-subtle" },
     bash: { icon: TerminalIcon, className: "text-foreground-subtle" },
   };

   /**
    * The icon spec for a file path: the LAST extension (lowercased) →
    * `{ icon, className }`; unmapped / no extension / dotfiles → the
    * neutral default. Pure — unit-testable.
    */
   export function fileIconFor(path: string): FileIconSpec {
     const name = basenameOfPath(path);
     const dot = name.lastIndexOf(".");
     const ext = dot >= 0 ? name.slice(dot + 1).toLowerCase() : "";
     return BY_EXTENSION[ext] ?? DEFAULT_SPEC;
   }
   ```

3. `src/lib/toolOutput.test.ts` (NEW file) — import the five new functions from `./toolOutput` and test:
   - `toolVerb`: `bash` completed → `"Ran"`, `bash` pending → `"Running"`, `read` completed → `"Read"`, `write` pending → `"Writing"`, `edit` completed → `"Edited"`, `grep` pending → `"Searching"`, `ls` completed → `"Listed"`, `web_search` completed → `"Searched"`, `fetch_content` pending → `"Fetching"`, `ask` completed → `"Asked"`, `manage_todo_list` completed → `"Todos"`, `subagent` pending → `"Delegating"`, `mcp` any status → `"MCP"`, `mystery_tool` → `undefined`.
   - `toolIcon`: `bash` → `SquareTerminalIcon` (identity, `toBe`), `edit` → `PencilIcon`, `mystery_tool` → `WrenchIcon`.
   - `fileSummaries`: `read` with `{ path: "/a/b/c.ts" }` → `[{ path: "/a/b/c.ts", fileName: "c.ts" }]`; `write` with `{ path: "x.md" }` → `[{ path: "x.md", fileName: "x.md" }]`; `edit` with `{ path: "/a/b/c.ts", edits: [] }` → one entry; `bash` with `{ command: "ls" }` → `[]`; `read` with `{}` → `[]`.
   - `editChangeStat`: `edit` with one edit `{ oldText: "a\nb", newText: "x\ny\nz" }` → `{ added: 3, removed: 2 }`; `edit` with two edits sums them; `edit` with `edits: []` → `undefined`; `edit` with all-empty texts → `undefined`; `write` with edits-shaped input → `undefined`; `edit` with `{}` → `undefined`.
   - `failureText`: `{ details: { error: "boom" }, content: [{ type: "text", text: "t" }] }` → `"boom"` (details wins); `{ content: [{ type: "text", text: "" }, { type: "text", text: "reason" }] }` → `"reason"` (first NON-EMPTY); `"bare error"` → `"bare error"`; `""` → `undefined`; `{}` → `undefined`; `undefined` → `undefined`.

4. `src/lib/fileIcons.test.ts` (NEW file) — import `fileIconFor` and test: `a.ts` → `FileCodeIcon` + `"text-brand"`; `a.js` → `FileCodeIcon` + `"text-warning"`; `a.json` → `FileJsonIcon`; `a.html` → `FileCodeIcon` + `"text-destructive"`; `a.png` → `FileImageIcon`; `a.unknownext` → default (`FileIcon` + `"text-foreground-subtlest"`); `a.txt` (unmapped) → default; `a.TS` (uppercase) → the ts spec (case-insensitive); `.gitignore` (dotfile) → default; `a.tar.gz` (last extension) → default.

**Steps:**
- [ ] Write `src/lib/toolOutput.test.ts` and `src/lib/fileIcons.test.ts` with the tests above.
- [ ] Run `pnpm vitest run src/lib/toolOutput.test.ts src/lib/fileIcons.test.ts`
  - Did it fail (the new functions do not exist yet — import errors / `undefined` calls)? If it passed unexpectedly, stop and investigate why.
- [ ] Implement the `src/lib/toolOutput.ts` additions and `src/lib/fileIcons.ts`.
- [ ] Run `pnpm vitest run src/lib/toolOutput.test.ts src/lib/fileIcons.test.ts`
  - Did all tests pass? If not, fix the failures and re-run before continuing.
- [ ] Run `pnpm vitest run src/components/ToolCallCard.test.tsx`
  - Did the EXISTING tests still pass (you must not have disturbed `summarizeToolCall` / `normalizeToolOutput`)? If not, fix and re-run.
- [ ] Run `pnpm build`
  - Did it succeed (tsc + vite build, 0 errors)? If not, fix and re-run before continuing.
- [ ] Commit with message: "feat(lib): tool-card verbs, icons, file summaries, edit stats"

**Acceptance criteria:**
- [ ] The five new functions exist in `src/lib/toolOutput.ts` with the exact signatures above; `summarizeToolCall` / `normalizeToolOutput` are byte-identical.
- [ ] `src/lib/fileIcons.ts` exports `fileIconFor` + `FileIconSpec`.
- [ ] All new tests pass; all pre-existing tests pass; `pnpm build` green.

---

### Task 2: `ToolCallCard` redesign — header, panel body, failure tooltip, `FlipMetricValue`

**Context:**
With the data layer in place, this task redesigns the card itself: the header becomes `icon [verb] [primary] [stats] [status word] [chevron]` (per-tool static icon, past-tense verb with a shimmer while running, file chip / command / summary as the primary text, `+N -M` stats for `edit`, a red `Failed` word with a tooltip + copy button on failure, a hover chevron), and the expanded body becomes a `rounded-xl border bg-panel` panel with a `$` prompt + mono command for shell tools. The pending spinner and the completed check icon are REMOVED (ZCode's deliberate choice: the verb is the signal). The existing deferred-normalization behavior (normalize only while open) is preserved. No Rust changes.

**Files:**
- Create: `src/components/FlipMetricValue.tsx`
- Create: `src/components/FileChip.tsx`
- Create: `src/components/DiffCount.tsx`
- Modify: `src/components/ToolCallCard.tsx`
- Modify: `src/index.css`
- Test: `src/components/ToolCallCard.test.tsx` (extend + update two assertions)
- Test: `src/components/FlipMetricValue.test.tsx` (new file)

**What to implement:**

1. `src/components/FlipMetricValue.tsx` (NEW):

   ```tsx
   /**
    * A number that plays a one-shot CSS flip when its value changes (the
    * `key` remount re-triggers the animation). `motion-safe`-equivalent
    * guard lives in the CSS (see `index.css`).
    */
   export default function FlipMetricValue({ value }: { value: number }) {
     return (
       <span key={value} className="flip-in inline-block">
         {value}
       </span>
     );
   }
   ```

2. `src/index.css` — add INSIDE the existing `@layer utilities { … }` block (the same block that already holds `@keyframes gradient-flow` / `.animated-gradient-text` — around lines 697–738 of the current file; NOT inside `@theme`, NOT at the file's top level):

   ```css
   @keyframes flip-in {
     from {
       transform: translateY(0.4em);
       opacity: 0;
     }
     to {
       transform: translateY(0);
       opacity: 1;
     }
   }

   .flip-in {
     animation: flip-in 200ms ease-out;
   }

   @media (prefers-reduced-motion: reduce) {
     .flip-in {
       animation: none;
     }
   }
   ```

3. `src/components/FileChip.tsx` (NEW):

   ```tsx
   import { basenameOfPath } from "../lib/paths";
   import { fileIconFor } from "../lib/fileIcons";

   /**
    * A file chip: a 16px language icon + the basename (`title` = the full
    * path). Non-clickable in v1 (the desktop has no code-viewer action
    * wired to the side pane yet).
    */
   export default function FileChip({ path }: { path: string }) {
     const spec = fileIconFor(path);
     const name = basenameOfPath(path) || path;
     const Icon = spec.icon;
     return (
       <span
         className="inline-flex min-w-0 max-w-full items-center gap-1.5 text-foreground-subtle"
         title={path}
       >
         <Icon className={`size-4 shrink-0 ${spec.className}`} />
         <span className="min-w-0 truncate">{name}</span>
       </span>
     );
   }
   ```

4. `src/components/DiffCount.tsx` (NEW):

   ```tsx
   import FlipMetricValue from "./FlipMetricValue";

   /**
    * A `+N -M` change stat: green additions, red removals, `font-mono
    * tabular-nums`, each number flip-animating on change.
    */
   export default function DiffCount({
     stat,
   }: {
     stat: { added: number; removed: number };
   }) {
     return (
       <span className="inline-flex items-center gap-1 font-mono text-ui-sm leading-none tabular-nums">
         {stat.added > 0 && (
           <span className="text-success">
            +<FlipMetricValue value={stat.added} />
           </span>
         )}
         {stat.removed > 0 && (
           <span className="text-destructive">
            -<FlipMetricValue value={stat.removed} />
           </span>
         )}
       </span>
     );
   }
   ```

5. `src/components/ToolCallCard.tsx` — REPLACE the component (keep the file's top doc comment, rewritten to describe the new design; keep the `DiffBlock` import and the deferred-normalization line):

   ```tsx
   import { useEffect, useRef, useState } from "react";
   import { CheckIcon, ChevronRightIcon, CopyIcon } from "lucide-react";
   import type { DiffRef, ToolCallUiStatus } from "../store/sessions";
   import {
     editChangeStat,
     failureText,
     fileSummaries,
     normalizeToolOutput,
     summarizeToolCall,
     toolIcon,
     toolVerb,
   } from "../lib/toolOutput";
   import FileChip from "./FileChip";
   import DiffCount from "./DiffCount";
   import {
     Tooltip,
     TooltipContent,
     TooltipProvider,
     TooltipTrigger,
   } from "./ui/tooltip";
   import DiffBlock from "./DiffBlock";

   const SHELL_TOOLS = new Set(["bash", "powershell", "sudo_exec"]);

   /**
    * One `h-8` tool row in the ZCode style: a static per-tool icon + a
    * past-tense verb (shimmer — `animated-gradient-text` — while running;
    * the past tense is the "done" signal, no check icon) + the primary
    * text (a file chip for read/write/edit, the command in `font-sans`
    * for shell tools, the `summarizeToolCall` summary otherwise) + a
    * `+N -M` change stat (edit only) + a red `Failed` word with a
    * dotted-underline tooltip (error text + copy) on failure + a hover
    * chevron. Expandable body: the diff via `DiffBlock` when present,
    * else a `rounded-xl border bg-panel` panel — a `$` prompt + mono
    * command for shell tools, the file chip + stat for file tools —
    * then the normalized `rawOutput` (scrollable, capped at 20k chars)
    * or a muted "No output."
    */
   export default function ToolCallCard({
     title,
     status,
     diff,
     rawInput,
     rawOutput,
   }: {
     title: string;
     status: ToolCallUiStatus;
     diff?: DiffRef;
     rawInput?: unknown;
     rawOutput?: unknown;
   }) {
     const [open, setOpen] = useState(false);
     const verb = toolVerb(title, status);
     const Icon = toolIcon(title);
     const files = fileSummaries(title, rawInput);
     const stat = editChangeStat(title, rawInput);
     const summary = summarizeToolCall(title, rawInput);
     const isShell = SHELL_TOOLS.has(title);
     const command =
       isShell && typeof rawInput === "object" && rawInput !== null
         ? ((rawInput as Record<string, unknown>).command as string | undefined)
         : undefined;
     // Defer output normalization until the card is expanded: the body is
     // the only consumer, and joining/serializing large results on every
     // streaming render while collapsed is wasted work.
     const output =
       open && !diff
         ? normalizeToolOutput(rawOutput, status === "failed")
         : undefined;
     const failure = status === "failed" ? failureText(rawOutput) : undefined;

     // The failure tooltip's copy button (ZCode's pattern: copy → check
     // for 1.5s; a no-op when there is no failure text).
     const [copied, setCopied] = useState(false);
     const resetRef = useRef<number | null>(null);
     const handleCopy = () => {
       if (!failure) return;
       void navigator.clipboard?.writeText(failure)?.then(() => {
         setCopied(true);
         if (resetRef.current !== null) window.clearTimeout(resetRef.current);
         resetRef.current = window.setTimeout(() => setCopied(false), 1500);
       });
     };
     useEffect(
       () => () => {
         if (resetRef.current !== null) window.clearTimeout(resetRef.current);
       },
       [],
     );

     return (
       <div className="w-full">
         <button
           type="button"
           onClick={() => setOpen((o) => !o)}
           className="group/tool-summary flex h-8 w-full items-center gap-2 rounded-lg px-2 text-left hover:bg-surface-hover"
         >
           <Icon className="size-4 shrink-0 text-foreground-subtle" />
           <span
             className={`shrink-0 whitespace-nowrap font-medium ${
               status === "pending"
                 ? "animated-gradient-text"
                 : "text-foreground-subtlest"
             }`}
           >
             {verb ?? title}
           </span>
           {files.length > 0 ? (
             <FileChip path={files[0].path} />
           ) : isShell && typeof command === "string" && command !== "" ? (
             <span className="min-w-0 truncate font-sans text-foreground-subtle">
               {command}
             </span>
           ) : summary ? (
             <span className="min-w-0 truncate text-ui-sm text-foreground-subtlest">
               {summary}
             </span>
           ) : null}
           {stat && <DiffCount stat={stat} />}
           {status === "failed" && (
             <TooltipProvider>
               <Tooltip>
                 <TooltipTrigger asChild>
                   <span className="shrink-0 cursor-help whitespace-nowrap text-destructive underline decoration-dotted underline-offset-2">
                     Failed
                   </span>
                 </TooltipTrigger>
                 {failure && (
                   <TooltipContent side="top" align="start" className="max-w-96">
                     <div className="flex max-w-96 items-center gap-2">
                       <span className="line-clamp-3 min-w-0 flex-1 whitespace-pre-wrap break-words">
                         {failure}
                       </span>
                       <button
                         type="button"
                         onClick={(event) => {
                           event.preventDefault();
                           event.stopPropagation();
                           handleCopy();
                         }}
                         aria-label={copied ? "Error copied" : "Copy error"}
                         title={copied ? "Error copied" : "Copy error"}
                         className="shrink-0 text-foreground-subtle hover:text-foreground"
                       >
                         {copied ? (
                           <CheckIcon className="size-3" />
                         ) : (
                           <CopyIcon className="size-3" />
                         )}
                       </button>
                     </div>
                   </TooltipContent>
                 )}
               </Tooltip>
             </TooltipProvider>
           )}
           <ChevronRightIcon
             aria-hidden
             className={`size-4 shrink-0 text-foreground-subtlest opacity-0 transition-transform transition-opacity duration-200 ease-out group-hover/tool-summary:opacity-100 ${
               open ? "rotate-90 opacity-100" : "rotate-0"
             }`}
           />
         </button>
         {open && (
           <div className="mt-1 rounded-xl border border-border bg-panel px-4 py-3">
             {diff ? (
               <DiffBlock path={diff.path} patch={diff.patch} />
             ) : (
               <>
                 {(isShell && typeof command === "string" || files.length > 0) && (
                   <div className="mb-2 space-y-1">
                     {isShell && typeof command === "string" && command !== "" && (
                       <div className="flex items-start gap-2 font-sans text-ui-base text-foreground">
                         <span className="shrink-0 text-foreground-subtle">$</span>
                         <pre className="block min-w-0 max-h-15 flex-1 overflow-auto whitespace-pre-wrap break-words font-mono">
                           {command}
                         </pre>
                       </div>
                     )}
                     {files.length > 0 && (
                       <div className="flex items-center gap-2">
                         <FileChip path={files[0].path} />
                         {stat && <DiffCount stat={stat} />}
                       </div>
                     )}
                   </div>
                 )}
                 {output !== undefined ? (
                   <div>
                     <pre className="font-mono max-h-80 overflow-auto whitespace-pre-wrap text-ui-sm">
                       {output.length > 20000
                         ? output.slice(0, 20000) + "…"
                         : output}
                     </pre>
                     {output.length > 20000 && (
                       <p className="text-ui-sm text-foreground-subtlest">
                         … (truncated — {output.length} chars total)
                       </p>
                     )}
                   </div>
                 ) : (
                   <p className="font-mono text-ui-base text-foreground-subtle">
                     No output.
                   </p>
                 )}
               </>
             )}
           </div>
         )}
       </div>
     );
   }
   ```

   Notes for the executing agent:
   - The `LoaderIcon` import goes away (spinner removed); `CheckIcon` is KEPT (the copy button's copied state). The status icon logic (pending spinner / completed check / failed ✕) is fully removed.
   - `text-foreground-subtle` / `text-foreground-subtlest` / `bg-panel` / `border-border` / `text-success` / `text-destructive` / `text-brand` / `text-warning` / `text-ui-sm` / `font-mono` are all existing `@theme` utilities (verified in `src/index.css`).
   - The `animated-gradient-text` class already exists in `src/index.css` (used by `Reasoning.tsx`) — do NOT add it.
   - `max-h-15` is a valid Tailwind v4 arbitrary-free utility (15 × 0.25rem); if tsc/vite complains, use `max-h-[3.75rem]`.

6. `src/components/FlipMetricValue.test.tsx` (NEW) — note the `firstElementChild as HTMLElement` cast: `container.firstChild` is typed `ChildNode | null` (no `className` property — a direct access is a `tsc` error, and `pnpm build` runs `tsc` over `src` including test files):

   ```tsx
   import { render } from "@testing-library/react";
   import { describe, it, expect } from "vitest";
   import FlipMetricValue from "./FlipMetricValue";

   describe("FlipMetricValue", () => {
     it("renders the number with the flip class", () => {
       const { container } = render(<FlipMetricValue value={5} />);
       const el = container.firstElementChild as HTMLElement | null;
       expect(el).not.toBeNull();
       expect(el.textContent).toBe("5");
       expect(el.className).toContain("flip-in");
     });
     it("re-mounts (re-triggers the animation) when the value changes", () => {
       const { container, rerender } = render(<FlipMetricValue value={5} />);
       const before = container.firstElementChild;
       rerender(<FlipMetricValue value={7} />);
       const after = container.firstElementChild;
       expect(after).not.toBeNull();
       expect(after.textContent).toBe("7");
       expect(after).not.toBe(before);
     });
   });
   ```

   (Do NOT assert `querySelector` results with `toBeDefined()` — `querySelector` returns `null`, which IS defined; always use `not.toBeNull()` for presence checks.)

7. `src/components/ToolCallCard.test.tsx` — UPDATE + EXTEND:
   - UPDATE the two "(no output)" render tests: `it("renders (no output) when there is no rawOutput")` and `it("renders (no output) for a result with only an empty text item")` — change the asserted text from `"(no output)"` to `"No output."` (the new body text).
   - RENAME + REWRITE `it("renders the summary next to the title")` as `it("renders the verb + command for a known tool")`: render `<ToolCallCard title="bash" status="completed" rawInput={{ command: "ls -la" }} />` → `expect(screen.getByText("Ran")).toBeTruthy(); expect(screen.getByText("ls -la")).toBeTruthy();` (the raw `title` no longer renders for known tools — the verb replaces it).
   - ADD:
     - `it("shows the running verb with the shimmer while pending")`: render `<ToolCallCard title="bash" status="pending" rawInput={{ command: "ls" }} />` → `expect(screen.getByText("Running")).toBeTruthy()` AND `expect(container.querySelector(".animated-gradient-text")).not.toBeNull()` (the verb span carries the class; `not.toBeNull` — NOT `toBeDefined`).
     - `it("shows the raw title for an unknown tool")`: render `<ToolCallCard title="mystery_tool" status="completed" rawInput={{ a: 1 }} />` → `expect(screen.getByText("mystery_tool")).toBeTruthy()` (the `verb ?? title` fallback).
     - `it("renders a file chip for read (basename + language icon)")`: render `<ToolCallCard title="read" status="completed" rawInput={{ path: "/a/b/c.ts" }} />` → `expect(screen.getByText("c.ts")).toBeTruthy()` AND the chip's `title` attribute is the full path (`screen.getByText("c.ts").closest("span[title]")?.getAttribute("title")` === `"/a/b/c.ts"`).
     - `it("renders edit change stats")`: render `<ToolCallCard title="edit" status="completed" rawInput={{ path: "/a/b/c.ts", edits: [{ oldText: "a\nb", newText: "x\ny\nz" }] }} />` → `expect(container.textContent).toContain("+3"); expect(container.textContent).toContain("-2");` — use `textContent`, NOT `getByText("+3")`: `DiffCount` renders `+` and the number in SEPARATE nested elements (the `FlipMetricValue` span), so no single element has the text `"+3"` and `getByText` can never match it.
     - `it("renders the $ prompt + command in the expanded shell body")`: render `<ToolCallCard title="bash" status="completed" rawInput={{ command: "node --check app.js" }} rawOutput={{ content: [{ type: "text", text: "ok" }] }} />`, `fireEvent.click(screen.getByRole("button"))` → `expect(screen.getByText("$")).toBeTruthy()`, `expect(screen.getByText("ok")).toBeTruthy()`, and the expanded body's `<pre>` holds the command: `const pres = screen.getAllByText("node --check app.js"); expect(pres.length).toBeGreaterThanOrEqual(2); expect(pres[1].tagName).toBe("PRE");` — the command text appears TWICE (header span + body `<pre>`), so `getByText` would throw "multiple elements"; use `getAllByText` and assert the second match is the `<pre>`.
     - `it("renders the Failed word on failure (stable header assertions)")`: render `<ToolCallCard title="bash" status="failed" rawInput={{ command: "ls" }} rawOutput={{ content: [{ type: "text", text: "" }], details: { error: "command not found" } }} />` → `expect(screen.getByText("Failed")).toBeTruthy()` and the word's className contains `cursor-help` and `text-destructive`. (The word is a plain header span — always rendered, no tooltip interaction needed.)
     - `it("opens the failure tooltip with the error text + copy button on pointer interaction")`: same fixture as above, then `fireEvent.pointerMove(screen.getByText("Failed"))` (Radix `TooltipTrigger` listens to pointer events — `fireEvent.mouseEnter` does NOT open it) — this repo's `TooltipProvider` sets `delayDuration = 0` (src/components/ui/tooltip.tsx), so NO fake timers are needed; the `TooltipContent` portal mounts synchronously. Then: `expect(document.body.textContent).toContain("command not found")` (the content renders into a portal outside the card container — assert on `document.body`) and `expect(screen.getByRole("button", { name: "Copy error" })).toBeTruthy()`. If `pointerMove` empirically does not open it in this Radix version, try `fireEvent.focus(screen.getByText("Failed"))` — use whichever works and keep the test green; the assertions above the interaction (the `Failed` word) stay in the previous test either way.
     - KEEP the existing tests: "renders the output text when expanded", "renders a bare-string rawOutput when expanded", "caps output at 20k chars…", "shows the failure reason from details for a failed call with empty text", "defers output normalization until the card is expanded" — they must keep passing (the deferral line is unchanged; the failure-reason test still works because the expanded body still renders the `normalizeToolOutput` output).

**Steps:**
- [ ] Write `src/components/FlipMetricValue.test.tsx` and make the `ToolCallCard.test.tsx` updates + additions above.
- [ ] Run `pnpm vitest run src/components/ToolCallCard.test.tsx src/components/FlipMetricValue.test.tsx`
  - Did it fail (the redesigned card is not implemented yet — wrong/missing `Ran`/`Failed`/chip text, or the new file imports a component that doesn't exist)? If it passed unexpectedly, stop and investigate why.
- [ ] Create `src/components/FlipMetricValue.tsx`, `src/components/FileChip.tsx`, `src/components/DiffCount.tsx`, add the `flip-in` CSS to `src/index.css`, and replace the `ToolCallCard` component.
- [ ] Run `pnpm vitest run src/components/ToolCallCard.test.tsx src/components/FlipMetricValue.test.tsx`
  - Did all tests pass? If not, fix the failures and re-run before continuing.
- [ ] Run `pnpm test`
  - Did the FULL frontend suite pass (including `MessageBubble.test.tsx` / `SubagentPanel.test.tsx` / `ChatStream.test.tsx`, which render `ToolCallCard` indirectly)? If not, fix and re-run.
- [ ] Run `pnpm build`
  - Did it succeed (tsc + vite build, 0 errors)? If not, fix and re-run before continuing.
- [ ] Commit with message: "feat(ui): zcode-style tool card — verbs, chips, stats, failure tooltip"

**Acceptance criteria:**
- [ ] The header shows `icon + verb + primary + stats + Failed word + chevron` per the design; no spinner, no check icon for completed.
- [ ] Pending → `animated-gradient-text` shimmer; failed → red `Failed` word with dotted underline + tooltip (error text + working copy button).
- [ ] The expanded body is the `bg-panel` box with the `$` prompt for shell tools; the 20k cap + truncation note + deferred normalization are preserved.
- [ ] `pnpm test` and `pnpm build` are green.

---

### Task 3: `Changes` grouping — `toolGroups`, `ChangesGroupCard`, both call sites

**Context:**
ZCode folds consecutive file writes into a single `Changes` card (header: `N files` + responsive file-chip list with `+N` overflow + total stats; expanded: the per-file cards indented under a left border). This task adds that grouping as a PURE function over the message array (the store is untouched — grouping is a render concern, so live streaming and reloads are both consistent) and applies it at both `ToolCallCard` render sites: `ChatStream` and the `SubagentPanel` stream. No Rust changes.

**Files:**
- Create: `src/lib/toolGroups.ts`
- Create: `src/components/ChangesGroupCard.tsx`
- Modify: `src/components/ChatStream.tsx`
- Modify: `src/components/SubagentPanel.tsx`
- Test: `src/lib/toolGroups.test.ts` (new file)
- Test: `src/components/ChangesGroupCard.test.tsx` (new file)

**What to implement:**

1. `src/lib/toolGroups.ts` (NEW) — note the `Extract`-narrowed message type: the `Message` union only has `title` / `status` / `rawInput` / `rawOutput` / `id` on the `tool-call` variant, so the grouped members are typed as `ToolCallMessage` (without this, `ChangesGroupCard`'s member accesses fail `tsc` — `pnpm build` red):

   ```ts
   import type { Message } from "../store/sessions";

   export type ToolCallMessage = Extract<Message, { kind: "tool-call" }>;

   export type RenderUnit =
     | { kind: "single"; message: Message }
     | { kind: "changes-group"; messages: ToolCallMessage[] };

   const GROUP_TOOLS = new Set(["write", "edit"]);

   /**
    * Fold a maximal run of ≥2 CONSECUTIVE `write`/`edit` tool-call
    * messages into one `changes-group` unit; everything else (a single
    * write/edit, or any run interrupted by another message kind —
    * `agent-text`, `agent-thought`, `user`, `diff`) passes through as
    * `single` units. Pure — re-derived per render, so live streaming
    * (a write joins the run mid-turn) and reloads are consistent.
    */
   export function groupConsecutiveFileWrites(messages: Message[]): RenderUnit[] {
     const units: RenderUnit[] = [];
     let run: ToolCallMessage[] = [];
     const flush = () => {
       if (run.length >= 2) units.push({ kind: "changes-group", messages: run });
       else for (const m of run) units.push({ kind: "single", message: m });
       run = [];
     };
     for (const m of messages) {
       if (m.kind === "tool-call" && GROUP_TOOLS.has(m.title)) {
         run.push(m);
       } else {
         flush();
         units.push({ kind: "single", message: m });
       }
     }
     flush();
     return units;
   }
   ```

2. `src/components/ChangesGroupCard.tsx` (NEW):

   ```tsx
   import { useLayoutEffect, useRef, useState } from "react";
   import { ChevronRightIcon, PencilIcon } from "lucide-react";
   import type { Message } from "../store/sessions";
   import { editChangeStat, fileSummaries, type ToolFileSummary } from "../lib/toolOutput";
   import FileChip from "./FileChip";
   import DiffCount from "./DiffCount";
   import ToolCallCard from "./ToolCallCard";

   const CHIP_GAP_PX = 8;
   const TRAILING_PX = 24;

   function dedupeFiles(files: ToolFileSummary[]): ToolFileSummary[] {
     const seen = new Set<string>();
     const out: ToolFileSummary[] = [];
     for (const f of files) {
       if (!seen.has(f.path)) {
         seen.add(f.path);
         out.push(f);
       }
     }
     return out;
   }

   function sumStats(
     stats: Array<{ added: number; removed: number } | undefined>,
   ): { added: number; removed: number } | undefined {
     let added = 0;
     let removed = 0;
     for (const s of stats) {
       if (s) {
         added += s.added;
         removed += s.removed;
       }
     }
     if (added === 0 && removed === 0) return undefined;
     return { added, removed };
   }

   function filesCountLabel(n: number): string {
     return `${n} file${n === 1 ? "" : "s"}`;
   }

   /**
    * A `Changes` card: consecutive `write`/`edit` tool calls folded into
    * one header (`Changes` + `N files` + a responsive file-chip list with
    * `+N` overflow + total `+N -M` stats; while any member is pending,
    * the latest member's chip shows instead of the full list) and,
    * expanded, the per-file cards indented under a left border.
    */
   export default function ChangesGroupCard({
     messages,
   }: {
     messages: ToolCallMessage[];
   }) {
     const [open, setOpen] = useState(false);
     const files = dedupeFiles(
       messages.flatMap((m) => fileSummaries(m.title, m.rawInput)),
     );
     const totalStat = sumStats(
       messages.map((m) => editChangeStat(m.title, m.rawInput)),
     );
     const anyPending = messages.some((m) => m.status === "pending");
     const latest = anyPending
       ? messages.reduce((a, b) => (b.at >= a.at ? b : a))
       : undefined;
     const latestFile = latest
       ? fileSummaries(latest.title, latest.rawInput)[0]
       : undefined;

     // Responsive chip list (port of ZCode's `resolveResponsiveFileChipCount`):
     // measure the chips against the row's right boundary; overflow → `+N`.
     const listRef = useRef<HTMLSpanElement>(null);
     const chipRefs = useRef<Array<HTMLSpanElement | null>>([]);
     const overflowRef = useRef<HTMLSpanElement>(null);
     const [visibleCount, setVisibleCount] = useState(files.length);
     useLayoutEffect(() => {
       const update = () => {
         const list = listRef.current;
         if (!list) return;
         const boundary = list.closest("[data-changes-group-row]") ?? list;
         const rect = list.getBoundingClientRect();
         const boundaryRight = boundary.getBoundingClientRect().right;
         const available = Math.max(0, boundaryRight - rect.left - TRAILING_PX);
         const chipWidths = files.map(
           (_, i) => chipRefs.current[i]?.offsetWidth ?? 0,
         );
         const overflowWidth = overflowRef.current?.offsetWidth ?? 0;
         if (available <= 0) {
           setVisibleCount(0);
           return;
         }
         const allWidth =
           chipWidths.reduce((a, b) => a + b, 0) +
           CHIP_GAP_PX * Math.max(0, chipWidths.length - 1);
         if (allWidth <= available) {
           setVisibleCount(files.length);
           return;
         }
         let visible = 0;
         let width = 0;
         for (let i = 0; i < files.length; i++) {
           const next = i + 1;
           const required = width + chipWidths[i] + CHIP_GAP_PX * next + overflowWidth;
           if (required > available) break;
           width += chipWidths[i];
           visible = next;
         }
         setVisibleCount((c) => (c === visible ? c : visible));
       };
       update();
       const boundary = listRef.current?.closest("[data-changes-group-row]") ?? listRef.current;
       if (!boundary || typeof ResizeObserver === "undefined") return;
       const observer = new ResizeObserver(update);
       observer.observe(boundary);
       window.addEventListener("resize", update);
       return () => {
         observer.disconnect();
         window.removeEventListener("resize", update);
       };
     }, [files.length, anyPending]);

     const hiddenCount = files.length - visibleCount;
     // While any member is pending, the chip list is NOT rendered — the
     // live latest-member chip (below) replaces it (ZCode's behavior).

     return (
       <div className="w-full" data-changes-group-row="">
         <button
           type="button"
           onClick={() => setOpen((o) => !o)}
           className="group/tool-summary flex h-8 w-full items-center gap-2 rounded-lg px-2 text-left hover:bg-surface-hover"
         >
           <PencilIcon className="size-4 shrink-0 text-foreground-subtle" />
           <span className="shrink-0 whitespace-nowrap font-medium text-foreground-subtlest">
             Changes
           </span>
           {files.length > 1 && (
             <span className="shrink-0 text-foreground-subtlest">
               {filesCountLabel(files.length)}
             </span>
           )}
           {files.length > 1 && (
             <span className="shrink-0 text-foreground-subtlest">·</span>
           )}
           {files.length > 1 && !anyPending && (
             <span
               ref={listRef}
               className="relative inline-flex min-w-0 max-w-full flex-1 items-center gap-2 overflow-hidden"
             >
               {files.map((file, index) => {
                 const isVisible = index < visibleCount;
                 return (
                   <span
                     key={file.path}
                     ref={(el) => {
                       chipRefs.current[index] = el;
                     }}
                     aria-hidden={isVisible ? undefined : true}
                     className={
                       isVisible
                         ? "inline-flex min-w-0 shrink-0"
                         : "pointer-events-none invisible absolute inline-flex shrink-0"
                     }
                   >
                     <FileChip path={file.path} />
                   </span>
                 );
               })}
               {hiddenCount > 0 && (
                 <span className="shrink-0 text-foreground-subtlest">
                   +{hiddenCount}
                 </span>
               )}
               <span
                 ref={overflowRef}
                 aria-hidden="true"
                 className="pointer-events-none invisible absolute shrink-0"
               >
                 +{files.length}
               </span>
             </span>
           )}
           {files.length === 1 && !anyPending && <FileChip path={files[0].path} />}
           {totalStat && <DiffCount stat={totalStat} />}
           {anyPending && latestFile && (
             <span className="inline-flex min-w-0 items-center gap-2">
               <FileChip path={latestFile.path} />
             </span>
           )}
           <ChevronRightIcon
             aria-hidden
             className={`size-4 shrink-0 text-foreground-subtlest opacity-0 transition-transform transition-opacity duration-200 ease-out group-hover/tool-summary:opacity-100 ${
               open ? "rotate-90 opacity-100" : "rotate-0"
             }`}
           />
         </button>
         {open && (
           <div className="ml-2 mt-1 space-y-2 border-l border-border pl-3.5">
             {messages.map((m, i) => (
               <ToolCallCard
                 key={m.id ?? i}
                 title={m.title}
                 status={m.status}
                 diff={m.diff}
                 rawInput={m.rawInput}
                 rawOutput={m.rawOutput}
               />
             ))}
           </div>
         )}
       </div>
     );
   }
   ```

3. `src/components/ChatStream.tsx` — replace the `messages.map((message, i) => { … })` block (~line 806) with a map over the grouped units. Hoist the grouping ABOVE the return so it runs ONCE per render (do NOT call `groupConsecutiveFileWrites` inside the map). KEEP the anchored-`ask` logic verbatim for single units; the `isStreaming` index comparison becomes `i === units.length - 1` (a single unit is the last message iff it is the last unit):

   ```tsx
   // (hoisted just above the `return (` of the component's main branch)
   const units = groupConsecutiveFileWrites(messages);
   // … inside the messages `<div>`:
   {units.map((unit, i) => {
     if (unit.kind === "changes-group") {
       return (
         <ChangesGroupCard
           key={`${activeSessionId}:${i}`}
           messages={unit.messages}
         />
       );
     }
     const message = unit.message;
     // (the existing anchored-ask check, verbatim — `message.kind ===
     // "tool-call"` + `askRequests.find(…)` → `AskQuestionCard`)
     if (message.kind === "tool-call") {
       const anchored = askRequests.find(
         (r) => r.source === "main" && r.toolCallId === message.id,
       );
       if (anchored) {
         return (
           <AskQuestionCard
             key={`${activeSessionId}:${i}`}
             sessionId={activeSessionId}
             requestId={anchored.requestId}
           />
         );
       }
     }
     return (
       <MessageBubble
         key={`${activeSessionId}:${i}`}
         message={message}
         isStreaming={
           message.kind === "agent-thought" &&
           inTurn &&
           i === units.length - 1
         }
       />
     );
   })}
   ```

   (Add the `groupConsecutiveFileWrites` import from `../lib/toolGroups` and the `ChangesGroupCard` import from `./ChangesGroupCard`. Keep the existing anchored-ask comment block and the `AskQuestionCard` branch byte-identical.)

4. `src/components/SubagentPanel.tsx` — in `SubagentSection`, replace the `messageList.map((m, i) => { … })` block with the same pattern: `const units = groupConsecutiveFileWrites(messageList);` then `units.map((unit, i) => …)` — a `changes-group` unit renders `<ChangesGroupCard key={i} messages={unit.messages} />`; a `single` unit keeps the existing per-kind logic verbatim with `m` = `unit.message` (`agent-text` → `MessageBubble`, `agent-thought` → `Reasoning` with the `isStreaming` expression adapted to the unit index — `entry.status === "running" && i === units.length - 1` for a single unit, 

5. `src/lib/toolGroups.test.ts` (NEW) — build `Message` fixtures inline (the `tool-call` variant needs `id`, `title`, `status`, `at`; use a small helper in the test: `const tool = (id: string, title: string, at: number): Message => ({ kind: "tool-call", id, title, status: "completed", at })` and `const text = (at: number): Message => ({ kind: "agent-text", messageId: "m1", text: "x", at })`):
   - a single `write` → one `single` unit.
   - `write` + `edit` consecutive → one `changes-group` unit with 2 messages.
   - `write`, `agent-text`, `write` → three `single` units (the run is broken).
   - `write` + `write` + `write` → one `changes-group` with 3.
   - `write` + `edit` + `write` + `bash` → one `changes-group` (3) + one `single` (`bash`).
   - `bash` + `read` → two `single` units (non-file tools never group).
   - an empty array → `[]`.

6. `src/components/ChangesGroupCard.test.tsx` (NEW) — fixture helper as in (5); render with `fireEvent`/`screen` (no `matchMedia` stub needed). CRITICAL test-writing rules (verified against the component code + jsdom): hidden chips stay MOUNTED (CSS `invisible absolute`, `aria-hidden`) — never assert their absence from the DOM, assert their hidden state instead; `getByText` throws on multiple matches — where the same text appears twice, use `getAllByText` / `queryByText`; `+N`-style stats are split across nested elements (`DiffCount`) — assert via `textContent`:
   - `it("renders the Changes header with the file count")`: 2 messages (`write` `/a/b/c.ts`, `write` `/a/b/d.ts`, both completed) → `getByText("Changes")` + `getByText("2 files")` + both basenames present (`getByText("c.ts")`, `getByText("d.ts")` — present in the DOM; in jsdom all measurements are 0 so the chips carry the `invisible` class — do NOT assert visibility, assert presence only).
   - `it("renders total edit stats")`: 2 `edit` messages with `edits` summing to `+5 -2` (e.g. one `+3 -1`, one `+2 -1`) → `expect(container.textContent).toContain("+5")` + `expect(container.textContent).toContain("-2")` (NOT `getByText` — the `+` and the number are in separate nested elements).
   - `it("collapses chips to +N when they do not fit")`: jsdom measures zero widths → `visibleCount` 0 → for 3 files: the overflow label is present — `const matches = screen.getAllByText("+3"); expect(matches.length).toBeGreaterThanOrEqual(1)` (the visible `+{hiddenCount}` span and the invisible measuring marker both read `+3` — hence `getAllByText`, never `getByText`), AND the chip basenames are hidden: `expect(screen.getByText("c.ts").closest("[aria-hidden]")?.getAttribute("aria-hidden")).toBe("true")` (the chip's wrapper span is `aria-hidden` + `invisible` — present in the DOM, hidden by CSS).
   - `it("shows the latest member's chip while any member is pending")`: `write` completed + `write` pending (later `at`, different paths) → the pending member's basename is present exactly ONCE — `expect(screen.getAllByText("d.ts").length).toBe(1)` (while pending the full chip list is NOT rendered — the latest chip replaces it — so the text appears once, and this also proves the list is not double-rendering it).
   - `it("renders the member cards indented when expanded")`: 2 completed `write` messages, `fireEvent.click(screen.getByRole("button"))` (unique before expansion — the member cards' buttons render only when open) → `expect(screen.getAllByText("Wrote").length).toBe(2)` (the member cards' verbs render) and `expect(container.querySelector(".border-l")).not.toBeNull()` (the indented container exists).

**Steps:**
- [ ] Write `src/lib/toolGroups.test.ts` and `src/components/ChangesGroupCard.test.tsx` with the tests above.
- [ ] Run `pnpm vitest run src/lib/toolGroups.test.ts src/components/ChangesGroupCard.test.tsx`
  - Did it fail (the new modules do not exist yet)? If it passed unexpectedly, stop and investigate why.
- [ ] Implement `src/lib/toolGroups.ts` and `src/components/ChangesGroupCard.tsx`; wire both call sites (`ChatStream`, `SubagentPanel`) per (3) and (4).
- [ ] Run `pnpm vitest run src/lib/toolGroups.test.ts src/components/ChangesGroupCard.test.tsx`
  - Did all tests pass? If not, fix the failures and re-run before continuing.
- [ ] Run `pnpm test`
  - Did the FULL frontend suite pass (especially `ChatStream.test.tsx` and `SubagentPanel.test.tsx` — the call-site rewiring must not break their existing assertions)? If not, fix and re-run.
- [ ] Run `pnpm build`
  - Did it succeed (tsc + vite build, 0 errors)? If not, fix and re-run before continuing.
- [ ] Commit with message: "feat(ui): changes-group card for consecutive file writes"

**Acceptance criteria:**
- [ ] `groupConsecutiveFileWrites` folds ≥2 consecutive `write`/`edit` runs and passes everything else through.
- [ ] `ChangesGroupCard` renders the header (count + responsive chips + `+N` overflow + total stats + live latest chip) and the indented member cards when expanded.
- [ ] Both `ChatStream` and `SubagentPanel` render groups; the anchored-`ask` and `isStreaming` behaviors are preserved.
- [ ] `pnpm test` and `pnpm build` are green.

---

### Task 4: Auto behaviors — auto-open single file edits, auto-collapse subagent sections

**Context:**
The last two ZCode behaviors: a single (non-grouped) `write`/`edit` card **auto-opens once** when it finishes (the user sees the result without clicking; they can close it; it never re-opens), and a `SubagentSection` **auto-collapses** when its subagent session finishes (edge-triggered, one-shot; the user can re-open it). Both are edge-triggered state in the components — no store changes, no Rust changes.

**Files:**
- Modify: `src/components/ToolCallCard.tsx`
- Modify: `src/components/SubagentPanel.tsx`
- Test: `src/components/ToolCallCard.test.tsx` (extend)
- Test: `src/components/SubagentPanel.test.tsx` (extend + update one existing test)

**What to implement:**

1. `src/components/ToolCallCard.tsx` — add the auto-open (after the existing state declarations):

   ```tsx
   // Auto-open a single file edit once when it finishes (ZCode's
   // one-shot `autoOpen`): the user sees the result without clicking;
   // they can close it, and it never re-opens on later updates.
   const prevStatusRef = useRef(status);
   const hasAutoOpenedRef = useRef(false);
   useEffect(() => {
     const was = prevStatusRef.current;
     prevStatusRef.current = status;
     const isFileTool = title === "write" || title === "edit";
     if (
       isFileTool &&
       was === "pending" &&
       status !== "pending" &&
       !hasAutoOpenedRef.current
     ) {
       hasAutoOpenedRef.current = true;
       setOpen(true);
     }
   }, [status, title]);
   ```

   (`useRef` / `useEffect` are already imported by Task 2's version of the file.)

2. `src/components/SubagentPanel.tsx` — in `SubagentSection`:
   - Add the open state + edge-triggered auto-collapse (import `useEffect` / `useRef` from `react` — add to the existing import line):

     ```tsx
     // Auto-collapse when the subagent session finishes (ZCode's
     // `autoCollapseOnComplete`): edge-triggered (running → terminal),
     // one-shot, doesn't affect later manual toggles. A section that is
     // ALREADY finished on first render starts collapsed (the header +
     // metrics line remain; the user expands on click).
     const [open, setOpen] = useState(() => entry.status === "running");
     const prevStatusRef = useRef(entry.status);
     useEffect(() => {
       const was = prevStatusRef.current;
       prevStatusRef.current = entry.status;
       if (was === "running" && entry.status !== "running") {
         setOpen(false);
       }
     }, [entry.status]);
     ```

   - Make the header row the toggle: the existing header `<div className="flex items-center gap-1.5">` gains `onClick={() => setOpen((o) => !o)}` and `cursor-pointer`. The dismiss `X` button inside does NOT currently stop propagation (its handler is `() => dismiss(entry.sessionId)`) — ADD `event.stopPropagation()` to its `onClick` so clicking `X` doesn't also toggle the section.
   - Wrap the stream block in the open state: the existing `{messageList.length > 0 && (<div className="mt-1 max-h-48 …">…</div>)}` becomes `{open && messageList.length > 0 && (…same div…)}`. (No collapse animation in v1 — a `Collapsible` port is a noted follow-up. The metrics line and the prompt/ask cards render regardless of `open` — they are outside the stream block, unchanged.)

3. `src/components/ToolCallCard.test.tsx` — ADD:
   - `it("auto-opens a single file edit once when it finishes")`: `render(<ToolCallCard title="edit" status="pending" rawInput={{ path: "/a/b/c.ts", edits: [{ oldText: "a", newText: "b" }] }} />)` → the expanded body is NOT visible yet (`expect(screen.queryByText("No output.")).toBeNull()` and `expect(container.querySelector(".bg-panel")).toBeNull()`); then `rerender(<ToolCallCard title="edit" status="completed" rawInput={…same…} rawOutput={{ content: [{ type: "text", text: "done" }] }} />)` → the panel IS visible (`expect(container.querySelector(".bg-panel")).not.toBeNull()` and `expect(screen.getByText("done")).toBeTruthy()`). Then `fireEvent.click(screen.getByRole("button"))` (user closes it) and `rerender(<ToolCallCard title="edit" status="completed" …same… />)` → it STAYS closed (`expect(container.querySelector(".bg-panel")).toBeNull()`) — the one-shot never re-opens.
   - `it("does not auto-open non-file tools")`: `bash` pending → completed rerender → `expect(container.querySelector(".bg-panel")).toBeNull()` (manual expand only).

4. `src/components/SubagentPanel.test.tsx` — ADD + UPDATE (follow the file's existing store-setup pattern — read the existing tests first and mirror how they seed `useSubagents.setState` / `useBridge.setState` / `useSessions.setState`):
   - UPDATE the existing `it("renders thinking block (completed)")`: it seeds an entry with `status: "completed"` and asserts the `Thought` text inside the stream block — after this task that section STARTS COLLAPSED, so the assertion fails as-is. Update the test to expand first: after the initial render, `fireEvent.click` the header row (the agent-name's `cursor-pointer` ancestor), then assert `Thought`. (This is an intended behavior change — do not "fix" it by keeping the stream always open.)
   - `it("auto-collapses a subagent section when the session finishes")`: seed one entry with `status: "running"` and its session's messages (one `agent-text`), render → the stream content is visible (assert a known message text is in the document); then `useSubagents.setState((s) => ({ ...s, entries: { …same entry with status: "completed" … } }))` (or the store's update helper the existing tests use) and re-render → the stream content is GONE (the message text is not in the document) while the header (the agent name) is still visible.
   - `it("lets the user re-open a collapsed section")`: after the collapse above, `fireEvent.click` the header row (find it by the agent name text's clickable ancestor — `screen.getByText(<agentName>).closest("div")` with the `cursor-pointer` class, or give the header a stable `data-testid` — if the existing tests don't use test ids, use the `closest` approach) → the stream content is visible again.
   - `it("starts collapsed for a session that is already finished")`: seed an entry with `status: "completed"` from the start → the stream content is NOT visible on first render.

**Steps:**
- [ ] Write the new `ToolCallCard.test.tsx` tests and the new `SubagentPanel.test.tsx` tests.
- [ ] Run `pnpm vitest run src/components/ToolCallCard.test.tsx src/components/SubagentPanel.test.tsx`
  - Did it fail (the auto behaviors are not implemented yet — the panel is always absent/present regardless of status transitions)? If it passed unexpectedly, stop and investigate why.
- [ ] Implement the `ToolCallCard` auto-open and the `SubagentPanel` open state + auto-collapse + header toggle + dismiss `stopPropagation`.
- [ ] Run `pnpm vitest run src/components/ToolCallCard.test.tsx src/components/SubagentPanel.test.tsx`
  - Did all tests pass? If not, fix the failures and re-run before continuing.
- [ ] Run `pnpm test`
  - Did the FULL frontend suite pass? If not, fix and re-run.
- [ ] Run `pnpm build`
  - Did it succeed (tsc + vite build, 0 errors)? If not, fix and re-run before continuing.
- [ ] Commit with message: "feat(ui): auto-open finished file edits, auto-collapse finished subagent sections"

**Acceptance criteria:**
- [ ] A `write`/`edit` card opens itself exactly once on the `pending → finished` edge; the user's close sticks.
- [ ] A `SubagentSection` starts open while running, collapses once on the `running → terminal` edge, starts collapsed when already terminal, and re-opens on header click (dismiss `X` doesn't toggle).
- [ ] `pnpm test` and `pnpm build` are green.

---

## Verification (whole feature)

- `pnpm test` — full frontend suite green (repo root).
- `pnpm build` — tsc + vite build green (repo root).
- Rust is untouched: `cargo test` / `cargo clippy` in `src-tauri/` are NOT required (no Rust changes).
- Manual check (optional, `pnpm dev` or `pnpm tauri dev`): start a session and have the agent read + write + edit files back-to-back — the transcript shows a `Changes · 3 files · +N -M` card (expand → the three indented cards), a `Ran <command>` card with the `$`-prompt panel when expanded, and a `Writing` shimmer that settles into `Wrote` when the write finishes (auto-opening once); a failing command shows a red `Failed` with a hover tooltip + copy; the subagents tab collapses a finished subagent's stream automatically.
