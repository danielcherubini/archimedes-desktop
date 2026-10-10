---
status: committed
done-when: typing `?~/.config/ht` offers `htop/` as a row, selecting it leaves a LIVE token that lists that directory, and a file row inserts an ABSOLUTE path that the agent's own `read` opens on the first try; `?README` behaves byte-for-byte as it does today; and a test proves the listing never reads more than one directory.
---

# Out-of-Space File Completion — Plan

**Goal:** let `?` complete a file anywhere on the machine by completing ONE directory at a time, while `?` inside the Space keeps behaving exactly as it does today.

**Architecture:** a token beginning `/`, `~/`, or a Windows drive root switches the composer from the cached Space **Listing** to **Directory completion**: Rust expands `~/`, takes the token's DIRECTORY PART (everything up to and including its last `/`), and lists exactly one level with the same `ignore` engine and flags `collect_files` already uses (ADR 0035). The whole directory is returned unfiltered so the renderer keeps filtering synchronously and a fetch happens only when the token's *directory* part changes. Rows carry an absolute `insert` and an abbreviated `display`; a directory row keeps the `?` so descent continues.

**Tech Stack:** Rust (Tauri 2 command, `ignore` 0.4 crate already a dependency), React 19 + TypeScript, Vitest.

**Durable decisions live in `docs/decisions/0035-out-of-space-completion-is-a-one-directory-peek.md`. Read it before Task 1 — it carries the *why* this is a peek and not a walk, and the reason the inserted path is absolute.**

---

## Ground rules for every task

- **TDD**: write the failing test first, run it, confirm it fails for the expected reason, then implement. Never skip the RED.
- Gates, from the repo root: `pnpm test`, `pnpm build`. From `src-tauri/`: `cargo test`, `cargo clippy --all-targets` (**must be 0 warnings**), `cargo fmt --check`. Clippy that finishes in well under a second with no `Checking` line replayed a cache — `touch` the touched `.rs` files and re-run.
- **Commit with `git commit -F -` and a QUOTED heredoc (`<<'MSG'`), never `git commit -m "…"`**. This codebase's prose is full of backticks, `$`, `@`, `#`, `?`; inside double quotes a backtick is command substitution and silently eats the message. Re-read `git log -1 --format=%B` after committing.
- Baseline at the time of writing: **1411 frontend tests / 72 files, 760 Rust tests, clippy 0 warnings, fmt clean.** If a number moves without you moving it, stop and find out why. **(Re-measured after the Post-review pass below: 1476 frontend tests and 798 Rust tests. The sentence above is the baseline THIS PLAN was written against, kept as a record; the pair to compare against now is the second one.)**
- **Execute the tasks in the printed order, 1 → 2 → 3 → 4 → 5 → 6.** The dependencies run one way only: Task 4 consumes `listCompletionEntries` from Task 2, and Task 5 consumes `isOutsideSpaceQuery` from Task 3 and `useCompletionDir` from Task 4. Each task must pass its own gate as the sole change on top of its predecessor; do not renumber or reorder them to "group the Rust and the TypeScript".
- Two known pre-existing flakes, NOT yours to fix: `src/components/settings/SettingsPage.test.tsx` (Radix select timing) and Rust `agent::mcp::http::tests::http_session_id_is_resent` (loopback race). If one fails, re-run it alone before believing it.
- `docs/decisions/` is append-only: never edit a merged ADR's body.

---

### Task 1: the one-directory engine (pure Rust, no command yet)

**Context:**
`?` currently reaches only the active **Space**: `collect_files` (`src-tauri/src/commands/files.rs`) walks a Space root and returns Space-relative paths. Out of the Space we need the *same visibility rules* — the repo's ignore files, the VCS-directory prune — applied to exactly one directory, because a second visibility philosophy would mean two opinions about what a directory contains (ADR 0035). The engine is deliberately a **pure function taking `home` as a parameter**: this repo already has one refusal that cannot be tested because testing it required mutating process-global `HOME` (see the `git_global` note in this same file's `mod tests`), and passing `home` in is how we avoid ever repeating that. Do not read `HOME` inside the engine.

**Files:**
- Modify: `src-tauri/src/commands/files.rs`
- Test: the existing `mod tests` at the bottom of that same file (follow its helpers: `tmp_dir(tag)`, `touch`, `write`, `git_repo`)

**What to implement:**

1. Two DTOs, mirroring `FileListDto`'s style exactly (`#[derive(Debug, Clone, PartialEq, Serialize)]`, `#[serde(rename_all = "camelCase")]`):

```rust
/// One row of an out-of-Space directory completion.
pub struct CompletionEntryDto {
    /// The entry's own name, no trailing separator (what the filter matches).
    pub name: String,
    /// ABSOLUTE, `/`-separated — this is what lands in the draft.
    pub insert: String,
    /// `~/…` when under the home directory, else the same as `insert`.
    pub display: String,
    /// A directory: the row descends instead of finishing.
    pub is_dir: bool,
}

/// One directory's worth of completion rows + whether the cap trimmed it.
pub struct CompletionDirDto {
    pub entries: Vec<CompletionEntryDto>,
    pub truncated: bool,
}
```

2. `pub fn expand_completion_root(query: &str, home: Option<&Path>) -> Option<PathBuf>` — pure. **`pub`, matching `collect_files` (`files.rs:189`), for a load-bearing reason:** in Task 1 nothing outside `#[cfg(test)]` calls these yet, and a private (or `pub(crate)`) fn compiled into the lib target without `cfg(test)` is reported by `cargo clippy --all-targets` as `dead_code` — which fails this task's own 0-warnings gate. `pub` in the `pub mod commands → pub mod files` chain makes the item publicly reachable and therefore live. Do NOT paper over it with `#[allow(dead_code)]`. `query` is the RAW token remainder (e.g. `~/.config/ht`, `/etc/pas`, `/`, `C:/Users/you/`). Rules:
   - starts with `~/` → `home.join(&query[2..])`; `home == None` → `None`.
   - starts with `/` → `PathBuf::from(query)`.
   - matches a drive root `X:/` (an ASCII letter, then `:/`) → `PathBuf::from(query)`. This is what lets a Windows descent continue: a directory row there inserts `C:/Users/you/…`, and if that did not parse, the kept-`?` token would die after one level (ADR 0035). It must be written so it does NOT depend on the host platform — a Linux test can and must assert that `C:/x` parses.
   - **anything else → `None`**, which is what makes `~user`, a bare `~`, and every relative query never reach the filesystem. Do NOT support `~user`.
   - `..` is just a component; no normalisation, no rejection (the **Boundary** canonicalises at gate time anyway — ADR 0035).

