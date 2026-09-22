---
status: approved
done-when: The desktop's main view renders the ZCode-styled three-pane shell (sidebar | conversation | tabbed side pane) in Zai Dark with all existing behavior preserved (spaces/sessions, permission prompts, bridge ask/confirm/password, todos, subagents, the braille working indicator), and the full validation suite is green (pnpm test, pnpm build at the repo root; cargo test, cargo clippy --all-targets with 0 warnings, cargo fmt --check from src-tauri/).
---

# ZCode Design Port — Main Agent View

**Goal:** Rebuild the Archimedes Desktop frontend's main agent view on ZCode's design system — the visual language, layout, and component treatments of the ZCode desktop main view (three-pane shell), scoped to what the desktop's backend actually supports.

## Scope

**In:**
- ZCode's design system (token layer + primitives + `DESIGN.md` rules) as the Client's UI source of truth
- The three-pane shell: left sidebar (New Session / Open Space / Sessions grouped by Space), center conversation (header + stream + composer), tabbed right side pane (Todos / Subagents)

**Out (explicitly):**
- Git tools (no git backend), Skills (agents own skills; no desktop picker), settings/config UI (no theme switcher, no model/effort/mode selectors — the ACP layer exposes none), user profile (no account system), terminal pane, Stop button (no `session/cancel` in the backend — noted as follow-up)

## Key decisions (from the design discussion)

| Decision | Choice |
|---|---|
| Approach | Port the design system, restyle Archimedes' components in place (not ZCode's components) — ADR 0006 |
| Theming | Both Zai token sets ported; `theme-zai-dark` applied on boot; no switcher |
| Working indicator | Bridge `agentState` (`working`/`blocked`/`idle`, `inTurn` fallback) → shadcn registry `braille-loader` (`typing`, `normal`, 14px) + core quips (15–45s rotation) |
| Sidebar rows | ZCode `TaskListItem` language: no dividers, status slot, gradient-fade title, relative time, green "Waiting" pill for pending prompts |
| Composer | `rounded-2xl` shell; send disabled while working (no cancel backend) |
| Side pane | Manual tabs (persisted), count badges; Todos = Goal/Progress language; Subagents = card stack |

## 1. Design system foundation

