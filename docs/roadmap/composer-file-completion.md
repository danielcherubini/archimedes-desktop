---
status: committed
done-when: Typing `?<query>` in the composer opens a picker of files in the active Space (`?REA` → `README.md`, `?src/comp/Form` → `src/components/Formula.tsx`); selecting replaces the token with the case-preserved relative path and the message sends with a plain path (no block, no chip); `??` and `a?.b` never open the picker and Enter still sends; ADR 0033 is live; all AGENTS.md validation green.
---

# `?` File-path Completion in the Composer — Plan

**Goal:** Let the user type `?<query>` to complete a file path from the active **Space**; the selection inserts the path (trigger consumed) as plain text.

**Architecture:** NOT a Mention (ADR 0033 / `CONTEXT.md`): no expansion, no block, no chip, no `MentionKind`. `expandMentions` / `splitMentionBlocks` / `MessageBubble` / `mergeDedupeKey` are NOT touched. One new read-only Tauri command walks the canonicalized Space (bounded); a 4th cached catalog row feeds the EXISTING synchronous `filtered` picker; `activeMentionToken` gains a `?` branch with its own stricter charset.

**Tech Stack:** Tauri 2 + Rust (`src-tauri/`, walkdir already a dependency), React 19 + TS + Vitest (repo root).

**Load-bearing invariants (every task preserves these):**
1. **`?` inserts a PATH, never contents.** The Agent reads it with its own `read`, which IS Boundary/policy-gated (ADR 0030). The Client reads no file bytes on `?`'s account.
2. **`?` never expands.** Do not add a `MentionKind`, a block, a chip, or route it through `expandMentions`. It therefore introduces **no tag of its own** — but its inserted path is ordinary text and so obeys the mention grammar exactly like hand-typed text (a path holding `$name`, or a root-level `@name`/`#name`, expands if a resource of that exact name exists). Pre-existing grammar, not a new surface — ADR 0031's exact-match tag-safety invariant still holds because expansion matching stays exact.
3. **`?` PRESERVES case** (`?REA` → `README.md`) and its charset includes `.` `_` `/` and UPPERCASE. Both are deliberate divergences from the mention grammar — do not "unify" them.
4. **`?` requires ≥1 path char** to open the picker (unlike `$`/`#`/`@`, which open on a bare glyph). This is what stops `??` (git porcelain) from opening the picker and turning Enter into an insertion instead of a send.
5. **Empty `filtered` ⇒ no picker ⇒ Enter still sends** (the ADR 0032 degradation path). An in-flight file listing must never block the send.

**Explicit v1 non-goals — do NOT build these:** no `?~/.bashrc` and no absolute-path completion — Space-only scope, ADR 0033. The two glyphs behave DIFFERENTLY and the wording must say so: `~` is OUTSIDE the `?` charset, so `?~` never forms a token and no picker opens; `/` IS in the charset (`FILE_TOKEN_RE` = `/(^|\s)\?([a-zA-Z0-9._/-]+)$/`), so `?/etc/hosts` DOES form a token and the picker DOES open — it simply lists nothing, because every row comes from the Space-relative walk. So the promise the code keeps is **only Space-relative entries are ever offered** (a CATALOG-side guarantee), not "absolute queries are rejected" — do not add home expansion or an absolute-path grammar branch, and do not re-describe `/` as outside the charset. No content injection via `?`, ever. The listing is cached per Space, so a file created mid-session appears only after a Space switch or remount — that staleness is the accepted `useSkillCatalog` behavior, do NOT add a refetch/poll/watcher. `exec_find` is NOT refactored to share the walker (a cross-reference comment carries the relationship). No settings toggle for `?` (unlike `#`, ADR 0032 — `?` has no collision class once it needs ≥1 char).

---

### Task 1: Rust — the bounded Space walk + `list_space_files` command

**Context:** The renderer can see NO filesystem today (no command lists a directory; `read_file_bytes` reads one allowlisted image). This task adds the only new surface: a bounded, Space-scoped file listing for the picker. Scope is the canonicalized Space ONLY (ADR 0033) — there is no caller-supplied sub-path, so nothing to `..`-check. `exec_find` is deliberately NOT shared: it shells out to `fd` with a per-directory glob, which would mean a subprocess per keystroke, and its dot-skip is per-FILE while the picker needs a per-DIR prune. Cross-reference comments at both sites carry the relationship (the discipline recorded for the mirrored `parse_value`).