3. `pub fn completion_dir(query: &str, home: Option<&Path>, cap: usize) -> (Vec<CompletionEntryDto>, bool)` — the listing (same `pub` reason as above). Take the directory as: if `query` ends with `/` the directory IS the expanded root; otherwise its `parent()`. (`~/notes` → list `$HOME`; `~/notes/` → list `$HOME/notes`; `/` → list `/`.) `None` from `expand_completion_root`, or no parent, → `(vec![], false)`.

4. The walk — **`collect_files`'s flags, one level deep**. Copy the builder chain from `collect_files` so the two cannot drift, changing exactly three things:

```rust
ignore::WalkBuilder::new(dir)
    .hidden(false)
    .parents(true)
    .git_ignore(true)
    .git_exclude(true)
    .git_global(false)
    .ignore(true)
    .follow_links(true)        // DIFFERENT from collect_files: out of the Space
                               // one level deep there is no cycle to follow and
                               // nothing to contain (ADR 0035). The Space walk
                               // keeps follow_links(false) — there a symlink's
                               // invisibility IS containment.
    .max_depth(Some(1))        // DIFFERENT: one directory, never a walk
    .sort_by_file_name(Ord::cmp)
    .filter_entry(|e| !(e.file_type().is_some_and(|t| t.is_dir())
        && is_skipped_dir(&e.file_name().to_string_lossy(), e.depth())))
                               // IDENTICAL: reuse `is_skipped_dir` unchanged so
                               // `.git`/.hg/.svn are pruned by one rule, not two
    .build()
    .filter_map(|e| e.ok());   // swallow: a broken symlink is an error entry,
                               // so it is simply invisible (ADR 0035)
```

   - Keep **both** directories and files (`collect_files` keeps files only): a row is kept when `file_type().is_dir() || file_type().is_file()`. With `follow_links(true)` a symlink-to-file reports `is_file()` and a symlink-to-dir reports `is_dir()`, which is exactly the behaviour we want.
   - Skip the root with `if entry.depth() == 0 { continue; }`. Do **not** reach for `.min_depth(Some(1))`: `collect_files` carries a note recording that it panicked under one configuration and not under this one, and a panic inside `spawn_blocking` empties the entire listing — that configuration-dependence is reason enough to use the loop idiom the Space walk already uses.
   - Build `insert` with a **new** small helper, `fn abs_to_slash(p: &Path) -> String`, and NOT by reusing `collect_files`' construction verbatim. `collect_files` is safe only because it STRIPS the root and joins relative components, so no `RootDir` component ever survives to be joined; here `insert` is absolute, and `Component::RootDir`'s `as_os_str()` is the SOURCE separator — `\` for a Windows path, which is exactly what a `\`-rooted `USERPROFILE` home produces. A `\` in `insert` is fatal twice over: it is outside the token charset, and `?C:\…` fails the absolute shape's `[A-Za-z]:/` root — so Windows `~`-descent would die after one level, the very failure the drive root was added to prevent. Emit `"/"` for `Component::RootDir`, keep `Prefix` (the `C:`) as-is, skip `CurDir`, keep `ParentDir` and `Normal` as-is, joined with `/`. **Say plainly in the comment what is and is not observable here: the `RootDir` arm RUNS on Linux for every absolute path (its `as_os_str()` is simply `/`), but a `\`-valued `RootDir` cannot occur on this CI** — so the arm itself is covered by the round-trip test and by mutation, while the Windows value it defends against never appears. Do not write "this branch cannot be exercised on Linux": it can, and a comment claiming otherwise fails this plan's own Task 6 mutation rule.
   - `display`: when `home` is `Some(h)` and `insert` is under `h`, `display` is `~/` + the remainder; otherwise `display == insert`.
   - Sort **directories before files**, each group in the walker's name order.
   - Cap at `MAX_PICKER_ENTRIES` (reuse it — do NOT invent a third constant; ADR 0035 explains why a smaller cap would silently hide the file being typed) and set `truncated = true` when it bites.
   - No `unwrap`, no `expect`, no `panic!`, no `debug_assert` that a hostile path can trip — this runs inside `spawn_blocking` in Task 2 and a panic empties the whole listing.

**What NOT to change:** `collect_files`, `is_skipped_dir`, `PRUNED_VCS_DIRS`, `FileListDto`, `MAX_PICKER_ENTRIES`, `list_space_files`, or any frontend file. The Space Listing is not in this task.

**Steps:**
- [x] Write these tests in `mod tests` FIRST and run `cargo test --lib commands::files` — all must fail (the functions don't exist yet, so a compile failure is the correct RED):
  - `expand_completion_root_expands_tilde_only_in_first_position` — `~/.config/x` → `home/.config/x`; `a~b` → `None`; `~` → `None`; `~user/x` → `None`; `/etc/pas` → `/etc/pas`; `home = None` + `~/x` → `None`.
  - `expand_completion_root_accepts_a_windows_drive_root` — `C:/Users/you/` parses to a path (assert the parse, not host-path semantics — this must pass on Linux, because CI can never run Windows).
  - `completion_dir_lists_directories_as_well_as_files` — a temp dir with `sub/f.txt` and `top.txt` → rows `sub` (`is_dir = true`) and `top.txt` (`is_dir = false`), `insert` absolute.
  - `completion_dir_sorts_directories_before_files` — fixture where alphabetical order would put the file first (`a_file.txt`, `z_dir/`) → `z_dir` row comes first.
  - `completion_dir_displays_home_relative_and_inserts_absolute` — under home: `display` starts with `~/`, `insert` is absolute and EQUALS the real path; outside home (use the temp root itself, not `/tmp`, as `home`) both are absolute.
  - `completion_dir_never_offers_the_vcs_directories` — a `git_repo` fixture: no `.git` row; then add `.hg/` and `.svn/` dirs → absent.
  - `completion_dir_hides_ignored_entries_in_another_repository` — a temp git repo with `.gitignore` containing `node_modules`, and `node_modules/pkg/index.js` → **no `node_modules` row**. This is the test that proves the engine was reused and not reimplemented.
  - `completion_dir_offers_symlinked_files_and_directories` (unix-gated `#[cfg(unix)]`, matching the module's existing symlink tests) — symlink to a file and to a dir → both present, `is_dir` correct on each.
  - `completion_dir_hides_a_broken_symlink` (`#[cfg(unix)]`, same as the symlink test above it — creating a dangling link needs `std::os::unix::fs::symlink`, and an ungated test would not compile on a Windows dev box) — a dangling symlink → absent, and no panic.
  - `completion_dir_truncates_at_the_cap` — more entries than a small `cap` → `truncated = true`.
  - `completion_degrades_to_empty_for_a_missing_directory` and `…_when_the_parent_is_a_file` and `…_when_tilde_has_no_home` → `(empty, false)`, no panic.
  - `completion_insert_never_contains_a_backslash` — assert no `\` in any `insert`, and add `abs_to_slash`'s own unit test asserting a `/`-rooted path round-trips unchanged. Annotate both the way `collect_files`' equivalent assertion is annotated: this is Windows-only protection that CANNOT fail on ubuntu CI (the `RootDir` branch never sees a `\` here), so the real guard is `abs_to_slash` emitting `/` for `Component::RootDir`. Do not let the comment imply the CI run exercises it.
- [x] Run `cargo test --lib commands::files` → all green, including the 39 pre-existing ones untouched.
- [x] `cargo clippy --all-targets` → 0 warnings. `cargo fmt` then `cargo fmt --check`.
- [x] Commit: `feat: list one directory for out-of-Space ? completion`

**Acceptance criteria:**
- [x] `expand_completion_root` and `completion_dir` are pure functions of `(query, home, cap)`; neither reads `HOME`, `dirs::`, or any env var.
- [x] The builder chain differs from `collect_files` in exactly `follow_links`, `max_depth`, and what is kept — everything else identical.
- [x] No frontend file touched. 760+1… Rust tests green, clippy 0, fmt clean.

---

### Task 2: the command and its TypeScript binding

**Context:**
Task 1's engine needs a Tauri command and a renderer-side binding. It follows `list_space_files` in this repo verbatim in shape, including its deliberate contract: **never return `Err`, degrade to an empty listing**, because a `?` picker that errors is worse than one that shows nothing — and log which arm fired, so a silent empty listing is never mysterious. The live `eprintln!` arms in `list_space_files` are the pattern to copy; read them in the file rather than hunting for the commit that introduced them.

**Files:**
- Modify: `src-tauri/src/commands/files.rs` (add the command)
- Modify: `src-tauri/src/lib.rs` (register it in the `invoke_handler` list, next to `list_space_files`)
- Modify: `src/lib/tauri.ts` (DTOs + `listCompletionEntries`, next to `listSpaceFiles` ~line 715)
- Test: `src-tauri/src/commands/files.rs` (`mod tests`), `src/lib/tauri.test.ts` if one exists for the module, else wherever `listSpaceFiles` is currently mocked (find it with `rg -n 'listSpaceFiles' src/`)

**What to implement:**

```rust
/// `?[~/…]q`, `?/…q` or `?C:/…q` — list ONE directory for an out-of-Space
/// completion (ADR 0035). `None` query = no active token = empty, and NO
/// filesystem access at all.
#[tauri::command]
pub async fn list_completion_entries(
    query: Option<String>,
) -> Result<CompletionDirDto, String>
```

- `None` → `CompletionDirDto { entries: vec![], truncated: false }` without touching the filesystem.
- `Some(q)` → `tauri::async_runtime::spawn_blocking` (the API this file already uses at `files.rs:324` — do not switch to `tokio::task::spawn_blocking` just because it also compiles) around `completion_dir(q, crate::skills::home_dir().as_deref(), MAX_PICKER_ENTRIES)` — note `home_dir()` is `pub(crate)` in `src-tauri/src/skills.rs`; call it, do not reimplement it.
- **Only the JoinError arm** degrades to the empty listing, with an `eprintln!` naming the query, exactly as `list_space_files`' failure arms do. `completion_dir` is infallible (it returns a tuple), so the `Ok` arm needs no log — an `Ok`-side log would fire on every directory fetch and bury the one message that matters.

In `src/lib/tauri.ts`:

```ts
export interface CompletionEntryDto {
  name: string; insert: string; display: string; isDir: boolean;
}
export interface CompletionDirDto { entries: CompletionEntryDto[]; truncated: boolean; }

