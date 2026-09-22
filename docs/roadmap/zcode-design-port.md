---
status: committed
done-when: The desktop's main view renders the ZCode-styled three-pane shell (sidebar | conversation | tabbed side pane) in Zai Dark with all existing behavior preserved (spaces/sessions, permission prompts, bridge ask/confirm/password, todos, subagents, the braille working indicator), and the full validation suite is green (pnpm test, pnpm build at the repo root; cargo test, cargo clippy --all-targets with 0 warnings, cargo fmt --check from src-tauri/).
---

# ZCode Design Port — Main Agent View — Plan

**Goal:** Rebuild the desktop frontend's main agent view on ZCode's design system (three-pane shell, Zai Dark), keeping every existing behavior.
**Architecture:** Presentation-layer-only change: a ported token layer (`src/index.css` + shadcn-style primitives in `src/components/ui/`) underpins restyled versions of the existing components; the `TodoBoardPanel`/`SubagentPanel` move into a new tabbed `SidePane` at the `App` level. Stores, ACP wiring, and the Rust backend are untouched.
**Tech Stack:** React 19, TypeScript, Tailwind 4 (`@theme`), zustand, Tauri 2, Radix primitives (`radix-ui`), `class-variance-authority`, `clsx`/`tailwind-merge`, `lucide-react`, `react-markdown`, `shiki`, vitest + @testing-library/react.

**Source repos (read-only reference):**
- ZCode: `/home/daniel/Coding/AI/ZCode` — design system: `DESIGN.md` (rules), `packages/ui/src/styles.css` (tokens), `packages/ui/src/components/ui/` (primitives to port).
- pi-archimedes: `/home/daniel/Coding/Javascript/pi-archimedes` — `packages/ui/src/editor/spin-quips.ts` (quips to port verbatim).
- shadcn registry: `https://shadcn-braille-loader.vercel.app/r/braille-loader.json` (braille-loader component).

**Approved design reference** (full spec in git history — commit `646fb1c`; the details below are the binding summary):

