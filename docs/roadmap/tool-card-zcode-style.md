---
status: approved
done-when: Tool cards in the transcript and subagent streams render in the ZCode style — per-tool icon + past-tense verb (shimmer while running; a red `Failed` word with a dotted-underline tooltip, error text and copy button on failure), file chips with language icons, `+N -M` edit stats with a one-shot flip, and a `rounded-xl border bg-panel` expanded body with a `$` prompt for shell commands; consecutive `write`/`edit` runs fold into a `Changes` card with responsive `+N` chip overflow; a single file edit auto-opens once on completion; a subagent section auto-collapses on completion. `pnpm test` + `pnpm build` green.
---

# ZCode-style tool cards

## Problem / motivation

The tool cards shipped in `tool-call-output` (PR #5) show a raw tool name + a plain-text summary and a bare `bg-surface` expanded block. ZCode (the sibling project whose design system this repo's ADR 0006 already ports) renders the same data with a richer visual language: past-tense **verbs** instead of raw tool names, **file chips** (language icon + basename) instead of plain paths, green/red **`+N -M` diff stats**, a **shimmer** instead of a spinner while running, a red **`Failed` word + tooltip with copy** instead of a ✕ icon, a **panel box** with a `$` prompt for shell output, **`Changes` grouping** of consecutive file writes, and **auto-open / auto-collapse** behaviors. This feature ports that full visual language onto the existing tool cards.

**Constraint:** frontend-only. All data already flows from the last feature (`rawInput` / `rawOutput` on the `tool-call` `Message`, live partials included). No Rust changes. The design system (`src/index.css` `@theme`) already carries the needed tokens (`--color-panel`, `--color-border`, `--font-mono`, `--text-ui-xs`, `--color-success`, `--color-destructive`); new tokens are added for the shimmer.

## Design

### 1. Data layer (pure functions)

Extend `src/lib/toolOutput.ts` (which already holds `summarizeToolCall` + `normalizeToolOutput`, both UNCHANGED) and add `src/lib/fileIcons.ts`:

- **`toolVerb(title: string, status: ToolCallUiStatus): string | undefined`** — verb table:

  | Tool(s) | completed | pending (running) |
  |---|---|---|
  | `bash` / `powershell` / `sudo_exec` | Ran | Running |
  | `read` | Read | Reading |
  | `write` | Wrote | Writing |
  | `edit` | Edited | Editing |
  | `grep` / `find` / `web_search` | Searched | Searching |
  | `ls` | Listed | Listing |
  | `fetch_content` | Fetched | Fetching |
  | `ask` | Asked | Asking |
  | `manage_todo_list` | Todos | Updating todos |
  | `subagent` | Delegated | Delegating |
  | `mcp` | MCP | MCP |
  | unknown | `undefined` | `undefined` |

  `undefined` → the card falls back to the raw tool `title` (today's behavior).

- **`toolIcon(title: string)`** — per-tool lucide icon map: `bash`/`powershell`/`sudo_exec` → `SquareTerminalIcon`; `read` → `FileTextIcon`; `write` → `FilePenIcon`; `edit` → `PencilIcon`; `grep`/`find` → `SearchIcon`; `ls` → `ListIcon`; `web_search` → `GlobeIcon`; `fetch_content` → `DownloadIcon`; `ask` → `MessageCircleQuestionIcon`; `manage_todo_list` → `ListTodoIcon`; `subagent` → `BotIcon`; `mcp` → `PlugIcon`; default → `WrenchIcon`. (Verify each name exists in the installed `lucide-react` version; substitute the closest available if a name is missing.)

- **`fileSummaries(title: string, rawInput: unknown): Array<{ path: string; fileName: string }>`** — for `read` / `write` / `edit`: `[{ path: rawInput.path, fileName: basenameOfPath(path) }]` (existing `src/lib/paths.ts`); any other tool → `[]`.

- **`editChangeStat(rawInput: unknown): { added: number; removed: number } | undefined`** — `edit` tool ONLY (ZCode's `getChangeStat` semantics): sum over `rawInput.edits[]` of `lineCount(newText)` / `lineCount(oldText)` (a line = split on `\n`; an empty string → 0). `undefined` when the tool is not `edit`, `edits` is missing/empty, or both totals are 0.

- **`failureText(rawOutput: unknown): string | undefined`** — `rawOutput.details?.error` (string) → else the first `content` text item (trimmed, non-empty) → else `undefined`. Feeds the failure tooltip.

- **`fileIconFor(path: string): { icon: LucideIcon; className: string }`** (new `src/lib/fileIcons.ts`) — a map of ~15 extensions → `{ icon, colorClass }`: `js`/`jsx` → `FileCodeIcon` + yellow-ish; `ts`/`tsx` → `FileCodeIcon` + blue-ish; `json` → `FileJsonIcon`; `html`/`htm` → `FileCodeIcon` + orange-ish; `css` → `PaintbrushIcon` (or `FileCodeIcon`) + purple-ish; `md`/`markdown` → `FileTextIcon` + slate; `rs` → `FileCodeIcon` + orange; `py` → `FileCodeIcon` + green; `toml`/`ini`/`yaml`/`yml` → `FileCogIcon`; `lock` → `FileLockIcon`; `png`/`jpg`/`jpeg`/`gif`/`svg`/`webp` → `FileImageIcon`; `sh`/`bash` → `TerminalIcon`. Colors use existing palette tokens (e.g. `text-warning`, `text-brand`, `text-success`, `text-destructive`, `text-foreground-subtle`) — no new color tokens. Default (no extension / unmapped) → `FileIcon` + `text-foreground-subtlest`. Pure function, unit-testable.

### 2. `ToolCallCard` redesign

**Header row** (same `h-8` row; `group/tool-summary inline-flex max-w-full items-center gap-2`): `icon [verb] [primary] [stats] [status word] [chevron]`

- **Icon**: `toolIcon(title)` at `size-4`, `text-foreground-subtle`, **always static** — the pending **spinner is removed** (ZCode's deliberate choice: running is shown by the text, the icon stays static). The completed **check icon is removed** too — the past-tense verb is the signal.
- **Verb**: `toolVerb(title, status)` in `font-medium whitespace-nowrap`; while `pending` it also gets the **`animated-gradient-text`** class (new in `src/index.css`: port ZCode's `--animated-gradient-text-strong/soft` tokens — `rgba(255,255,255,1)` / `rgba(255,255,255,0.2)` in the dark theme, the neutral equivalents in light — plus a gradient-sweep keyframe on a `background-clip: text` mask; `@media (prefers-reduced-motion: reduce)` disables the animation). While `completed`: plain `text-foreground-subtlest` (today's completed color).
- **Primary text**: file tools (`read`/`write`/`edit`) → a **file chip**: `[16px language icon via fileIconFor] [basename]` in `text-foreground-subtle`, `min-w-0 truncate`, `title` = the full path (non-clickable in v1 — the desktop has no code-viewer action wired to the side pane yet; noted as a follow-up). Shell tools → the **command** (`rawInput.command`) in `font-sans` (ZCode's deliberate collapsed-state choice: the code font is reserved for the expanded body). All other tools → the existing `summarizeToolCall` summary, unchanged.
- **Stats**: when `editChangeStat` returns a value: `+N` in `text-success` and `-M` in `text-destructive`, `font-mono tabular-nums text-ui-sm`, each number rendered through the new **`FlipMetricValue`** component.
- **Status word**: `failed` → a `Failed` word in `text-destructive` with `underline decoration-dotted underline-offset-2 cursor-help`, wrapped in the existing `ui/tooltip.tsx` `Tooltip`: content = `failureText` (if any) in a `max-w-96` box, `line-clamp-3 whitespace-pre-wrap break-words`, plus a **copy button** (`CopyIcon` → `CheckIcon` for 1.5s via `navigator.clipboard.writeText`; a no-op when `failureText` is `undefined` — then the word renders without a tooltip). `pending` / `completed` → **no word**. The red ✕ icon is **dropped** — the word carries the failure.
- **Chevron**: `ChevronRightIcon size-4 text-foreground-subtlest`, `opacity-0 group-hover:opacity-100 transition`, `rotate-90` when open (ZCode's pattern; the row stays the click target).

**Expanded body** — a single `rounded-xl border border-border bg-panel px-4 py-3` panel (replaces today's `bg-surface` block; the existing `diff` branch keeps priority when present):

- **Shell tools** (`bash`/`powershell`/`sudo_exec`): a `$` prompt line — `text-foreground-subtle` `$` + the command in `font-mono` (`max-h-15 overflow-auto truncate whitespace-pre-wrap break-words`) — then the output: `normalizeToolOutput` in a scrollable `max-h-80 pre` (the existing 20k-char cap + truncation note stay as-is), or a muted `No output.` when there is none.
- **File tools** (`read`/`write`/`edit`): the file chip + stats, then the output (`read` → the file content; `write`/`edit` → the tool's confirmation/error text — a real diff preview needs backend diff data, **out of scope**).
- **Other tools**: the query/command line (existing summary), then the output.

**`FlipMetricValue`** (new `src/components/FlipMetricValue.tsx`): renders a number; on value change, plays a one-shot CSS flip (new `@keyframes` in `index.css` — a `transform: translateY` + opacity swap on a `key={value}` remount; `motion-safe:` guarded).

### 3. Changes-group + auto behaviors

- **`groupConsecutiveFileWrites(messages: Message[]): RenderUnit[]`** (new pure fn, `src/lib/toolGroups.ts`): `RenderUnit = { kind: "single"; message: Message } | { kind: "changes-group"; messages: Message[] }`. A maximal run of **≥2 consecutive** `tool-call` messages whose `title` is `write` or `edit` folds into one `changes-group` unit; a single write/edit, or a run interrupted by any other message kind (`agent-text`, `agent-thought`, `user`, `diff`), passes through as `single` units. Pure → re-derived per render, so live streaming (a write joins the run mid-turn) and reloads are both consistent. The store is untouched.
- **`ChangesGroupCard`** (new `src/components/ChangesGroupCard.tsx`):
  - Header: `PencilIcon` + `Changes` + `N files` + `·` + a **responsive file-chip list**: the members' file chips (deduped by path, `fileIconFor` + basename), measured with `useLayoutEffect` + `ResizeObserver` against the row's right boundary; overflow collapses to `+N` (port of ZCode's `resolveResponsiveFileChipCount`: available width = boundary right − list left − 24px trailing; chips + the `+N` measured via refs; hidden chips `aria-hidden`/invisible; zero available width → 0 visible chips, `+N` stays).
  - **Total stats**: sum of members' `editChangeStat` → the same green/red `FlipMetricValue` markup.
  - **Live state** (any member `pending`): `N files` + `·` + the **latest member's chip** (max `at`) — the group visibly grows as writes arrive.
  - **Expanded**: member cards indented `ml-2 border-l pl-3.5 space-y-2` (each a normal `ToolCallCard`). Manual expand only (no auto-open for the group).
- **Application sites** — the grouping is applied at BOTH render sites: `ChatStream` (the `messages.map` at ~line 806) and the `SubagentPanel` stream (the `messageList.map`), via the shared pure fn.
- **Auto-open single write/edit** (non-grouped): on the `pending → completed|failed` edge, open the card **once** (edge-triggered `useEffect` + a `hasAutoOpened` ref — ZCode's one-shot `autoOpen`: opens the first time it finishes, the user can close it, it never re-opens on later updates).
- **SubagentPanel auto-collapse**: `SubagentSection` gains an `open` state — default `true` while `entry.status === "running"`; on the `running → completed|failed` **edge** collapse once (ZCode's `autoCollapseOnComplete`: edge-triggered, one-shot, doesn't affect later manual toggles); the header row becomes the toggle. Collapsed = header + the existing metrics/error line only (conditional render of the stream; no collapse animation in v1 — a `Collapsible` port is a noted follow-up).

## Out of scope (YAGNI)

- Real **diff previews** for `write`/`edit` (needs backend diff data — a separate feature).
- **Clickable** file chips opening a code viewer (no side-pane file action exists yet).
- Collapse **animations** (Radix `Collapsible` + delayed unmount) — v1 uses conditional renders.
- **`QueuedSummaryContent`**-style queued rolling of the live summary (the group's latest-chip update covers the visible behavior; the rolling animation is not ported).
- Any **Rust/backend** changes.

## Test plan (TDD)

- `src/lib/toolOutput.test.ts` (new): `toolVerb` (every row of the table × both statuses; unknown → `undefined`); `fileSummaries` (read/write/edit → one entry with the basename; bash → `[]`); `editChangeStat` (single edit, multi-edit sum, both-zero → `undefined`, non-edit → `undefined`); `failureText` (`details.error` wins, text fallback, `undefined`).
- `src/lib/fileIcons.test.ts` (new): a sample of extensions → icon + class; unmapped → default.
- `src/lib/toolGroups.test.ts` (new): single write → passthrough; consecutive `write`+`edit` → one group; a run interrupted by `agent-text` → two separate groups; three in a row → one group of 3; non-file tools never group.
- `src/components/ToolCallCard.test.tsx` (extend): verb rendering (`bash` completed → `Ran`; `pending` → `Running` + the `animated-gradient-text` class); file chip (`read` → basename + language icon); stats (`edit` → `+5 -2`); `failed` → `Failed` word + tooltip content + copy button; expanded shell body → `$` prompt + command + output; expanded file body → chip + output.
- `src/components/ChangesGroupCard.test.tsx` (new): header `Changes` + `N files` + total stats; `+N` overflow (mocked measurements); expanded members indented; live state shows the latest chip.
- `src/components/FlipMetricValue.test.tsx` (new): renders the number; re-triggers the animation on value change (assert the `key`/class swap).
- `src/components/SubagentPanel.test.tsx` (extend): a `running` section is open; the `running → completed` edge collapses it once; a manual re-open sticks.