export async function listCompletionEntries(query: string | null): Promise<CompletionDirDto> {
  // `null` is PASSED THROUGH as `{ query: null }`, exactly as `listSpaceFiles`
  // passes `spacePath: null` (pinned in `src/lib/tauri.test.ts`). Do NOT add a
  // client-side short-circuit: the Rust `None` arm already touches no
  // filesystem at all, and a short-circuit here would contradict the
  // established binding shape for no gain.
  return invoke<CompletionDirDto>("list_completion_entries", { query: query ?? null });
}
```

**What NOT to change:** `list_space_files`, `FileListDto`, the Space Listing's TS binding.

**Steps:**
- [x] Write the Rust test first — `list_completion_entries_degrades_to_empty_without_a_query` (await the command with `None` → empty, `truncated == false`) and `list_completion_entries_lists_one_directory` (a temp tree two levels deep → only the immediate children come back; this is the test that proves **no walk beyond one directory**, and it is the one the `done-when` hinges on). Run `cargo test --lib commands::files` and confirm RED.
- [x] Implement the command; register it in `src-tauri/src/lib.rs`.
- [x] Run `cargo test` (whole suite) → green.
- [x] Add the TS binding, and pin its wire contract the way `listSpaceFiles` is pinned in `src/lib/tauri.test.ts`: the command name, and `null` passed through as `{ query: null }`. Then update every test that mocks `invoke` wholesale so the new command name is covered where a real `invoke` could otherwise fire (this is how `listSpaceFiles` is handled — `rg -n 'listSpaceFiles' src/ --glob '*.test.*'` and follow it).
- [x] `pnpm test` and `pnpm build` → green. `cargo clippy --all-targets` → 0 warnings. `cargo fmt --check` → clean.
- [x] Commit: `feat: add the list_completion_entries command`

**Acceptance criteria:**
- [x] The command never returns `Err`. Its ONE failure arm (the JoinError) logs; the `Ok` arm does NOT — an `Ok`-side log fires on every directory fetch and buries the message that matters. (`list_space_files` has two failure arms because `canonicalize` can fail; this command has one, so do not copy its shape blindly.)
- [x] A `None` query performs no filesystem access.
- [x] `done-when`'s "no code path walks more than one directory" is pinned by a Rust test, not by a comment.

---

### Task 3: the grammar — two token shapes for `?`

**Context:**
`FILE_TOKEN_RE` in `src/lib/skills.ts` is `/(^|\s)\?([a-zA-Z0-9._/-]+)$/`. It has no `~`, so `~/…` cannot be typed; and it stops at whitespace, so a path containing a space cannot be typed either — which strands `~/Library/Application Support/…`, because selecting a row consumes the `?` and you cannot continue descending. The fix (ADR 0035) is TWO shapes: relative Space tokens stay strict, and an absolute one admits spaces and a Windows drive root. Nothing else about recognition changes, and `activeMentionToken`'s return shape `{ prefix, remainder, start }` is **unchanged** — the mode is derived from `remainder`.

**Files:**
- Modify: `src/lib/skills.ts`
- Test: `src/lib/skills.test.ts`

**What to implement:**

1. Add, immediately above the existing `FILE_TOKEN_RE`:

```ts
/**
 * The OUT-OF-SPACE `?` token (ADR 0035): a `/`-, `~/`- or drive-rooted path.
 * It admits SPACES, which the relative shape never may — `?foo bar` must not
 * eat the sentence, while `?~/Library/Application Support/Font Book` has to be
 * typeable. The `[A-Za-z]:/` alternation is NOT a nicety: on Windows `~/`
 * expands to `C:\Users\you`, so the absolute path a DIRECTORY row inserts is
 * `C:/Users/you/…`, and if `:` cannot form a token the kept-`?` token dies
 * after ONE level of descent — invisibly to ubuntu-only CI. Tried FIRST,
 * because `/` is also in the relative charset, so both shapes would match
 * `?/etc` and only this one means "leave the Space". `[^\n?]` keeps `??`
 * unformable and stops one absolute token swallowing a later `?`.
 *
 * ACCEPTED COST: admitting spaces means prose typed after an absolute token is
 * absorbed into the query until a newline. USUALLY harmless, and NOT because
 * an absorbed tail cannot match: `fuzzyMatch` is a SUBSEQUENCE test, so a
 * directory holding `todo list of items.md` DOES match the token plus the prose
 * ` list of items`, and Enter completes that row. Usually nothing matches
 * because the tail makes the segment longer than any entry name, the picker
 * renders nothing, and Enter sends. Accepted because it edits the user's own
 * draft rather than a policy, and because refusing spaces would refuse
 * `~/Library/Application Support`. (A revision of this plan said "an absorbed
 * tail matches no row", which a review disproved by naming one file.)
 *
 * ⚠ GROUP 2 MUST BE THE WHOLE REMAINDER. `activeMentionToken` returns `f[2]`
 * as `remainder`, so the OUTER group has to wrap the root AND the tail. If the
 * root alternation is the only group — `\?(~?\/|[A-Za-z]:\/)[^\n?]*$` — then
 * group 2 is just the root and the query is silently discarded: measured, that
 * form gives `?/etc/pas` → remainder `/`, while
 * `\?((?:~?\/|[A-Za-z]:\/)[^\n?]*)$` gives `/etc/pas`. The inner alternation is
 * written `(?:…)` so the group numbering stays obvious to a reader.
 */