**Files:**
- Modify: `src-tauri/src/commands/files.rs` (the command + a pure walker + tests)
- Modify: `src-tauri/src/lib.rs` (register)
- Modify: `src-tauri/src/agent/tools/exec.rs` (comment only, at `exec_find`)

**What to implement:** in `commands/files.rs`, extend the module doc (it currently says "the composer's `+` file picker" — it now also serves the `?` completion) and add. `serde::Serialize` and `std::path::Path` are NOT currently imported here (this file imports only `crate::agent::MAX_IMAGE_BYTES`) — add them.
```rust
/// The `?` file-completion cap (ADR 0033). A big repo must not hang the
/// Client; past the cap the picker says "too many files" instead.
pub const MAX_PICKER_ENTRIES: usize = 5_000;

/// Directories skipped by name in EVERY listing (dot-dirs are skipped by
/// prefix separately — `node_modules` is not one).
pub const PICKER_SKIP_DIRS: &[&str] = &["node_modules"];

/// One `?`-picker listing: paths RELATIVE to the Space root, `/`-separated
/// (normalized so a Windows walk never inserts `\` into the composer's
/// token), sorted. `truncated` = the cap was hit.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileListDto {
    pub entries: Vec<String>,
    pub truncated: bool,
}

/// Walk `root` for the `?` picker: files only, dot-directories PRUNED at
/// the directory level, plus `PICKER_SKIP_DIRS`. Dot-named FILES are
/// KEPT (`.gitignore`, `.env` are real completion targets — `.` is in the
/// token charset), which is why the skip is a directory-level prune and not
/// a name-prefix filter over every entry.
///
/// Deliberately NOT shared with `exec_find` (see the cross-reference there):
/// that shells out to `fd` with a `walkdir` FALLBACK whose dot-skip is
/// per-FILE and which descends INTO `.git`.
///
/// Symlinks are NOT followed (walkdir's default: a symlink's `file_type()`
/// is symlink, so `is_file()` is false and it is not listed).
/// Bounded by `cap`. Blocking by nature, so callers run it on
/// `spawn_blocking`.
pub fn collect_files(root: &Path, cap: usize) -> (Vec<String>, bool) {
    // depth 0 is the root itself (which may legitimately be a dot-dir, e.g.
    // a Space opened at `~/.dotfiles`) — never pruned.
    // NO depth limit: a deeply-nested file is still addressable. Count-bound
    // only. Skip dirs; skip a dot-named dir at depth > 0; skip
    // PICKER_SKIP_DIRS at any depth.
    //
    // SEPARATORS — load-bearing, and INVISIBLE TO CI (`ci.yml` runs on
    // ubuntu-latest; only the release job builds windows-latest): build each
    // entry by joining `path.strip_prefix(root)?.components()` with `"/"`.
    // Do NOT `to_string_lossy()` the stripped path — that is what
    // `exec_find`'s fallback does (exec.rs:913) and it yields `\` on
    // Windows, which is NOT in the token charset, so every nested
    // completion would silently truncate at the first separator. There is no
    // reusable normalizer in this repo to call instead.
    // … then SORT the result (walk order is filesystem-dependent; the picker
    //   must be deterministic).
}

/// The `?` file-completion listing for a Space (ADR 0033). `None`, or a
/// Space that fails to canonicalize, → EMPTY (never an arbitrary-root
/// walk, never an error: the picker degrades to "no files"). Reads NO file
/// bytes — names only.
#[tauri::command]
pub async fn list_space_files(space_path: Option<String>) -> Result<FileListDto, String> {
    // canonicalize(space_path) inside spawn_blocking; on any failure
    // return FileListDto { entries: vec![], truncated: false }.
    // Uses MAX_PICKER_ENTRIES.
}
```
`use serde::Serialize;` and `use std::path::Path;` as needed (check what the file already imports). In `lib.rs`, add `commands::files::list_space_files,` to `generate_handler!` next to `commands::files::read_file_bytes`. In `exec_find` (`agent/tools/exec.rs`), add a comment above it: this walker is deliberately NOT shared with `commands::files::collect_files` (the `?` picker) — `fd` shell-out + per-directory glob vs a per-keystroke whole-tree listing.

