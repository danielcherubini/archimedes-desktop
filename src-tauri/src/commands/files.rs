//! File-reading commands (the composer's `+` file picker and its `?` file
//! completion).
//!
//! `read_file_bytes` is the backend half of the image picker: the frontend
//! opens the native file dialog (`tauri-plugin-dialog` — a selection is a
//! PATH, not a `File`), then reads the bytes here. The webview cannot read
//! an arbitrary local path itself (no `fs` plugin), so the read is a
//! Tauri command.
//!
//! `list_space_files` is the backend half of the `?` file completion (ADR
//! 0033): it lists NAMES in the active **Space** so the picker can offer a
//! PATH. It reads no file bytes — `?` completes to a path and the agent's
//! own Boundary/policy-gated `read` stays the enforcement point.

use crate::agent::MAX_IMAGE_BYTES;
use serde::Serialize;
use std::path::Path;

/// The image extensions the picker accepts — the SAME allowlist as the
/// frontend's `SUPPORTED_IMAGE_TYPES` (the MIMEs the vision APIs accept:
/// an SVG passes an `image/*` filter but fails at the provider, so the
/// allowlist is enforced here AND in the frontend).
pub const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp"];

/// Is the path's extension in the image allowlist (case-insensitive; a
/// path with no extension is `false`)?
pub fn is_supported_image_path(path: &str) -> bool {
    std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| IMAGE_EXTENSIONS.contains(&e.to_lowercase().as_str()))
}

/// Is a file of `len` bytes within the prompt path's 10 MiB cap
/// (`MAX_IMAGE_BYTES`, ADR 0008)? Factored out as a pure comparison so
/// the check is unit-testable without writing a 10 MiB file (the same
/// pattern as the clipboard path's `encoded_png_within_cap`).
pub fn image_within_cap(len: u64) -> bool {
    len <= MAX_IMAGE_BYTES
}

