---
status: committed
done-when: In a repo with build output, `?` offers source files instead of artifacts — in this repo NO build-output path (`src-tauri/target/**`, `dist/**`) can be offered at all, and the Listing is the repo's non-ignored files minus ONLY the VCS metadata directories (`.git`, `.hg`, `.svn`) — re-measured 2026-10-09 after Task 6 (`2e63869`) at 426 entries with `truncated = false`, reconciling exactly with `git ls-files` (426 tracked), with nothing untracked-and-not-ignored in the tree at that moment and with the three dot-DIRECTORY files this plan once called unlistable now listed (`.github/workflows/ci.yml`, `.github/workflows/release.yml`, `.vscode/extensions.json`; the only other dot-named entries are the dot-FILES `.gitignore` and `src-tauri/.gitignore`, which were always listable). That exit condition is NOT "the tracked set" — untracked-but-not-ignored files ARE listed and any tracked file an ignore file excludes is NOT, so the two counts matching is a property of one tree at one moment rather than a rule — and the ~12,625 / ~424 pair quoted in the tasks below is a dated design-time measurement, not a live figure. Also, the `?` picker is at most 10 single-line rows (~308px) while an uncapped mention list scrolls inside a height-clamped box.
---

# The `?` file listing: repo-decided visibility, and a picker that stops taking over the window