- **`src/index.css`** = port of ZCode's `packages/ui/src/styles.css`: `:root { --ui-font-size: 14px }`; the `@theme` block verbatim (`--font-mono`, the `--text-ui-*` scale, all `--color-*` semantic tokens — light defaults); `.theme-zai-light` + `.theme-zai-dark` verbatim; global rules (full-viewport `html/body/#root`, `body { margin: 0; overflow: hidden }`, focus-visible outline/ring cleanup, reduced-motion guards). Dropped: Electron vibrancy transparency, CJK text-wrapping helper.
- **Theme application** — `src/main.tsx` applies `theme-zai-dark` to `document.documentElement` at boot.
- **`src/components/ui/`** — shadcn-style primitives ported from `packages/ui/src/components/ui/`: `button` (all variants, `icon-*` sizes), `card`, `badge`, `input`, `textarea`, `select`, `dropdown-menu`, `tabs`, `collapsible`, `kbd`, `separator`, `spinner`, `progress`, `tooltip`, `dialog`, `alert-dialog`. New deps: Radix primitives, `class-variance-authority`, `clsx`, `tailwind-merge`.
- **`@` alias** — tsconfig `paths` + vite `resolve.alias` → `./src` (prerequisite for `shadcn add`).
- **Rules** (DESIGN.md, enforced in review): `text-ui-*` only for UI text (no bare `text-sm`/`text-xs`/arbitrary px — exceptions: code/diff/terminal + the braille loader's numeric `fontSize`); 4px spacing rhythm; radius nesting `xl → lg → md → sm` with approved `2xl` exceptions (composer shell, toasts, dialogs); semantic tokens only (all `neutral-*`/`sky-*` values replaced); `font-mono` for paths/commands/identifiers; dense over decorative.

## 2. App shell and 3-pane layout

- **Root** — `h-screen w-screen flex bg-background text-foreground` (replaces `bg-neutral-950 text-neutral-100`); three children: sidebar | conversation frame | side pane frame.
- **Conversation frame** — `flex-1 min-w-0`, `bg-background-alt`, `rounded-xl`, 4px inset.
- **Side pane frame** — 320px default, `bg-background-alt`, `rounded-xl`, 4px gap; 4px drag handle (transparent hit area, 2px `foreground-subtlest/50` line on hover/drag); collapsible via the header toggle; width in `localStorage`.
- **Sidebar** — fixed 260px, `bg-sidebar`.
- **Structural change** — `TodoBoardPanel`/`SubagentPanel` move out of `ChatStream` into a new `SidePane` at the `App` level (renders in every state, including the empty state — today's double-return-path hack goes away); `ChatStream` becomes the conversation frame only.
- **Window** — Tauri config unchanged.
- **Empty state** — no active session: centered `text-ui-base text-foreground-subtle` hint in the conversation frame; side pane renders its empty-tab state.

## 3. Left sidebar

- **Action cluster** (`p-3`, `border-b border-border/50`) — two `h-8 rounded-lg` buttons (icon `size-4` + label `text-ui-base` + right `kbd` hint `text-ui-xs text-foreground-subtlest`, `hover:bg-surface-hover`):
  - **New Session** (`MessageCirclePlus`, `⌘N`) — starts a Session in the selected Space (existing `startNewConversation` logic, hoisted); opens the Open Space dialog when no Space is selected.
  - **Open Space** (`FolderOpen`, `⌘O`) — existing `NewSpaceDialog`.
- **"Sessions" section header** (`px-2.5 py-2`, `text-ui-base text-foreground-subtlest`).
- **Space groups** — collapsible: folder icon + base name (`text-ui-base text-foreground-subtlest`, `hover:text-foreground-subtle`), chevron, `+` hover-action (new Session in that Space). Data: existing `spaceViewFor` (live + stored per Space).
- **Session rows** — `rounded-lg pl-2.5 pr-1 py-1`; active `bg-selected`, hover `bg-surface-hover`, **no dividers**. Leading 16px slot: live + working → circular `LoaderIcon size-4 animate-spin text-foreground-subtle`; else empty. Title `text-ui-base text-foreground` with **gradient-fade mask** truncation. Right: relative time of last activity (`text-ui-sm text-foreground-subtle`). **Attention badge**: pending Permission prompt or bridge `ask`/`confirm` → green "Waiting" pill (`bg-success/14 text-success`). Hover action (live only): **Pause** (existing `closeSession`). No context menu, no pin/rename/archive.
- **No** Skills row, no user profile.

## 4. Center pane: header + conversation stream

**Header** (`h-12`, `border-b border-border/50`, `p-2`):
- Session **title** (`text-ui-base font-medium`, gradient-fade): first user message truncated to ~80 chars, else the Space name.
- **Space chip** (`bg-surface rounded-lg`, folder icon `size-3.5` + base name `text-ui-sm`).
- **Session selector** (existing Live/`#1`/`#2`, ZCode `select` styling).
- **"…" menu** (`dropdown-menu`): Pause (live) / Resume (stored + `loadSession`) / New Session in this Space.
- Right: **side-pane toggle** icon.

**Stream** (scroll region, `p-4`, existing auto-scroll):

| Content | Treatment | Source |
|---|---|---|
| User message | plain text row — no bubble/avatar: `text-ui-base text-foreground` | `MessageBubble` |
| Assistant text | `text-ui-base` markdown (existing `react-markdown` + shiki), ZCode markdown scale (h1 `text-ui-xl`, h2 `text-ui-lg`, h3–h6 `text-ui-base` + weight ramp, inline code `font-mono text-ui-sm`, code blocks `rounded-lg bg-surface` + mono 14px body) | `MessageBubble` |
| Tool-call row | one row: icon `size-4` + label `text-ui-base`, `h-8`, `rounded-lg`, `hover:bg-surface-hover`; completed → `text-foreground-subtle`, failed → `text-destructive`; expandable — body in nested `rounded-md bg-surface`; diffs via `DiffBlock` with `--color-diff-added`/`--color-diff-removed` | `ToolCallCard` |
| File summary card | `rounded-xl border border-border bg-card`; `h-10` header "N files changed" + `+X`/`−Y` (`tabular-nums`, diff colors); per-file `h-8` rows (file-type icon, `font-mono` path, stats). **Derived** per turn from `diff` messages + tool-call `diff` refs via new `parseDiffStats(patch)` in `lib/diff.ts`. No Undo | new |
| Permission prompt | in-stream card: `rounded-xl border` + green confirmation treatment (`--color-interaction-confirmation-*`); tool name + diff preview + Allow (primary) / Deny (outline) | `PermissionPrompt` |
| Bridge `ask` | `--color-interaction-ask-*` treatment; anchored-in-place + stacking behavior unchanged | `AskQuestionCard` |
| Bridge `confirm`/`password` | `dialog` primitive, `rounded-2xl` shell | `SudoConfirmModal` / `SudoPasswordModal` |
| **Working indicator** | state source: bridge `agentState[sessionId]` (bridge agents) else `inTurn` (fallback). `working` → **`BrailleLoader`** (registry component: `variant="typing"`, `speed="normal"`, `fontSize={14}`, `label="Agent working"`, vendored 1-line tweak `fontFamily: var(--font-mono, monospace)`) + **quip** (`SPIN_QUIPS`/`pickQuip`/15–45s rotation from the core, `useSpinQuip(working)`, `text-ui-sm text-foreground-subtle`). `blocked` → "Waiting for your input…" (`text-ui-sm text-foreground-subtle`). `idle` → hidden | new |
| Stop reason | `text-ui-sm text-foreground-subtlest` | existing |
| Fresh session | centered `text-ui-base text-foreground-subtlest` "Send a prompt to start" | new |

**No changes** to stores, ACP wiring, or anchored-ask correlation.

## 5. Center pane: composer

- **Shell** — `rounded-2xl bg-input border-input-border`, `hover:border-input-border-hover`, `focus-within:border-input-border-focused`; auto-growing `textarea` (`resize-none`, `text-ui-base`), placeholder `text-foreground-subtlest`.
- **Placeholders** — live + idle: "Send a prompt…"; live + working: "Agent is working…"; stored + resumable: "Paused — Resume to reconnect"; history-only: "This session is closed".
- **Footer** — left empty; right: static agent label (`text-ui-xs text-foreground-subtlest`, the `agentId`) + circular send `size-8 rounded-full bg-primary text-primary-foreground` (up-arrow, `disabled:opacity-50`). **Disabled while working** — no `session/cancel` in the backend (follow-up candidate: Rust `session/cancel` command + Stop button).
- **Dropped** (no backend support): `+` attachments, mode selector, model/effort selectors.
- **Keyboard** — Enter sends, Shift+Enter newline. Error line above the shell (`text-ui-sm text-destructive`).

## 6. Right pane: tabbed side pane

- **Tab bar** (`h-9`, `px-2`, `gap-0.5`): **Todos** / **Subagents** — `text-ui-sm`, `rounded-md`, active `bg-selected`; inactive tabs show count badges (`text-ui-xs`). Manual selection, `localStorage`, default Todos; no auto-switching.
- **Todos tab** — progress header: `N/M` (`text-ui-base font-medium`) + `progress` bar (primary fill) when M > 0. Checklist: `h-8` rows `rounded-md hover:bg-surface-hover`; done = check circle `text-success` + label `text-foreground-subtle`, in-progress = `◉` `text-warning`, pending = `○` `text-foreground-subtlest`; label `text-ui-base`. Subagent todo columns: indented sub-rows (`pl-6`, `text-ui-sm`). Empty: "No todos yet" (`text-ui-sm text-foreground-subtlest`).
- **Subagents tab** — card stack (`rounded-xl border-card-border bg-card`, 8px gaps): header = agent name (`text-ui-base font-medium`) + status chip (`running` `text-warning` / `completed` `text-success` / `failed` `text-destructive`); body = task (`text-ui-sm text-foreground-subtle`); footer = metrics snapshot (`text-ui-xs text-foreground-subtlest`). Empty: "No subagent sessions".
- **Data** — existing stores only (`useBridge().todos[activeSessionId]`, `useSubagents()`); existing test suites retargeted.

## Follow-ups (noted, not in scope)

1. Rust `session/cancel` command + composer Stop button.
2. Cost/metrics rendering in the Todos progress header (bridge `cost` is stored, not rendered in v1).
3. Light theme activation (tokens already present; a switcher if ever wanted).

## Verification

Per AGENTS.md, the branch is ready when: `pnpm test` (repo root), `pnpm build` (type-check + build), and from `src-tauri/`: `cargo test`, `cargo clippy --all-targets` (0 warnings), `cargo fmt --check`. New unit tests: `parseDiffStats` (added/removed counting, multi-file, malformed patches), `useSpinQuip` (episode re-pick, rotation window, no-repeat), `BrailleLoader` (frame-table determinism, reduced-motion static frame).