/// Read a picked file's bytes (an image for the composer's attachments).
///
/// TWO-TIER BOUND (defense in depth — the dialog already filters by these
/// extensions, but the command is public IPC): the extension must be in
/// the image allowlist (a miss is `Ok(None)` — "not an image", the
/// frontend skips it silently) AND the size must fit `MAX_IMAGE_BYTES`
/// (an over-cap file is an `Err` — the frontend shows it on the
/// composer's error line). The read is blocking (a disk I/O), so it runs
/// on a worker thread.
#[tauri::command]
pub async fn read_file_bytes(path: String) -> Result<Option<Vec<u8>>, String> {
    if !is_supported_image_path(&path) {
        return Ok(None);
    }
    tauri::async_runtime::spawn_blocking(move || {
        let meta = std::fs::metadata(&path).map_err(|e| e.to_string())?;
        if !image_within_cap(meta.len()) {
            return Err(format!(
                "the file is {} bytes, which exceeds the 10 MiB limit",
                meta.len()
            ));
        }
        std::fs::read(&path).map(Some).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// The `?` file-completion cap (ADR 0033). A big repo must not hang the
/// Client. Past the cap the picker says the listing is capped rather than
/// implying it is complete — BUT THAT NOTE IS NOT ALWAYS WHAT THE USER SEES:
/// `ChatStream.tsx`'s `pickerNote` tries the render-cap arm FIRST, so when the
/// query matched more rows than the frontend renders (`MAX_PICKER_ROWS`) the
/// line reads "Too many matches — keep typing" and the cap goes unreported.
/// The cap is reported UNLESS the too-many-matches arm wins.
pub const MAX_PICKER_ENTRIES: usize = 5_000;

/// One `?`-picker listing: paths RELATIVE to the Space root, `/`-separated
/// (normalized so a Windows walk never inserts `\` into the composer's
/// token), sorted. `truncated` = the cap was hit.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileListDto {
    pub entries: Vec<String>,
    pub truncated: bool,
}

/// Directory names the app ALWAYS prunes from the `?` listing, at every depth
/// below the Space root.
///
/// WHY exactly these three, and NOT a broader list of heavy directories
/// (`node_modules`, `dist`, `target`, `.venv`, …): those are precisely what a
/// repository's own ignore rules exist to decide, and re-adding an app-side list
/// would recreate the defect ADR 0034 removed — a hand-rolled approximation of
/// what the repo declares, wrong in both directions (it hides a committed
/// `node_modules` and misses an oddly-named build tree). ADR 0034's principle is
/// that the REPOSITORY decides what the `?` listing shows, and the blanket
/// dot-prefix rule this list replaced was the app overriding the repo on that
/// exact axis.
///
/// What the app must still own is the ONE thing no ignore file is ever asked to
/// protect: the VCS metadata directories. They are machine state rather than
/// project content, and `.gitignore` does NOT exclude them and never can — `git`
/// cannot ignore its own `.git/`. Without this prune the picker would offer
/// thousands of `.git/objects/…` paths. `.hg` (Mercurial) and `.svn`
/// (Subversion, which keeps one per working directory, hence the any-depth rule)
/// are the same machine state for the other two VCSs whose checkouts get opened
/// as a Space.
const PRUNED_VCS_DIRS: &[&str] = &[".git", ".hg", ".svn"];

/// Is a DIRECTORY named `name` one we never descend into / never list?
///
/// DIRECTORIES ONLY, and that gate lives in the caller (`filter_entry` checks
/// `is_dir()`): a FILE named `.git` — a submodule's gitfile, `gitdir: …` — is
/// still offered, because the dot-FILE rule (ADR 0033) keeps dot-named files
/// completable. Pinned by
/// `collect_files_lists_a_file_named_git_because_the_prune_is_directories_only`.
fn is_skipped_dir(name: &str, depth: usize) -> bool {
    // Depth 0 is the Space root itself, which may legitimately be a
    // dot-dir (a Space opened at `~/.dotfiles`) — never pruned. Belt and
    // braces: the crate already refuses to apply `filter_entry` to a depth-0
    // entry (see `collect_files_keeps_a_dot_named_root`), so no test covers
    // this half of the condition.
    depth > 0 && PRUNED_VCS_DIRS.contains(&name)
}

/// Walk `root` for the `?` picker: files only, and the VCS metadata directories
/// ([`PRUNED_VCS_DIRS`]) PRUNED at the directory level. Dot-named FILES are KEPT
/// (`.gitignore`, `.env` are real completion targets — `.` is in the token
/// charset), which is why the skip is a directory-level prune and not a
/// name-prefix filter over every entry — and consequently why a FILE named `.git`
/// (a submodule's gitfile) is still listed.
///
/// What the app decides, and what the repo decides. The ONLY name rule the app
/// applies is the VCS-directory prune above: unconditional (an APP rule, not the
/// repo's) precisely because no ignore file can exclude `.git/` — that is the one
/// thing the repo is never asked to decide. EVERYTHING ELSE IS THE REPO'S CALL,
/// dot-named or not: `.github/`, `.vscode/`, `.claude/` and a `.venv` nobody
/// ignored are all walked and listed unless an ignore file says otherwise, which
/// is what `rg --hidden` does. The blanket dot-prefix rule this replaced
/// contradicted that (this repo's own tracked `.github/workflows/ci.yml` was not
/// listable here); dropping it applies ADR 0034's principle consistently rather
/// than introducing a new one.
///
/// Apart from that prune NOTHING is skipped by name — no `node_modules` /
/// `dist` / `target` blacklist or anything like it, because a hand-rolled list is
/// an approximation of what the repo itself declares, and the repo's own ignore
/// rules are the real thing (ADR 0034). The cost of that position is ACCEPTED and
/// pinned by `collect_files_lets_a_non_ignored_dot_directory_eat_the_cap`: a heavy
/// tree the repo does not ignore can consume the entry cap and crowd real source
/// out of the listing. VISIBILITY for everything else is decided by
/// `.gitignore`, `.ignore` and `.git/info/exclude`, which is why a dot-file the
/// repo ignores is NOT listable even though `hidden(false)` keeps dot-FILES
/// listable in general.
///
/// Three user-facing rules about those rules:
///
/// * **`.ignore` is the override knob.** It outranks `.gitignore`, and it needs
///   no git — so it is also the documented remedy for a Space that is not a
///   repository.
/// * **A non-git Space lists everything** git could otherwise exclude, because
///   git rules are repo-scoped (`require_git` left at its crate default of
///   `true`). Say "add a `.ignore` file", not "run `git init`".
/// * **A file under an excluded DIRECTORY cannot be re-included** — git's rule,
///   inherited here. Once `dist/` is excluded, `!dist/app.js` resurrects
///   NOTHING: the negation has to target the pattern that excluded the *file*.
///
/// REACH — where those rules are read from — is the repo's own directories, and
/// no machine-wide rule is ever consulted (`git_global(false)`). There is
/// however ONE leak that survives this design, and it is a known boundary
/// rather than a hidden contradiction: `parents(true)` reads **`.ignore` files
/// from ancestor directories above the Space, and `.ignore` is NOT gated by the
/// `.git` barrier the git matchers are.** So an `~/.ignore` above a repo root
/// does apply to every Space under `$HOME`, while a `~/.gitignore` does not.
/// That is `ripgrep`'s own behaviour and we accept it deliberately — but nothing
/// in this file or in ADR 0034 may say reach is bounded to the repo without
/// carrying this caveat.
///
/// Deliberately NOT shared with `exec_find` (see the cross-reference there):
/// that shells out to `fd` with a `walkdir` FALLBACK whose dot-skip is
/// per-FILE and which descends INTO `.git`.
///
/// Symlinks are NOT followed (`ignore`'s default with `follow_links(false)`,
/// inherited from the `walkdir` it wraps: a symlink's `file_type()` is
/// symlink, so `is_file()` is false and it is not listed, nor descended into).
/// Bounded by `cap`. Blocking by nature, so callers run it on
/// `spawn_blocking`.
pub fn collect_files(root: &Path, cap: usize) -> (Vec<String>, bool) {
    let mut entries: Vec<String> = Vec::new();
    let mut truncated = false;
    // The root itself is never an entry (and its own name is never a skip
    // decision) — the loop below skips `depth() == 0` rather than gating a min
    // depth on the builder. `min_depth(Some(1))` was measured on 0.4.33 to
    // COMPILE AND THEN PANIC (`called Option::unwrap() on a None value`, in
    // `walk.rs`) on any tree holding a subdirectory — but under the
    // all-matchers-off flag set this call used to carry, and re-measured with
    // the flags below it does NOT panic. A configuration-dependent panic inside
    // `spawn_blocking` (which would trip `list_space_files`' JoinError arm and
    // empty the whole picker) is reason enough to avoid it, and skipping depth 0
    // in the loop is equivalent, holds on every 0.4.x whatever the flag set, and
    // keeps the file-root invariant: a Space opened at a FILE yields nothing, so
    // the `debug_assert!` below can never see a `""` relative path. No depth
    // LIMIT — a deeply-nested file is still addressable; the walk is
    // count-bound only.
    //
    // TWO sorts, two jobs. `sort_by_file_name` makes the WALK deterministic,
    // which is what decides WHICH entries survive when the walk is truncated
    // mid-iteration by the cap — without it the surviving set is `readdir`
    // order, i.e. filesystem luck. The `entries.sort()` at the end makes the
    // DISPLAY order deterministic, and cannot substitute for the builder
    // sort because it runs only after the `break`.
    //
    // FOUR of `WalkBuilder`'s six default-ON filters are ON: the repo decides
    // which of its files are source (ADR 0034). The TWO refusals below are
    // deliberate and load-bearing, and they are spelled out because a future
    // reader will otherwise "helpfully" enable them for `ripgrep` parity.
    //
    //   hidden(false)     — dot-FILES stay listable (ADR 0033): `?.env`
    //                       completes. A dot-file the repo ignores (`*.local`
    //                       here) still does not — that is the matchers below
    //                       deciding, not a name rule up here.
    //   git_global(false) — NEVER read `~/.gitconfig`'s `core.excludesFile` or
    //                       `$XDG_CONFIG_HOME/git/ignore`. Refused so two
    //                       people on the same Space get the same listing
    //                       (ADR 0034): machine-wide rules would make the
    //                       listing a function of the user's environment.
    //                       OF EVERY FLAG ON THIS BUILDER, THIS IS THE ONE NO
    //                       TEST COVERS: flipping it to `true` leaves the whole
    //                       suite green (mutation-checked). See the `mod tests`
    //                       note for why the pin was refused; until someone
    //                       builds a harness that can isolate the environment,
    //                       this comment is the only guard — so treat flipping
    //                       the flag as a design change needing its own ADR.
    //
    // `require_git` is also left alone — at the crate default of `true`, and
    // deliberately not called here. It is what makes git rules repo-scoped AND
    // what stops the crate reading `.gitignore` from parents above the git
    // root; `require_git(false)` would re-import machine/home reach through a
    // different door (a stray `~/.gitignore` would decide every Space under
    // it). `parents(true)` below is what lets a Space opened at a monorepo
    // PACKAGE still inherit the repo root's rules; see the REACH paragraph in
    // the doc comment for the ancestor-`.ignore` leak that comes with it.
    let walker = ignore::WalkBuilder::new(root)
        .hidden(false)
        .parents(true)
        .git_ignore(true)
        .git_exclude(true)
        // UNPINNED REFUSAL (no test can reach it without mutating process-
        // global `HOME`/`XDG_CONFIG_HOME`; see the `mod tests` note and the
        // flag table above). Do not read the green suite as evidence here.
        .git_global(false)
        .ignore(true)
        .follow_links(false)
        .sort_by_file_name(Ord::cmp)
        .filter_entry(|e| {
            // A directory whose own name is skipped: prune the whole subtree
            // (and it would never be listed anyway — only files are).
            !(e.file_type().is_some_and(|t| t.is_dir())
                && is_skipped_dir(&e.file_name().to_string_lossy(), e.depth()))
        })
        .build()
        // `Walk::Item` is `Result<DirEntry, ignore::Error>`, NOT a DirEntry.
        // Swallow: a subtree we cannot read is ONE missing subtree. An
        // `unwrap` here panics inside `spawn_blocking`, trips the JoinError arm
        // of `list_space_files`, and empties the ENTIRE listing.
        .filter_map(|e| e.ok());
    for entry in walker {
        if entry.depth() == 0 {
            continue; // the root is never an entry — see the note above
        }
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        if entries.len() >= cap {
            truncated = true;
            break;
        }
        // SEPARATORS: build the relative path by joining `components()` with
        // `/`. `to_string_lossy()` on a stripped path (what `exec_find`'s
        // walkdir fallback does) yields `\` on Windows, which is NOT in the
        // composer's token charset — every nested completion would silently
        // truncate at the first separator. Invisible to CI (ubuntu-latest),
        // so this is load-bearing by construction rather than by test.
        let Ok(rel) = entry.path().strip_prefix(root) else {
            continue;
        };
        let joined = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        // A non-empty relative path is guaranteed by the depth-0 skip above
        // (a child's stripped path is never empty) — asserted rather than
        // skipped, so the invariant keeps a tripwire without dead code.
        debug_assert!(!joined.is_empty());
        entries.push(joined);
    }
    // Display order: the walk is already deterministic, but per-directory
    // name sorting is depth-first, so a nested `src/a.rs` can precede a
    // root-level `t.txt`. The picker must be deterministic AND sorted.
    entries.sort();
    (entries, truncated)
}

/// The `?` file-completion listing for a Space (ADR 0033). `None`, or a
/// Space that fails to canonicalize, → EMPTY (never an arbitrary-root
/// walk, never an error: the picker degrades to "no files"). Reads NO file
/// bytes — names only.
#[tauri::command]
pub async fn list_space_files(space_path: Option<String>) -> Result<FileListDto, String> {
    let empty = || FileListDto {
        entries: vec![],
        truncated: false,
    };
    let Some(space_path) = space_path else {
        return Ok(empty());
    };
    // Canonicalizing and walking are both blocking (disk I/O), so they run
    // on a worker thread — the same pattern `read_file_bytes` above uses.
    // Kept for the diagnostics below (`space_path` itself moves into the
    // blocking closure): which Space failed is the one thing a log line needs.
    let space_path_for_log = space_path.clone();
    let walked = tauri::async_runtime::spawn_blocking(move || {
        let root = Path::new(&space_path)
            .canonicalize()
            .map_err(|e| e.to_string())?;
        Ok::<_, String>(collect_files(&root, MAX_PICKER_ENTRIES))
    })
    .await;

    let walked = match walked {
        Ok(Ok(w)) => w,
        // BOTH arms below degrade to an EMPTY listing (never an `Err`: the
        // picker must not fail an IPC round-trip over a Space it cannot read).
        // The `Ok(Err(_))` arm is pinned by
        // `list_space_files_degrades_to_empty_for_an_uncanonicalizable_space`;
        // the shape the same way — `Ok` + empty + not-truncated — is also what
        // `list_space_files_degrades_to_empty_for_a_space_that_is_a_file` pins
        // for a walk that succeeds and finds nothing. The `Err(_)` (JoinError)
        // arm has NO test and cannot get one from outside: nothing a caller can
        // pass makes that closure panic, so reaching it would mean inserting a
        // panic into this file. That is precisely why it is not left silent.
        //
        // That contract is exactly what makes these arms INDISTINGUISHABLE to
        // the Client from a Space that legitimately holds no files, and the
        // frontend only logs a REJECTED command, so without the `eprintln!`s
        // below a panic in the walk would be invisible: every file in the Space
        // silently stops being completable, with no trace anywhere. Which arm
        // fired is in the message because the two mean very different things —
        // one is a bad path, the other is a BUG in this file. `eprintln!` is
        // this crate's convention for command-level diagnostics (compare
        // `commands/sessions.rs` and `commands/spaces.rs`).
        Ok(Err(e)) => {
            eprintln!(
                "list_space_files: no listing for Space {space_path_for_log} (the path failed to canonicalize: {e}); degrading to an empty listing"
            );
            return Ok(empty());
        }
        Err(join) => {
            // A `JoinError` means the blocking closure DIED — i.e. the walk
            // panicked, which every "never unwrap in here" comment in
            // `collect_files` exists to prevent. Loud on purpose.
            eprintln!(
                "list_space_files: the file walk PANICKED for Space {space_path_for_log} ({join}); degrading to an empty listing"
            );
            return Ok(empty());
        }
    };
    Ok(FileListDto {
        entries: walked.0,
        truncated: walked.1,
    })
}

#[cfg(test)]
mod tests {
    use crate::agent::MAX_IMAGE_BYTES;
    use crate::commands::files::{
        collect_files, image_within_cap, is_supported_image_path, list_space_files, read_file_bytes,
    };

    // COVERAGE GAP, STATED ON PURPOSE: `collect_files`' `git_global(false)`
    // refusal is the ONLY builder flag in this function with no test — flipping
    // it to `true` (so the walk would read `~/.gitconfig`'s `core.excludesFile`
    // and `$XDG_CONFIG_HOME/git/ignore`) leaves every test here green, verified
    // by mutation. Every OTHER flag here does have a test that dies when it is
    // flipped: `hidden(false)` (`collect_files_keeps_dot_named_files`),
    // `parents(true)` (`…_inherits_the_repo_root_rules_for_a_package_space`),
    // `git_ignore(true)` / `git_exclude(true)` / `ignore(true)` (their own
    // fixture each), `follow_links(false)` (the two symlink tests, unix-gated)
    // and `sort_by_file_name` (`…_keeps_the_alphabetically_first_entries…`).
    // This one is the exception, and it is ADR 0034's central refusal.
    //
    // The pin was considered and REFUSED, not overlooked: observing
    // the difference requires pointing `HOME` (or `XDG_CONFIG_HOME`) at a
    // fixture whose global ignore file names a file, then asserting the file is
    // STILL listed — and `std::env::set_var` mutates PROCESS-GLOBAL state while
    // `cargo test` runs these tests as threads of one process. This lib test
    // binary is full of code that READS that state without holding the shared
    // `test_support::ENV_LOCK`: `tools::sandbox`'s `git_config_files` (its
    // `$HOME/.gitconfig` read, which decides the Landlock ruleset of every
    // sandbox test), its `private_dir_outside_the_read_allow_list` (which
    // deliberately uses the REAL `$HOME` and takes no lock), `crashlog`,
    // `debuglog`, `session::router` and `mcp::oauth` resolving through
    // `dirs::data_dir()`, and `commands::mcp` through `dirs::home_dir()`. A
    // `HOME` mutation held under `ENV_LOCK` still races all of those, so the
    // suite would become order-dependent and flake on whichever unrelated test
    // lost the race — a worse outcome than the gap.
    //
    // So the guard for this refusal is the flag itself plus the comment at the
    // call site, NOT a test. If this gap is ever closed, it needs an
    // environment-isolating harness (a per-test process, or a `collect_files`
    // that takes the reach configuration as an argument) — not a `set_var` in
    // this module.

    /// A fresh temp dir for one test (the same pattern the `read_file_bytes`
    /// tests below use).
    fn tmp_dir(tag: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("files-cmd-test-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn touch(dir: &std::path::Path, rel: &str) {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"x").unwrap();
    }

    /// Write `contents` to `dir/rel`, creating parent directories (`touch`
    /// writes a fixed body; these fixtures need rule FILES whose contents ARE
    /// the rule).
    fn write(dir: &std::path::Path, rel: &str, contents: &str) {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    /// Make `dir` a git repository for fixture purposes: an EMPTY `.git`
    /// DIRECTORY.
    ///
    /// Every git-behaviour fixture below needs this. `require_git` is left at
    /// the crate default of `true`, so the git matchers are consulted only when
    /// some ancestor of the walked directory holds `.git` — a fixture without
    /// it proves nothing about `.gitignore` (it passes for the wrong reason).
    /// A plain directory is the deterministic choice: a worktree-style `.git`
    /// FILE is also supported by the crate, but a malformed one silently
    /// disables `.git/info/exclude`.
    ///
    /// ANCESTOR ASSUMPTION shared by every git fixture here: `std::env::temp_dir()`
    /// has no `.git` directory and no `.ignore` file among its ANCESTORS
    /// (`parents(true)` reads ancestor ignore files, and an ancestor `.git`
    /// would make these fixtures repo-scoped by accident). If a git-related
    /// fixture ever fails only on one machine, check `$TMPDIR`'s ancestry
    /// before suspecting the walker.
    fn git_repo(dir: &std::path::Path) {
        std::fs::create_dir_all(dir.join(".git")).unwrap();
    }

    #[test]
    fn collect_files_lists_nothing_when_the_root_is_a_file() {
        // A Space opened at a FILE must yield an EMPTY listing ("this Space has
        // no files"), never one entry for the Space itself. The depth-0 skip in
        // `collect_files` is the ONLY thing that makes this true: delete it and
        // the walker yields the root, whose stripped path is `""`, so the
        // listing holds a single empty name AND the
        // `debug_assert!(!joined.is_empty())` below the loop fires — which is
        // why that assertion is honest only while the skip is there, and why it
        // is pinned here instead of left to a comment.
        let dir = tmp_dir("root-is-file");
        let file = dir.join("not-a-space.txt");
        std::fs::write(&file, b"x").unwrap();
        let (entries, truncated) = collect_files(&file, 5_000);
        assert!(
            entries.is_empty(),
            "a file root has no children to list; got {entries:?}"
        );
        assert!(!truncated, "an empty listing is complete, not capped");
    }

    #[test]
    fn collect_files_lists_nothing_for_an_empty_directory_root() {
        // The other empty listing: a real, empty Space. Same shape as the
        // file-root case above (`truncated == false`), and the case the
        // `list_space_files` degrade arms are deliberately indistinguishable
        // from — see the note there.
        let root = tmp_dir("empty-root");
        let (entries, truncated) = collect_files(&root, 5_000);
        assert!(
            entries.is_empty(),
            "an empty directory has no files to list; got {entries:?}"
        );
        assert!(!truncated);
    }

    #[test]
    fn collect_files_lists_nested_files_relative_and_sorted() {
        let root = tmp_dir("nested");
        touch(&root, "a.txt");
        touch(&root, "src/b.rs");
        touch(&root, "src/c/d.md");
        let (entries, truncated) = collect_files(&root, 5_000);
        assert_eq!(entries, vec!["a.txt", "src/b.rs", "src/c/d.md"]);
        assert!(!truncated);
    }

    #[test]
    fn collect_files_prunes_vcs_directories_and_lists_other_dot_directories() {
        // Formerly `collect_files_prunes_dot_directories`, which pinned the
        // BLANKET dot-prefix rule. That rule is gone — it was the app
        // overriding the repo on the exact axis ADR 0034 assigns to the repo —
        // so this test now pins the narrow rule, both halves in one fixture:
        // the VCS metadata directory is pruned, an ordinary dot-directory the
        // repo does not ignore is listed.
        let root = tmp_dir("prune");
        touch(&root, ".git/config");
        touch(&root, ".hidden/z");
        touch(&root, "keep.rs");
        let (entries, truncated) = collect_files(&root, 5_000);
        assert_eq!(
            entries,
            vec![".hidden/z", "keep.rs"],
            "`.git` is pruned; `.hidden` is the repo's call, and it declared nothing"
        );
        assert!(!truncated);
    }

    #[test]
    fn collect_files_lists_a_tracked_dot_directory() {
        // THE regression this change removes. This fixture mirrors this repo's
        // own `.github/workflows/ci.yml`, which `git ls-files` reports as
        // tracked — a directory the repository ships is not the app's to hide,
        // and under the blanket dot-prefix rule it was not completable at all.
        let root = tmp_dir("tracked-dot-dir");
        git_repo(&root);
        touch(&root, ".github/workflows/ci.yml");
        touch(&root, "src/app.rs");
        let (entries, truncated) = collect_files(&root, 5_000);
        assert_eq!(
            entries,
            vec![".github/workflows/ci.yml", "src/app.rs"],
            "a dot-directory the repo tracks is listable"
        );
        assert!(!truncated);
    }

    #[test]
    fn collect_files_lists_a_dot_directory_the_repo_does_not_ignore() {
        // The general case, and the `.venv` half matters: a virtualenv nobody
        // ignored is now walked, exactly as `rg --hidden` walks it. `dist/` is
        // here ONLY to show the pruning that remains is the repo's decision —
        // flip `.git_ignore(true)` off and this fixture goes red, whereas no
        // amount of app-side naming can now hide `.claude` or `.venv`.
        let root = tmp_dir("dot-dir-not-ignored");
        git_repo(&root);
        write(&root, ".gitignore", "dist/\n");
        touch(&root, ".claude/settings.json");
        touch(&root, ".venv/pyvenv.cfg");
        touch(&root, "dist/bundle.js");
        touch(&root, "keep.rs");
        let (entries, _) = collect_files(&root, 5_000);
        assert_eq!(
            entries,
            vec![
                ".claude/settings.json",
                ".gitignore",
                ".venv/pyvenv.cfg",
                "keep.rs"
            ],
            "everything the repo does not ignore is listed, dot-named or not"
        );
    }

    #[test]
    fn collect_files_never_lists_a_nested_git_directory() {
        // The case the blanket rule used to cover FOR FREE, which the narrow
        // rule must not lose: a submodule / nested checkout carries its own
        // `.git` at depth > 1, and neither it nor its contents may be listed.
        // The prune is by NAME at any depth, not by position in the tree —
        // `git` cannot ignore its own `.git/`, so this is the one thing no
        // ignore file is asked to protect (see `PRUNED_VCS_DIRS`).
        let root = tmp_dir("nested-git");
        git_repo(&root);
        touch(&root, ".git/config");
        touch(&root, "modules/x/.git/config");
        touch(&root, "modules/x/src/app.rs");
        let (entries, _) = collect_files(&root, 5_000);
        assert_eq!(
            entries,
            vec!["modules/x/src/app.rs"],
            "a nested `.git` is pruned while the nested repo's real source survives"
        );
        assert!(
            entries.iter().all(|e| !e.contains(".git/")),
            "no `.git/…` path may reach the picker; got {entries:?}"
        );
    }

    #[test]
    fn collect_files_prunes_mercurial_and_svn_metadata_directories() {
        // Same reasoning as `.git`, and the nested `.svn` asserts the any-depth
        // rule for them too (SVN keeps one `.svn` per working directory).
        let root = tmp_dir("vcs-other");
        touch(&root, ".hg/store/00changelog.i");
        touch(&root, "sub/.svn/entries");
        touch(&root, "keep.rs");
        let (entries, _) = collect_files(&root, 5_000);
        assert_eq!(entries, vec!["keep.rs"]);
    }

    #[test]
    fn collect_files_lists_a_file_named_git_because_the_prune_is_directories_only() {
        // PINNED ON PURPOSE, so the choice is explicit rather than an accident
        // of `filter_entry`: a submodule's gitfile is a FILE named `.git`
        // (`gitdir: …`), and the dot-FILE rule (ADR 0033 — `?.env` and
        // `?.gitignore` must complete) keeps it. Dropping the `is_dir()` gate
        // on the prune would make this fixture lose `modules/x/.git` and go red.
        let root = tmp_dir("git-file");
        write(&root, "modules/x/.git", "gitdir: ../../.git/modules/x\n");
        touch(&root, "keep.rs");
        let (entries, _) = collect_files(&root, 5_000);
        assert_eq!(entries, vec!["keep.rs", "modules/x/.git"]);
    }

    #[test]
    fn collect_files_lets_a_non_ignored_dot_directory_eat_the_cap() {
        // THE ACCEPTED COST, pinned so it reads as a decision and not an
        // oversight. This is the mirror image of
        // `collect_files_build_output_cannot_eat_the_cap_and_hide_source`: there
        // the repo ignores the heavy tree and the source survives; here nobody
        // has, so the dot-tree is walked and consumes the entry budget — `.`
        // sorts before every letter, so `.venv/lib/…` is walked BEFORE `src/`.
        // That is deliberate (it matches `rg --hidden`, and the app no longer
        // second-guesses the repo), and the remedy is the repo's own: a
        // `.gitignore` or `.ignore` naming `.venv/`. NOT a bug — do not
        // "fix" it by re-adding a name blacklist.
        let root = tmp_dir("dot-dir-cap");
        git_repo(&root);
        for i in 0..4 {
            touch(&root, &format!(".venv/lib/pkg/mod{i}.py"));
        }
        touch(&root, "src/app.rs");

        let (entries, truncated) = collect_files(&root, 3);
        assert!(
            truncated,
            "a non-ignored dot-tree bigger than the cap must report truncation"
        );
        assert_eq!(entries.len(), 3);
        assert!(
            entries.iter().all(|e| e.starts_with(".venv/")),
            "the crowding is done by the dot-tree; got {entries:?}"
        );
        assert!(
            !entries.contains(&"src/app.rs".to_string()),
            "test shape: real source is what got crowded out; got {entries:?}"
        );

        // And the same tree under a generous cap is COMPLETE — which is what
        // makes the truncation above the cap's doing, not a prune.
        let (entries, truncated) = collect_files(&root, 5_000);
        assert!(!truncated);
        assert!(entries.contains(&"src/app.rs".to_string()));
    }

    #[test]
    fn collect_files_lists_node_modules_when_the_repo_declares_it_source() {
        // The hand-rolled skip-by-name blacklist is gone, so this is decided by
        // the repo: a fixture that declares NOTHING (no `.git`, so no git rules;
        // no `.ignore`) lists a committed `node_modules` — which is what you
        // want while debugging a dependency. `collect_files_honours_ignore_files_without_git`
        // shows the repo saying the opposite.
        let root = tmp_dir("node-modules");
        touch(&root, "node_modules/pkg/index.js");
        touch(&root, "keep.rs");
        let (entries, truncated) = collect_files(&root, 5_000);
        assert_eq!(entries, vec!["keep.rs", "node_modules/pkg/index.js"]);
        assert!(!truncated);
    }

    #[test]
    fn collect_files_build_output_cannot_eat_the_cap_and_hide_source() {
        // THE regression this whole feature exists for, and the only test that
        // could not have been written before it: the old fixtures held a handful
        // of tiny files and no build output, so nothing could see a build tree
        // consuming the entry cap — which is exactly how the bug shipped (CI has
        // no build output either, so the class is CI-invisible by construction).
        //
        // Shape: `build/` holds MORE files than the cap, and the one real source
        // file sits in a directory sorting AFTER `build` (`b` < `s`). Ignoring
        // `build/` is the ONLY thing that lets the walker reach the source at
        // all — with the git rules off, `build/artifact-*.o` fills the budget,
        // `truncated` is true, and `src/app.rs` is silently absent.
        //
        // Pinned to `.gitignore` + a `.git` directory, deliberately NOT to an
        // `.ignore` file: this test must go red if the git rules are ever
        // switched back off, and `.ignore` alone would stay green either way.
        let root = tmp_dir("cap-build");
        git_repo(&root);
        write(&root, ".gitignore", "build/\n");
        for i in 0..4 {
            touch(&root, &format!("build/artifact-{i}.o"));
        }
        touch(&root, "src/app.rs");

        let (entries, truncated) = collect_files(&root, 3);
        assert!(
            entries.contains(&"src/app.rs".to_string()),
            "source must survive a build tree larger than the cap; got {entries:?}"
        );
        assert!(
            entries.iter().all(|e| !e.starts_with("build/")),
            "the repo declared build/ not-source; got {entries:?}"
        );
        assert!(!truncated, "the listing is complete once build/ is pruned");
        // `.gitignore` itself is a dot-FILE and stays listable (ADR 0033).
        assert_eq!(entries, vec![".gitignore", "src/app.rs"]);
    }

    #[test]
    fn collect_files_honours_gitignore() {
        let root = tmp_dir("gitignore");
        git_repo(&root);
        write(&root, ".gitignore", "secret.txt\n");
        touch(&root, "secret.txt");
        touch(&root, "keep.rs");
        let (entries, _) = collect_files(&root, 5_000);
        assert_eq!(entries, vec![".gitignore", "keep.rs"]);
    }

    #[test]
    fn collect_files_honours_git_info_exclude_while_never_listing_git_itself() {
        // ADR 0034 refused to ASSERT this belief (that pruning `.git` from the
        // walk does not disable `.git/info/exclude`) and said a test should
        // decide it. This is that test. It holds because the crate builds a
        // directory's git matchers when it PUSHES the directory — reading
        // `<dir>/.git/info/exclude` — which is independent of `filter_entry`,
        // and `filter_entry` only suppresses yielded entries and descent.
        let root = tmp_dir("git-exclude");
        write(&root, ".git/info/exclude", "secret.txt\n");
        touch(&root, ".git/config");
        touch(&root, "secret.txt");
        touch(&root, "keep.rs");
        let (entries, _) = collect_files(&root, 5_000);
        assert_eq!(
            entries,
            vec!["keep.rs"],
            "the exclude-listed file is pruned AND no `.git/…` path is ever listed"
        );
    }

    #[test]
    fn collect_files_honours_ignore_files_without_git() {
        // `.ignore` is the documented remedy for a Space that is NOT a git
        // repository — it is not git-scoped, so it works with no `.git`
        // directory at all. This fixture has none (and `git_repo` is not
        // called), so ONLY the `.ignore` rule can prune `vendor/` here.
        let root = tmp_dir("ignore-no-git");
        write(&root, ".ignore", "vendor/\n");
        touch(&root, "vendor/lib.c");
        touch(&root, "keep.rs");
        let (entries, _) = collect_files(&root, 5_000);
        assert_eq!(entries, vec![".ignore", "keep.rs"]);
    }

    #[test]
    fn collect_files_lists_a_dotfile_unless_the_repo_ignores_it() {
        // The two rules composed: `hidden(false)` keeps dot-FILES listable, so
        // `?.env` completes (ADR 0033); the repo's own rules outrank that
        // generosity, so `?.env.local` does not where `*.local` is ignored.
        let root = tmp_dir("dotfile-ignored");
        git_repo(&root);
        write(&root, ".gitignore", "*.local\n");
        touch(&root, ".env");
        touch(&root, ".env.local");
        let (entries, _) = collect_files(&root, 5_000);
        assert_eq!(entries, vec![".env", ".gitignore"]);
    }

    #[test]
    fn collect_files_lets_an_ignore_file_outrank_gitignore() {
        // A PIN — expected to pass BEFORE the change too, because in the
        // nothing-is-ignored starting state the file is present for the
        // opposite reason. Its job is to keep the precedence order honest:
        // `.ignore` is the override knob, so `!secret.txt` resurrects what
        // `.gitignore` hid.
        let root = tmp_dir("ignore-outranks");
        git_repo(&root);
        write(&root, ".gitignore", "secret.txt\n");
        write(&root, ".ignore", "!secret.txt\n");
        touch(&root, "secret.txt");
        touch(&root, "keep.rs");
        let (entries, _) = collect_files(&root, 5_000);
        assert!(
            entries.contains(&"secret.txt".to_string()),
            ".ignore outranks .gitignore; got {entries:?}"
        );
    }

    #[test]
    fn collect_files_applies_no_git_rules_without_a_git_directory() {
        // A PIN, and the non-git-Space guarantee: git rules are repo-scoped
        // (`require_git` left at its default `true`), so a Space that is not a
        // repository lists everything it holds. Passes before and after —
        // here it is `git_repo` being absent, not the rules being off, that
        // keeps the file listed.
        let root = tmp_dir("no-git-dir");
        write(&root, ".gitignore", "secret.txt\n");
        touch(&root, "secret.txt");
        let (entries, _) = collect_files(&root, 5_000);
        assert!(
            entries.contains(&"secret.txt".to_string()),
            "a non-git Space lists everything; got {entries:?}"
        );
    }

    #[test]
    fn collect_files_inherits_the_repo_root_rules_for_a_package_space() {
        // `parents(true)` is what lets a Space opened at a monorepo PACKAGE
        // inherit the rules at the repo root — without it, opening `package/`
        // as the Space would silently list build output the repo already
        // declared not-source, so two ways of opening one repo get two
        // different listings. A PIN (green before and after: `parents(true)` is
        // on), but NOT a vacuous one — flipping `.parents(false)` reddens it,
        // because `build/` then fills the listing again.
        let outer = tmp_dir("package-space");
        git_repo(&outer);
        write(&outer, ".gitignore", "build/\n");
        let pkg = outer.join("package");
        touch(&pkg, "build/artifact.o");
        touch(&pkg, "src/app.rs");
        let (entries, truncated) = collect_files(&pkg, 5_000);
        assert_eq!(
            entries,
            vec!["src/app.rs"],
            "the repo root's `.gitignore` must reach a package Space"
        );
        assert!(!truncated);
    }

    #[test]
    fn collect_files_does_not_let_a_gitignore_above_the_git_root_reach_in() {
        // A PIN for the REACH axis: a `.gitignore` in a directory ABOVE the git
        // root does not apply, because `require_git` stays at its default of
        // `true` — the alternative would let a stray `~/.gitignore` decide every
        // Space under it (ADR 0034). NB the surviving leak this pin does NOT
        // cover: `.ignore` files ARE read from ancestors. See `collect_files`.
        let outer = tmp_dir("reach-bound");
        write(&outer, ".gitignore", "leaked.txt\n");
        let repo = outer.join("repo");
        git_repo(&repo);
        touch(&repo, "leaked.txt");
        let (entries, _) = collect_files(&repo, 5_000);
        assert!(
            entries.contains(&"leaked.txt".to_string()),
            "git rules are repo-scoped; got {entries:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn collect_files_skips_an_unreadable_subdirectory_without_losing_the_listing() {
        // A subtree we cannot read is ONE missing subtree, not an empty
        // listing: a walk error must be swallowed, never unwrapped. Unwrapping
        // would panic inside `spawn_blocking`, trip the JoinError arm of
        // `list_space_files`, and empty the ENTIRE picker because one
        // permission-denied directory exists.
        use std::os::unix::fs::PermissionsExt;
        let root = tmp_dir("unreadable");
        touch(&root, "a.txt");
        touch(&root, "locked/hidden.txt");
        let locked = root.join("locked");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        // Restored before the assert so a failure still leaves the temp dir
        // removable.
        let result = std::panic::catch_unwind(|| collect_files(&root, 5_000));
        let restored = std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755));
        let (entries, truncated) = result.expect("an unreadable directory must not panic the walk");
        restored.unwrap();
        assert_eq!(
            entries,
            vec!["a.txt"],
            "the readable part of the tree survives"
        );
        assert!(!truncated);
    }

    #[test]
    fn collect_files_keeps_dot_named_files() {
        // The deliberate split: only the VCS metadata DIRECTORIES are pruned,
        // dot-FILES are completion targets (`.` is in the token charset).
        // Unchanged by the narrowing — this fixture holds no directories, so
        // it never discriminated the dot-rule either way; it is the
        // `hidden(false)` flag it pins.
        let root = tmp_dir("dotfile");
        touch(&root, ".gitignore");
        touch(&root, ".env");
        let (entries, _) = collect_files(&root, 5_000);
        assert_eq!(entries, vec![".env", ".gitignore"]);
    }

    #[test]
    fn collect_files_keeps_a_dot_named_root() {
        // Depth 0 is the Space itself, which may legitimately be a dot-dir
        // (a Space opened at `~/.dotfiles`).
        //
        // WHAT THE SECOND CASE IS NOT: a mutation guard. No test in this file
        // can discriminate the `depth > 0` half of `is_skipped_dir`, because
        // the crate never asks us about a depth-0 entry — `Walk::skip_entry`
        // returns `Ok(false)` for `ent.depth() == 0` (ignore 0.4.33,
        // `src/walk.rs:1149-1152`) BEFORE the `filter_entry` predicate is
        // consulted at `:1177-1181`. That makes the app-side guard
        // unreachable-at-depth-0 defence-in-depth, and it was confirmed as
        // such: dropping `depth > 0` leaves all 39 `commands::files` tests
        // green.
        //
        // What it IS: a behaviour pin on the observable contract — the root we
        // were asked to walk is always listed, whatever it is called — which is
        // what a future crate upgrade that starts filtering roots would have to
        // answer to (the parallel visitor is already capable of it: it applies
        // the filter with no depth-0 exemption). Kept for that, not as evidence
        // the guard is exercised.
        let parent = tmp_dir("dotroot");
        let root = parent.join(".dotfiles");
        std::fs::create_dir_all(&root).unwrap();
        touch(&root, "a.txt");
        let (entries, _) = collect_files(&root, 5_000);
        assert_eq!(entries, vec!["a.txt"]);

        let vcs_root = parent.join(".git");
        std::fs::create_dir_all(&vcs_root).unwrap();
        touch(&vcs_root, "HEAD");
        let (entries, _) = collect_files(&vcs_root, 5_000);
        assert_eq!(
            entries,
            vec!["HEAD"],
            "depth 0 is never a prune decision, whatever the directory is called"
        );
    }

    #[test]
    fn collect_files_caps_and_reports_truncation() {
        let root = tmp_dir("cap");
        for i in 0..5 {
            touch(&root, &format!("f{i}.txt"));
        }
        let (entries, truncated) = collect_files(&root, 3);
        assert_eq!(entries.len(), 3);
        assert!(truncated);
    }

    #[test]
    fn collect_files_truncation_keeps_the_alphabetically_first_entries_not_readdir_order() {
        // The determinism pin. The walk is truncated mid-iteration, so
        // WITHOUT a deterministic walk order WHICH `cap` files survive is
        // `readdir` order — whether a given file is completable at all would
        // be filesystem luck, while `FileListDto` promises a sorted — i.e.
        // deterministic — listing. `sort_by_file_name` on the walker makes the
        // SURVIVING SET deterministic; the final `sort` only fixes display
        // order and cannot help here (it runs after the `break`).
        //
        // Creating ASCENDING matters: `readdir` hands back raw directory
        // order, which on the tmpfs/ext4 dirs these tests run on is the
        // REVERSE of insertion order — so creating a.txt..l.txt makes the
        // walk order disagree with alphabetical order, and a walker without
        // `sort_by_file_name` keeps the alphabetically-LATE names and goes
        // red. (`read_dir_order_is_sorted` below says so out loud if a
        // filesystem ever returns an already-sorted order, in which case
        // this pin would be vacuous rather than wrong.)
        let root = tmp_dir("cap-order");
        let names: Vec<String> = ('a'..='l').map(|c| format!("{c}.txt")).collect();
        for name in &names {
            touch(&root, name);
        }
        assert!(
            !read_dir_order_is_sorted(&root),
            "test setup: this filesystem already returns readdir in alphabetical order, so the layout cannot discriminate"
        );

        let (entries, truncated) = collect_files(&root, 4);
        assert!(
            truncated,
            "12 files under a cap of 4 must report truncation"
        );
        assert_eq!(entries, vec!["a.txt", "b.txt", "c.txt", "d.txt"]);
    }

    /// Is `std::fs::read_dir` (i.e. raw `readdir`, which is what an
    /// unsorted `walkdir` iterates) already in alphabetical order?
    fn read_dir_order_is_sorted(dir: &std::path::Path) -> bool {
        let seen: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        let mut sorted = seen.clone();
        sorted.sort();
        seen == sorted
    }

    #[test]
    fn collect_files_sorts_display_order_across_directories() {
        // The OTHER sort's job, pinned separately so the "two sorts, two
        // jobs" comment is not an untested claim: `sort_by_file_name` sorts
        // each DIRECTORY's children, so the walk is depth-first — with a
        // `main.rs` and a `main/` sibling, `main` sorts first and the walk
        // yields `main/util.rs` BEFORE `main.rs` (`/` is not less than `.`).
        // Only the final `entries.sort()` makes the display order right.
        let root = tmp_dir("display-order");
        touch(&root, "main.rs");
        touch(&root, "main/util.rs");
        let (entries, truncated) = collect_files(&root, 5_000);
        assert_eq!(entries, vec!["main.rs", "main/util.rs"]);
        assert!(!truncated);
    }

    #[test]
    fn collect_files_at_exactly_the_cap_is_not_truncated() {
        // The boundary: the check is `>= cap` BEFORE pushing, so the
        // cap+1'th file is what trips `truncated`. A `>` <-> `>=` edit would
        // flip this, so the exact-cap case is pinned.
        let root = tmp_dir("cap-exact");
        for i in 0..4 {
            touch(&root, &format!("f{i}.txt"));
        }
        let (entries, truncated) = collect_files(&root, 4);
        assert_eq!(entries, vec!["f0.txt", "f1.txt", "f2.txt", "f3.txt"]);
        assert_eq!(entries.len(), 4);
        assert!(!truncated, "exactly `cap` files are a complete listing");
    }

    #[cfg(unix)]
    #[test]
    fn collect_files_does_not_descend_into_a_symlinked_directory() {
        // The symlink-to-FILE case above only pins half of it: a symlinked
        // DIRECTORY is the traversal surface. Guards against a future
        // `.follow_links(true)` silently reintroducing a walk that leaves the
        // Space (ADR 0033's scope model is Space-only).
        use std::os::unix::fs::symlink;
        let outside = tmp_dir("symdir-outside");
        std::fs::create_dir_all(outside.join("nest")).unwrap();
        std::fs::write(outside.join("nest/secret.txt"), b"secret").unwrap();
        let root = tmp_dir("symdir-root");
        touch(&root, "a.txt");
        symlink(&outside, root.join("escape")).unwrap();
        let (entries, truncated) = collect_files(&root, 5_000);
        assert_eq!(
            entries,
            vec!["a.txt"],
            "a symlinked dir is neither listed nor walked"
        );
        assert!(!truncated);
    }

    #[cfg(unix)]
    #[test]
    fn collect_files_does_not_follow_symlinks() {
        use std::os::unix::fs::symlink;
        let outside = tmp_dir("symlink-outside");
        std::fs::write(outside.join("secret.txt"), b"secret").unwrap();
        let root = tmp_dir("symlink-root");
        touch(&root, "a.txt");
        symlink(outside.join("secret.txt"), root.join("link.txt")).unwrap();
        let (entries, _) = collect_files(&root, 5_000);
        assert_eq!(entries, vec!["a.txt"]);
    }

    #[tokio::test]
    async fn list_space_files_none_is_empty() {
        let dto = list_space_files(None).await.unwrap();
        assert!(dto.entries.is_empty());
        assert!(!dto.truncated);
    }

    #[tokio::test]
    async fn list_space_files_returns_relative_slash_separated_entries() {
        let root = tmp_dir("space");
        touch(&root, "README.md");
        touch(&root, "src/main.rs");
        let dto = list_space_files(Some(root.to_string_lossy().into()))
            .await
            .unwrap();
        assert_eq!(dto.entries, vec!["README.md", "src/main.rs"]);
        assert!(!dto.truncated);
        for entry in &dto.entries {
            assert!(
                !std::path::Path::new(entry).is_absolute(),
                "{entry} is absolute"
            );
            // AVOID READING THIS LINE AS COVERAGE: it can only fail on Windows,
            // and CI is ubuntu-only, where `MAIN_SEPARATOR` is `/` — so on every
            // machine that runs this suite it is unfailable. Mutation-checked:
            // replacing the `components()`-joined construction in
            // `collect_files` with `to_string_lossy()` (which emits `\` on
            // Windows) leaves every test here green. The REAL guard for that
            // regression is the construction itself and its comment; this
            // assertion only pays off on a platform this suite never runs on.
            assert!(!entry.contains('\\'), "{entry} has a backslash");
        }
    }

    #[tokio::test]
    async fn list_space_files_degrades_to_empty_for_a_space_that_is_a_file() {
        // The degrade-to-empty CONTRACT from the other side: the path
        // canonicalizes FINE (no arm of `list_space_files`' match fires — the
        // walk itself succeeds and simply finds no files), yet the Client still
        // gets `Ok` + empty + not-truncated, exactly the shape
        // `list_space_files_degrades_to_empty_for_an_uncanonicalizable_space`
        // pins for the failing arm. The two together say the command never
        // fails, whether the Space is unreadable or merely empty. (No
        // diagnostic is expected here and none is asserted: nothing went wrong.
        // The `eprintln!`s belong to the two failure arms, and a log line is
        // not part of this contract.)
        let dir = tmp_dir("space-is-a-file");
        let file = dir.join("not-a-space.txt");
        std::fs::write(&file, b"x").unwrap();
        let dto = list_space_files(Some(file.to_string_lossy().into()))
            .await
            .unwrap();
        assert!(dto.entries.is_empty());
        assert!(!dto.truncated);
    }

    #[tokio::test]
    async fn list_space_files_degrades_to_empty_for_an_uncanonicalizable_space() {
        let dto = list_space_files(Some("/no/such/space".to_string()))
            .await
            .unwrap();
        assert!(dto.entries.is_empty());
        assert!(!dto.truncated);
    }

    #[test]
    fn is_supported_image_path_accepts_the_allowlist_case_insensitively() {
        for ext in ["png", "jpg", "jpeg", "gif", "webp", "PNG", "Jpg"] {
            assert!(
                is_supported_image_path(&format!("/home/u/pics/a.{ext}")),
                "{ext} should be accepted"
            );
        }
    }

    #[test]
    fn is_supported_image_path_rejects_non_images_and_extensionless_paths() {
        for path in [
            "/home/u/docs/notes.txt",
            "/home/u/docs/diagram.svg",
            "/home/u/pics/noext",
            "/home/u/pics",
        ] {
            assert!(!is_supported_image_path(path), "{path} should be rejected");
        }
    }

    #[test]
    fn image_within_cap_bounded_by_max_image_bytes() {
        assert!(image_within_cap(0));
        assert!(image_within_cap(MAX_IMAGE_BYTES));
        assert!(!image_within_cap(MAX_IMAGE_BYTES + 1));
    }

    #[tokio::test]
    async fn read_file_bytes_returns_the_file_content_for_a_picked_image() {
        let dir = std::env::temp_dir().join(format!("files-cmd-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("shot.png");
        let bytes: Vec<u8> = vec![0x89, 0x50, 0x4E, 0x47, 1, 2, 3];
        std::fs::write(&path, &bytes).unwrap();
        let result = read_file_bytes(path.to_string_lossy().into()).await;
        assert_eq!(result, Ok(Some(bytes)));
    }

    #[tokio::test]
    async fn read_file_bytes_returns_none_for_a_non_image_extension() {
        let dir = std::env::temp_dir().join(format!("files-cmd-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("notes.txt");
        std::fs::write(&path, b"hello").unwrap();
        let result = read_file_bytes(path.to_string_lossy().into()).await;
        assert_eq!(result, Ok(None));
    }

    #[tokio::test]
    async fn read_file_bytes_errors_when_the_file_cannot_be_read() {
        // A supported extension but a missing file: the read fails (the
        // dialog selected a path that is gone by the time we read it —
        // e.g. the user deleted it).
        let result = read_file_bytes("/no/such/dir/shot.png".to_string()).await;
        let err = result.expect_err("a missing file is an error");
        assert!(!err.is_empty());
    }
}