**Steps:**
- [x] FAILING tests added to the EXISTING `#[cfg(test)] mod tests` in `commands/files.rs` (it already exists — around line 63, with 5 image-picker tests; do NOT create a second one, that is a duplicate-module compile error). Copy this module's OWN established temp-dir pattern: `std::env::temp_dir().join(format!("files-cmd-test-{}", uuid::Uuid::new_v4()))` with `#[tokio::test]`. Do NOT reach for `scratch()` — it is private inside `commands/agents.rs`'s test module and not importable from here:
  - `collect_files_lists_nested_files_relative_and_sorted`: `a.txt`, `src/b.rs`, `src/c/d.md` → `["a.txt", "src/b.rs", "src/c/d.md"]`, `truncated == false`.
  - `collect_files_prunes_dot_directories_and_node_modules`: `.git/x`, `node_modules/y`, `.hidden/z` → none listed; a normal file still is.
  - `collect_files_keeps_dot_named_files`: a `.gitignore` at the root IS listed (pins the deliberate dot-FILE vs dot-DIR split).
  - `collect_files_keeps_a_dot_named_root`: root dir named `.dotfiles` containing `a.txt` → `["a.txt"]` (depth-0 is never pruned).
  - `collect_files_caps_and_reports_truncation`: 5 files, `cap = 3` → 3 entries + `truncated == true`.
  - `collect_files_does_not_follow_symlinks`: a symlink to a file outside the root is NOT listed (skip the test on platforms where symlink creation needs privileges — `#[cfg(unix)]`).
  - `list_space_files_none_is_empty` + a canonicalized-space round trip asserting `entries` are relative (NOT absolute) and `/`-separated.
- [x] `cargo test --lib commands::files` (from `src-tauri/`) — confirm they FAIL (symbols missing).
- [x] Implement `collect_files` + `FileListDto` + `list_space_files` + the `lib.rs` registration + the `exec_find` cross-reference comment. In `lib.rs`, `commands::files::read_file_bytes` is the LAST entry in `generate_handler!` (line 194) with NO trailing comma — you must add the comma to it, then the new line:
  ```rust
  commands::files::read_file_bytes,
  commands::files::list_space_files
  ```
  In `exec_find`, the comment must be accurate about what it actually does: it shells out to `fd` (`fd -g <pattern>`, a glob over paths relative to the search dir) with a `walkdir` FALLBACK whose dot-skip is per-FILE and which DESCENDS INTO `.git` rather than pruning it.
- [x] `cargo test --lib commands::files` — pass. Then `cargo test` (full).
- [x] `cargo clippy --all-targets` (0 warnings; `touch` touched files) + `cargo fmt`.
- [x] Commit: `feat: list a Space's files for the ? completion`

**Acceptance:** relative, sorted, `/`-separated entries (built by joining `components()`, not `to_string_lossy`); dot-dirs + `node_modules` pruned while dot-FILES survive; a dot-named ROOT survives; the cap reports `truncated`; `None`/uncanonicalizable → empty, never an error; no file bytes read; `exec_find` behaviorally untouched (its tests still pass).

---

### Task 2: Frontend — the wire type + the 4th cached catalog row

**Context:** The listing must be cached per Space like the skills/agents/MCP catalogs. A wrinkle the spec glossed: `useCatalogRow<T>` is typed `Map<string, Promise<T[]>> → T[]`, but this payload is `{ entries, truncated }`. Generalize the shared hook to a PAYLOAD type rather than faking a row array — the tested cache logic moves VERBATIM (the existing 7 hook tests are the guard).

**Files:**
- Modify: `src/lib/tauri.ts`
- Modify: `src/hooks/useMentionCatalogs.ts`
- Test: `src/hooks/useMentionCatalogs.test.ts`

**What to implement:** in `src/lib/tauri.ts`, next to the other catalog DTOs:
```ts
/** One `?` file-completion listing (camelCase over IPC — the Rust `FileListDto`). */
export interface FileListDto {
  /** Paths RELATIVE to the Space root, `/`-separated, sorted. */
  entries: string[];
  /** The walk hit `MAX_PICKER_ENTRIES` — the picker says so instead of lying. */
  truncated: boolean;
}

export async function listSpaceFiles(spacePath: string | null): Promise<FileListDto> {
  return invoke<FileListDto>("list_space_files", { spacePath: spacePath ?? null });
}
```
In `src/hooks/useMentionCatalogs.ts`: generalize `useCatalogRow<T>` into `useCatalogPayload<P>(cache: Map<string, Promise<P>>, spacePath: string | null, fetch: (spacePath: string | null) => Promise<P>, fetchName: string, empty: P): P`, moving the body VERBATIM (the `{ key, payload }` state, the in-flight promise cache, the left-key eviction, the identity-guarded failure delete, the stale-response guard). Re-express `useCatalogRow<T>` as a thin wrapper over it (`empty: [] as T[]`) so agents/mcp keep their exact behavior — the existing `.catch` that returns `[] as T[]` becomes `.catch(… return empty)`. Add `const filesCache = new Map<string, Promise<FileListDto>>();` and a MODULE-LEVEL `const EMPTY_FILE_LIST: FileListDto = { entries: [], truncated: false };` (a stable identity — do not pass a fresh literal per render), a `files: FileListDto` row in `useMentionCatalogs`' return (document that the hook now feeds FOUR composer catalogs), and clear `filesCache` in `clearMentionCatalogsCache()`.