- **Design rules (DESIGN.md, enforced in every task):** `text-ui-*` only for UI text (no bare `text-sm`/`text-xs`/arbitrary px; exceptions: code/diff/terminal content + `BrailleLoader`'s numeric `fontSize`); 4px spacing rhythm; radius nesting `rounded-xl → lg → md → sm` with approved `2xl` exceptions (composer shell, dialogs, toasts); semantic `--color-*` tokens only (no `neutral-*`/`sky-*` ad-hoc values); `font-mono` for paths/commands/identifiers; dense over decorative.
- **Tokens:** `:root { --ui-font-size: 14px }`; `@theme` block verbatim from ZCode `styles.css` (light defaults: `--font-mono`, `--text-ui-*` scale — xl +4px, lg +2px, base 14px, caption −1px, sm −2px, xs −4px, 2xs −5px — and every `--color-*` semantic token); `.theme-zai-light` + `.theme-zai-dark` verbatim; global rules: full-viewport `html/body/#root`, `body { margin: 0; overflow: hidden }`, focus-visible outline/ring cleanup, reduced-motion guards. Dropped: Electron vibrancy transparency, CJK text-wrapping helper.
- **Shell:** root `h-screen w-screen flex bg-background text-foreground`; three children — sidebar (fixed 260px, `bg-sidebar`) | conversation frame (`flex-1 min-w-0`, `bg-background-alt`, `rounded-xl`, 4px inset) | side pane frame (320px default, `bg-background-alt`, `rounded-xl`, 4px gap, resizable via a 4px drag handle — transparent hit area, 2px `foreground-subtlest/50` line on hover/drag — collapsible via a header toggle, width in `localStorage`).
- **Sidebar:** action cluster (`p-3`, `border-b border-border/50`): **New Session** (`MessageCirclePlus`, `⌘N`) and **Open Space** (`FolderOpen`, `⌘O`) — each `h-8 rounded-lg`, icon `size-4` + label `text-ui-base` + right `kbd` hint `text-ui-xs text-foreground-subtlest`, `hover:bg-surface-hover`. "Sessions" section header (`px-2.5 py-2`, `text-ui-base text-foreground-subtlest`). Space groups: collapsible, folder icon + base name (`text-ui-base text-foreground-subtlest`, `hover:text-foreground-subtle`), `+` hover-action = new Session in that Space. Session rows: `rounded-lg pl-2.5 pr-1 py-1`, active `bg-selected`, hover `bg-surface-hover`, **no dividers**; leading 16px slot (live + working → circular `LoaderIcon size-4 animate-spin text-foreground-subtle`); title `text-ui-base text-foreground` with gradient-fade mask truncation; right: relative time of last activity `text-ui-sm text-foreground-subtle`; **attention badge**: pending Permission prompt or bridge `ask`/`confirm`/`password` → green "Waiting" pill `bg-success/14 text-success`; hover action (live only): Pause (existing `closeSession`). No Skills row, no user profile, no context menu.
- **Center header:** `h-12`, `border-b border-border/50`, `p-2`. Session title (`text-ui-base font-medium`, gradient-fade): first user message truncated to ~80 chars, else the Space name. Space chip (`bg-surface rounded-lg`, folder icon `size-3.5` + base name `text-ui-sm`). Session selector (existing Live/`#1`/`#2`, `select` styling). "…" `dropdown-menu`: Pause (live) / Resume (stored + `loadSession`) / New Session in this Space. Right: side-pane toggle icon.
- **Stream** (scroll region, `p-4`, existing auto-scroll): user message = plain `text-ui-base text-foreground` row (no bubble/avatar); assistant text = `text-ui-base` markdown (existing `react-markdown` + shiki) with ZCode's markdown scale (h1 `text-ui-xl`, h2 `text-ui-lg`, h3–h6 `text-ui-base` + weight ramp — h3/h4 `font-semibold`, h5 `font-medium`, h6 `font-normal`; inline code `font-mono text-ui-sm`; code blocks `rounded-lg bg-surface`, mono 14px body); tool-call row = one row, icon `size-4` + label `text-ui-base`, `h-8`, `rounded-lg`, `hover:bg-surface-hover`, completed → `text-foreground-subtle`, failed → `text-destructive`, expandable with body in nested `rounded-md bg-surface`, diffs via `DiffBlock` with `--color-diff-added`/`--color-diff-removed`; **file summary card** = `rounded-xl border border-border bg-card`, `h-10` header "N files changed" + `+X`/`−Y` (`tabular-nums`, diff colors), per-file `h-8` rows (file-type icon, `font-mono` path, stats) — derived per turn from the standalone `kind: "diff"` messages via `parseDiffStats` (Task 2; the store already emits tool-call extracted diffs as standalone `diff` messages — the `tool-call.diff` refs are **not** summed, that would double-count), no Undo; permission prompt = in-stream card `rounded-xl border` + green confirmation treatment (`--color-interaction-confirmation-*`), tool name only (the store holds no content/diff — `PermissionPromptData` = `{ requestId, toolTitle, options }`) + one button per agent-defined option (first = primary, rest = outline) + Cancel (outline); bridge `ask` = `--color-interaction-ask-*` treatment (anchored-in-place + stacking unchanged); bridge `confirm`/`password` = `dialog` primitive `rounded-2xl` shell; **working indicator** = state source bridge `agentState[sessionId]` (bridge agents) else `inTurn` (fallback): `working` → `BrailleLoader` (`variant="typing"`, `speed="normal"`, `fontSize={14}`, `label="Agent working"`) + quip (`useSpinQuip`, `text-ui-sm text-foreground-subtle`), `blocked` → "Waiting for your input…" (`text-ui-sm text-foreground-subtle`), `idle` → hidden; stop reason `text-ui-sm text-foreground-subtlest`; fresh session = centered `text-ui-base text-foreground-subtlest` "Send a prompt to start".
- **Composer:** `rounded-2xl bg-input border-input-border`, `hover:border-input-border-hover`, `focus-within:border-input-border-focused`; auto-growing `textarea` (`resize-none`, `text-ui-base`), placeholder `text-foreground-subtlest` — live + idle: "Send a prompt…"; live + working: "Agent is working…"; stored + resumable: "Paused — Resume to reconnect"; history-only: "This session is closed". Footer: left empty; right = static agent label (`text-ui-xs text-foreground-subtlest`, the `agentId`) + circular send `size-8 rounded-full bg-primary text-primary-foreground` (up-arrow, `disabled:opacity-50`), **disabled while working** (no `session/cancel` backend — follow-up). Dropped: `+` attachments, mode/model/effort selectors. Enter sends, Shift+Enter newline. Error line above the shell (`text-ui-sm text-destructive`).
- **Side pane:** tab bar (`h-9`, `px-2`, `gap-0.5`): **Todos** / **Subagents** — `text-ui-sm`, `rounded-md`, active `bg-selected`; inactive tabs show count badges (`text-ui-xs`); manual selection, `localStorage`, default Todos, no auto-switching. Todos tab: progress header `N/M` (`text-ui-base font-medium`) + `progress` bar (primary fill) when M > 0; checklist `h-8` rows `rounded-md hover:bg-surface-hover` — done = check circle `text-success` + label `text-foreground-subtle`, in-progress = `◉` `text-warning`, pending = `○` `text-foreground-subtlest`, label `text-ui-base`; subagent todo columns = indented sub-rows (`pl-6`, `text-ui-sm`); empty: "No todos yet" (`text-ui-sm text-foreground-subtlest`). Subagents tab: card stack (`rounded-xl border-card-border bg-card`, 8px gaps) with **every existing behavior preserved** — header = agent name (`text-ui-base font-medium`) + the bridge `agentState` chip + the status chip (`running` `text-warning` / `completed` `text-success` / `failed` `text-destructive`) + the pending-permission badge (green confirmation treatment) + a dismiss button; the subagent's pending `ask`/`confirm`/`password`/permission rendering + the compact message stream + the metrics/error line on close all keep rendering (the panel is the ONLY render site for subagent-session requests); empty: "No subagent sessions" (the one existing test asserting "renders nothing" is rewritten to assert the empty state).

**Out of scope (do NOT build):** git tools, skills picker, settings/config UI (no theme switcher, no model/effort/mode selectors), user profile, terminal pane, Stop button (no `session/cancel` backend — follow-up), Rust backend changes.

**Validation per task (AGENTS.md):** `pnpm test` (repo root) and `pnpm build` (type-check + build) must be green. Rust is untouched by this plan — no cargo steps per task, but the final task runs the full suite.

---

### Task 1: Design system foundation (tokens, theme, alias, deps, primitives)

**Context:**
Every later task builds on a shared design system. This task creates the token layer (so Tailwind 4 generates the `text-ui-*`, `bg-card`, `border-border`, … utilities the same way ZCode does), applies the dark theme on boot, adds the `@` import alias the shadcn registry files expect, and lands the shadcn-style primitives the rest of the UI is composed from. The Rust backend and all stores are untouched.

**Files:**
- Modify: `src/index.css`
- Modify: `src/main.tsx`
- Create: `src/lib/theme.ts`
- Create: `src/lib/theme.test.ts`
- Modify: `tsconfig.json`
- Modify: `vite.config.ts`
- Modify: `package.json` (deps)
- Create: `src/components/lib/utils.ts`
- Create: `src/components/lib/utils.test.ts`
- Create: `src/components/ui/button.tsx`
- Create: `src/components/ui/card.tsx`
- Create: `src/components/ui/badge.tsx`
- Create: `src/components/ui/input.tsx`
- Create: `src/components/ui/textarea.tsx`
- Create: `src/components/ui/select.tsx`
- Create: `src/components/ui/dropdown-menu.tsx`
- Create: `src/components/ui/tabs.tsx`
- Create: `src/components/ui/collapsible.tsx`
- Create: `src/components/ui/kbd.tsx`
- Create: `src/components/ui/separator.tsx`
- Create: `src/components/ui/spinner.tsx`
- Create: `src/components/ui/progress.tsx`
- Create: `src/components/ui/tooltip.tsx`
- Create: `src/components/ui/dialog.tsx`
- Create: `src/components/ui/alert-dialog.tsx`
- Create: `src/components/ui/button.test.tsx`

**What to implement:**

1. **`src/index.css`** — replace the single `@import "tailwindcss";` line with the port of `/home/daniel/Coding/AI/ZCode/packages/ui/src/styles.css`, assembled as follows (ZCode line numbers in parentheses):
   - `@import "tailwindcss";` — keep Archimedes' plain form (ZCode's `source(".")` suffix is a pnpm-workspace scanning workaround, not needed here — the Vite plugin scans the project).
   - `@import "tw-animate-css";` and `@import "shadcn/tailwind.css";` — **required by the ported primitives** (ZCode's own comment at lines 28–41 documents the failure mode without them: the `@custom-variant` definitions in `shadcn/tailwind.css` map Radix's `data-state`/`data-orientation` onto the `data-active:`/`data-horizontal:` custom variants used by `tabs.tsx`/`separator.tsx`/`select.tsx`/`badge.tsx`, and `tw-animate-css` provides the `animate-in`/`fade-in`/`zoom-in` utilities used by `dialog.tsx`/`alert-dialog.tsx`/`dropdown-menu.tsx`/`collapsible.tsx` — without them those components render "class present, effect absent"; the `animate-collapsible-*` keyframes come from `tw-animate-css` — imported alongside. Note for accuracy: the `--color-muted`/`--color-muted-foreground`/`--color-ring` tokens are defined NOWHERE (not in the `@theme` block) — so `bg-muted`/`border-ring`/`ring-ring` in the ported `tabs.tsx`/`progress.tsx`/`alert-dialog.tsx` are dead classes and the progress track renders unfilled/transparent. That is ZCode's own rendering — accept it verbatim (do NOT add the three tokens; "verbatim" wins over "sensible"), and the Task 4 progress test asserts the indicator `width` style, NOT a track background).
   - `:root { --ui-font-size: 14px; }` (line 58)
   - The `@theme { … }` block **verbatim** (lines 137–306: `--font-mono`, the `--text-ui-*` scale, and every `--color-*` token — light defaults). Do not "clean up" or reorder tokens.
   - The `.dark { … }` block **verbatim** (lines 308–459 — a separate block from `@theme`; it re-maps the `--color-*` tokens under the `.dark` class that the primitives' `dark:` utilities key off).
   - The `.theme-zai-light { … }` (lines 461–604) and `.theme-zai-dark { … }` (lines 606–750) blocks **verbatim**.
   - **Keep** the global scrollbar rules (lines 750–782: the `*` block + the four `*::-webkit-scrollbar*` selectors — theme-token-driven scrollbar styling, part of the calm/dense look).
   - **Drop** (ZCode-internal, not portable): the `@source` directives (lines 7, 42, 44 — package scanning for the ZCode workspace); `@plugin "@tailwindcss/typography"` (line 9 — the `prose*` classes are dropped in Task 6, so the plugin is not needed); the `@xterm/xterm` import (line 5 — no terminal); the `katex` import (line 6 — no math); the `tailwind-scrollbar-hide/v4` import (line 3 — used only by `command.tsx`, which is NOT ported); the `html/body/#root` vibrancy transparency (Tauri draws its own chrome); the `body { margin: 0; overflow: hidden }` block stays; the `#root { height: 100dvh; overflow: hidden }` block stays; the `*:focus, *:focus-visible` outline/ring cleanup block (with its `forced-colors` media variant) stays; **drop** `.text-wrap-phrase` (lines 68–80, CJK helper), `.side-pane-open-tab-*` + the `@container` block (lines 81–106 — the block ENDS before the `#root` rule at line 108; do NOT cut to 133, which would swallow the kept `#root` (108–113) and `*:focus`/`forced-colors` cleanup (117–133) blocks — cut BY NAME, not by range), `.terminal-xterm-shell` (lines 788–795), `[data-zcode-pptx-render-surface]` (lines 803–811 — the selector starts at 803, not 806), the entire `@layer utilities` block (lines 813–~890: `.button-gradient`, `.animated-gradient-text*`, `.cua-group-gradient-text` + their `@keyframes`), and **everything from line ~890 to the end of the file** (all feature-scoped keyframes/classes: `zcode-stream-*`, `zcode-draft-*`, `markdown-image-loading-shimmer`, `zcode-collapsible-*`, `zcode-reaction-*`, `zcode-alarm-ring`, `workspace-remote-*`, `zcode-task-interaction-countdown`, `zcode-update-charge-*`, `browser-use-*`, `task-search-result-highlight`, `fork-highlight-pulse`, `.wf-*` — none are used by the 16 primitives; the `animate-collapsible-up/down` utilities the primitives use come from the shadcn layer, NOT from the `zcode-collapsible-*` keyframes).
2. **`src/lib/theme.ts`** — new module:
   ```ts
   export type AppTheme = "zai-light" | "zai-dark";
   export function applyThemeToDocument(theme: AppTheme): void {
     const root = document.documentElement;
     // Mirrors ZCode's useTheme.ts:66-68 — the `dark` class is toggled ALONGSIDE
     // the theme classes because the ported primitives use `dark:` utilities;
     // without it, `dark:` styles key off `prefers-color-scheme` instead.
     root.classList.toggle("dark", theme === "zai-dark");
     root.classList.toggle("theme-zai-light", theme === "zai-light");
     root.classList.toggle("theme-zai-dark", theme === "zai-dark");
   }
   ```
3. **`src/main.tsx`** — call `applyThemeToDocument("zai-dark")` before `ReactDOM.createRoot(…).render(…)` (the app is dark by default; no switcher).
4. **Alias** — `tsconfig.json` `compilerOptions.paths`: `{ "@/*": ["./src/*"] }` (keep everything else); `vite.config.ts`: add `resolve: { alias: { "@": new URL("./src", import.meta.url).pathname } }` (or `path.resolve(__dirname, "src")` — match the file's existing ESM style; the `@ts-expect-error` on the `node:process` import stays).
5. **Deps** — add to `package.json` dependencies (versions per ZCode `packages/ui/package.json`): `radix-ui@^1.4.3`, `class-variance-authority@^0.7.1`, `clsx@^2.1.1`, `lucide-react@^1.17.0`, `tailwind-merge@^3.5.0`, `shadcn@4.1.1`, `tw-animate-css@^1.4.0`; then `pnpm install`.
6. **`src/components/lib/utils.ts`** — verbatim port of `/home/daniel/Coding/AI/ZCode/packages/ui/src/components/lib/utils.ts` (the `cn` = `extendTailwindMerge` with the `font-size` classGroup for the `text-ui-*` tokens + `clsx`).
7. **Primitives** — port each file from `/home/daniel/Coding/AI/ZCode/packages/ui/src/components/ui/` into `src/components/ui/`, with exactly two adaptations:
   - Import `cn` from `@/components/lib/utils` (or the relative `../lib/utils`) instead of the ZCode `@/components/lib/utils.js` specifier — same function, new location.
   - `spinner.tsx`: drop the `useZCodeIntl` import (ZCode-internal i18n) — the `aria-label` becomes the literal `"Loading..."` (the i18n id `common.loading`'s en-US value, ellipsis included).
   - Everything else verbatim, including `button.tsx`'s `buttonVariants` cva export, `select.tsx`'s lucide icons, and `dialog.tsx`/`alert-dialog.tsx`'s `Button`/`buttonVariants` imports (adjust the specifier to the new location).

**Steps:**
- [ ] Write `src/lib/theme.test.ts`: `applyThemeToDocument("zai-dark")` adds `theme-zai-dark` AND `dark` to `document.documentElement.classList` and removes `theme-zai-light`; calling with `"zai-light"` flips all three (removes `dark`). (jsdom provides `document`.)
- [ ] Write `src/components/lib/utils.test.ts`: `cn("text-ui-base", "text-foreground")` keeps **both** classes (the `text-ui-*` font-size classes must not be merged away by tailwind-merge); `cn("bg-card", "bg-surface")` keeps only the last (normal tailwind-merge behavior still works).
- [ ] Write `src/components/ui/button.test.tsx`: render `<Button>Go</Button>` (default variant) and assert the rendered `button` has the `bg-primary` class; render `<Button variant="ghost">G</Button>` and assert it has the ghost variant's classes (read the exact class strings from the ported `button.tsx` `buttonVariants`); render `<Button size="icon">` and assert it is square (`size-*` class present).
- [ ] Run `pnpm test` — the three new test files fail (modules don't exist yet). Confirm the failure is the missing-module error, not a test-logic error.
- [ ] Implement the files per "What to implement" (1–7).
- [ ] Run `pnpm test` — all tests pass (existing suite included; this task changes no existing component, so existing tests must stay green).
- [ ] Run `pnpm build` — tsc type-check + vite build succeed (proves the `@` alias resolves in both tsconfig and vite, and the `@theme` block compiles).
- [ ] Commit with message: "feat: port the ZCode design system (tokens, theme, primitives)"

**Acceptance criteria:**
- [ ] `pnpm test` and `pnpm build` are green.
- [ ] `document.documentElement` carries `theme-zai-dark` after boot (verifiable in `pnpm dev` — the app renders with the Zai Dark palette: `#161616` background, `#2b2b2b` cards).
- [ ] All 16 primitives exist under `src/components/ui/` and import only React, `radix-ui`, `class-variance-authority`, `lucide-react`, the local `cn`, and — for `dialog.tsx`/`alert-dialog.tsx` only — `./button` (they import `Button`/`buttonVariants`) — no ZCode-internal imports (`@zcode/*`, i18n, telemetry, stores).
- [ ] `src/index.css` contains the verbatim `@theme` (137–306) / `.dark` (308–459) / `.theme-zai-light` / `.theme-zai-dark` blocks, the `tw-animate-css` + `shadcn/tailwind.css` imports, and the scrollbar rules — and none of the dropped ZCode-internal rules (no `@source`, no `@plugin`, no xterm/katex/scrollbar-hide imports, no gradient utilities, no feature keyframes).

---

### Task 2: Lib utilities — `parseDiffStats`, spin quips, `useSpinQuip`

**Context:**
The file-summary card (Task 6) needs per-file added/removed counts from unified-diff patches, and the working indicator (Task 6) needs the quip text. Both are pure logic that must exist and be tested before the components that consume them. The quips come from the pi-archimedes core verbatim — the same text the TUI shows, so the desktop and TUI read identically.

**Files:**
- Modify: `src/lib/diff.ts`
- Create: `src/lib/diff.test.ts`
- Create: `src/lib/spin-quips.ts`
- Create: `src/lib/spin-quips.test.ts`
- Create: `src/hooks/useSpinQuip.ts`
- Create: `src/hooks/useSpinQuip.test.ts`

**What to implement:**

1. **`src/lib/diff.ts`** — add (keep the existing `unifiedPatch` untouched):
   ```ts
   export interface DiffStats { additions: number; deletions: number; }
   /** Count added/removed lines in a unified-diff patch. Lines starting with "+" count as additions and "−"/"-" as deletions, EXCEPT the 3-char file headers ("+++"/"---") which are metadata, not content. Malformed input (no valid lines) → { additions: 0, deletions: 0 }. */
   export function parseDiffStats(patch: string): DiffStats
   ```
   Edge cases: multi-hunk patches (sum across all hunks); a patch with only context lines → 0/0; a line like `+---foo` (a real added line that itself starts with `---`) counts as an addition (the 3-char-header exclusion applies only to the exact `+++ `/`--- ` file-header forms — i.e. `+++ ` / `--- ` with a following path, or simply: skip exactly the lines matching `^(\+\+\+|---)\s` since content lines are `^(\+|-)` — document the rule you implement in the JSDoc); empty string → 0/0.
2. **`src/lib/spin-quips.ts`** — port VERBATIM from `/home/daniel/Coding/Javascript/pi-archimedes/packages/ui/src/editor/spin-quips.ts`: the `SPIN_QUIPS` frozen array (all entries, in order), `QUIP_ROTATION_MIN_SECS` (15), `QUIP_ROTATION_MAX_SECS` (45), and `pickQuip(exclude?, rand = Math.random)` with its documented no-repeat semantics (first draw; if it equals `exclude`, exactly one re-roll restricted to entries ≠ `exclude`). Do not "improve" the quips.
3. **`src/hooks/useSpinQuip.ts`** — new hook:
   ```ts
   export function useSpinQuip(working: boolean, deps?: { rand?: () => number }): string
   ```
   Behavior: when `working` transitions false→true, pick a fresh quip via `pickQuip(lastQuip, rand)` — `lastQuip` is the PREVIOUS episode's quip (per `pickQuip`'s documented semantics, the previous episode's quip is never returned back-to-back; `undefined` for the first episode) — and schedule a re-pick via `pickQuip(current, rand)` at a random delay drawn inclusive between `QUIP_ROTATION_MIN_SECS` and `QUIP_ROTATION_MAX_SECS` seconds: `delay = MIN + rand() * (MAX - MIN)` (so `rand: () => 0` → exactly 15s). While the episode continues the quip re-picks at that window; when `working` goes true→false, clear the timer and keep the last quip (the indicator is hidden while idle, so no reset-to-default is needed). Returns the current quip. Before the first `working` episode, return `pickQuip(undefined, rand)` at mount (a quip is always available — the test's "initial `working=false` returns some quip" case; no timer is scheduled until the first false→true transition). Implement the timer with `window.setTimeout`/`clearTimeout` inside a `useEffect` keyed on `working` so the test can drive it with vitest fake timers. (No clock injection is needed — the delay is a fixed function of `rand`, and the test advances fake time.)

**Steps:**
- [ ] Create `src/lib/diff.test.ts` with (a) failing cases for `parseDiffStats`: a single-hunk patch with 2 additions + 1 deletion → `{ additions: 2, deletions: 1 }`; a multi-hunk patch (sum across hunks); a `+++ b/file`/`--- a/file` header pair → 0/0 (headers are not content); a real added line whose content starts with `---` (e.g. `+--- separator`) → counts as an addition; context-only patch → 0/0; `""` → 0/0; AND (b) `unifiedPatch` regression cases (NO existing test covers it — verified: the current suite has zero `diff.ts` coverage): `unifiedPatch(path, oldText, newText)` emits exactly ONE file's patch per call (verified in `src/lib/diff.ts`) — so call it twice: a NEW file (`oldText: null` — the JSDoc: "`oldText` is `null` for new files"; an empty `""` would yield `--- a<path>`) → assert the `--- /dev/null` / `+++ b…` headers and the `+` content lines; a MODIFIED file → assert the `--- a…` / `+++ b…` headers and the `+`/`-` content lines.
- [ ] Write `src/lib/spin-quips.test.ts`: `SPIN_QUIPS` has 20–60 entries, all 1–64 printable-ASCII chars, all unique, and `"Working..."` is the first entry (mirrors the core's invariants); `pickQuip(undefined, () => 0)` returns `SPIN_QUIPS[0]`; `pickQuip("Working...", () => 0)` returns the second entry (the re-roll excludes the first); `pickQuip` with a `rand` that always returns a value mapping to an entry ≠ `exclude` returns that entry (no re-roll).
- [ ] Write `src/hooks/useSpinQuip.test.ts` (vitest fake timers + `@testing-library/react`'s `renderHook`): initial `working=false` returns some quip and schedules no timer; `working=true` picks a quip and, with `rand: () => 0` (delay = exactly 15s), advancing `QUIP_ROTATION_MIN_SECS - 1s` (14s) of fake time does NOT change the quip while advancing to exactly 15s DOES (the inclusive boundary); advancing far past (46s) changes it again to a different entry; `working` false again clears the timer (advancing time changes nothing); a second `working` episode does not return the first episode's quip (the `exclude` semantics).
- [ ] Run `pnpm test` — the new cases fail (function/hook don't exist). Confirm the failure is missing-module/missing-export.
- [ ] Implement 1–3.
- [ ] Run `pnpm test` — all pass, including the new `unifiedPatch` regression cases in `diff.test.ts` (they pass trivially since `unifiedPatch` is unchanged — the point is they exist as a regression guard for the file).
- [ ] Run `pnpm build` — green.
- [ ] Commit with message: "feat: add parseDiffStats, spin quips, and useSpinQuip"

**Acceptance criteria:**
- [ ] `parseDiffStats` passes all the listed edge cases; `unifiedPatch` behavior is unchanged.
- [ ] `SPIN_QUIPS` is byte-identical to the core's file (diff the two arrays).
- [ ] `useSpinQuip` re-picks at the exact inclusive 15s boundary when `rand: () => 0` (14s → no change, 15s → change), never earlier; back-to-back episodes never repeat a quip.

---

### Task 3: `BrailleLoader` (shadcn registry component)

**Context:**
The working indicator's spinner is the shadcn registry `braille-loader` (25 variants, built-in a11y + reduced-motion handling). It is vendored from the registry JSON (the deterministic path — the `npx shadcn@latest add <url>` CLI resolves the `@ui`/`@lib` targets from a `components.json` that this repo does not have; the CLI is an optional later convenience once a `components.json` exists). The only local adaptation is the mono font stack. This task lands it and proves the frame engine is deterministic and reduced-motion-safe.

**Files:**
- Create: `src/components/ui/braille-loader.tsx` (vendored from the registry JSON)
- Create: `src/lib/braille-loader.ts` (vendored from the registry JSON)
- Create: `src/components/ui/braille-loader.test.tsx`

**What to implement:**

1. Vendor the registry's two files into the exact paths (deterministic — do NOT run the interactive CLI): fetch `https://shadcn-braille-loader.vercel.app/r/braille-loader.json`; take `files[0].content` → `src/components/ui/braille-loader.tsx` and `files[1].content` → `src/lib/braille-loader.ts` (the registry's `target` fields: `@ui/braille-loader.tsx` / `@lib/braille-loader.ts` — the `@` alias from Task 1 makes the component's `import … from "@/lib/braille-loader"` resolve).
2. Apply exactly one adaptation to `src/components/ui/braille-loader.tsx`: the inner `<span>`'s inline `fontFamily: "monospace"` → `fontFamily: "var(--font-mono, monospace)"` (the design system's mono stack; the `--font-mono` token exists from Task 1). Everything else — `variant`/`speed`/`label`/`fontSize` props, `role="status"` + `aria-live="polite"` + sr-only label, the `usePrefersReducedMotion` hook, the `setInterval` frame loop — stays verbatim.
3. Do NOT modify `src/lib/braille-loader.ts` (registry lib, verbatim: `brailleLoaderVariants` (25 names incl. `typing`), `VARIANT_CONFIGS`, `generateFrames` (with its `frameCache`), `getVariantGridSize`, `normalizeVariant` — unknown/absent variant → `"breathe"`).

**Steps:**
- [ ] Write `src/components/ui/braille-loader.test.tsx` (vitest fake timers):
  - **Determinism:** `const [w, h] = getVariantGridSize("typing"); generateFrames("typing", w, h)` (import both from `@/lib/braille-loader`; the real signature is `generateFrames(variant: string, width: number, height: number)` — pass the grid size explicitly) returns the same `frames` array content on repeated calls (the `frameCache`), and `frames[0]` for `typing` matches `new RegExp('^[\\u2800-\\u28ff]{' + w + '}$')` — braille codepoints U+2800–U+28FF (including the blank U+2800), **never ASCII spaces** (the field buffer maps mask 0 → `\u2800`, not `" "`).
  - **Animation:** render `<BrailleLoader variant="typing" speed="fast" />`, capture the visible span's `textContent` at mount (frame 0), `vi.advanceTimersByTime` by `generateFrames("typing", …).interval * 0.6` (the `speedMultiplier` map is module-private in the component — use the literal `0.6`; `typing`'s interval is 50ms → 30ms tick), and assert the text changed to `frames[1]`.
  - **Reduced motion:** mock `window.matchMedia` with a FULL stub — the component's `usePrefersReducedMotion` calls `mediaQuery.addEventListener("change", …)`/`removeEventListener`, so a bare `{ matches: true }` throws: `vi.spyOn(window, "matchMedia").mockImplementation((q) => ({ matches: q === "(prefers-reduced-motion: reduce)", media: q, addEventListener: vi.fn(), removeEventListener: vi.fn(), addListener: vi.fn(), removeListener: vi.fn(), dispatchEvent: vi.fn() }))`; render; advance timers; assert the text stays `frames[0]` (static).
  - **Normalization:** `normalizeVariant("bogus")` → `"breathe"`; `normalizeVariant("typing")` → `"typing"`; `normalizeVariant(undefined)` → `"breathe"`.
- [ ] Run `pnpm test` — fails (component not installed yet).
- [ ] Vendor the files (step 1) + adapt (step 2).
- [ ] Run `pnpm test` — all pass.
- [ ] Run `pnpm build` — green (the `@/lib/braille-loader` import resolves via the Task 1 alias).
- [ ] Commit with message: "feat: add the shadcn braille-loader (mono-stack adaptation)"

**Acceptance criteria:**
- [ ] The component + lib exist at the exact paths with only the single `fontFamily` adaptation (diff against the registry JSON content).
- [ ] All four test groups pass; the component's a11y attributes (`role="status"`, `aria-live`, sr-only label) are present in the rendered DOM.

---

### Task 4: App shell + tabbed side pane

**Context:**
The app frame becomes three panes and the two right-rail panels move into a new tabbed `SidePane` at the `App` level. This is the structural heart of the redesign: `ChatStream` shrinks to the conversation frame, and `TodoBoardPanel`/`SubagentPanel` are restyled to the approved design and hosted by `SidePane`. Stores and Tauri wiring are untouched — only components and their tests move/change.

**Files:**
- Modify: `src/App.tsx`
- Create: `src/components/SidePane.tsx`
- Create: `src/components/SidePane.test.tsx`
- Create: `src/lib/sidePaneState.ts`
- Create: `src/lib/sidePaneState.test.ts`
- Modify: `src/components/TodoBoardPanel.tsx`
- Create: `src/components/TodoBoardPanel.test.tsx`
- Modify: `src/components/SubagentPanel.tsx`
- Modify: `src/components/SubagentPanel.test.tsx`
- Modify: `src/components/ChatStream.tsx` (remove the `TodoBoardPanel`/`SubagentPanel` imports + renders + the flex-row wrapper — the panels are hosted by `SidePane` here; this is what the acceptance criterion below requires)

**What to implement:**

1. **`src/App.tsx`** — replace the `<div className="flex h-screen overflow-hidden bg-neutral-950 text-neutral-100"><SpacesList /><ChatStream /></div>` body with:
   ```tsx
   <div className="flex h-screen w-screen bg-background text-foreground">
     <SpacesList />
     <ChatStream />
     <SidePane />
   </div>
   ```
   (All the existing `useEffect` event-listener blocks stay verbatim — do not touch them.)
2. **`src/components/SidePane.tsx`** — new component, no props (it reads the stores itself):
   - Frame: `bg-background-alt rounded-xl m-1` (the 4px inset/gap per the design reference), `flex flex-col`, 320px default width.
   - **Resizable + collapsible:** a `useState` width (default 320, persisted to `localStorage` key `"side-pane-width"` on change — write-through, no debounce) and a `collapsed` boolean (persisted to `"side-pane-collapse"`). **Collapse mechanism: the frame's `width: 0` + `overflow: hidden` — NOT `display: none`, NOT a transform, NOT unmount** (the content stays mounted and `fixed` overlays escape `overflow` clipping, so a collapsed pane never hides a pending `SudoConfirmModal`/`SudoPasswordModal` — the invariant the panel's doc comment requires). A 4px drag handle on the frame's left edge: a `w-1` (4px) `cursor-col-resize` div with `onMouseDown` starting a drag (track `mousemove`/`mouseup` on `window` in a `useEffect`; new width = start width − delta, clamped 240–480px; while dragging, show a 2px `bg-foreground-subtlest/50` line — implement as the handle's `hover:`/drag-state styling; the hit area stays transparent otherwise). The collapsed flag is shared with the header toggle (Task 6) via **`src/lib/sidePaneState.ts`** (below): `SidePane` hydrates `collapsed` from `localStorage` on mount by calling `setSidePaneCollapsed` (a harmless same-value echo — the module's own write is the single persistence owner; SidePane has no collapse control of its own, the Task 6 header toggle is the only one).
   - **`src/lib/sidePaneState.ts`** — a tiny shared module (no store, no React): `getSidePaneCollapsed(): boolean`, `setSidePaneCollapsed(v: boolean): void` (notifies subscribers AND writes `localStorage["side-pane-collapse"]` itself — the single owner of persistence, so the Task 6 header toggle's `setSidePaneCollapsed(!collapsed)` reaches localStorage with no extra step), `subscribeSidePane(fn: () => void): () => void`. Task 6's header toggle consumes it via `useSyncExternalStore(subscribeSidePane, getSidePaneCollapsed)` — so the toggle (a different component tree) both reads the state for its pressed visual and sets it on click; no event guessing. (Test: the three functions round-trip; a subscriber fires on `setSidePaneCollapsed`.)
   - **Tab bar** (`h-9 px-2`): a `Tabs` primitive (Task 1) with two `TabsTrigger`s — "Todos" and "Subagents" — `text-ui-sm`, `rounded-md`, active `bg-selected`; the selected tab in `useState` (persisted to `localStorage` key `"side-pane-tab"`, default `"todos"`). Each trigger shows a **count badge** when its tab has content and is not the active tab: Todos badge = the resolved main-column todo count for the active session — **derived via the SAME extraction `TodoBoardPanel` uses** (below), so the badge and the rendered list can never disagree; Subagents badge = the subagent entry count.
   - **Content**: `flex-1 overflow-y-auto p-3` — renders `<TodoBoardPanel sessionId={activeSessionId} />` (tab `todos`; it takes the `sessionId` prop, read via `useSessions((s) => s.activeSessionId)`) or `<SubagentPanel />` (tab `subagents`; it takes **no props** — it reads its stores globally, exactly as today).
   - **`TodoBoardPanel` extraction** (enables the badge): the panel's main-column derivation — today `mainItems = column?.main ?? rawTodos ?? []` where `rawTodos` is the non-bridge `manage_todo_list` `rawInput` fallback (a non-trivial scan of the session's messages) — is extracted as an exported `useMainTodoItems(sessionId): TodoItem[]` hook in `TodoBoardPanel.tsx` (or `src/lib/useMainTodoItems.ts`); the panel's checklist AND the `SidePane` badge both consume it.
3. **`src/components/TodoBoardPanel.tsx`** — restyle to the Todos-tab treatment in the design reference: progress header (`N/M` `text-ui-base font-medium` + `progress` bar (Task 1) primary fill, rendered only when M > 0); `h-8` checklist rows `rounded-md hover:bg-surface-hover` with the three-state indicators (done = a `size-4` circle with a check icon `text-success` + label `text-foreground-subtle`; in-progress = `◉` `text-warning`; pending = `○` `text-foreground-subtlest`; label `text-ui-base`); subagent todo columns as indented sub-rows (`pl-6`, `text-ui-sm`, one block per subagent source, header = the source label `text-ui-xs text-foreground-subtlest`); empty state "No todos yet" (`text-ui-sm text-foreground-subtlest`, centered). The component's data logic (which store it reads, how it derives the columns) is unchanged — presentation only.
4. **`src/components/SubagentPanel.tsx`** — restyle to the Subagents-tab treatment, **preserving every existing behavior** (the panel is the ONLY render site for subagent-session `confirm`/`password`/`ask`/permission requests — `ChatStream` renders those only for the ACTIVE session, and a subagent's session id is never active; an unrendered request would hang until the bridge timeout. The existing panel doc comment says exactly this — keep it): one card per subagent session (`rounded-xl border-card-border bg-card`, `gap-2` stack, replacing today's `rounded-md border-neutral-800 bg-neutral-900/40` sections):
   - **Header row** (all four existing elements, restyled): agent name (`text-ui-base font-medium`, truncated) + the bridge `agentState` chip (restyled to token colors: `working` → `text-warning` on a `bg-warning/14` pill, `idle` → `text-foreground-subtlest`, `blocked` → `text-foreground-subtle`) + the session `status` chip (a `badge`-style span: `running` → `text-warning`, `completed` → `text-success`, `failed` → `text-destructive`) + the pending-permission "N awaiting" badge (restyled to the green confirmation treatment — `bg-interaction-confirmation-surface text-interaction-confirmation-foreground`, per DESIGN.md's unified waiting-badge rule) + the dismiss button (icon button `size-6`, an `X` icon replacing the text `✕`, `text-foreground-subtlest hover:text-foreground`, the existing `dismiss` handler).
   - **Compact stream** (unchanged behavior, restyled container): the existing condensed message rendering (`MessageBubble` for `agent-text`, `ToolCallCard` collapsed-by-default for `tool-call`, `DiffBlock` for `diff`) in a `max-h-48 overflow-y-auto rounded-md bg-surface p-2` block (replacing `bg-neutral-950`; the components themselves are restyled in Task 6 — here they render as-is from their current state, and Task 6's restyle flows through automatically).
   - **Pending requests** (unchanged behavior): the subagent's `PermissionPrompt`s, `AskQuestionCard`s, and the `SudoConfirmModal`/`SudoPasswordModal` (rendered at panel level, outside the scrollable content — a collapsed/hidden pane never hides a pending modal) all keep rendering exactly as today; only their own component styling changes (Task 6).
   - **Metrics/error line** (unchanged behavior, restyled): the `subagent-closed` SNAPSHOT line (error in `text-destructive`, metrics joined with `·` in `text-ui-xs text-foreground-subtlest` — NOT `useBridge.cost`, per the existing comment).
   - **Empty state**: "No subagent sessions" (`text-ui-sm text-foreground-subtlest`, centered) — NEW; the one existing test asserting "renders nothing when there are no entries" is the ONLY assertion rewritten (to assert the empty-state text; it is a presentation assertion, not a data-logic one). All other existing `SubagentPanel.test.tsx` assertions (confirm/password/ask rendering, compact stream, dismiss, state chip, "1 awaiting" badge, metrics line) are KEPT.
   - **Dropped** (superseded by the new frame model): the old rail collapse UI (the "Expand subagents" slim bar) and the auto-expand behavior (0 → >0 entries, and 0 → >0 pending-prompt) — the `SidePane` frame owns collapse now (via `sidePaneState`), and the count badge on the tab trigger covers discoverability. **Consequently the existing `SubagentPanel.test.tsx` needs mechanical rewrites — NOT "keep everything unmodified": ~8 of its 11 tests change mechanically (every one of the six "Expand subagents" click sites is removed), and NO data-logic assertion changes**: (a) "renders nothing when there are no entries" (`expect(container.firstChild).toBeNull()`) → assert the "No subagent sessions" empty state instead; (b) "auto-expands when the first entry arrives" — RETIRE the test body (auto-expand is dropped behavior) — keep only the "a new entry's section renders" assertion; (c) the **six** tests that click `screen.getByRole("button", { name: "Expand subagents" })` — "renders a header…", "renders a closed entry's metrics line…", "renders a failed entry's error", "renders an `ask` request for the subagent session id as an AskQuestionCard in the section", "renders the compact stream (agent-text) for the subagent session", and "dismisses an entry via the header's dismiss button" — DELETE the Expand click line in each (sections always render in the new model; the button no longer exists; the dismiss test KEEPS its `screen.getByRole("button", { name: "Dismiss reviewer" })` click — only the Expand click goes); (d) "auto-expands on a pending permission prompt" — DELETE the collapsed pre-assertion (`expect(screen.queryByText("reviewer")).toBeNull()` before the `addPrompt`) and the auto-expand framing; keep the "1 awaiting" badge assertion. Every other assertion (the confirm/password/ask rendering, the compact stream, the dismiss button, the state chip, the metrics line surviving `dismissSession`) stays byte-for-byte intact — and the true count is **~8 of 11 tests change mechanically** (the (a) rewrite, the (b) retire, the (d) pre-assertion deletion, and the six (c) click deletions); NO data-logic assertion changes.

**Steps:**
- [ ] Write `src/lib/sidePaneState.test.ts` FIRST: `getSidePaneCollapsed`/`setSidePaneCollapsed`/`subscribeSidePane` round-trip (a subscriber fires when the flag flips; unsubscribing stops it).
- [ ] Create `src/components/TodoBoardPanel.test.tsx` (NO existing test file exists — verified) with the new-presentation cases: assert the progress header renders `N/M` for a fixture column with 1 done / 3 total and the `progress` indicator's `width` style parses to ≈ 33.33 (assert `Math.abs(parseFloat(indicatorEl.style.width) - 100 / 3) < 0.01` — the ported `progress.tsx` normalizes `value` to 0–100 and applies it ONLY to the indicator's `width` style, and there is no `@testing-library/jest-dom` in the deps for a `toHaveStyle` helper; pass the percent value, not a fraction); assert the three indicator renderings (check icon + `text-success` for done, `◉` + `text-warning` for in-progress, `○` + `text-foreground-subtlest` for pending); assert the empty-state text "No todos yet"; assert the subagent-column indented sub-rows render for a fixture `subagents` column; assert `useMainTodoItems` returns the `rawInput` fallback when the bridge column is absent (the non-bridge path).
- [ ] Update `src/components/SubagentPanel.test.tsx` per the mechanical-rewrite inventory in item 4 (the four listed rewrites + retire the auto-expand test body); ADD assertions for the new presentation (card header shows the agent name and the status chip class per status — fixture a running + a failed entry; the "No subagent sessions" empty state). Every unlisted assertion stays byte-for-byte intact.
- [ ] Write `src/components/SidePane.test.tsx`: renders the two tab triggers ("Todos", "Subagents"); clicking "Subagents" swaps the content (the `SubagentPanel` empty state shows); the selected tab persists to `localStorage` (`"side-pane-tab"`); with a fixture todo column, the inactive Todos trigger shows the count badge (via the `useMainTodoItems` derivation); call `setSidePaneCollapsed(true)` AFTER `render` (the mount-time hydration from empty localStorage runs first and would clobber a pre-set flag) → the frame's width style is 0 (collapsed) and a pending subagent `password` request's `SudoPasswordModal` is STILL in the document (the `w-0 overflow-hidden` mechanism, not `display:none`/unmount); `setSidePaneCollapsed(false)` re-opens.
- [ ] Run `pnpm test` — the updated/new tests fail (the new presentation/`SidePane`/`sidePaneState` don't exist).
- [ ] Implement 1–4 (including the `TodoBoardPanel` `useMainTodoItems` extraction + the `SubagentPanel` presentation rewrite + the `sidePaneState` module).
- [ ] Run `pnpm test` — all pass.
- [ ] Run `pnpm build` — green.
- [ ] Commit with message: "feat: three-pane shell with a tabbed side pane (Todos/Subagents)"

**Acceptance criteria:**
- [ ] The app renders three panes; `ChatStream` no longer renders `TodoBoardPanel`/`SubagentPanel` (their imports are removed from it in THIS task — the panels are already hosted by `SidePane` here, so removing them from `ChatStream` now avoids double-rendering; `ChatStream`'s header rework is Task 6).
- [ ] Pane width persists across a re-mount (localStorage); collapse/re-open works via `sidePaneState` (the shared module Task 6's toggle consumes); tab selection persists; a collapsed pane never hides a pending subagent modal.
- [ ] Both restyled panels pass their suites: `TodoBoardPanel.test.tsx` (new) fully green; `SubagentPanel.test.tsx` green with the ~5 mechanical rewrites applied and every data-logic assertion intact. No store or Tauri file was modified.

---

### Task 5: Left sidebar (New Session / Open Space / grouped Sessions)

**Context:**
The sidebar is the navigation spine: the two action buttons, the Space groups, and the Session rows with their status/attention language. All data comes from the existing `useSessions` store via `spaceViewFor` — no store changes.

**Files:**
- Modify: `src/components/SpacesList.tsx`
- Create: `src/components/SpacesList.test.tsx` (verified absent — create it)
- Modify: `src/components/NewSpaceDialog.tsx` (presentation only)
- Create: `src/hooks/useStartNewConversation.ts`
- Create: `src/hooks/useStartNewConversation.test.ts`
- Modify: `src/components/ChatStream.tsx` (swap the inline `startNewConversation` for the hook — presentation-neutral, keeps the build green)

**What to implement:**

1. **`src/components/SpacesList.tsx`** — rebuild as the sidebar (fixed 260px, `bg-sidebar`, `flex flex-col`):
   - **Action cluster** (`p-3`, `border-b border-border/50`): two full-width buttons in the design-reference shape (`h-8 rounded-lg`, icon `size-4` + label `text-ui-base` + right `kbd` hint `text-ui-xs text-foreground-subtlest`, `hover:bg-surface-hover`): **New Session** (`MessageCirclePlus`, hint `⌘N`) and **Open Space** (`FolderOpen`, hint `⌘O`).
     - New Session behavior: the `useStartNewConversation` hook (below) called with **the view owning `activeSessionId`** (the same `view` `ChatStream` computes — the active session's Space, matched by live or stored membership); when `activeSessionId === null` (no view), the button opens the Open Space dialog instead. Register a `⌘N`/`Ctrl+N` keydown listener (a `useEffect` on `window`, matching the key combo, invoking the same handler) — and `⌘O`/`Ctrl+O` for Open Space.
     - Open Space behavior: opens the existing `NewSpaceDialog` (its logic unchanged).
   - **"Sessions" section header** (`px-2.5 py-2`, `text-ui-base text-foreground-subtlest`).
   - **Space groups**: for each `spaceViewFor` view — a collapsible group (a `collapsible`-style header: folder icon `size-4` + base name `text-ui-base text-foreground-subtlest`, `hover:text-foreground-subtle`, a chevron that toggles the group's visibility in local `useState` (default open), and a `+` icon button (`hover:bg-surface-hover rounded-md size-6`) that starts a Session in that Space (the same hook, pinned to that view's path)).
   - **Session rows** (the group's live session row — `view.liveSessionId`, when non-null — + one row per `view.storedSessionIds` entry, live first — the existing `SpaceView` order; note `SpaceView` exposes a SINGLE `liveSessionId` (the most-recently-started live session), so this is not "every live session" — it matches today's Live/`#1`/`#2` selector): `rounded-lg pl-2.5 pr-1 py-1`, `cursor-pointer`, active (the `activeSessionId`) → `bg-selected`, hover → `bg-surface-hover`; leading 16px slot: live + `inTurn` → circular `LoaderIcon size-4 animate-spin text-foreground-subtle` (the `spinner` primitive), else an empty `size-4` placeholder (keeps alignment); title `text-ui-base text-foreground` truncated with a **gradient-fade mask** — apply `mask-image: linear-gradient(to right, black calc(100% - 1.5rem), transparent)` (plus the `-webkit-` prefix) to the title element ITSELF (the ZCode `TaskTitleOverflowText` technique — a mask on the text is background-agnostic, so it works over `bg-sidebar`, `bg-selected`, and `bg-surface-hover` alike; do NOT use a `bg-gradient-*` overlay div, which would mismatch the row's background; the title text is NOT `truncate`-ellipsized — it overflows under the fade; skip ZCode's hover-marquee — YAGNI); right side: relative time of the session's last activity (`text-ui-sm text-foreground-subtle`) — **derive it ONLY from the latest message's `at` when the session's messages are loaded this boot (`useSessions.messages[sessionId]`); `SessionInfo` has NO `createdAt` field (the store's ordering note says so explicitly) and stored sessions not opened this boot have no loaded messages → those rows render an empty slot (no time text)**; format: `<60s` → `now`, `<60m` → `Nm`, else `Nh`; **attention badge**: when the session has a pending permission prompt (`usePermissions`) OR a pending bridge `ask`/`confirm`/`password` request (`useBridge` — `password` included: a pending sudo password shows no "Waiting" cue anywhere else), render a green "Waiting" pill (`rounded-full bg-success/14 px-2 text-ui-sm font-medium text-success`) in place of the time; hover action (live sessions only): a Pause icon button (revealed on row hover, `hover:bg-surface-hover rounded-md size-6`, the existing `closeSession` path) replacing the time/badge slot.
   - Row click = the existing `openSession` (unchanged).
   - **Remove** the old sidebar presentation entirely (the current list markup in `SpacesList.tsx`); the component name/exports stay.
2. **`src/hooks/useStartNewConversation.ts`** — new hook (the extracted logic, verbatim semantics):
   ```ts
   export function useStartNewConversation(view: SpaceView | undefined): {
     startNewConversation: () => Promise<void>;
     error: string | null;
     clearError: () => void;
   }
   ```
   Body = today's `ChatStream` logic moved in, as **store lookups** (the `SpaceView` type is `{ path, title, liveSessionId: string | null, storedSessionIds: string[], lastReason }` — it has NO `liveSession` object and NO `storedMostRecent` field; both are derived, exactly as `ChatStream` does today): `const live = useSessions((s) => s.sessions).find((s) => s.sessionId === view?.liveSessionId)`, `const storedMostRecent = useSessions((s) => s.historySessions).find((s) => s.sessionId === view?.storedSessionIds[0])`, then `agentId = live?.agentId ?? storedMostRecent?.agentId ?? firstAgentId` (the `agents` list fetched here via `listAgents` in a `useEffect`, `firstAgentId = agents?.[0]?.id ?? ""`). `startNewConversation` = `startSession(agentId, view.path)` → `addSession(info)` + `addSpace(info.cwd)` on success, error capture on failure; **`view === undefined` → no-op, and `agentId === ""` → no-op** (today's `ChatStream` guard includes `newConversationAgentId === ""` — the `view`-based guard — intentional: the sidebar's `+` buttons pass a group's view, and the top button passes `undefined` when no session is active, in which case the button opens the Open Space dialog instead). `ChatStream` calls it with the active session's `view`; the sidebar calls it with the view owning `activeSessionId` (top button) or a group's view (`+` buttons).
3. **`src/components/NewSpaceDialog.tsx`** — restyle to the `dialog` primitive (Task 1): `rounded-2xl` shell, `bg-popover border-popover-border`, the folder-picker form with `input`/`button` primitives (`text-ui-base`), destructive/primary per the design. Logic unchanged — including the dynamic title, whose actual condition is the `spaceForPath(cwd)` check's `isSpace` (the folder is an **existing space**), NOT "cwd is set": `title = existingSpace ? \`New conversation in ${titleBase !== "" ? titleBase : cwd}\` : "New space"` (`titleBase = basenameOfPath(cwd)`). (The test asserts the literal `"New space"` for a fresh folder.)

**Steps:**
- [ ] Write `src/hooks/useStartNewConversation.test.ts` FIRST (TDD for the extraction): seed `useSessions` with `useSessions.setState({ spaces: […], sessions: […], historySessions: […], activeSessionId: "s1" })` — **NOT** `getState().setSpaces(…)` (that triggers boot auto-selection + `openSession` → unmocked `loadHistory` IPC); mock Tauri with the pattern every existing component test uses: `vi.mock("../lib/tauri", async () => { const actual = await vi.importActual("../lib/tauri"); return { ...actual, startSession: vi.fn().mockResolvedValue({ sessionId: "new1", agentId: "pi", cwd: "/tmp/ws", capabilities: { loadSession: false } }), listAgents: vi.fn().mockResolvedValue([{ id: "pi", name: "pi" }]), respondPermission: vi.fn(), respondBridgeRequest: vi.fn(), loadHistory: vi.fn().mockResolvedValue([]) }; })`. Assert: with a fixture view (a `spaceViewFor`-shaped object, or the output of `spaceViewFor` for fixture `spaces` rows, whose `liveSessionId` matches a fixture `sessions` entry with `agentId: "a-live"` and whose `storedSessionIds[0]` matches a fixture `historySessions` entry with `agentId: "a-stored"`), `startNewConversation` calls `startSession` with `"a-live"` (the fallback chain's first hit) and then `addSession`/`addSpace`; with a view whose `liveSessionId` is `null`, it uses the stored session's `agentId` ("a-stored"); with no agents loaded, it no-ops (empty `agentId`); with `view === undefined`, it no-ops.
- [ ] Write `src/components/SpacesList.test.tsx` (jsdom + testing-library; same `setState` seeding + tauri mock as above):
  - The two action buttons render with their labels + `kbd` hints; clicking "Open Space" opens the dialog (an element with the title `"New space"` is in the document).
  - A Space group renders its folder icon + base name; clicking the chevron hides its rows.
  - A live session row (fixture `view.liveSessionId` set) renders its title and a `LoaderIcon` (spinner) when `inTurn` is true for that session; a stored row renders no spinner; a row with no loaded messages renders an empty time slot (no time text).
  - A session with a pending permission prompt (seed `usePermissions` via `setState`) renders the "Waiting" pill (and NOT the relative time).
  - Clicking a row calls `openSession` for that id (assert via the store's `activeSessionId` change — seed it via `setState` and assert the component's selection styling, since `openSession`'s IPC side-effect is mocked).
  - `⌘N`/`⌘O` shortcuts: `dispatchEvent(new KeyboardEvent("keydown", { key: "n", metaKey: true }))` on `window` triggers the New Session handler (and `"o"` the Open Space dialog).
- [ ] Run `pnpm test` — the new cases fail (the hook/markup don't exist yet).
- [ ] Implement 1–3 (the hook, the sidebar, the dialog restyle) and update `ChatStream` to consume the hook (remove the inline function + orphaned locals + `listAgents` effect; the header button stays, working via the hook — its removal is Task 6).
- [ ] Run `pnpm test` — all pass.
- [ ] Run `pnpm build` — green (proves the `ChatStream` hook swap compiles with `noUnusedLocals` on).
- [ ] Commit with message: "feat: rebuild the sidebar (New Session / Open Space / grouped Sessions)" — one commit including the `ChatStream` hook swap (the swap is a prerequisite for the sidebar to exist, and both are one logical change).

**Acceptance criteria:**
- [ ] All the listed behaviors pass; the `⌘N`/`⌘O` shortcuts work.
- [ ] No store/Tauri file modified; `pnpm build` is green at the task's commit (the `ChatStream` hook swap keeps it compiling — its header rework is Task 6).
- [ ] The old sidebar markup is gone (no `bg-neutral-*` classes remain in `SpacesList.tsx`).

---

### Task 6: Center pane — header, stream, working indicator

**Context:**
The conversation frame gets its ZCode header, the restyled stream (messages, tool rows, the new file-summary card, the working indicator), and the restyled prompt/ask/modals. This task removes the header's "New conversation" button (its action moved to the sidebar in Task 5, and the header's "…" menu item uses the same `useStartNewConversation` hook) and finishes the `ChatStream` panel removal flagged in Task 4. All data logic is unchanged.

**Files:**
- Modify: `src/components/ChatStream.tsx`
- Modify: `src/components/MessageBubble.tsx`
- Modify: `src/components/ToolCallCard.tsx`
- Modify: `src/components/DiffBlock.tsx`
- Create: `src/components/FileSummaryCard.tsx`
- Create: `src/components/FileSummaryCard.test.tsx`
- Modify: `src/components/PermissionPrompt.tsx`
- Modify: `src/components/PermissionPrompt.test.tsx`
- Modify: `src/components/AskQuestionCard.tsx`
- Modify: `src/components/AskQuestionCard.test.tsx`
- Modify: `src/components/SudoConfirmModal.tsx`
- Modify: `src/components/SudoPasswordModal.tsx`

**What to implement:**

1. **`src/components/ChatStream.tsx`** — restructure to the conversation frame:
   - **`!activeSessionId` early-return branch** (no active session): KEEP the branch; restyle the "No active session" hint to the design-reference empty state (centered `text-ui-base text-foreground-subtle` "No active session — open a Space from the list on the left"), and DELETE the stale wrapper comment ("Both return paths wrap in a flex row for the `TodoBoardPanel` right rail…") — the rails moved to `SidePane` in Task 4, so the frame is a single child again.
   - Frame: `flex-1 min-w-0 bg-background-alt rounded-xl m-1 flex flex-col` (the 4px inset per the design reference); confirm the `TodoBoardPanel`/`SubagentPanel` imports + renders are gone (removed in Task 4 — if any remain, remove them here).
   - **Header** (`h-12 border-b border-border/50 p-2`, `flex items-center gap-2`): session title — derive from the first `kind: "user"` message of the active session (truncate to ~80 chars, `text-ui-base font-medium`, gradient-fade mask like the sidebar rows) falling back to the Space name (the existing `spaceTitle` logic); the **Space chip** (`bg-surface rounded-lg px-2 py-0.5`, folder icon `size-3.5` + base name `text-ui-sm`) — replace the current plain `spaceTitle` paragraph; the **session selector** (existing Live/`#1`/`#2` options logic, restyled as a `select` primitive `text-ui-base`); a **"…" `dropdown-menu`** (trigger: `MoreHorizontal` icon button) with items — Pause (live only, the existing `pause` handler), Resume (stored + `canResume`, the existing `resume` handler), New Session in this Space (the `useStartNewConversation` hook from Task 5 — the menu item calls it with the active session's `view`, exactly like the sidebar's top button); a right-aligned **side-pane toggle** icon button (`PanelRight` icon) wired to the `sidePaneState` shared module (Task 4): read the collapsed flag via `useSyncExternalStore(subscribeSidePane, getSidePaneCollapsed)` — so the button's `aria-pressed` (pane open) reflects the real state — and on click call `setSidePaneCollapsed(!collapsed)` (no events, no store); the button also shows a small `bg-warning` dot when the ACTIVE session has a pending permission/`ask`/`confirm`/`password` request (a collapsed pane gives no other cue that a request is waiting — the sidebar "Waiting" badge covers only the row the user is looking at).
   - Remove the old header's "New conversation"/"Pause" buttons and the old stored-session banner: the stored/resumable states move into the "…" menu items (Pause/Resume) — but KEEP the existing `isHistoryOnly` banner's *information* as a thin strip above the header when `isHistoryOnly` (`text-ui-sm`, `bg-surface rounded-md m-2`, the existing two copy variants + the Resume button as a `button` primary — the banner is the one place Resume is discoverable without the menu).
   - **Stream** (existing scroll region, `p-4`): the existing message rendering loop stays (anchored-ask correlation, stacked asks, permission prompts, working/stop lines — all logic unchanged); the `inTurn` line becomes the **working indicator**: read `useBridge((s) => s.agentState[activeSessionId])` — the rule is **"use the bridge `agentState` entry when present, else the `inTurn` fallback"** (the `AgentEntryDto` is `{ id, name }` — it does NOT expose a `bridge` flag, so the presence of an `agentState` entry is the only signal a bridge agent has pushed; a bridge agent that hasn't pushed yet falls back to `inTurn`, which is correct — its turn IS in flight): `working` → render `<BrailleLoader variant="typing" speed="normal" fontSize={14} label="Agent working" />` (Task 3) + the quip text in `text-ui-sm text-foreground-subtle` — **call `const quip = useSpinQuip(workingOrInTurn)` UNCONDITIONALLY at the top of the component body** (the hook contains `useState`/`useEffect` — invoking it inside the `working` branch would be a conditional hook call and crash React when the state flips; the pre-first-episode return (Task 2) exists precisely so it can always be called) and render `{quip}` in the `working` branch; `blocked` → render "Waiting for your input…" (`text-ui-sm text-foreground-subtle`); `idle` and not `inTurn` → render nothing.
   - **File summary card**: at the end of the rendered stream (after the last message, before the working indicator), when the active session's messages contain standalone `kind: "diff"` messages for the most recent turn (a turn = the messages after the last `kind: "user"` message): render `<FileSummaryCard diffs={…} />` (below). **SINGLE SOURCE OF TRUTH — the standalone `diff` messages ONLY**: `applySessionUpdate`/`rowToMessages` in `sessions.ts` emit BOTH `tool-call.diff = diffs[0]` AND standalone `kind: "diff"` messages for the same extracted diffs — summing both sources would count every file (at least the first) twice, so the `tool-call.diff` refs are deliberately NOT used.
   - **Fresh session**: when the message list is empty, render the centered `text-ui-base text-foreground-subtlest` "Send a prompt to start".
   - The composer block is Task 7 — leave the existing composer markup in place untouched by this task.
2. **`src/components/FileSummaryCard.tsx`** — new:
   ```tsx
   export default function FileSummaryCard({ diffs }: { diffs: Array<{ path: string; patch: string }> })
   ```
   - **Dedupe by `path` (last wins) before computing stats** — `applySessionUpdate`/`rowToMessages` re-emit a standalone `diff` message on EVERY `tool_call_update` carrying content, so the same file can appear multiple times in a turn (an agent re-sending its diff); the summary counts each file once (its latest patch).
   - Derive per-file stats via `parseDiffStats` (Task 2) and totals (sum).
   - Card: `rounded-xl border border-border bg-card overflow-hidden`; header `h-10 flex items-center justify-between px-2 hover:bg-hover`: left = file icon (`FileCode` `size-4`) + "N files changed" (`text-ui-base font-medium`) + `+X` (`text-diff-added`) `−Y` (`text-diff-removed`, `tabular-nums`); right = nothing (no Undo — no backend).
   - Body: `border-t border-border`; one `h-8` row per file (`flex items-center gap-2 px-2`): file-type icon (a `FileCode` `size-4 text-foreground-subtle`), `font-mono text-ui-base` path (truncated), right `+a`/`−d` (`tabular-nums`, diff colors, `text-ui-base`).
3. **`src/components/MessageBubble.tsx`** — restyle per the design reference: user = plain `text-ui-base text-foreground` row (drop the bubble styling entirely); agent-text = `text-ui-base` markdown — **drop ALL `prose*` classes from the markdown wrapper** (the current `prose prose-invert prose-sm max-w-none break-words prose-p:my-2 prose-headings:my-3` requires the `@tailwindcss/typography` plugin, which is NOT added — the component mapping below replaces them) and update the `react-markdown` component mapping to the ZCode markdown scale (h1 `text-ui-base`→`text-ui-xl`, h2 `text-ui-lg`, h3/h4 `text-ui-base font-semibold`, h5 `text-ui-base font-medium`, h6 `text-ui-base font-normal`, inline code `font-mono text-ui-sm bg-markdown-inline-code rounded-sm px-1`, `pre` code blocks `rounded-lg bg-surface p-3` with the existing shiki highlighting inside at 14px mono, links `text-ui-base` with the icon-blue token, `p`/`ul`/`ol`/`table` `text-ui-base` with the 4px rhythm — `my-2`/`space-y-2` between blocks). Logic (message selection) unchanged.
4. **`src/components/ToolCallCard.tsx`** — restyle to the tool-row treatment: one `h-8` row `rounded-lg hover:bg-surface-hover` (icon `size-4` — a status-aware icon: pending/in-progress `LoaderIcon animate-spin text-foreground-subtle`, completed `CheckIcon text-foreground-subtle`, failed `XIcon text-destructive` — + label `text-ui-base`, completed label `text-foreground-subtle`, failed `text-destructive`); expandable (a `collapsible` primitive or the existing expand state) — expanded body in a `rounded-md bg-surface p-2` block (nested radius one level down) showing the diff via `DiffBlock` when `diff` is present, else a muted "(no output)" line (`text-ui-sm text-foreground-subtlest`) — the `tool-call` message has a `rawInput` field but **no raw output field**, and today's card renders only title + diff, so do not invent new output rendering; the existing anchored-ask replacement behavior is in `ChatStream` (unchanged).
5. **`src/components/DiffBlock.tsx`** — restyle: the diff lines use `--color-diff-added`/`--color-diff-removed` (the `text-diff-added`/`text-diff-removed` utilities) for added/removed lines, `font-mono` at the content font size, on a `bg-surface` block; `@@` hunk headers → `text-icon-blue` (replacing today's `text-sky-300`); the patch-parsing logic unchanged.
6. **`src/components/PermissionPrompt.tsx`** — restyle: in-stream card `rounded-xl border border-border` + the green confirmation treatment — a `bg-interaction-confirmation-surface` header strip with the tool name (`text-interaction-confirmation-foreground text-ui-base font-medium`); body: the tool name line only — **there is NO tool detail or diff preview to render** (`PermissionPromptData` = `{ requestId, toolTitle, options }`; the store discards everything but title + options, and extending the wire type is out of scope); footer: **one `button` per `prompt.options` entry** (the options are agent-defined and dynamic — e.g. `allow_once`/`reject_once`, any count: first option = `button` primary, the rest = outline) + **Cancel** (`button` outline, the existing `respond("cancelled")`). The `respond` handler (per-option `respondPermission(sessionId, requestId, { selected: { option_id } })` + `removePrompt`, busy/error states) is unchanged.
7. **`src/components/AskQuestionCard.tsx`** — restyle: the `--color-interaction-ask-*` treatment — card `rounded-xl border` with a `bg-interaction-ask-surface` tint header (question text `text-interaction-ask-foreground`), option list with the existing radio semantics (selected option `bg-interaction-ask-fill`), footer hint `text-ui-xs`. The existing anchor/stacking/correlation logic and the `respond` handler (local `respond(cancelled)` → `respondBridgeRequest(sessionId, requestId, payload)` + `removeRequest`) unchanged — do not rename or re-wrap it.
8. **`src/components/SudoConfirmModal.tsx`** / **`SudoPasswordModal.tsx`** — restyle to the `dialog`/`alert-dialog` primitives (Task 1): `rounded-2xl` shell, `bg-popover border-popover-border`, `text-ui-base` content, the existing masked-password input as the `input` primitive; logic unchanged.

**Steps:**
- [ ] Write `src/components/FileSummaryCard.test.tsx`: a fixture of 2 files (one `+2 −1`, one `+0 −3`) → header reads "2 files changed" with `+2` and `−4`; each file row shows its path + stats; a single-file patch renders one row; a `diff` list with a malformed patch (no `+`/`-` lines) renders `+0 −0` for that file (no crash).
- [ ] Update `src/components/PermissionPrompt.test.tsx` FIRST to the new presentation: assert the confirmation-treatment classes are present, the footer renders ONE button per fixture option (the fixture has 2 — assert both option names render) + a Cancel button, and the existing dynamic-option assertions stay green ("renders the tool title and every offered option", "sends the second option id" — the `respondPermission` mock is already in place via the test's `vi.mock("../lib/tauri")` pattern).
- [ ] Update `src/components/AskQuestionCard.test.tsx` FIRST: assert the ask-treatment classes, the option list renders the question + options, selecting an option + submit calls `respondBridgeRequest` (the existing test already mocks it from `../lib/tauri` — keep that pattern; the local handler is `respond`, not a `submitAskResponse` — no such function exists).
- [ ] Add a `ChatStream` test file `src/components/ChatStream.test.tsx` if absent (create it): seed a session with one user message → the header title derives from it (truncated); seed `inTurn` true + no bridge state → the working indicator renders the `BrailleLoader` (assert an element with `role="status"`); seed a bridge `agentState` of `blocked` → "Waiting for your input…" renders; seed a turn with a `diff` message → the `FileSummaryCard` renders with the right totals; seed the SAME file's `diff` twice in one turn (the `tool_call_update` re-emit) → the card counts the file once (last patch wins); click the `PanelRight` toggle → `getSidePaneCollapsed()` flips (and the button's `aria-pressed` follows).
- [ ] Run `pnpm test` — the new/updated tests fail.
- [ ] Implement 1–8 (including the `sidePaneState` toggle wiring + the `ChatStream` panel removal + the `!activeSessionId` empty-state restyle — the `useStartNewConversation` hook was created in Task 5; this task just wires the "…" menu item to it).
- [ ] Run `pnpm test` — all pass.
- [ ] Run `pnpm build` — green.
- [ ] Commit with message: "feat: restyle the conversation frame (header, stream, working indicator, file summary)"

**Acceptance criteria:**
- [ ] All the listed tests pass; the anchored-ask correlation and stacked-ask behavior are unchanged (their existing tests still pass unmodified).
- [ ] No `neutral-*`/`sky-*` ad-hoc classes remain in any modified component; the working indicator honors `agentState` (`blocked` → text, `working` → braille + quip, `idle` → hidden) with the `inTurn` fallback.
- [ ] No store/Tauri file modified.

---

### Task 7: Composer + full validation sweep

**Context:**
The last visual piece — the composer — plus the final validation pass that proves the branch is merge-ready per AGENTS.md. The composer keeps the existing send/pause/resume logic; only its presentation and the working-state disable change.

**Files:**
- Modify: `src/components/ChatStream.tsx` (the composer block only)
- Modify: `src/components/ChatStream.test.tsx`

**What to implement:**

1. **Composer block** (replace the existing `<div className="border-t border-neutral-800 p-3">…` block in `ChatStream.tsx`):
   - Shell: `rounded-2xl bg-input border-input-border hover:border-input-border-hover focus-within:border-input-border-focused p-2 m-3` (the `m-3` replaces the old `p-3` wrapper; the shell is the last element of the frame, above the frame's bottom edge).
   - `textarea`: `resize-none text-ui-base bg-transparent outline-none` (drop the old `bg-neutral-900 border` styling — the shell carries the border), auto-growing (a `useEffect` on the value setting `height = auto` then `scrollHeight`, min 2 rows / max 6 rows via `max-h` + `overflow-y-auto`), placeholder = **the existing THREE-state logic** (`isLive` / `canResume` / else — see the current composer) **plus ONE new state** (live + working), with **NEW copy — the four strings below REPLACE the current ones** (the test asserts exactly these four literals): live + idle → "Send a prompt…"; live + working → "Agent is working…"; stored + resumable → "Paused — Resume to reconnect"; history-only → "This session is closed"; `disabled` when not live or while working.
   - Footer (`flex items-center justify-between mt-1`): left = an empty `span` (reserved); right = a `flex items-center gap-2` with the static agent label (`text-ui-xs text-foreground-subtlest`, the active session's `agentId`) + the send button: `size-8 rounded-full bg-primary text-primary-foreground` (a `Button` `size="icon"` restyled — override with `rounded-full bg-primary text-primary-foreground`), an `ArrowUp` `lucide` icon `size-4`, `disabled:opacity-50`, **disabled when** `!isLive || inTurn || draft.trim() === ""` (the existing condition + `inTurn` — it already includes it).
   - The error line stays above the shell (`text-ui-sm text-destructive`); the Enter/Shift+Enter keyboard handling stays.
2. **Validation sweep** (AGENTS.md merge gate):
   - `pnpm test` (repo root) — the whole frontend suite green.
   - `pnpm build` (repo root) — tsc type-check + vite build green.
   - From `src-tauri/`: `cargo test`, `cargo clippy --all-targets` (0 warnings), `cargo fmt --check` — all green (the Rust side is untouched by this plan; this is a regression gate, not an expectation of change).
   - Grep gate: `rg '(bg|text|border|ring)-(neutral|sky|amber|red|green|emerald)-' src --glob '*.tsx'` returns **no hits** — note the anchor is deliberately NOT `(^|[" ])`: variant-prefixed classes (`hover:bg-sky-500`, `focus:border-sky-600`) are the common ad-hoc form in the current code and must be caught; the extended palette list covers the other ad-hoc palettes the design rule bans (semantic utilities like `text-destructive`/`bg-success/14` are token-based and match nothing here). The token DEFINITIONS in `src/index.css` are explicitly EXEMPT — the verbatim ZCode token blocks contain `var(--color-neutral-50)`/`var(--color-sky-400)`-style values (e.g. `--color-background: var(--color-neutral-50)`), and those are the design system, not ad-hoc usage; do NOT "fix" them.

**Steps:**
- [ ] Update `src/components/ChatStream.test.tsx` FIRST: the composer renders the shell with the `rounded-2xl` class; the placeholder matches the session state (assert all four variants — the three existing states + the new live + working one — by seeding the store); the send button is disabled while `inTurn` and when the draft is empty; pressing Enter with a draft calls `sendPrompt` (the existing behavior assertion).
- [ ] Run `pnpm test` — the new composer cases fail.
- [ ] Implement 1.
- [ ] Run `pnpm test` — green.
- [ ] Run the full sweep (2) — all green; fix anything the sweep surfaces and re-run until clean.
- [ ] Commit with message: "feat: restyle the composer; validation sweep green"

**Acceptance criteria:**
- [ ] The four placeholder variants render per session state (three existing + one new); the send button's disable logic is exactly `!isLive || inTurn || draft.trim() === ""`.
- [ ] `pnpm test`, `pnpm build`, `cargo test`, `cargo clippy --all-targets` (0 warnings), `cargo fmt --check` are all green.
- [ ] The grep gate returns no hits — the old palette is fully gone from the components (`src/*.tsx`); the `src/index.css` token definitions are untouched (exempt).

---

## Follow-ups (recorded, NOT in this plan)

1. Rust `session/cancel` command + composer Stop button (the send button is disabled while working until this lands).
2. Cost/metrics rendering in the Todos progress header (bridge `cost` is stored, not rendered in v1).
3. Light theme activation (tokens already present; a switcher if ever wanted).