**Goal:** Make the `?` file-completion listing show files a human would choose (the repo's own ignore rules decide it) and stop the picker from filling the window.

**Architecture:** `collect_files` in `src-tauri/src/commands/files.rs` swaps `walkdir::WalkDir` for `ignore::WalkBuilder` (the walker ripgrep is built on — a library, statically linked, nothing the user installs), so `.gitignore` / `.ignore` / `.git/info/exclude` prune the listing while dot-FILES stay listable and the user's machine-wide gitignore is never read — and, apart from those repo rules, ONE app-level rule the repo cannot override stays in force: the **VCS metadata directories** (`.git`, `.hg`, `.svn`) are pruned at any depth, which is what keeps `.git/` unwalked and is the one thing no ignore file can be asked to exclude (`git` does not ignore its own metadata). So visibility is repo rules PLUS that one prune, never the repo alone — and nothing ELSE is pruned by name, no dot-directory rule and no `node_modules` / `dist` / `target` blacklist. *(As written this line said every dot-**directory** is pruned, which made `.github/` unlistable; Task 6 narrowed it at `2e63869`.)* Frontend: the `?` row cap drops 100 → 10, `?` rows render on one line, and the picker box gains a font-size-relative `max-height` + scroll with the active row scrolled into view.

**Tech Stack:** Rust 2021 (Tauri 2, pinned toolchain in `src-tauri/rust-toolchain.toml` — do not float it), the `ignore` crate 0.4.x; React 19 + TypeScript + Tailwind **v4** + Vitest.

**Decisions already made — do not re-litigate:** ADR 0034 (`docs/decisions/0034-the-file-listing-is-decided-by-the-repo.md`) decides visibility and names its rejected options; ADR 0033 (`0033-file-completion-inserts-a-path-not-content.md`, partially superseded by 0034) still governs what `?` IS. The **Listing** term and its three axes (scope / visibility / reach) are in `CONTEXT.md`.

---

## Repo-wide rules for every task

- **Two validation roots.** Frontend from the repo root (`pnpm test`, `pnpm build`); Rust from `src-tauri/` (`cargo test`, `cargo clippy --all-targets` must be **0 warnings**, `cargo fmt --check`). `cargo` from the repo root FAILS — there is no `Cargo.toml` there.
- **Clippy caches.** If `cargo clippy` finishes in well under a second with no `Compiling` line it replayed a cache and has proven nothing; `touch` the touched `.rs` files (or `cargo clean -p archimedes -p archimedes_lib`) and re-run.
- **Commit messages use `git commit -F -` with a quoted heredoc (`<<'MSG'`)** — NEVER `git commit -m "…"`. A backtick inside a double-quoted message is command substitution and silently eats the text, and this feature's prose is full of `$ @ # ?`.
- **TDD:** write the failing test first, run it, confirm it fails for the expected reason, then implement. **Exception, and it is not a licence to skip the step:** a test that asserts something is *still allowed* (a whitelist / negative pin) is EXPECTED to pass before the implementation too, because in the all-rules-off starting state nothing filters anything. Only "must be absent" assertions can fail first. Do not manufacture a failure for a pin, and do not treat a pin passing early as a broken fixture.
- **Known pre-existing flakes** (if one is the ONLY failure, re-run once before believing it): `src/components/settings/SettingsPage.test.tsx`'s font-picker ordering test (a `setTimeout(0)` waiting for Radix to un-`aria-hide`), and Rust `agent::mcp::http::tests::http_session_id_is_resent` (`src-tauri/src/agent/mcp/http.rs`, a loopback timing race).
- Baselines, verified by running both suites: **1408** frontend tests (72 files), **740** Rust tests.

---

### Task 1: Swap the walker to `ignore`, keep every current guarantee

**Context:**
The `?` picker's listing is produced by `collect_files`, which walks with `walkdir` and skips directories via `PICKER_SKIP_DIRS` — a list containing exactly one entry, `"node_modules"`. That is a hand-rolled approximation of `.gitignore`, and it is why this repo's picker could reach 12,625 files of which 11,880 were build output — a DESIGN-TIME measurement (2026-10-09), since overtaken by more builds (`src-tauri/target` alone is past 30,000 files now); the load-bearing fact is the ratio, not the number. This task replaces the engine WITHOUT yet turning on any ignore rules, so the change is verifiable as "same behaviour, better engine" apart from the one deliberate removal. Turning on the repo's ignore rules is Task 2; doing them together would make it impossible to tell a walk-engine regression from an ignore-rule regression.

**Critical, and easy to get wrong:** `ignore::WalkBuilder` turns **every** filter ON by default — `hidden`, `parents`, `git_ignore`, `git_exclude`, `git_global` **and `.ignore` files**. Task 1 must therefore disable all six, `.ignore` included, or the "same behaviour" premise is false from the first commit: a `.ignore` file in a Space would start pruning the listing during Task 1, and several of Task 2's tests would pass before Task 2 exists.

`PICKER_SKIP_DIRS` is deleted in this task, including its `node_modules` entry: a repo that *commits* `node_modules` must be able to list into it, and once ignore rules exist in Task 2 the repo has a real way to say what is not source.

**Files:**
- Modify: `src-tauri/Cargo.toml` (add the dependency, next to the existing `walkdir = "2"` block)
- Modify: `src-tauri/src/commands/files.rs` (`collect_files`, `is_skipped_dir`; DELETE `PICKER_SKIP_DIRS`)
- Test: `src-tauri/src/commands/files.rs` — the file already has exactly ONE `#[cfg(test)] mod tests` (at ~line 205). Do NOT add a second; it is a compile error.

**What to implement:**
1. Add `ignore = "0.4"` to `[dependencies]` in `src-tauri/Cargo.toml`, with a comment saying it is the walker ripgrep is built on, a library (no user-installed binary), that it decides the `?` listing's visibility (ADR 0034), and **the version this plan was verified against (`0.4.33`)** — the crate's semantics are load-bearing here and the house style is a float, so the verified version is the only reproducible record we keep. Keep `walkdir` — `agent/tools/exec.rs` still uses it.
2. In `files.rs`, delete `pub const PICKER_SKIP_DIRS` entirely and rewrite `is_skipped_dir` to the dot-directory rule alone: `(depth > 0 && name.starts_with('.'))`. KEEP the existing depth-0 comment (a Space opened at `~/.dotfiles` must not prune its own root). *(As executed at c49d43d. Task 6 later narrowed the rule to `PRUNED_VCS_DIRS` — `.git` / `.hg` / `.svn` — while keeping this task's depth-0 exemption exactly as it asked.)*
3. Replace the `walkdir::WalkDir` builder in `collect_files` with `ignore::WalkBuilder`. All six default-on filters are switched off here and turned back on selectively in Task 2:

```rust
let walker = ignore::WalkBuilder::new(root)
    .hidden(false)       // dot-FILES stay listable (ADR 0033) — stays false in Task 2
    .parents(false)      // ↓ Task 2 turns these three back on
    .git_ignore(false)
    .git_exclude(false)
    .git_global(false)   // NOTE: stays false forever — machine-wide rules are refused (ADR 0034)
    .ignore(false)       // .ignore FILES: off in Task 1, on in Task 2
    .follow_links(false) // a symlink must not carry the walk out of the Space
    // NO .min_depth() — see the note below; the root is skipped in the loop instead.
    .sort_by_file_name(|a, b| a.cmp(b))  // decides WHICH entries survive a truncated walk
    .filter_entry(|e| {
        !(e.file_type().is_some_and(|t| t.is_dir())
            && is_skipped_dir(&e.file_name().to_string_lossy(), e.depth()))
    })
    .build()
    // `Walk::Item` is `Result<DirEntry, ignore::Error>`, NOT a DirEntry. Keep the
    // existing swallow: an unreadable subdirectory must skip and the walk must
    // continue. `.unwrap()` here is a REGRESSION — it panics inside
    // `spawn_blocking`, trips the JoinError arm in `list_space_files`, and empties
    // the ENTIRE listing where today only the unreadable subtree is missing.
    .filter_map(|e| e.ok());
```

   **Do NOT add `.min_depth(Some(1))`, even though it looks like the direct translation of `walkdir`'s `.min_depth(1)`.** It compiles, and then panics inside the crate: verified on `ignore` 0.4.33, `WalkBuilder::new(dir).hidden(false).parents(false).git_ignore(false).git_exclude(false).git_global(false).ignore(false).filter_entry(…).build()` panics at `ignore-0.4.33/src/walk.rs:1221` with `called Option::unwrap() on a None value` — on any tree containing a subdirectory, i.e. every fixture in this task. The mechanism is the filter stack: with the ignore matchers switched off the root's matcher is never pushed, so the root's `Exit` event pops a stack it cannot `parent()`. Note the panic is CONFIGURATION-dependent, not universal — with the filters left at their defaults the same call does not panic, which is exactly the kind of bug that survives a casual check. In production it is worse than in tests: it fires inside `spawn_blocking`, trips the JoinError arm, and **silently empties the whole listing**. Skip the root in the loop instead (step 4), which is equivalent, works on every 0.4.x, and additionally keeps the file-root invariant: a Space opened at a FILE yields nothing today, and `ignore` with no depth gate yields one entry whose relative path is `""` — which would trip the existing `debug_assert!(!joined.is_empty())`.

   API differences from `walkdir` that will bite you: **`ignore::DirEntry::file_type()` returns `Option<FileType>`**, so BOTH the `filter_entry` closure AND the loop's existing `if !entry.file_type().is_file()` line fail to compile as written — both become `.is_some_and(|t| t.is_file())` / `.is_some_and(|t| t.is_dir())`. `sort_by_file_name` takes `Fn(&OsStr, &OsStr) -> Ordering + Send + Sync + 'static`. `filter_entry` exists on the BUILDER and does prune non-matching directories (it does not descend into them) — same contract as the `walkdir` code you are replacing. If clippy flags `redundant_closure` on `|a, b| a.cmp(b)` under this repo's 0-warning gate, use `Ord::cmp`.
4. The loop gains ONE line at the top, replacing what `min_depth` used to do, and its `is_file()` check changes shape (both verified against 0.4.33):

```rust
for entry in walker {
    if entry.depth() == 0 {
        continue; // replaces walkdir's .min_depth(1): the root is never an entry
    }
    if !entry.file_type().is_some_and(|t| t.is_file()) {
        continue;
    }
    // …everything below stays exactly as it is
```

   **Everything below that stays unchanged — do not "simplify" it:** the `entries.len() >= cap` / `truncated = true` / `break` block; the `strip_prefix(root)` + `components()` joined with `"/"` logic and its whole comment (a Windows `\` is outside the composer's token charset and is invisible to ubuntu CI — load-bearing by construction); the `debug_assert!(!joined.is_empty())` (still reachable-and-honest only because of the depth-0 skip above); the trailing `entries.sort()` (display order, which the builder sort cannot provide); the `FileListDto` struct; `MAX_PICKER_ENTRIES`; and `list_space_files`.
5. Update `collect_files`' doc comment: it no longer uses walkdir, so replace the "walkdir's default" wording with `ignore`'s, and keep the cross-reference to `exec_find` (which is still `fd` + a `walkdir` fallback that descends into `.git` — that description of `exec_find` remains accurate; do not "fix" it).
6. **Do NOT touch** `src/lib/tauri.ts`, `src/hooks/useMentionCatalogs.ts`, `src/components/ChatStream.tsx`, or any frontend file. Verified: `FileListDto` is mirrored at `src/lib/tauri.ts:508` and `listSpaceFiles` at `:715`, `useMentionCatalogs` consumes only `{ entries, truncated }`, and every mock stubs the function rather than the walker — so nothing upstream moves.

**Steps:**
- [x] In the existing test module, use its own helpers: `tmp_dir(tag)` (defined at ~line 213, `files-cmd-test-{tag}-{uuid}`) and `touch(dir, rel)`, with plain `#[test]` — `#[tokio::test]` is only needed for the `#[tauri::command]` wrappers. (`scratch()` in `commands/agents.rs` is private to that module's tests; do not reach for it.)
- [x] Change the test asserting `node_modules` is skipped so it asserts the OPPOSITE: `node_modules` entries ARE listed, because nothing ignores them in the all-rules-off Task 1 state (and note in the test name that Task 2 makes this conditional on the repo).
- [x] Confirm dot-FILE-listed / dot-DIR-pruned tests already exist (there are `dotfile` and `dotroot` fixtures); add them only if missing.
- [x] Run `cargo test` from `src-tauri/`.
  - Did the `node_modules` test fail with an entry present that the old assertion forbade? If it passed, your edit did not land.
- [x] Implement steps 1–5.
- [x] Run `cargo test` from `src-tauri/` — all green. The two symlink pins (`collect_files_does_not_follow_symlinks`, `collect_files_does_not_descend_into_a_symlinked_directory`, both asserting `entries == ["a.txt"]`) must survive UNCHANGED: `ignore`'s `Walk` is built on `walkdir` with `follow_links(false)`, so a symlink's `file_type()` is `symlink`, `is_file()` is false, and it is neither listed nor descended into. If one fails, fix the filter, never the test.
- [x] Run `cargo clippy --all-targets` from `src-tauri/` — 0 warnings (respect the cache rule).
- [x] Run `cargo fmt --check` from `src-tauri/`; if it reports anything, run `cargo fmt` and re-check.
- [x] Run `pnpm test` from the repo root — still 1408 green. Nothing frontend changed; this is the check that the IPC shape did not move.
- [x] Commit: `feat: walk the ? listing with ripgrep's ignore crate` (c49d43d — 740 → 742 Rust tests)

**Acceptance criteria:**
- [x] `PICKER_SKIP_DIRS` exists nowhere (`rg -n PICKER_SKIP_DIRS src-tauri/src` → no hits).
- [x] Dot-files listed, dot-directory subtrees pruned, directories never listed, symlinks not followed and not listed, entries `/`-joined and sorted, `truncated` reported at the cap. *(True of this task's state; Task 6 replaced "every dot-directory" with the three VCS metadata directories.)*
- [x] `node_modules` is listed, because in this task NOTHING is ignored — all six filters are off. *(True of c49d43d; Task 2 made it conditional on the repo, which is why the test is named `…_when_the_repo_declares_it_source`.)*
- [x] An unreadable subtree skips; the listing does not go empty.
- [x] Frontend tests untouched and green; `FileListDto` unchanged on the wire.

---

### Task 2: Turn on the repo's ignore rules, and bound their reach

**Context:**
This is the behaviour change the feature needed. With `PICKER_SKIP_DIRS` gone, the only name rule left in app code is the dot-directory prune *(true as shipped; Task 6 narrowed it to the VCS metadata directories at `2e63869`, which is why `.github/workflows/ci.yml` IS listable here now)*; everything else about visibility is decided by one rule: what the repository itself declares not to be source. Measured on this very repo AT DESIGN TIME (2026-10-09 — build output has grown since, so read these as the numbers that motivated the change rather than a live figure), the picker's reachable set was 12,625 files, of which 11,880 were `src-tauri/target` + `dist` build output and only 424 git-tracked — so the 5,000-entry cap is consumed inside `target/` and any real file in a directory sorting after `src-tauri` is silently absent from the picker.

Reach is deliberately bounded to the repository: `git_global(false)` stays OFF (never read `~/.gitconfig`'s `core.excludesFile` or `$XDG_CONFIG_HOME/git/ignore`) and `require_git` stays at its crate default of `true`, which makes git rules repo-scoped AND stops the crate reading `.gitignore` from parents above the git root. Both refusals exist so two people on the same Space get the same listing. `parents(true)` is turned ON so a Space opened at a monorepo package still inherits the repo root's rules.

**Know which tests can fail first, before you write them.** In the Task 1 state nothing filters anything, so:
- An **"X is absent"** assertion (a gitignored file gone, build output gone, exclude-listed file gone) CAN fail first. Write those as the red step.
- A **"X is still present"** assertion — the `.ignore` override (`!name` brings it back), the no-`.git` case, the reach bound — passes BOTH before and after Task 2, because in the starting state nothing is ignored either. Those are regression pins, not red steps. Do not twist a fixture to force them red, and do not conclude the engine is broken when they pass early.

**Files:**
- Modify: `src-tauri/src/commands/files.rs` (four builder flags + doc comment)
- Test: `src-tauri/src/commands/files.rs` (same test module, `tmp_dir` + `#[test]`)

**What to implement:**
1. Flip **four** flags: `.ignore(true)`, `.git_ignore(true)`, `.git_exclude(true)`, `.parents(true)`. **Leave `.git_global(false)` and `.hidden(false)` alone**, and leave `require_git` at its default. State the reason for each refusal in a comment, because a future reader will "helpfully" enable them.
2. In the doc comment, record the user-facing rules: `.ignore` is the override knob, it outranks `.gitignore`, and it works without git; git's rule that a file under an excluded **directory** cannot be re-included means `!dist/app.js` resurrects nothing once `dist/` is excluded, so the negation must target the pattern that excluded the *file*; a non-git Space lists everything, with a `.ignore` file as the documented remedy.
3. Record the ONE reach leak that survives this design, so it is a known boundary rather than a hidden contradiction: `parents(true)` reads **`.ignore` files from ancestor directories, and `.ignore` is NOT gated by the `.git` barrier the way the git matchers are.** So an `~/.ignore` above the repo root does apply to every Space under `$HOME`, while a `~/.gitignore` does not. That is ripgrep's own behaviour, and we accept it deliberately — but do not write anywhere that reach is bounded to the repo without carrying this caveat.
4. Keep dot-files listable (`hidden(false)`): `?.env` completes; `?.env.local` does not where the repo ignores `*.local`.

**Steps:**
- [x] Write these tests in the existing module. **Every git-behaviour fixture needs a `.git` DIRECTORY** (an empty one suffices — `require_git` defaults to true and the crate's own tests do exactly this). One assumption these fixtures make, worth stating so a future flake is diagnosable: `std::env::temp_dir()` has no `.git` directory and no `.ignore` file among its ANCESTORS — `parents(true)` reads ancestor ignore files, so a machine whose `$TMPDIR` sits inside a repo or under a directory carrying an `.ignore` would prune temp fixtures unexpectedly.
  - **The regression that matters:** `<repo>/.git/`, a `<repo>/.gitignore` naming `build/`, a `build/` directory holding MORE files than the cap, and one source file in a directory sorting AFTER `build`. Call `collect_files(root, small_cap)` with a small cap (e.g. 3) so the fixture is a handful of files, not thousands. Assert the source file IS present and no `build/` path is. **Pin this fixture to `.gitignore` + `.git` — never "an `.ignore` file or a `.gitignore`"** — because `.ignore` is already live after Task 1 and would make the test pass before Task 2 exists. This is the test that could not exist before, and the reason it must: the old fixtures had no build output, which is exactly why the bug shipped.
  - `.gitignore` is honoured: `<repo>/.git/` + `.gitignore` naming an existing file → that file is absent. (Fails first.)
  - `.git/info/exclude` is honoured while `.git` is pruned from the RESULTS: `<repo>/.git/info/exclude` naming a file → that file is absent, and no `.git/…` path is ever listed. This settles the belief ADR 0034 refused to assert, and it CAN pass: the crate builds the git matchers when it pushes a directory — reading `<dir>/.git/info/exclude`, resolving worktree commondirs — which is independent of `filter_entry`, which only suppresses yielded entries and descent. Use a plain `.git` **directory** in the fixture: a fully-valid worktree-style `.git` file is also supported by the crate (`resolve_git_commondir` parses `gitdir:` + `commondir`), but a malformed one silently disables `exclude`, and the directory is the deterministic choice.
  - `parents(true)` reaches a repo root above a package: `<repo>/.git/` + `<repo>/.gitignore` naming `build/` + walk root `<repo>/package`, with `<repo>/package/build/…` present → those files are absent. **Without `<repo>/.git/` this test fails AFTER the implementation** — the git matchers are only consulted when some ancestor holds `.git` — so the `.git` directory is not optional in this fixture. (Fails first.)
  - **Pins (pass before and after):** `.ignore` outranks `.gitignore` (`!name` in `.ignore` brings a gitignored file back); no `.gitignore` effect when there is no `.git` directory (the file IS listed); reach bound — a `.gitignore` in a parent ABOVE the git root does not apply.
- [x] Run `cargo test` from `src-tauri/`.
  - Did each **"must be absent"** test fail because its rule is not on yet? A PIN passing here is correct, not suspicious — see the fail-first note above.
- [x] Implement steps 1–4.
- [x] Run `cargo test` from `src-tauri/` — all green.
- [x] Run `cargo clippy --all-targets` (0 warnings) and `cargo fmt --check` from `src-tauri/`.
- [x] **Prove it on the real Space** — the only test of the actual bug. Run the listing against this repo's root via a throwaway probe (a temporary `#[test]` you delete before committing — do NOT commit a test that depends on this repo's build output existing). Expected: no path contains `src-tauri/target` or `dist/`, and `truncated` is false at `cap = 5_000`. Record the number you saw in the commit message. — **Done at `b0330dc`: 423 entries, `truncated = false`, zero paths under `src-tauri/target` or `dist/`.** Re-run 2026-10-09 with a throwaway probe (outside the repo, nothing committed) after Task 6 changed the prune — **426 entries, `truncated = false`, no `.git/…` path, `git ls-files` = 426**, the 423 → 426 delta being exactly `.github/workflows/ci.yml`, `.github/workflows/release.yml` and `.vscode/extensions.json`. Between `b0330dc` and Task 6 it was NOT re-run, and that interval's tick is that commit's evidence rather than this box's — the walker and `is_skipped_dir` were byte-unchanged across those commits, which is what made 423 stable, and `2e63869` is precisely the commit that rewrote `is_skipped_dir`, which is why the probe was run again rather than trusted forward.
- [x] Commit: `feat: let the repo's ignore rules decide the ? listing` (b0330dc — 742 → 750 Rust tests; real-Space probe: 423 entries, `truncated = false`)

**Acceptance criteria:**
- [x] `.gitignore`, `.ignore` and `.git/info/exclude` all prune; `.ignore` outranks `.gitignore`; `parents(true)` reaches a repo root above a package Space. *(The first four are pinned by Task 2; the package-Space clause had no test in b0330dc and is pinned here by `collect_files_inherits_the_repo_root_rules_for_a_package_space`, verified non-vacuous by flipping `.parents(false)` and watching it redden.)*
- [x] No machine-wide rule is read; `require_git` left at its default; the surviving `.ignore`-above-the-git-root leak is documented, not hidden.
- [x] Dot-files remain listable unless the repo ignores them.
- [x] The cap-regression test exists, is pinned to `.gitignore` + `.git`, and fails if the git rules are switched back off.

---

### Task 3: `?` rows on one line, capped at 10

**Context:**
`MAX_PICKER_ROWS` is 100 (`src/components/ChatStream.tsx:55`) and a picker row is two lines tall: `name` on the first line and `description` on the second, which for a `?` row re-renders the basename that already ends the path in `name`. At the default 14px UI font (`--ui-font-size: 14px`, `src/styles/base.css:29`) a two-line row is ≈49px, so 100 rows overshoot the top of the window entirely and even 10 would be ≈490px. Dropping the duplicated second line makes a `?` row ≈29px, so 10 rows sit at ≈308px — a normal dropdown — with no information lost.

Mention rows (`$` / `@` / `#`) keep two lines because their second line is a real description, and they stay UNCAPPED — an explicit earlier decision, because the cap is sliced inside the rows memo and a sliced row is an unreachable row. That asymmetry must be recorded in a comment in this task or the next reader will "fix" it.

**Files:**
- Modify: `src/components/ChatStream.tsx` (`MAX_PICKER_ROWS` line 55; the `?` branch of the rows memo lines ~487–496; the `pickerNote` derivation lines ~546–563)
- Test: `src/components/ChatStream.test.tsx` ONLY (the `?` picker tests, ~3150–3490)

**What to implement:**
1. `const MAX_PICKER_ROWS = 100;` → `= 10;`. Keep the slice inside the rows memo (`fileRows.slice(0, MAX_PICKER_ROWS)`) — NEVER in render, because `activeIndex` wraps on `% filtered.length` and a render-time cap lets the highlight land on a row that is not displayed.
2. In the `?` row mapping, `description: basenameOfPath(p)` → `description: ""`. `ComposerMentions` already renders the second line only when `row.description !== ""`, so this alone makes file rows single-line. Do NOT change `name` — `selectMention` inserts `row.name`, which must stay the full path.
3. `basenameOfPath` is STILL used at `ChatStream.tsx:969` (the attachment filename), so leave the import on line 16 alone. Removing it breaks `pnpm build`.
4. Update the `MAX_PICKER_ROWS` comment to state both numbers and WHY they differ: `?` is capped at 10 because narrowing a path query is cheap and its catalog is the whole listing; `$`/`@`/`#` are uncapped because their lists are small and a sliced row cannot be reached by keyboard. Point at Task 4's clamp as what actually protects the uncapped case.
5. Leave both note strings EXACTLY as they are (`Too many matches — keep typing to narrow the list` at :556 / `File listing capped — some files may be missing` at :558). The first becomes the common case rather than the rare one, and it is honest — every match is in the listing, and narrowing genuinely helps.

**Steps:**
- [x] **Rewrite the test at `ChatStream.test.tsx:3262`, "a file row shows the basename as its secondary line (and tooltip)"** — this test EXISTS today, asserts the secondary line and its `title` tooltip, and will crash on a `!` non-null deref once `description` is `""`. Replace it with the inverse: a `?` row has **no `[title]` element** and **no secondary `.text-ui-sm` span**, while the `.text-ui-base` name span still shows the full path. (A `?` row button always contains TWO spans on the primary line — the prefix badge and the name — so "one span" is the wrong assertion; assert on the absence of the description node.)
- [x] In the first 150-entry cap test (starting ~line 3319) change **all three** assertions: the rendered-button count `100` → `10`; `buttons[99]` → `buttons[9]` (the ArrowUp-wrap-to-last-row assertion, ~line 3342); and the Enter-insert expectation `"file99.ts "` → `"file9.ts "` (~line 3347). Leaving the entry count at 150 keeps the cap biting.
- [x] In the second 150-entry test (~line 3413) change only its button count — it asserts count + note text, and the note text stays.
- [x] Run `pnpm test src/components/ChatStream.test.tsx`.
  - Did these fail on the counts and the missing description node? If they passed, your edits did not land.
- [x] Implement steps 1–5.
- [x] Run `pnpm test src/components/ChatStream.test.tsx` — green.
- [x] **Do not touch the mention tests.** The `it.each` at ~line 3435 builds 150 rows per prefix, asserts `toHaveLength(150)` and wraps ArrowUp to index 149 — that asserts mentions are UNCAPPED and must still pass unchanged. If it fails, you capped the wrong branch.
- [x] Run `pnpm test` at the repo root — green (1408 baseline plus additions). These two 150-entry tests plus the 3262 test are the ONLY tests in the suite affected by the cap change: `App.test.tsx` mocks `entries: []`, and the `useMentionCatalogs` / `tauri` tests use ≤2 entries. If a third file reddens, look at it before assuming the plan is right.
- [x] Run `pnpm build` — `tsc` clean.
- [x] Commit: `feat: cap the ? picker at 10 single-line rows` (64e3fac)

**Acceptance criteria:**
- [x] A `?` query matching 150 files renders exactly 10 buttons, wraps ArrowUp onto `buttons[9]`, and inserts `file9.ts`.
- [x] `?` rows are one line; the basename is no longer duplicated or tooltiped.
- [x] Inserting still yields the full path with `?` consumed and case intact.
- [x] A 150-row mention list still renders all 150 rows.

---

### Task 4: Clamp the picker's height so no list can fill the window

**Context:**
`ComposerMentions`' container is `absolute left-3 right-3 -top-2 z-10 -translate-y-full …` with **no `max-height` and no overflow**, growing upward from the composer. Task 3 bounds `?` to 10 rows, but the mention pickers are deliberately uncapped, so a user with many skills can still push the list past the top of the window. The clamp is the structural promise that the box never dominates the page, and it must scroll.

Two subtleties: `--ui-font-size` is a USER setting applied at runtime as an inline px value on `<html>` by `src/lib/settings.ts:30` (with `:root { --ui-font-size: 14px }` in `src/styles/base.css:29` as the fallback, so `var()` always resolves), so a `rem`-based Tailwind clamp like `max-h-64` does NOT track it and would show fewer rows at a larger font; and once the box scrolls, arrow-keying must keep the highlighted row visible or the selection silently moves off-screen.

**Files:**
- Modify: `src/components/chat/ComposerMentions.tsx` (container className; a ref + effect for scroll-into-view; and its doc comment — see step 3)
- Test: `src/components/ChatStream.test.tsx` — use this file, do NOT create a new one. There is no `ComposerMentions` test file today, and `ChatStream.test.tsx:90` and `:94` already stub `Element.prototype.scrollIntoView` / `HTMLElement.prototype.scrollIntoView` and already carry the picker harness.

**What to implement:**
1. Add to the picker container: `max-h-[calc(var(--ui-font-size)*22)] overflow-y-auto`. `22` comes from ≈10 single-line rows: a one-line row is `line-height(≈1.5 × var(--ui-font-size)) + 8px padding` ≈ `2.2 × F`, so 10 rows ≈ `22 × F` ≈ **308px at the default 14px**. Put that arithmetic in a comment next to the class, and say it is a deliberate ESTIMATE: jsdom has no layout engine, so no test in this repo can measure row height. Tailwind is v4 and arbitrary `calc()` classes already compile here (`max-h-[calc(70vh-7.5rem)]` in `SkillsDialog.tsx:60`), and this class contains no spaces, so it generates.
2. Scroll the active row into view when `activeIndex` changes: hold a ref on the active row and `useEffect` on `activeIndex` calling `scrollIntoView({ block: "nearest" })` — `nearest` so an already-visible row does not jump. This is a NEW pattern in production code: the `scrollIntoView` stubs across the test suites exist for Radix, so copy the stub idiom but nothing else.
3. Update `ComposerMentions`' doc comment. It currently opens "**Props-only:** the `useMentionCatalogs` hook, the `picker` state … all stay in `ChatStream`" — after this task the component has hooks, so rewrite that paragraph to say the state and derivations still live in `ChatStream` while the row-scroll effect is purely local presentation. Leaving the comment as-is misdescribes the file, and this repo treats those comments as decisions.
4. Do NOT cap the mention rows, do NOT touch `filtered`, `activeIndex`, the note rendering, `role="status"`, or the `open && (filtered.length > 0 || note)` gate. Geometry and focus-follows only.

**Steps:**
- [x] Write a failing test in `ChatStream.test.tsx`: render the `?` picker with more matches than fit, assert the container has `overflow-y-auto` and a `max-h-[` class, then move `activeIndex` with an arrow key and assert `scrollIntoView` was called on a row.
- [x] Run `pnpm test src/components/ChatStream.test.tsx` — confirm the new assertions fail.
- [x] Implement steps 1–3.
- [x] Run `pnpm test src/components/ChatStream.test.tsx`, then `pnpm test` at the repo root, then `pnpm build`.
- [x] Check the `useEffect` dependency array BY HAND — this repo has no lint script, so nothing guards react-hooks rules; a wrong deps array is invisible until it loops. It must re-run on `activeIndex` (and on `filtered`, or the ref may be stale after a re-render) and must not re-run on every keystroke unnecessarily.
- [x] Commit: `fix: clamp the picker's height and keep the active row visible` (a9f58ee — 1408 → 1410 frontend tests)

**Acceptance criteria:**
- [x] The picker box has a font-size-relative `max-height` and scrolls.
- [x] Arrow-keying past the visible window scrolls the highlighted row into view.
- [x] Mention lists are still uncapped in `filtered`; every row stays keyboard-reachable.
- [x] `ComposerMentions`' doc comment describes the component that now exists.

---

### Task 5: Record the asymmetry, and run the full gate

**Context:**
Two decisions in this feature look like oversights to the next reader: that `?` is capped at 10 while `$`/`@`/`#` are uncapped, and that `.ignore` rather than an app setting is the override knob. Neither is visible in the code as a *reason*. The repo's convention is that reversible reasoning lives in the code comment and irreversible reasoning in `docs/decisions/` — ADR 0034 already carries the irreversible part.

**Files:**
- Modify: `src/components/ChatStream.tsx` (the `MAX_PICKER_ROWS` comment, if Task 3's needs tightening)
- Modify: `src-tauri/src/commands/files.rs` (the `collect_files` doc comment, if Task 2's is missing the `.ignore` / re-include / non-git / ancestor-`.ignore` rules)
- No new files. No `docs/features/` entry: the composer's `?`/`$`/`@`/`#` affordances are documented through ADRs 0031–0034, and a parallel prose copy would be a second source of truth.

**Landed here beyond the list above:** the sweep found one Task 2 step that never shipped — its `parents(true)` "repo root above a package" test — so `collect_files_inherits_the_repo_root_rules_for_a_package_space` was added to `src-tauri/src/commands/files.rs` (750 → 751 Rust tests) rather than ticking that Task 2 box as if it had. Verified non-vacuous: `.parents(false)` reddens it. Behaviour was established first with a throwaway probe against the real crate (`package/` under a repo whose `.gitignore` names `build/` → `["src/app.rs"]`), so the pin records what the code already did.

**Step 3, however, was ticked on an incomplete sweep, and the correction is recorded here as well as in the docs.** A later accuracy pass found several claims in ADR 0034 / `CONTEXT.md` / this plan that the shipped code contradicts, and corrected them in place (alongside two smaller ones — 0034's `.gitignore` list omitting our `node_modules` line, which made its committed-`node_modules` example unreproducible here, and this box's own tick, now annotated with the commit its probe actually ran at): that 0034 supersedes 0033's dot-DIRECTORY half (it supersedes only the `node_modules` half — `is_skipped_dir` still prunes every dot-directory, which is why this repo's own `.github/workflows/ci.yml` can never be offered) *[both halves are superseded now, and `.github/workflows/ci.yml` IS offered — Task 6 landed after this parenthetical was written, and it describes the code as the accuracy pass found it]*; that visibility is the repo's decision alone (it is the repo's ignore rules PLUS that one app-level prune) *[still true in shape, but the prune is now the VCS three rather than every dot-directory]*; that 0033's "an empty `filtered` means no picker renders" clause stood untouched (the render gate is `open && (filtered.length > 0 || note)`, so a note renders over zero rows — while Enter still sends, because the keyboard branches stay gated on `filtered.length > 0`); that 12,625 / 11,880 / 424 described this repo (they are a dated design-time measurement); and that the cap-regression fixture had to be "large enough to overrun the cap" (the shipped test pits four build files against `cap = 3`). The lesson the sweep should have carried into its own box: confirming that docs agree with code means reading the code, not re-reading the docs.

**What to implement:**
1. Confirm the `MAX_PICKER_ROWS` comment states WHY `?` is capped and mentions are not — not merely THAT they differ.
2. Confirm the `collect_files` doc comment states the `.ignore` override, git's cannot-re-include-under-an-excluded-directory caveat, the non-git-Space behaviour, and the ancestor-`.ignore` reach leak that we accept deliberately.
3. Confirm ADR 0034 and `CONTEXT.md` agree with the shipped code: the "pinned by test, not asserted" belief about `.git/info/exclude` is now closed by a NAMED test from Task 2, and nothing in the docs still claims `node_modules` is skipped by a hardcoded list.

**Steps:**
- [x] Full frontend gate from the repo root: `pnpm test` and `pnpm build`. — On the FINAL commit of this task: **1410 tests passed / 72 files, 0 failed**, and `pnpm build` clean (`tsc` + vite: `✓ built`, only the pre-existing >500 kB chunk-size advisory). No flake appeared.
- [x] Full Rust gate from `src-tauri/`: `cargo test`, `cargo clippy --all-targets` (0 warnings, cache rule respected), `cargo fmt --check`. — On the FINAL commit of this task: **751 passed / 0 failed** across 19 test binaries (750 + the package-Space pin added here); `cargo clippy --all-targets` **0 warnings** with the cache rule honoured (`touch src/commands/files.rs`, and the run printed `Checking archimedes` rather than replaying a cache); `cargo fmt --check` clean. No flake appeared.
- [x] `rg -n 'PICKER_SKIP_DIRS|nothing re-matches|keep typing to narrow'` across `src/`, `src-tauri/src/` and `docs/`, and confirm every remaining hit is intentional — `keep typing to narrow` belongs ONLY to the too-many-matches arm and its test. — Run with `100-row|100 rows|basenameOfPath` added, and every hit is intentional: `PICKER_SKIP_DIRS` survives ONLY as history in `docs/` (0 hits in `src/` and `src-tauri/src/`); `nothing re-matches` survives only inside merged ADR 0033, where the sentence is a negation ("NOT that nothing re-matches it") and therefore true; `keep typing to narrow` appears in exactly two code places — `ChatStream.tsx:578` (the too-many-matches arm) and its test at `ChatStream.test.tsx:3442` — plus this plan's prose; `100-row|100 rows` survives only in this plan's Task 3 context, which describes the pre-change state, and no longer in any source file; `basenameOfPath` is still imported at `ChatStream.tsx:16` and used at `:991` for the attachment filename (and `:496` is the comment recording why the `?` row stopped using it).
- [ ] **Manual smoke**, which nothing automated here can do (jsdom has no layout; the Rust tests use temp fixtures, not a real Space): run the app, type `?` in a repo with build output, and confirm the listing shows source rather than artifacts, `?.env` completes, `??` opens nothing, Enter always sends, and the picker is ~10 single-line rows that scrolls instead of filling the window. Tick this box ONLY if actually done at a real window manager. — **left open, honestly**: this needs a human at a real window manager and nothing in this environment can run the app's GUI — jsdom has no layout engine (so no test here can measure a row's height or whether the box fills the window), and the Rust tests walk temp fixtures rather than a real Space. No visual result was observed, so none is claimed here.
- [x] Commit: `docs: record why ? is row-capped while the Mentions are not`

**Acceptance criteria:**
- [x] Both gates green at the final commit, with counts recorded in this file. (1410 frontend / 72 files + `pnpm build` clean; 751 Rust + clippy 0 warnings + fmt clean.)
- [x] A reader who disagrees with the 10-vs-uncapped asymmetry has to argue with a written reason, not infer an omission.
- [x] ADR 0034's open belief is closed by a named test.

---

### Task 6: Narrow the app prune to the VCS metadata directories

**Context:**
This was item 5 of "Out of scope" below — the open design question ADR 0034 called "raised separately". It landed on this branch after Task 5, at `2e63869`, and ADR 0034 carries the decision in its "The one app rule the repo cannot override" section (amended IN PLACE, because 0034 had not merged yet — an append-only rule binds merged documents, not unmerged ones on your own branch). **Why it moved:** the blanket dot-prefix rule was the app overriding the repo on the exact axis ADR 0034 assigns to the repo — this repo's own tracked `.github/workflows/ci.yml` was not completable — and keeping `.git/` out never needed the blanket rule, it needed three names. `.gitignore` cannot exclude `.git/` (`git` does not ignore its own metadata), so this is the one rule no repository can be asked to declare; `hidden(false)` stays ON-as-false so `?.env` completes, and the `ignore` crate never excludes `.git` by itself (it only reads `.git/info/exclude`), so the prune is load-bearing rather than redundant. The crate-level fact item 5 rested on was re-checked for this docs pass, on the app's exact flag set with the app's `filter_entry` REMOVED: the walk yields `.git`, `.git/COMMIT_EDITMSG`, `.git/FETCH_HEAD`, `.git/HEAD`, so nothing in `ignore` 0.4.x excludes `.git` by itself and the app-side prune is load-bearing, not redundant.

**What shipped:** `PRUNED_VCS_DIRS = [".git", ".hg", ".svn"]` and `is_skipped_dir` = `depth > 0 && PRUNED_VCS_DIRS.contains(name)` — still DIRECTORIES ONLY (a submodule's gitfile stays listable), still never at depth 0 (a Space opened at `~/.dotfiles`), still any depth > 0 (a nested / submodule `.git`, and SVN's per-directory `.svn`). `node_modules`, `dist`, `target` and `.venv` are deliberately NOT in the list, and `PRUNED_VCS_DIRS`' comment says why. Accepted cost, stated rather than discovered later: a heavy dot-directory nobody ignored (`.venv`-class tooling) is now walked and can spend the entry cap, which is what `rg --hidden` does; the remedy is the repo's own ignore file, never a name list up here. No frontend surface moved — `FileListDto` is the same `{ entries, truncated }`.

**Steps:**
- [x] Write the four tests that go RED against the blanket rule, before narrowing it. Re-verified 2026-10-09 for this docs pass by running each fixture against a byte-exact copy of the shipped `collect_files` with `is_skipped_dir` reverted to `depth > 0 && name.starts_with('.')` — red under the old rule, green under the new (which is the fail-first evidence, obtained without touching the repo):
  - `collect_files_lists_a_tracked_dot_directory` — the regression this change removes (a `.github/workflows/ci.yml` the repository ships).
  - `collect_files_lists_a_dot_directory_the_repo_does_not_ignore` — `.claude/` and `.venv/` listed, `dist/` still excluded BECAUSE the fixture's `.gitignore` says so.
  - `collect_files_prunes_vcs_directories_and_lists_other_dot_directories` — formerly `collect_files_prunes_dot_directories`; its `.git/config` stays pruned and its `.hidden/z` becomes listable, so the assertion changed rather than disappeared.
  - `collect_files_lets_a_non_ignored_dot_directory_eat_the_cap` — the accepted cost pinned as a decision (`.` sorts before every letter, so `.venv/lib/…` is walked before `src/`).
- [x] Write the pins, which are whitelist / negative tests and were GREEN before and after (same two-rule check, so this is stated from the check and not from hope) — they guard what the blanket rule used to cover for free: `collect_files_never_lists_a_nested_git_directory` (a submodule's `.git` at depth > 1, with its real source still listed), `collect_files_prunes_mercurial_and_svn_metadata_directories` (`.hg`, and a nested `.svn` — the any-depth rule), `collect_files_lists_a_file_named_git_because_the_prune_is_directories_only` (the gitfile), and `collect_files_keeps_a_dot_named_root`, which gained a `.git`-named ROOT case. *(Corrected 2026-10-09: this step justified that case by saying the `.dotfiles` fixture "no longer discriminates the depth guard". Nothing discriminates that guard and nothing can — `ignore` 0.4.33's `Walk::skip_entry` returns `Ok(false)` for a `depth() == 0` entry BEFORE the `filter_entry` predicate is consulted (`src/walk.rs:1149-1152`; the predicate is applied at `:1177-1181`), so `is_skipped_dir`'s `depth > 0` is unreachable-at-depth-0 defence-in-depth: weakening it to drop `depth > 0` leaves all 39 `commands::files` tests green, re-checked 2026-10-09. The case stays, but as a behaviour pin on the contract "the root we were asked to walk is always listed" — i.e. against a future crate upgrade that starts filtering roots — not as mutation coverage.)*
- [x] Rust gate from `src-tauri/` at `2e63869` — **760 passed** (was 754), `cargo clippy --all-targets` 0 warnings, `cargo fmt --check` clean (counts as recorded by that commit).
- [x] Re-prove it on the real Space, as Task 2 required — throwaway probe outside the repo, 2026-10-09: **423 entries under the old rule → 426 under the new one, `truncated = false`, no `.git/…` path, `git ls-files` = 426**, and `rg --hidden --files --glob '!.git/'` prints the same 426. The three gained are the `.github/workflows/` pair and `.vscode/extensions.json`. Dated probe, not a live figure.
- [x] Commit: `feat: prune only VCS directories from the ? listing` (`2e63869`), then `docs: the ? Listing prunes VCS directories, not every dot-directory` for the four documents the narrowing invalidated — ADR 0034 (amended in place, unmerged), 0033's *Partial* note (the only line of 0033 this branch owns), the **Listing** / **File completion** entries in `CONTEXT.md`, and this plan.
- [ ] **Manual smoke** stays open — Task 5's box is still unticked for the same reason (no window manager here), and narrowing the prune widened what it should now check: `?.github/workflows/ci.yml` completes, and a `.venv`-class tree the repo does not ignore visibly reports the cap rather than silently dropping source.

---

## Out of scope — adjacent defects found while investigating, recorded so they are not lost

None of these are caused by this work and none are fixed by it. They live here because this investigation found them and no other artifact does. All verified against the code. *(There used to be a fifth entry here — the OPEN design question of whether the app's dot-directory prune should narrow to VCS directories only. It is neither open nor out of scope, it was decided at `2e63869` and recorded in ADR 0034, and its task is above as Task 6.)*

1. **`exec_grep` has no fallback** — its own doc comment says so at `src-tauri/src/agent/tools/exec.rs:1008`, and the failure text is at `:1083`. On a machine without `ripgrep`, which is the default on macOS and Windows, the agent's `grep` tool always fails. Its shell-out also carries the `--pre=sh` option-injection hardening comment at `:1052` (tested at `:2341`).
2. **`exec_find` is two different tools selected by `$PATH`.** `fd -g` matches the **basename**; the `walkdir` fallback's `glob_match` (`exec.rs:951`) matches the **full relative path** with `*` unable to cross `/`. Measured with pattern `*.ts`: with `fd` → `top.ts`, `src/app.ts`; without → `top.ts`, **`node_modules/lib.ts`** — nested source missing, vendored code returned. The fallback also descends `.git/` (`**/config` → `.git/config`) and `.gitignore` plays no part, while `fd` prunes hidden dirs and honours `.gitignore`. It fails *wrong*, not loudly.
3. **The CI tests for both tools are hollow.** `.github/workflows/ci.yml:46-52` installs only webkit2gtk/GTK deps — neither `ripgrep` nor `fd` — and the guards read `if !rg_available() { return; }` (`exec.rs:2324`, `:2346`, `:2381`; `fd_available` at `:2244`). A bare `return` **passes**, so on CI the entire grep implementation is unverified while local development exercises only `find`'s `fd` path.
4. Two pre-existing flakes unrelated to the above: `SettingsPage.test.tsx`'s font-picker ordering test (`setTimeout(0)` waiting for Radix to un-`aria-hide`) and `agent::mcp::http::tests::http_session_id_is_resent` (`src-tauri/src/agent/mcp/http.rs`, ~1-in-6 loopback timing race).