**Deps trap (no ESLint here to catch it):** the effect deps MUST stay exactly `[spacePath, fetchName]`. Do NOT add `empty`, `cache`, or `fetch`. All three are stable by construction (module constants / a module-level `EMPTY_FILE_LIST`), and if `empty` were tracked, callers passing a fresh literal would re-fire the effect every render whose `.then` sets a NEW payload object → an infinite render loop. EXTEND the existing deps comment (which currently covers only `cache`/`fetch`) to name `empty` too.

**Steps:**
- [x] FIRST: add `listSpaceFiles: vi.fn().mockResolvedValue({ entries: [], truncated: false })` to the EXISTING `vi.mock("../lib/tauri")` factory in `useMentionCatalogs.test.ts` (line 15, a spread-`actual` factory). This is REQUIRED, not optional: the hook now calls it on every mount, and if it stays the REAL function `invoke` rejects in jsdom — which breaks the existing `a failed fetch degrades to empty and a remount retries` test, which asserts `errSpy` was called EXACTLY 2 times (line 250) and would see 3.
- [x] FAILING tests in `useMentionCatalogs.test.ts` (`beforeEach` already clears all caches): one fetch per key with two consumers; the `[]`-gap on key change and the left-key eviction (assert the payload identity degrades to the `empty` value — `{ entries: [], truncated: false }` — until the new fetch resolves); a rejected fetch → the empty payload + `console.error`; the `null` key calls the wrapper with `null`; the `truncated` flag SURVIVES the cache (a warm read of a truncated listing is still truncated).
- [x] Run `pnpm test src/hooks/useMentionCatalogs.test.ts` — confirm FAIL.
- [x] Implement; re-run the file, then `pnpm test`. The 7 pre-existing tests in this file are the extraction's real guard — they exercise exactly the semantics being moved (in-flight dedupe, the gap, the left-key eviction, the identity-guarded failed-promise delete, the `null` key) through the public hook, so they catch a behavioral change the moment the mock factory above lets them run at all.
- [x] `pnpm build` (tsc green).
- [x] Commit: `feat: cache a Space's file listing alongside the mention catalogs`

**Acceptance:** one IPC call per catalog per key; empty-payload gap on key change; failed fetch → empty + `console.error`; `truncated` cached; the pre-existing agent/mcp tests unchanged and green.

---

### Task 3: Frontend — the `?` token in `activeMentionToken`

**Context:** `activeMentionToken`'s single shared regex `/(^|\s)([$#@][a-z0-9-]*)$/` cannot express `?`'s rules (different charset, ≥1 char, uppercase). Add a SECOND regex tried for `?` only. Keep `activeSkillToken` (the retained `$` oracle) untouched.

**Files:**
- Modify: `src/lib/skills.ts`
- Test: `src/lib/skills.test.ts`

**What to implement:** widen the return type to `{ prefix: "$" | "#" | "@" | "?"; remainder: string; start: number } | null` (add a module-level `export type MentionPrefix = …` if it reads cleaner — then reuse it in the 5 sites the union is currently duplicated at: `skills.ts` (×2), `ChatStream.tsx`, `ComposerRow.tsx` (×2), `ComposerMentions.tsx`), and:
```ts
// The `?` token (ADR 0033) — NOT the mention charset: a PATH query needs
// `.` `_` `/` and UPPERCASE (README.md is spelled that way, and nothing
// re-matches an inserted path, so case is preserved). It ALSO requires
// ≥1 char (a bare `?` opens nothing): that is what keeps `??` (git-status
// porcelain) from opening the picker and turning Enter into an INSERTION
// instead of a send. The `(^|\s)` boundary does the rest (`a?.b`, `x ? y`
// and `url?query` never match because a `?` preceded by non-whitespace
// fails it).
const FILE_TOKEN_RE = /(^|\s)\?([a-zA-Z0-9._/-]+)$/;
```
Try the existing regex first; on `null`, try `FILE_TOKEN_RE`; return `{ prefix: "?", remainder: m[2], start }` (the remainder keeps its case — do NOT lowercase here).