const FILE_ABSOLUTE_TOKEN_RE = /(^|\s)\?((?:~?\/|[A-Za-z]:\/)[^\n?]*)$/;
```

2. In `activeMentionToken`, try the Mention regex (unchanged), then `FILE_ABSOLUTE_TOKEN_RE`, then `FILE_TOKEN_RE`. Both file branches return `prefix: "?"` — the mode is not part of the return value.
3. Export one tiny pure predicate so the composer can branch without re-parsing (it must accept the drive root too, or a Windows token parses but never enters the completion branch):

```ts
/** Is a `?` remainder an out-of-Space path (ADR 0035)? */
export function isOutsideSpaceQuery(remainder: string): boolean {
  return (
    remainder.startsWith("/") ||
    remainder.startsWith("~/") ||
    /^[A-Za-z]:\//.test(remainder)
  );
}
```

**What NOT to change:** `FILE_TOKEN_RE`'s charset (the RELATIVE shape keeps no `~`, no space, no `:` — the drive root lives only in the absolute shape), `expandMentions`, `splitMentionBlocks`, `MentionKind`, `MentionBlock`, `ComposerPrefix`, the Mention regexes, or `mergeDedupeKey`. `?` stays NOT a Mention. Adding `~`, spaces and a drive root to the *absolute* shape only is the whole charset change.

**Steps:**
- [x] **Read `src/lib/skills.test.ts` around the `a_tilde_never_makes_a_token` test first.** It currently pins `activeMentionToken("?~/.bashrc", …)` → `null`, with a comment saying a tilde path "cannot even form a token". That is the behaviour this task deliberately reverses, so the test MUST be rewritten as part of this task: keep its `?~` → `null` case (still true — a bare `~` opens nothing), flip its `?~/.bashrc` case to a recognised out-of-Space token, and rewrite its comment. Do this in the same commit, or you will hit a red that no instruction explains. **(That rewrite renamed the test: it is `a_bare_tilde_is_null_but_a_tilde_slash_path_is_an_out_of_Space_token` in the tree that ships, and `a_tilde_never_makes_a_token` no longer exists. Read the current name, not this one.)**
- [x] Write the new tests in `src/lib/skills.test.ts`. Note that `?/etc` ALREADY returns `prefix "?"` today, so a bare "is it recognised" assertion is vacuous — assert on `isOutsideSpaceQuery(remainder)` as well:
  - `?README` and `?src/comp/Form` → recognised, `isOutsideSpaceQuery` false.
  - `?/etc/pas` → recognised, **remainder `/etc/pas` and NOT `/`**, outside true. Write this assertion first and watch it fail with remainder `"/"` if the outer group does not wrap the whole remainder — that is the trap described above. `?/` → recognised, remainder `/`, outside true.
  - `?~/.config/ht` → recognised, remainder `~/.config/ht` (full, not `~/`), outside true.
  - `?~user/x` → **NOT a token at all**: the root needs `~/` and the relative charset has no `~`, so another user's home cannot even be typed.
  - `?a~b` → **NOT a token at all** (`~` is in the absolute shape's ROOT only and is not in the relative charset, so no picker opens). Do not write this as "recognised but relative" — it is not recognised.
  - `?~` and `??` and bare `?` → NOT a token (Enter must still send).
  - `a?.b`, `x ? y`, `url?query` → not a token (regression pins for the boundary).
  - `?~/Library/Application Support/Font` → recognised, remainder includes both spaces.
  - `?foo bar` → NOT a token (the relative shape refuses the space).
  - `?/a/b ?/c` → the token's remainder does not contain a second `?`.
  - `?C:/Users/you/` → recognised, outside true (the Windows descent case; it parses on Linux even though Linux has no such path, which is the point — CI can never execute it).
  - a token does NOT survive a newline: `"line one\n?~/notes"` recognises `~/notes` from the newline boundary, and `?~/a\nb` does not extend across the break.
- [x] Run `pnpm test src/lib/skills.test.ts` → RED, then implement, then green.
- [x] `pnpm test` (full) and `pnpm build` → green.
- [x] Commit: `feat: let a ? token name any directory`

**Acceptance criteria:**
- [x] Every pre-existing `?` and Mention recognition test passes untouched, **with the single sanctioned exception of `a_tilde_never_makes_a_token`**, which this task rewrites by design. If any OTHER pre-existing test reddens, stop — that is a regression, not a re-pinning. **(Same correction as the step above: the rewritten test is `a_bare_tilde_is_null_but_a_tilde_slash_path_is_an_out_of_Space_token`.)**
- [x] `??` and a bare `?` still do not open the picker.
- [x] `activeMentionToken`'s return type is unchanged.
- [x] A drive-rooted remainder is recognised by BOTH the regex and `isOutsideSpaceQuery`, and group 2 carries the FULL remainder (`?/etc/pas` → `/etc/pas`).

---

### Task 4: the completion catalog hook

**Context:**
The Space **Listing** is fetched once per Space and cached, and `useCatalogPayload` in `src/hooks/useMentionCatalogs.ts` is that machinery — already generic over a payload, already keyed, already evicting the key you left, already guarding a stale response, already degrading a failed fetch to a stable empty value. Directory completion needs the same thing with a different key: the token's **directory prefix** instead of the Space. Reusing it is what makes "no refetch while you type inside one directory" true for free, and it is why the engine returns a whole unfiltered directory (Task 1). Do not write new freshness machinery.

**Files:**
- Modify: `src/hooks/useMentionCatalogs.ts`
- Test: wherever `useMentionCatalogs` is tested today (`rg -n 'useMentionCatalogs' src/ --glob '*.test.*'`), plus the composer tests that mock catalogs

**What to implement:**
- `const completionCache = new Map<string, Promise<CompletionDirDto>>();`
- `const EMPTY_COMPLETION_DIR: CompletionDirDto = { entries: [], truncated: false };` — module-level, stable identity, for the same reason `EMPTY_FILE_LIST` exists (read that comment; a fresh literal here creates an infinite render loop).
- ```ts
  /** The out-of-Space completion rows for ONE directory (ADR 0035). `null`
   *  = no out-of-Space token open; it maps to the `__global__` cache key and
   *  the command's `None` arm, which touches no filesystem at all. */
  export function useCompletionDir(dirPrefix: string | null): CompletionDirDto
  ```
  implemented as `useCatalogPayload(completionCache, dirPrefix, listCompletionEntries, "listCompletionEntries", EMPTY_COMPLETION_DIR)`. **Do NOT add a client-side short-circuit for `null`** — `useCatalogPayload` calls `fetch` on every cache miss, so the wrapper IS called with `null` (exactly as `listSpaceFiles(null)` is called for a Space-less window). The guarantee you are relying on is Rust-side, and Task 2 pins it there.
- Extend the existing test cache-clearing helper to clear `completionCache` too.

**What NOT to change:** `useMentionCatalogs`' return shape, `agentsCache`/`mcpCache`/`filesCache`, `useCatalogPayload`'s deps array or its eviction rules.

**Steps:**
- [x] Write the tests first:
  - a mount with `dirPrefix = null` renders nothing and returns `EMPTY_COMPLETION_DIR` — assert the returned value and its identity, NOT "`listCompletionEntries` was not called": the hook calls it on every cache miss, so that assertion can never go green;
  - the same `dirPrefix` across two renders invokes the fetcher **once** (the cache dedupes);
  - a `dirPrefix` change serves the empty value immediately and then the new payload;
  - a rejected fetch degrades to `EMPTY_COMPLETION_DIR` and logs;
  - a response for a key already left behind is not applied.
  - **Do not try to prove "typing inside one directory does not refetch" here.** With `dirPrefix` as a hook ARGUMENT, that hook-level test is vacuous — it proves only that the same input gives the same output. The property lives in `ChatStream`'s DERIVATION of `dirPrefix` from the token, and Task 5 pins it where it actually lives.
- [x] `pnpm test` → RED then green. `pnpm build` → green.
- [x] Commit: `feat: cache completion rows per directory`

**Acceptance criteria:**
- [x] `useCatalogPayload` is unchanged. **(Corrected after review: true of its LOGIC at this commit, and it is no longer the whole story. One comment inside it was rewritten, and `560f610` later widened its return from a bare payload to `{ payload, pending }` so the composer could tell "fetching" from "found nothing". Its deps array and eviction rules remain untouched, which is what this box was really guarding. The same applies to the signature in the code block above: `useCompletionDir` now returns `{ dir: CompletionDirDto; pending: boolean }` and `ChatStream` destructures `.dir` / `.pending`, so that block is Task 4's history and not the shipped signature.)**
- [x] `EMPTY_COMPLETION_DIR` is a module constant and is the only empty value passed.
- [x] No filesystem access when there is no out-of-Space token (pinned Rust-side in Task 2; the hook passes `null` through by design).

---

### Task 5: wire the rows and the two insertion rules

**Context:**
The picker is built in `ChatStream.tsx`: `pickerRows` (a `useMemo`, ~line 452) maps a catalog to `MentionRow` and caps `?` at `MAX_PICKER_ROWS`, and `selectMention` (~line 1035) inserts `${row.name} ` for a `?` row — the trigger consumed, case preserved. Out of the Space two things differ (ADR 0035): the row must DISPLAY `~/…` while INSERTING the absolute path, and a **directory** row must keep the `?` and append `/` so the descent continues — otherwise selecting a directory consumes the trigger, ends the token, and the user is stranded after one level.

**Files:**
- Modify: `src/components/chat/ComposerMentions.tsx` (`MentionRow` gains two optional fields; render the label)
- Modify: `src/components/ChatStream.tsx` (`pickerRows`, `pickerNote`, `selectMention`)
- Test: `src/components/ChatStream.test.tsx`

**What to implement:**

1. `MentionRow` gains `label?: string` (what the primary line shows; defaults to `name`) and `isDir?: boolean`. `ComposerMentions` renders `row.label ?? row.name` and appends `/` when `isDir`. `description` stays `""` for every `?` row — rows are single-line by decision, do not reintroduce a second line.
2. In `ChatStream.tsx`, derive from the active `?` token:
   - `outside = isOutsideSpaceQuery(token.remainder)`
   - `dirPrefix` = the remainder up to and including its last `/` (`~/.config/ht` → `~/.config/`; `~/notes` → `~/`; `/etc/pas` → `/etc/`), `null` when not `outside`;
   - `segment` = the remainder after the last `/` (what the rows get filtered by).
3. `const completion = useCompletionDir(dirPrefix);`
4. In `pickerRows`, split the `prefix === "?"` branch on `outside`:
   - outside → **filter the DTO entries FIRST, on each entry's own `name`, then map**: `completion.entries.filter((e) => fuzzyMatch(segment, e.name))`, sorted **directories before files**, then `.slice(0, MAX_PICKER_ROWS)`, each mapped to `{ key: e.insert, prefix: "?", name: e.insert, label: e.display, isDir: e.isDir, description: "" }`. Filtering the absolute `insert` instead of `name` is a real bug, not a stylistic one: the path is a strict superset of its own basename, so path-filtering offers rows basename-filtering would drop.
   - **Return `matched`** (the unbounded filtered count) from this branch too — `pickerNote`'s "too many matches" arm reads it (`ChatStream.tsx:585`), and an outside branch that omits it silently kills that note.
   - Add `completion` to the `useMemo` deps (`ChatStream.tsx:558`, currently `[skills, agents, mcpServers, files, mcpMentionsEnabled, picker, draft]`). Without it the rows never re-derive when a fetch lands, which looks like a dead picker rather than a bug.
   - **Rewrite the `pickerRows` comment block above it** (`ChatStream.tsx:490-499`), which describes the `?` rows as carrying "the relative path" as the name — after this task an outside row carries an ABSOLUTE path in `name` and the abbreviated `~/…` in `label`.
   - not outside → today's code, untouched.
5. `pickerNote`: keep both existing strings and their precedence, and make the capped arm read the RIGHT source — `outside ? completion.truncated : files.truncated`. A plain `files.truncated || completion.truncated` would show the Space's cap note while the user is completing somewhere else.
6. `selectMention`, the `?` branch only:
   - `row.isDir` → insert `` `?${row.name}/` `` (NO trailing space, the `?` KEPT), then instead of `setPicker(null)` recompute the token at the new caret (`token.start + inserted.length`) with `activeMentionToken` and `setPicker(next)` so the picker stays live on the new directory.
   - otherwise → insert `${row.name} ` exactly as today (trigger consumed, path + space), `setPicker(null)`.
   - The three Mention prefixes are untouched.

**What NOT to change:** the render gate `open && (filtered.length > 0 || note)`, the height clamp, the scroll effect and its `[activeIndex, filtered]` deps, `role="status"` on the note, `MAX_PICKER_ROWS`, arrow-wrap, Enter-sends-when-empty, or the two note strings.

**Steps:**
- [x] Write the tests first in `src/components/ChatStream.test.tsx` (mock `listCompletionEntries` the way `listSpaceFiles` is mocked in this file):
  - `?~/.config/ht` renders rows from the completion payload, labelled `~/…` with `/` on directories, and NOT two-line;
  - selecting a directory row leaves `?/home/u/.config/htop/` in the textarea with a live picker (assert the draft AND that a row is still rendered);
  - selecting a file row inserts the absolute path with no `?` and closes the picker;
  - rows are filtered on the entry's own name, and the test must DISCRIMINATE: with `dirPrefix` `/home/u/.config/` and segment `eu`, a name like `htop` must NOT be offered even though the absolute path `/home/u/.config/htop` contains `e`…`u`. If the implementation filters the absolute path, every row is offered and this test reddens — a fixture without that property proves nothing;
  - **the design-critical no-refetch property, pinned where it lives**: set the draft to `?~/.config/ht` in ONE `change` event (`fireEvent.change(textarea, { target: { value: "?~/.config/ht" } })`) — NOT character by character; per-keystroke typing legitimately fetches twice, once for `~/` and once for `~/.config/`, and the assertion would then redden for a reason that has nothing to do with the bug. **Count calls per argument, not total calls**: the component mounts with an empty draft, `dirPrefix` is `null`, and Task 4 already establishes that the hook DOES call the fetcher with `null` on that cache miss — so a bare `toHaveBeenCalledTimes(1)` is false from the first render. Assert exactly one call **with `~/.config/`**, over the mock log filtered to non-null queries. THEN append one character via a second `change` and assert STILL exactly one non-null call, still with `~/.config/`. This is the derivation of `dirPrefix` (up to and including the last `/`), not the hook's caching — a regression that keyed the cache on the whole remainder passes every other test in this file;
  - a directory row whose `insert` is `C:/Users/you/proj/` leaves a token that `activeMentionToken` STILL recognises after insertion (the Windows descent case — assertable in jsdom on Linux, because it is a grammar property, not a filesystem one);
  - directories sort before files;
  - at most 10 rows; the capped note appears for `completion.truncated`, and the Space's cap note does NOT appear while an outside token is active;
  - `?README` still offers Space Listing rows, unchanged (a regression pin on the branch split);
  - a relative `?query` renders Space Listing rows and never enters the completion branch.
- [x] `pnpm test src/components/ChatStream.test.tsx` → RED, implement, green.
- [x] Full `pnpm test` + `pnpm build` → green.
- [x] Commit: `feat: complete ? across directories in the picker`

**Acceptance criteria:**
- [x] A directory selection leaves a live `?` token; a file selection consumes it.
- [x] Typing inside one directory fetches once (pinned at the derivation, not the hook).
- [x] Space Listing behaviour is bit-identical (its tests pass unchanged).
- [x] No new user-facing string. **(Wrong, and corrected after review: a directory row now RENDERS a trailing `/`, from `primaryLabel` in `ComposerMentions.tsx` — the one helper that builds the row's primary line, shared with its `title` tooltip; it was called `rowLabel` when this box was first corrected. It is one character, it is not translatable, and it is the only new string this feature puts on screen, but it is a user-facing string and this box said there was none.)**
- [x] The `pickerRows` comment no longer says `?` rows carry a relative path.

---

### Task 6: consistency sweep and the gate

**Context:**
This branch's predecessor needed three separate commits to fix overclaims in its own comments and docs — including a comment claiming coverage a mutation test disproved. Do the sweep while it is cheap, and never assert in a comment what a mutation would disprove.

**Files:**
- Modify: whatever the sweep finds, in `src/`, `src-tauri/src/`, `CONTEXT.md`, `docs/`
- Modify: `docs/roadmap/out-of-space-file-completion.md` (tick what you did)

**Steps:**
- [x] Sweep with a wider pattern set than the obvious one: `rg -n 'Space only|Space-only|lists nothing|cannot even form|relative path|mid-sentence|~ cannot' src/ src-tauri/src CONTEXT.md docs/` and check every hit against the shipped code. Two known-bait targets: sentences written before this feature that say the picker can only see the Space, and `ChatStream.tsx`'s `pickerRows` comment describing `?` rows as carrying a "relative path" (Task 5 rewrites it — confirm it actually did). Do NOT edit a merged ADR's body (append a dated note in ADR 0011's voice instead).
- [x] For every claim a new comment makes about what a test proves, verify it by mutating the code and confirming the test reddens. If a claim cannot survive mutation, fix the claim, not the test. Record in the comment which mutants you ran. Start with the three that this plan's own review found people getting wrong: that absolute-path filtering is equivalent to name filtering (it is not), that a hook-level caching test proves the no-refetch property (it does not), and that a `null` mount proves no call was made (the hook does call).
- [x] Re-confirm `CONTEXT.md`'s **File completion**, **Directory completion** and **Listing** entries and ADR 0035 against the shipped code. They were corrected twice already — `?a~b` forms NO token (it is not "a Space query"), drive roots ARE in the absolute shape and in the **Directory completion** entry's opening sentence (not a documented gap), and an absolute token DOES absorb trailing prose. **(Corrected after review on the last clause: the prose runs to a NEWLINE, and a `?` inside it does not end the token, it DESTROYS it — `?/etc/pas?x` forms no token at all. "Until a newline or a `?`" made the second case sound like a clean stop. The Post-review section below records the pass this step missed.)**
- [x] Run the full gate: `pnpm test`, `pnpm build`, then from `src-tauri/` `cargo test`, `cargo clippy --all-targets` (0 warnings), `cargo fmt --check`.
- [x] Leave the manual-smoke box **open** unless you can actually run the app in a window manager — visual claims (the label truncating `~/…` at 10 rows, the descent feeling right) must not be ticked on the strength of tests. Say so in the doc rather than ticking it. Done: swept, mutation-battered and re-read; the box below is what was NOT done.
- [ ] **Manual smoke, in a real window: NOT DONE, deliberately open.** Nobody on this branch has run the app under a window manager, so these claims rest on tests plus reading and are NOT verified as SEEN: the `~/…` label actually reading well at 10 rows and truncating gracefully at a narrow chat width; the descent feeling immediate rather than laggy (one synchronous-ish fetch per directory, but perceived latency is a feel judgment no assertion makes); a real `read` of an inserted absolute path by the AGENT (the round trip is tested against the REAL Rust engine for the LISTING side and against the real grammar for the token side, but the agent opening the inserted path in a live session is the one link nothing here observes); and how a real `node_modules`-sized directory LOOKS when it hits the 5,000-entry cap. The `done-when` sentence's "the agent's own `read` opens on the first try" is therefore the ONLY clause this branch cannot close from CI.
- [x] Commit: `docs: reconcile the out-of-Space completion docs and sweep for overclaims` — ticked, and that is self-referential by nature (a commit cannot contain evidence of its own existence), so the check is external: `git log -1 --format=%s` prints exactly the subject named here.

**Acceptance criteria:**
- [x] No surviving sentence says the picker can only see the Space, or that a tilde path cannot form a token. (Two did, both true before this branch and both now corrected: **File completion**'s opening in `CONTEXT.md` said the picker "lists files in the active **Space**", and **ADR 0033**'s Decision said "**Allow `~` and absolute paths**" — the latter reversed by a dated note, since a merged ADR's body is append-only.)
- [x] Every "pinned by test" comment survives its own mutation. (One did not, and it was this branch's own prose in `ChatStream.tsx`: the arm-order comment claimed "the order changes no behaviour" while pointing at a test-only constraint. Measured: swapping the two `?` arms reddens **fourteen** `ChatStream.test.tsx` tests, because the Listing arm is guarded by `!outside` and the out-of-Space arm is the unguarded fall-through — so putting it first swallows relative queries. The comment now states the asymmetry as a behavioural constraint, with the guard it would need before any reorder. Separately, the source-reading pin in `skills.test.ts` used to add a SECOND, textual reason for that order (it scanned forward from the `prefix === "?"` gate, so a swap broke it with `ReferenceError: segment is not defined` — reproduced against the shipped version); it now anchors on each arm's own declaration and binds each predicate's own parameter names, so it no longer depends on the order, which leaves the behavioural reason as the only one. Also corrected: a Rust note saying `max_depth(Some(2))` puts the grandchild "appended" (measured: it is INSERTED in walk order, between the child directory and the root-level file), and a note saying that mutation "reddens BOTH" tests. **(Re-measured in the closing pass: `max_depth(Some(2))` now reddens SIX tests — the two whole-list engine pins first recorded, plus `completion_dir_lists_a_directory_named_git_when_it_is_the_one_queried`, `completion_dir_offers_dot_named_files_and_directories`, `completion_dir_refuses_names_the_token_grammar_cannot_hold` and the command-level `list_completion_entries_lists_one_directory` — and the `files.rs` note no longer states a count at all, precisely because every one of these counts has been wrong once already. The same applies to "fourteen" above: re-measured by swapping the two `?` arms again, it is now FIFTEEN `ChatStream.test.tsx` tests, and the comment in `ChatStream.tsx` still says FOURTEEN — which is the countdown working as advertised. What is durable in both places is the WHY (the `!outside` guard's asymmetry; a whole-list assertion that a deeper walk must break), never the number.)**
- [ ] `CONTEXT.md` and ADR 0035 agree with the shipped grammar, including the drive root. **Left OPEN on purpose, because this box was once ticked on a claim that was not true.** ADR 0035 had been corrected to the shipped rule (the read takes the directory the token names: its parent while a last component is being typed, ITSELF once the trailing `/` is) while `CONTEXT.md`'s **Directory completion** entry still said "the one named by the token's **parent**" after the box was checked. The Post-review section below fixes that entry and seven other drifted sentences, so the claim is now true of the tree. It stays unticked anyway: this is the second time this box has been ticked from inside the pass that could see only part of the diff, and the property it asserts (docs match code) belongs to a reader who can look at both without that constraint. Re-read the three glossary entries, ADR 0035 and this plan against `files.rs`, `skills.ts` and `ComposerRow.tsx`, then tick it.
- [x] Full gate green.

---

## Post-review changes (2026-10-10)

A review of the whole branch found five behaviours that were wrong rather than undocumented, and a set of sentences that were wrong in the other direction. All five are fixed; the code this plan described in Tasks 1-5 is not the code that ships.

- **`237cbad`: the cap may no longer drop a DIRECTORY.** `completion_dir` used to fill `MAX_PICKER_ENTRIES` — 5,000 slots, the cap it has always had on this branch — in walk order and `break`. Directories are how this mode MOVES, so a directory whose first entries are files rendered a picker with no way down. Directories now claim their slots first and files get what is left, with `truncated` computed from that arithmetic instead of from the early `break`.
- **`8528830`: a completion root must be ABSOLUTE.** `expand_completion_root` had a byte test that said a query LOOKED like a Windows drive root, and on Unix `PathBuf::from("C:/x")` is relative: with a literal `C:` directory in the process working directory, `C:/secret/` listed files there and `C:/../../../etc/` listed `/etc` through it, both reporting an `insert` that claimed to be absolute. The function now returns nothing unless the expansion is absolute, so a drive query is refused where it names nothing.
- **`a540e23`: a name the token grammar cannot carry is refused, in both walks.** A file named `notes?draft.md`, or one whose name holds a newline, produced a row that formed no token once inserted and could not be continued. `name_is_completable` is applied as a prune, so a directory with such a name is not descended into either. It is a REPRESENTABILITY rule (`?` and a newline are the only two bytes the grammar cannot carry) and not a naming convention, which is why it is not the skip-list ADR 0034 deleted.
- **`560f610`: `Enter` is held while a descent's rows are in flight.** Selecting a directory leaves the picker with no rows and no note for one fetch, so the box did not render, so nothing intercepted `Enter`, and the file path the user had just asked for went to the model as raw text. `useCatalogPayload` now reports `pending` alongside its payload and the composer holds `Enter` for exactly that round trip. A REJECTED or EMPTY read counts as settled, so a directory that cannot be read never holds `Enter`.
- **`bb210c0`: the directory is the whole remainder's last `/`, not its first space.** `2562452` tried cutting the query at the first space; that sent the picker into `~/Library` for `?~/Library/Application Support/`, which is the exact class of path this mode exists to reach. Reverted: `dirPartOfOutsideQuery` slices to the last `/` of the entire remainder.

Three more commits tightened the surface without changing a rule: `c772087` elides a long path in the row instead of clipping it, `86afd0d` pins the picker's two filters by driving the picker rather than by reading its source, and `2e9b9c3` pins that the 10-row render cap cannot drop a directory row.

### Docs reconciliation (this commit)

The same review found nine sentences in `CONTEXT.md`, ADR 0033, ADR 0034, ADR 0035 and this plan that the shipped code contradicts. Every one was checked against the code, and against a mutation of the code, before being rewritten; the wording changes are in the commits that changed the behaviour, and this pass carried them into the glossary and the two older ADRs. In short: `?/` did not "offer nothing" on `main` (it matched every multi-segment path in the Space, so this is a scope change and not a dead-key repair), an absorbed prose tail CAN match a row (the matcher is a subsequence test and the directory's contents decide), a queried `.git` IS listed (the prune is an entry rule below the queried root), the peek is decided by an ancestor repository's `.gitignore` and `.git/info/exclude` and not only by `.ignore`, the read takes the token's DIRECTORY PART rather than its "parent", nothing-about-caching was wrong (rows are cached per directory prefix and the left prefix is evicted), a file row inserts a trailing space, and `..` is not a missing traversal gesture. Three tests were added where a corrected sentence had no witness at all: `completion_dir_reads_the_nearest_ancestor_repositorys_exclude_and_stops_above_it`, `completion_dir_lists_the_parent_while_a_last_component_is_being_typed`, and `completion_dir_offers_a_non_utf8_name_with_a_lossy_insert`. Each was checked by mutating the code it pins and watching only that test redden.

- [ ] **Manual smoke, still open, and now more open than before.** The box above this one is untouched on purpose. Nobody has run this branch in a window manager, and the five fixes above are exactly the kind that a test cannot close: whether holding `Enter` for one round trip FEELS right or feels like a stuck key, whether an elided path reads, whether a directory big enough to hit the 5,000-entry `MAX_PICKER_ENTRIES` cap while holding only 3 directories is navigable in practice (the sibling shape — directories lost to a walk-order cut — was measured in `/usr/lib64`: 45 of its 199 directories, before `237cbad`). Run the app before this plan is believed, not before it is merged.

### Closing pass (2026-10-10, adversarial verification)

A verification pass found no behavioural defect. It found one test gap and three sentences that measurement had disproved; all four are closed.

- **The DIRECTORIES half of `truncated` had no witness at all.** Mutating `completion_dir` to `truncated = file_keep < file_total` — deleting the directories' term — left all 798 Rust tests green, because every existing fixture that drops directories ALSO drops files, and the files term was true wherever the directories term was. `completion_dir_reports_truncation_when_only_directories_are_dropped` is the missing fixture: a directory holding MORE directories than the cap and no files, asserting the first `cap` in sorted order AND `truncated`. Under that mutation it is the suite's only red test, which is the evidence that it is not decoration.
- **ADR 0035's `?~/notes no` worked example was false.** `dirPartOfOutsideQuery("~/notes no")` is `~/` — there is no `/` after `notes` to cut at — so that token reads the HOME directory and filters it on the segment `notes no`. Corrected in place to the shape that is true (`?~/notes/ no` reads `notes`; `?~/notes no` reads `~/`), keeping the point the sentence was making: absorbed prose changes which ROWS match, and moves the read only when it carries a `/` of its own.
- **`completion_dir`'s cost sentence overreached on CPU.** "Costs nothing the sorted walk was not already paying" was true of PEAK MEMORY (sorted mode materialises and sorts the directory before entry #1 — `walkdir-2.5.0` source and RSS agree) and false of WALL CLOCK: ~37 ms with the old break-at-cap loop against ~108 ms for the full scan of a 55,000-entry directory, ~280 ms at 100,000. The comment now states both axes, says the sort is only about a tenth of the bill, and names the consequence `560f610` created — that window is exactly when `Enter` is held, so a huge directory is also a longer held keystroke. ADR 0035's held-`Enter` bullet now says the same.
- **Three stale claims, plus a sweep.** The `max_depth` note said a mutation "reddens THREE tests" (measured: SIX at this HEAD — the note now names them and states no count); this plan said the one-directory cap gives "2,000 slots" (it is `MAX_PICKER_ENTRIES` = 5,000, and `git log -S` shows 2,000 never existed in `files.rs`); and this plan named `rowLabel` where the helper is `primaryLabel`. Also corrected here: the "fourteen" arm-swap count, which re-measures as fifteen (`ChatStream.tsx`'s own copy of that number still says fourteen — outside this pass's files, same countdown hazard), the renamed `a_tilde_never_makes_a_token` (now `a_bare_tilde_is_null_but_a_tilde_slash_path_is_an_out_of_Space_token`), `useCompletionDir`'s signature (it returns `{ dir, pending }`), and the plan's own baseline counts. **Checked and correct as written:** `MAX_PICKER_ROWS` = 10 and every "10 rows"/"10-row" claim; the "33 deleted `docs/roadmap/*.md`" count; the five Post-review fix commits and every other cited SHA; every Rust and TS test name cited in either doc (all resolve); every quoted `ChatStream.test.tsx` title (all resolve); `dir_keep = dirs.len().min(cap)`; both token charsets as printed; `home_dir`'s `pub(crate)` visibility; `Sandboxed` as a policy name; `FsBackend::resolve`'s shared-seam wording; and the `250,000`-entry ~26 MiB peak-memory figure, which survives because it is a memory claim. Left alone deliberately: the plan-time line references (`files.rs:189`, `files.rs:324`, `ChatStream.tsx:558`/`:585`/`:490-499`, `tauri.ts ~715`) — all stale by construction, since they pointed at the tree each task started from, and this file is deleted at ship.


### What happens to this file

**This plan is deleted when the feature ships**, in the merge commit, not kept as a record. That is the convention `AGENTS.md` states and 33 deleted `docs/roadmap/*.md` files in this repo's history follow: the durable decisions are in ADR 0035 (with pointers from ADR 0033 and ADR 0034), the vocabulary is in `CONTEXT.md`, and a plan that survives its own merge becomes a second, staler account of the same behaviour. This section exists so the review's findings are attributable while the branch is open, which is the only window in which they are load-bearing.