**Steps:**
- [x] FAILING tests in `skills.test.ts` — `?README` → `{prefix:"?", remainder:"README"}`; `?src/comp/Form` → remainder `src/comp/Form`; `?a_b.c-d/e` matches; **uppercase is accepted** (`?REA` matches); a bare `?` → `null`; `??` → `null`; `x??` → `null`; `a?.b` → `null`; `x ? y` (caret after `y`) → `null`; `url?query=1` → `null`; `?has space` (caret after `?has`) matches, and `?a b` with the caret after `b` → `null`; a mid-word `foo?bar` → `null`; and REGRESSIONS: `$de`, `@sc`, `#po` still return their prefixes, and a bare `$`/`@`/`#` STILL returns `remainder: ""` (the ≥1-char rule is `?`-ONLY).
- [x] Run `pnpm test src/lib/skills.test.ts` — FAIL, then implement, then pass.
- [x] Confirm `expandMentions` / `splitMentionBlocks` are untouched (`git diff` shows no changes to them) — a `?` in text must still be verbatim.
- [x] `pnpm build` + `pnpm test`. Commit: `feat: recognize the ? file-path token`

**Acceptance:** the 12+ `?` cases above; the three mention prefixes byte-identical; a bare `?` never yields a token; `expandMentions` untouched.

---

### Task 4: Frontend — wire `?` into the picker (rows, insertion, truncation)

**Context:** Three sites change, and two carry traps: `filtered` must branch for `?` and gain the new dep, and `selectMention` hard-codes `` `${row.prefix}${row.name.toLowerCase()} `` — for `?` the trigger is CONSUMED (no prefix inserted) and case is PRESERVED.

**Files:**
- Modify: `src/components/ChatStream.tsx`
- Modify: `src/components/chat/ComposerMentions.tsx`
- Modify: `src/components/chat/ComposerRow.tsx` (only if a prop type widens)
- Test: `src/components/ChatStream.test.tsx`

**What to implement:**
- `ChatStream.tsx` (line 399 destructures `{ skills, agents, mcpServers }` — add `files`; it is the hook's ONLY production consumer and no test asserts the full return shape, so a 4th field is safe). In `filtered`, add the `?` branch BEFORE the skill fallback: rows from `files.entries` mapped to `{ key: "?" + p, prefix: "?", name: p, description: basenameOfPath(p) }`, filtered with `fuzzyMatch(query, r.name)` while the other three prefixes KEEP their substring filter — a path query is path-SHAPED (`src/comp/Form` must reach `src/components/Formula.tsx`). **Imports:** `basenameOfPath` is ALREADY imported at line 16 from `"../lib/paths"` (reuse it); `fuzzyMatch` comes from `"../lib/fuzzy"` (new import). Do NOT write `../../lib/…` — this file is `src/components/`, one level down.
- **Bound the rendered rows (a real perf gap):** the walk cap is 5,000, and `fuzzyMatch` is a SUBSEQUENCE match, so a query like `?e` can match nearly everything → thousands of `<button>`s re-rendered per keystroke. Add `const MAX_PICKER_ROWS = 100;` and apply `.slice(0, MAX_PICKER_ROWS)` INSIDE `filtered`, AFTER the filter — so every downstream consumer (`activeIndex`, the `% filtered.length` keyboard wrap, the `filtered.length > 0` gates) sees one consistent bounded list. Never cap in the render.
- **Deps gain `files`** (the same class of bug the `agents`/`mcpServers` deps pin guards). In `selectMention` (line 901): `` const inserted = row.prefix === "?" ? `${row.name} ` : `${row.prefix}${row.name.toLowerCase()} `; `` with a comment that `?` is consumed and case-bearing (ADR 0033) — the existing splice from `token.start` already removes the `?`. Do NOT gate the fetch on anything and do NOT touch `send()`/`expandMentions`.
- Update the stale "three-prefix"/"three glyphs" comments in the files you touch (`ChatStream.tsx` ~381/400/416/887, `ComposerRow.tsx` 18–20, `ComposerMentions.tsx` 3) — this repo treats comments as load-bearing documentation.
- `ComposerMentions.tsx`: the prefix badge renders `?`. Add a `note?: string` prop rendering a NON-selectable note (a `<div>`, not a `<button>`) — deliberately NOT a row, so `filtered`/`activeIndex`/the keyboard model are untouched. WIDEN the container gate from `open && filtered.length > 0` (line 43) to `open && (filtered.length > 0 || note)` so the note is visible when a query matches NOTHING — that is exactly when "too many files" explains the empty picker. This is safe: the keydown intercept is gated on `filtered.length > 0` (`ComposerRow.tsx:157`), so Enter still sends. Pass `files.truncated ? "too many files — the listing is capped" : …` (and a "too many matches" variant when the render cap trimmed results). Pass `files.truncated` through `ComposerRow`.

**Steps:**
- [x] FIRST: add `listSpaceFiles` to the EXISTING spread-`actual` `vi.mock("../lib/tauri")` factory in `ChatStream.test.tsx` (line 112) as a `vi.fn()` you can control per test — every session-mounting test calls it, and Task 4's in-flight test needs a deferred promise. Also mock it in `SkillsDialog`-adjacent files if the mount surfaces one.
- [x] FAILING tests in `ChatStream.test.tsx` (seed the caches as the existing mention tests do): typing `?RE` opens the picker with `README.md` and a `?` badge; a path-shaped query `?src/comp/Form` lists `src/components/Formula.tsx`; Enter/Tab/mousedown on a row inserts `README.md ` **with the `?` GONE and the case INTACT** (assert the textarea value); the row's secondary shows the basename and its `title` tooltip matches; the cap note renders and is NOT a row (assert it is not in `data-testid="mention-picker"`'s button set, and that Enter still SENDS the draft); **the render cap holds** — 150 seeded entries yield ≤ 100 buttons and Down-arrow wraps within those rows; while `listSpaceFiles` is PENDING, typing `?RE` opens nothing and **Enter SENDS** (the in-flight degradation); a Space switch serves no rows until the new listing resolves; regression: `$` and `@` pickers still work, and a draft containing `?foo` sends VERBATIM (no expansion, byte-identical).
- [x] Run `pnpm test src/components/ChatStream.test.tsx` — FAIL, implement, pass.
- [x] `pnpm test` + `pnpm build`. Commit: `feat: complete ? to a Space file path in the composer`

**Acceptance:** `?REA` → `README.md` inserted with no `?` and real case; the truncation note is not a row; Enter sends while loading; `$`/`@`/`#` behavior unchanged; no `?` text ever expands.

---

### Task 5: Regressions + the full validation matrix

**Context:** The risk in this feature is drift in the SHARED picker. This task pins the guarantees that make `?` safe to have added, then runs the whole gate.

**Files:**
- Test: `src/lib/skills.test.ts`, `src/components/MessageBubble.test.tsx` (additive only)

**What to implement:** tests asserting (a) `expandMentions` on text containing `?README.md`, `??`, `a?.b` and a `~/.bashrc` returns the text BYTE-IDENTICAL with every catalog populated (so `?` can never expand); (b) `splitMentionBlocks` still round-trips the three real block literals unchanged; (c) `$`/`@` substring filtering results are unchanged now that `?` uses `fuzzyMatch` (a subsequence is a superset of a substring — state that in the test comment); (d) a user message with no blocks still renders verbatim.

**Steps:**
- [x] Add the four tests; run `pnpm test src/lib/skills.test.ts src/components/MessageBubble.test.tsx`.
- [x] FULL matrix (AGENTS.md): repo root `pnpm test` + `pnpm build`; `src-tauri/` `cargo test` + `cargo clippy --all-targets` (0 warnings, `touch` touched files so clippy re-analyzes) + `cargo fmt --check`. — Re-run TOGETHER on the final commit `08292fb` after every review batch landed: `pnpm test` 1408 passed / `pnpm build` clean / `cargo test` 740 passed / clippy 0 warnings / fmt clean.
- [ ] Manual smoke if you can run the app: `?` opens the picker in a Space, selection inserts a path, `??` does nothing, Enter always sends. — **still open**: needs a human at a real window manager (`?` + a real Space); never automated here, so it stays honest.
- [x] Commit: `test: pin the ? completion's non-expansion and picker regressions`

**Acceptance:** all four pins exist; the entire matrix is green; the `done-when` above is observable end to end.
