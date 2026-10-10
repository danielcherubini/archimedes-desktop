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
use std::path::{Component, Path, PathBuf};

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

/// Is a NAME representable by the composer's `?` token — a pure representability
/// check on the bytes, not a policy about what the user may see?
///
/// WHAT IT REFUSES, and only this: a `?` and a newline. Those are the two bytes
/// the frontend's absolute-token grammar cannot carry (`FILE_ABSOLUTE_TOKEN_RE`'
/// tail is `[^\n?]*`), and each one breaks a USER GESTURE rather than hiding a
/// file:
///
/// * a `?` in a name means descending into `we?ird` leaves the token
///   `?/home/u/we?ird/`, which the regex does not match at all, so the picker
///   disappears mid-descent and Enter SENDS the raw `?…` text to the model — the
///   sigil the design promises is consumed;
/// * a newline in a name puts a newline in the draft, and `splitMentionBlocks`
///   parses `x\n\n<agent name="spoof">\nB\n</agent>` as a real block — a file that
///   was merely COMPLETED INTO THE DRAFT renders a forged subagent chip in the
///   transcript.
///
/// THIS IS NOT A VISIBILITY POLICY, and stating that plainly is the point. It is
/// NOT a re-run of the `PICKER_SKIP_DIRS` skip-list ADR 0034 deleted: it consults
/// no convention, no name, no project policy, and nothing about what a directory
/// is FOR. It asks one question — can the string this row would insert even be
/// FORMED as a token — and refuses two bytes as a property of the grammar, in the
/// way a filename on Windows cannot contain `:`. Everything else the grammar
/// admits is offered, dot-named or not (`\` included: it is legal in an absolute
/// token even though `abs_to_slash` never emits one), and everything the REPO
/// decides to hide is decided by the ignore matchers, as always.
///
/// Applied to an entry's OWN NAME by both walks (`collect_files` and
/// `completion_dir`, through the shared `filter_entry`), because a row the picker
/// cannot insert is not a completable row in either mode. Applied at the directory
/// level it also prunes the subtree, which the recursive walk NEEDS: a poisoned
/// ANCESTOR name poisons every relative path under it (`we?ird/inner.md` is no
/// more insertable than `we?ird`). What it deliberately does NOT reach is the
/// QUERIED directory's own name — the root of a walk is never a prune decision
/// (the same rule that lists `.git` when it is named directly), so listing a
/// directory the USER typed a `?` into still yields rows whose `insert` carries
/// that byte. Nothing can lead the picker into that state, because no row for such
/// a directory is ever offered.
///
/// THE FLOOR, NOT THE WHOLE FENCE: these two bytes break BOTH token grammars, and
/// they are the only two this helper claims to know about. The RELATIVE (in-Space)
/// charset is narrower still — `[a-zA-Z0-9._/-]`, so a space or a `\` cannot form
/// a relative token either — and names carrying them are still offered, by decision
/// and with a test that asserts it
/// (`collect_files_refuses_names_the_token_grammar_cannot_hold` offers
/// `a file with spaces.md`). Closing that gap is a change to what the Space picker
/// offers, which is ADR 0033's contract and not this filter's business; if it is
/// ever widened, it widens to a second, RELATIVE-mode rule rather than pretending
/// these two bytes are the whole charset.
fn name_is_completable(name: &str) -> bool {
    !name.contains('?') && !name.contains('\n')
}

/// Walk `root` for the `?` picker: files only, and the VCS metadata directories
/// ([`PRUNED_VCS_DIRS`]) PRUNED at the directory level. Dot-named FILES are KEPT
/// (`.gitignore`, `.env` are real completion targets — `.` is in the token
/// charset), which is why the skip is a directory-level prune and not a
/// name-prefix filter over every entry — and consequently why a FILE named `.git`
/// (a submodule's gitfile) is still listed.
///
/// What the app decides, and what the repo decides. The ONLY name rules the app
/// applies are the VCS-directory prune above and [`name_is_completable`]'s
/// representability rule — the first unconditional (an APP rule, not the repo's)
/// precisely because no ignore file can exclude `.git/`, that being the one thing
/// the repo is never asked to decide; the second a property of the composer's
/// grammar rather than a judgement about the file (see its doc comment, which is
/// explicit that it is NOT the `PICKER_SKIP_DIRS` list ADR 0034 deleted).
/// EVERYTHING ELSE IS THE REPO'S CALL,
/// dot-named or not: `.github/`, `.vscode/`, `.claude/` and a `.venv` nobody
/// ignored are all walked and listed unless an ignore file says otherwise, which
/// is what `rg --hidden` does. The blanket dot-prefix rule this replaced
/// contradicted that (this repo's own tracked `.github/workflows/ci.yml` was not
/// listable here); dropping it applies ADR 0034's principle consistently rather
/// than introducing a new one.
///
/// Apart from those TWO rules nothing is skipped by name — no `node_modules` /
/// `dist` / `target` blacklist or anything like it, because a hand-rolled list is
/// an approximation of what the repo itself declares, and the repo's own ignore
/// rules are the real thing (ADR 0034). The two rules that remain are the VCS
/// prune above and [`name_is_completable`]'s representability rule, and neither
/// one is a visibility decision: the first protects machine state no ignore file
/// can name, the second refuses the two bytes the composer's token grammar cannot
/// carry. Both are pinned for this walk by
/// `collect_files_refuses_names_the_token_grammar_cannot_hold`,
/// `collect_files_refuses_a_name_holding_a_newline` and
/// `collect_files_does_not_descend_into_a_directory_whose_name_the_grammar_cannot_hold`.
/// The cost of that position is ACCEPTED and
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
                // …AND the representability rule, on EVERY entry's own name: a
                // directory whose name cannot form a token is pruned (every path
                // under it inherits the byte), a file whose name cannot is simply
                // not a row. See `name_is_completable` — this is the grammar's
                // limit, not a policy about the file.
                && name_is_completable(&e.file_name().to_string_lossy())
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

/// One row of an out-of-Space **Directory completion** (ADR 0035): one entry of
/// ONE directory, as the `?` picker should show it.
///
/// `name` is what the renderer filters on and `display` is what it shows; they
/// differ only by the `~/` abbreviation. `insert` is the ABSOLUTE path — the
/// draft gets that and NOT the abbreviation, because nothing in this app expands
/// a tilde (`FsBackend::resolve` would read `~/x` as `<cwd>/~/x`), and teaching
/// it to would put a normalisation rule on the seam the **Boundary** is decided
/// at. See ADR 0035 before changing any of the three.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionEntryDto {
    /// The entry's own name, no trailing separator (what the filter matches).
    pub name: String,
    /// ABSOLUTE, `/`-separated — this is what lands in the draft. ONE caveat,
    /// because the word means one less thing on one platform: on WINDOWS a row
    /// under a `?/…` query (as opposed to a `?C:/…` or `?~/…` one) is rooted but
    /// drive-LESS, i.e. `/x/f`, which the platform resolves against the current
    /// drive. Every other row is absolute in the plain sense; nothing is ever
    /// relative to the CWD DIRECTORY (see `expand_completion_root`, which owns
    /// that refusal).
    pub insert: String,
    /// `~/…` when under the home directory, else the same as `insert`. The
    /// abbreviation is a PATH-PREFIX test on the path as the walker produced it,
    /// NOT a test of which directory it is: a queried path that reaches home
    /// THROUGH A SYMLINK (a `link-home -> $HOME`, measured: `display =
    /// "/tmp/…/link-home/notes"` where `$HOME/notes` would read `~/notes`) does not
    /// strip and is therefore displayed in full. That is the no-canonicalisation
    /// rule of `expand_completion_root` showing up in the UI, and it is display
    /// only — `insert` is the same path either way.
    pub display: String,
    /// A directory: the row descends instead of finishing.
    pub is_dir: bool,
}

/// One directory's worth of completion rows + whether the cap trimmed it.
/// The whole directory comes back UNFILTERED so the renderer keeps filtering
/// synchronously — a fetch happens only when the token's DIRECTORY part changes
/// (ADR 0035), which is why this returns rows and not matches.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionDirDto {
    pub entries: Vec<CompletionEntryDto>,
    pub truncated: bool,
}

/// Expand the RAW remainder of an out-of-Space `?` token into the directory it
/// names, WITHOUT touching the filesystem: `~/x` → `<home>/x`, `/x` → `/x`,
/// `C:/x` → `C:/x` ON THE PLATFORM WHERE THAT IS AN ABSOLUTE PATH, and ANYTHING
/// ELSE → `None`.
///
/// THE OUTPUT CONTRACT, which every arm upholds: `Some(path)` is a path the
/// caller may treat as rooted — never a path that resolves against the process
/// CWD. It is not decoration: `CompletionEntryDto::insert` is documented
/// ABSOLUTE and is built from a path under this answer, so a relative answer here
/// becomes a draft that resolves against whatever directory the app happens to be
/// running in — a DIFFERENT directory than the one the user typed, and, through
/// `..`, one they never named. That is the concrete defect the `is_absolute` guard
/// in the drive-root arm exists to prevent (see the comment there), and
/// `expand_completion_root_never_returns_a_relative_path` asserts the contract
/// across a battery of queries.
///
/// ONE PLATFORM CAVEAT ON THE WORD "ABSOLUTE", because `Path::is_absolute` means
/// slightly different things and pretending otherwise would just move the lie: on
/// Unix and macOS every `Some` below IS `is_absolute()`. On WINDOWS a `?/…` token
/// expands to a ROOTED but drive-less path (`/x`), and Rust's `is_absolute` is
/// false for that shape because it wants a drive prefix. It is still rooted — the
/// platform resolves it against the current drive, which is what a Windows user
/// means by `/x` — so this function keeps expanding it, and the refusal is
/// reserved for paths that would land in the CWD DIRECTORY. Both halves are
/// `cfg`-gated in the tests, never asserted silently across platforms.
///
/// WHY `home` IS A PARAMETER. This function never reads `HOME`, `dirs::`, or any
/// environment variable, and that is not tidiness — the same refusal is the
/// reason this file's `git_global(false)` flag has NO test (see the note at the
/// top of `mod tests`): observing it meant mutating process-global `HOME` inside
/// a threaded test binary that other tests read `$HOME` through. The caller
/// resolves the home directory once (`crate::skills::home_dir()`) and hands it
/// in, so every arm below — including `home == None` — is testable as written.
///
/// `None` is the safety property, not an error: it is what makes `~user`, a bare
/// `~`, and EVERY relative query never reach the filesystem. `~user` is a
/// documented refusal (ADR 0035) — supporting it would invite a walk of `/home`.
///
/// No normalisation and no `..` rejection: the **Boundary** canonicalises at gate
/// time and the picker is a view of the filesystem, not the enforcement point
/// (ADR 0033/0035). Two opinions about the same path is the failure mode this
/// file keeps refusing.
///
/// ONE CONSEQUENCE OF READING THE RAW STRING, said plainly because it looks like a
/// bug and is not: a trailing `.` selects a DIFFERENT directory than the token
/// reads. The "ends with `/`" rule below is applied to the query as typed, and
/// `Path::parent()` works on `components()`, which normalises `.` away — so the
/// final component of `<home>/.ssh/.` is `.ssh`, and its PARENT is `<home>`:
/// measured, `~/.ssh/.` lists HOME, while `~/.ssh/` lists `~/.ssh`'s contents. The
/// same split on `..`: `~/notes/..` takes the parent arm and lists `notes` (the
/// `..` is the component `parent()` removes), while `~/notes/../` ends in `/`, so
/// it takes the directory arm and lists the parent. `abs_to_slash` then drops `.`
/// components from the OUTPUT paths (they carry nothing a token can use), which is
/// why the rows can read inconsistently with the query rather than echoing it.
/// Nothing here normalises, and no `..`/`.` is resolved away — the asymmetry IS
/// the refusal above, and the Boundary canonicalises at gate time either way.
pub fn expand_completion_root(query: &str, home: Option<&Path>) -> Option<PathBuf> {
    if let Some(rest) = query.strip_prefix("~/") {
        // FIRST POSITION ONLY. `a~b` and `~user/x` fail this test and fall
        // through to `None` below.
        //
        // The `is_absolute` filter is the SAME contract as the drive-root arm's
        // guard, applied to the one input this arm cannot vouch for on its own: the
        // `home` is the CALLER's, resolved from `$HOME`/`USERPROFILE` by
        // `crate::skills::home_dir()`, so a relative one would turn every `~` row
        // into a CWD-relative path. A real home is absolute on all three platforms,
        // so this filter refuses only the broken case.
        return home
            .map(|h| h.join(rest))
            .filter(|expanded| expanded.is_absolute());
    }
    if query.starts_with('/') {
        // Rooted. Absolute on Unix/macOS; on Windows this is drive-LESS (see the
        // "ONE PLATFORM CAVEAT" paragraph above), which is the platform's own
        // meaning for `/x` and NOT the CWD-relative shape the guard below refuses.
        return Some(PathBuf::from(query));
    }
    // A Windows drive root, `X:/`. Written as a BYTE test on purpose: nothing
    // here may consult the host platform, because CI is ubuntu-only and a
    // Linux-visible failure is the only kind this branch can have. It exists for
    // the Windows descent: a directory row there inserts `C:/Users/you/…`, and
    // if that did not parse the kept-`?` token would die after one level —
    // invisibly to this CI.
    //
    // …AND IT IS NOT ENOUGH ON ITS OWN, which is what the `is_absolute` guard
    // below supplies. The byte test says the query LOOKS like a drive root; only
    // the platform's own `Path` says whether it IS one, and off Windows it is
    // not: `PathBuf::from("C:/x")` is a RELATIVE path on Unix, so the arm used to
    // hand back a path under the process CWD (measured: with a literal `C:`
    // directory in the CWD, `C:/secret/` listed files there, and
    // `C:/../../../etc/` listed `/etc` through it — both as rows whose `insert`
    // claimed to be absolute). So the guard is the contract, and the arm under it
    // is Windows-only IN EFFECT while remaining platform-neutral IN CODE: on
    // Windows the byte test yields an absolute path and the descent works; on
    // Unix and macOS a drive-root query is refused, which is the honest answer,
    // because that syntax names nothing there. Pinned by
    // `expand_completion_root_never_returns_a_relative_path` (the invariant, true
    // on every platform) and by the `cfg`-gated arms of
    // `expand_completion_root_accepts_a_windows_drive_root` (which side each
    // platform takes).
    let bytes = query.as_bytes();
    if bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'/' {
        let path = PathBuf::from(query);
        return path.is_absolute().then_some(path);
    }
    // Nothing else expands — INCLUDING a drive-relative `C:x` and any bare
    // relative path, both of which would otherwise be resolved against whatever
    // directory the app happens to be running in.
    None
}

/// Render an absolute path as the `/`-separated string the composer's token
/// charset accepts.
///
/// WHY THIS IS NOT `collect_files`' construction reused. That one is safe only
/// because it STRIPS the root and joins RELATIVE components, so no
/// `Component::RootDir` ever survives to be joined. `insert` here is absolute,
/// and `RootDir`'s `as_os_str()` is the SOURCE separator — `\` for a Windows
/// path, which is exactly what a `\`-rooted `USERPROFILE` home produces. A `\` in
/// `insert` is fatal twice over: it is outside the token charset, and `?C:\…`
/// fails the absolute token shape's `[A-Za-z]:/` root — so Windows `~`-descent
/// would die after one level, the very failure the drive root above was added to
/// prevent.
///
/// WHAT IS AND IS NOT OBSERVABLE HERE, stated plainly: the `RootDir` arm below
/// RUNS on Linux for every absolute path this function is given — it is covered,
/// by `abs_to_slash_round_trips_a_slash_rooted_path`, which fails if the arm
/// emits anything but a single leading `/` (a `components().join("/")` rewrite
/// produces `//home/u/x` and reddens it). What CANNOT occur on this host is a
/// `\`-VALUED `RootDir`, so the Windows value this arm defends against never
/// appears on CI; mutation-checked: replacing the arm with the raw component
/// leaves every test here GREEN here, which is exactly why the sentence above
/// points at the round-trip test rather than at that one.
///
/// PRIVATE on purpose (unlike the two entry points above): `completion_dir`
/// calls it, so it is live in the lib target whatever its visibility, and a
/// helper with no caller outside this file has no business in the module's API.
/// `mod tests` still reaches it, being a child module.
fn abs_to_slash(p: &Path) -> String {
    let mut out = String::new();
    for component in p.components() {
        match component {
            // `.` carries no information a token can use.
            Component::CurDir => {}
            Component::RootDir => {
                // `/`, NEVER `as_os_str()` — see the doc comment. On Linux this
                // yields `/`; on Windows it replaces `\`.
                if !out.ends_with('/') {
                    out.push('/');
                }
            }
            // `Prefix` (`C:`) stays as-is: it is charset-safe and it is the
            // drive. `ParentDir` (`..`) stays as-is — no normalisation here
            // (see `expand_completion_root`).
            Component::Prefix(_) | Component::ParentDir | Component::Normal(_) => {
                if !out.is_empty() && !out.ends_with('/') {
                    out.push('/');
                }
                out.push_str(&component.as_os_str().to_string_lossy());
            }
        }
    }
    out
}

/// List ONE directory for an out-of-Space `?` token (ADR 0035): the directory is
/// the expanded root when `query` ends with `/` (the token is INSIDE it), else
/// the root's parent (`~/notes` completes names inside `$HOME`, `~/notes/` lists
/// `$HOME/notes`, `/` lists `/`). Unresolvable root, no parent, or a directory
/// that cannot be read → `(empty, false)`. Never panics and never errors: this
/// runs inside `spawn_blocking` in the command, where a panic would empty the
/// whole listing.
///
/// The visibility rules are `collect_files`' — the SAME builder chain, one level
/// deep — because two opinions about what a directory contains is exactly what
/// ADR 0035 rejects (raw `read_dir` was considered and refused for it). It
/// therefore honours another repository's `.gitignore`, `.ignore` and
/// `.git/info/exclude`, prunes `.git`/`.hg`/`.svn` through the SAME
/// `is_skipped_dir`, and refuses names the token grammar cannot hold through the
/// SAME `name_is_completable`. Pinned by
/// `completion_dir_hides_ignored_entries_in_another_repository`,
/// `completion_dir_never_offers_the_vcs_directories` and
/// `completion_dir_refuses_names_the_token_grammar_cannot_hold`.
///
/// The queried directory's OWN name is never a prune decision, so naming `.git`
/// directly LISTS it (`?/repo/.git/` → `objects`, `HEAD`, `config`). That matches
/// `is_skipped_dir`'s `depth > 0` gate, the depth-0 rule in `collect_files`, and a
/// Space opened at `.git`: the prune is an ENTRY rule for what a directory
/// contains, not a veto on what may be asked about.
///
/// REACH, including the part that surprises: the rules are read from the queried
/// directory AND ITS ANCESTORS (`parents(true)`), and the git matchers are gated
/// by the nearest `.git` ANYWHERE ABOVE rather than by the queried directory being
/// a repository. So a peek into a monorepo package is filtered by that repo's
/// rules (the intended half), and a peek into ANY directory under a repository —
/// including `$HOME` when it is a dotfiles checkout, which is common — is decided
/// by rules that live outside the directory the user named, with no note in the
/// listing (the surprising half, and the one thing a reader of this function must
/// not miss). Both are pinned, with the `.parents(false)` mutation named in the
/// test comments, by
/// `completion_dir_inherits_an_ancestor_repositorys_gitignore` and
/// `completion_dir_treats_a_gitignore_as_inert_without_an_ancestor_repository`;
/// nothing above the nearest `.git` reaches in, and `core.excludesFile` never
/// does (`git_global(false)`).
///
/// The chain differs from `collect_files` in exactly THREE ways, each with its
/// own reason below: `follow_links` (true, not false — out here a symlink is the
/// norm and there is nothing to contain), `max_depth` (`Some(1)` — one directory,
/// NEVER a walk; that is the whole design), and what is kept (directories AS
/// WELL AS files, because a directory row is how the descent continues).
///
/// THE CAP AND WHAT IT MAY DROP. The rows are composed **all directories first,
/// then files, each group in the walker's name order, `total` bounded by `cap`**,
/// and the whole directory is read BEFORE that composition happens — it is not
/// cut off mid-walk. `truncated` is true when ANYTHING was dropped, and WHICH
/// HALF was dropped is decided by this order, always: **files are dropped first**
/// (the directories keep the cap's first claim, so a capped listing is always
/// descendable), and **directories are dropped only when they alone exceed
/// `cap`**, in which case the alphabetically first `cap` directories are kept,
/// EVERY file is dropped, and `truncated` is still true. That is a decision and
/// not an accident: a directory row continues the gesture (select it and you are
/// inside), a file row ends it, so when there is not room for both the answer
/// that keeps the user moving is the one that keeps the descent open. Pinned by
/// `completion_dir_keeps_every_directory_when_the_directory_exceeds_the_cap`,
/// `completion_dir_keeps_the_first_cap_directories_when_directories_alone_exceed_the_cap`
/// and — for the case where ONLY directories were dropped, which is the only
/// fixture whose `truncated` cannot be carried by the files term —
/// `completion_dir_reports_truncation_when_only_directories_are_dropped`.
///
/// WHAT `cap` BOUNDS, and what removing the early `break` COSTS — the two are
/// different and this paragraph used to conflate them. MEMORY: `walkdir`'s sorted
/// mode (what `sort_by_file_name` selects) collects the ENTIRE directory into a
/// `Vec` and sorts it before yielding entry #1, so PEAK MEMORY scales with the
/// size of the directory no matter how early the loop stops — a reviewer measured
/// a 250,000-entry directory at ~26 MiB peak, and the `walkdir-2.5.0` source says
/// the same. That is why the `break` never bounded memory, which is what the
/// retracted comment here got wrong. WALL CLOCK is the OTHER axis and does NOT
/// behave that way: it scales with how far the loop is allowed to go, so removing
/// the `break` was a real cost, roughly 3-5x on the loop (see the comment inside
/// it for the measured numbers, both halves of that claim, and what it means for
/// the held-`Enter` window). Do not re-derive one axis from the other: the
/// sentence this paragraph replaced asserted the wall clock was free because the
/// memory was already paid, and measurement disproved it.
///
/// What `cap` bounds is downstream of the walk: the rows materialised as DTOs (at
/// most `2 * cap` are ever built below, so a huge directory does not allocate a
/// row per entry) and hence the size of the IPC payload. Dropping
/// `sort_by_file_name` is NOT the fix for the cost above: the sort STEP is a
/// minority of the bill (measured at about a tenth of a full scan), and the
/// materialise-plus-sort that sorted mode does before entry #1 is a further
/// fraction of it and not the whole of it (a walk-only pass over a 55,000-entry
/// directory measured here in a debug build at ~170 ms sorted against ~100 ms
/// unsorted). Removing the sorted order would also make WHICH rows survive the
/// cap `readdir` luck, which is the worse trade. Nothing here is cheap, and no
/// comment may claim it is.
pub fn completion_dir(
    query: &str,
    home: Option<&Path>,
    cap: usize,
) -> (Vec<CompletionEntryDto>, bool) {
    let Some(root) = expand_completion_root(query, home) else {
        return (Vec::new(), false);
    };
    let dir: &Path = if query.ends_with('/') {
        &root
    } else {
        match root.parent() {
            Some(parent) => parent,
            None => return (Vec::new(), false),
        }
    };
    // Two groups, kept in the walker's name order, so the composition after the
    // loop is a concatenation and never a re-sort. The `_total` counters, not the
    // vector lengths, are what `truncated` is computed from, because the loop
    // below REFUSES to materialise more than `cap` rows of either kind.
    let mut dirs: Vec<CompletionEntryDto> = Vec::new();
    let mut files: Vec<CompletionEntryDto> = Vec::new();
    let mut dir_total = 0usize;
    let mut file_total = 0usize;
    let walker = ignore::WalkBuilder::new(dir)
        // THE FLAGS INHERITED FROM `collect_files`, each with its OWN pin for THIS
        // walk (the visibility claim above is only as good as these, and four of
        // them were comment-only until they were mutation-checked here):
        //   hidden(false)   — `completion_dir_offers_dot_named_files_and_directories`
        //                     (`~/.ssh`, `~/.config`, `~/.aws` are the feature's
        //                     headline targets, and they are all dot-named);
        //   parents(true)   — `completion_dir_inherits_an_ancestor_repositorys_gitignore`
        //                     (the reach rule, including the case where the
        //                     repository is NOT the queried directory);
        //   git_ignore(true) — `completion_dir_hides_ignored_entries_in_another_repository`;
        //   git_exclude(true) — `completion_dir_honours_another_repositorys_git_info_exclude`;
        //   ignore(true)    — `completion_dir_honours_an_ignore_file_in_a_directory_that_is_not_a_repository`.
        .hidden(false)
        .parents(true)
        .git_ignore(true)
        .git_exclude(true)
        // UNPINNED REFUSAL, inherited from `collect_files` and unpinnable for
        // the same reason (see the `mod tests` note): reading a machine-wide
        // `core.excludesFile` here would make a completion of ANY directory a
        // function of the user's environment.
        .git_global(false)
        .ignore(true)
        // DIFFERENT from `collect_files` (out of the Space): one level deep
        // there is no cycle to follow and nothing to contain, and symlinks are
        // the norm rather than the exception (`/etc/os-release`, `~/.local/bin`,
        // most of macOS `~/Library`). The Space walk keeps `follow_links(false)`
        // because there a symlink's invisibility IS containment (ADR 0033). It
        // does not widen what the agent may READ: `FsBackend` judges the
        // canonicalised target.
        .follow_links(true)
        // DIFFERENT from `collect_files`: the one-directory rule IS the design
        // (ADR 0035) — no code path walks further. MUTATION-CHECKED, and NOT on
        // one test: `max_depth(Some(2))` reddens every test that states the WHOLE
        // row list of a fixture holding a directory with something inside it, because
        // the grandchild appears in walk order and those assertions are exhaustive.
        // At the time of writing that is
        // `completion_dir_lists_directories_as_well_as_files` (the grandchild of the
        // listed directory), `completion_dir_offers_symlinked_files_and_directories`
        // (the file INSIDE a symlinked directory),
        // `completion_dir_lists_a_directory_named_git_when_it_is_the_one_queried`
        // (`objects/` gains a child),
        // `completion_dir_offers_dot_named_files_and_directories`
        // (`.config/` gains one), `completion_dir_refuses_names_the_token_grammar_cannot_hold`
        // (`docs/` gains one) and the command-level
        // `list_completion_entries_lists_one_directory`. NO COUNT IS WRITTEN HERE ON
        // PURPOSE: the set grows with every whole-list assertion anyone adds, and
        // this comment's own count had already rotted once (it said "two", then
        // "THREE", and the measurement is six) — which is all a counted claim in a
        // comment ever is: a countdown.
        .max_depth(Some(1))
        .sort_by_file_name(Ord::cmp)
        .filter_entry(|e| {
            // IDENTICAL to `collect_files`: one prune rule for both walks, so
            // `.git` cannot be visible here and invisible there — and the SAME
            // representability rule for the same reason.
            !(e.file_type().is_some_and(|t| t.is_dir())
                && is_skipped_dir(&e.file_name().to_string_lossy(), e.depth()))
                && name_is_completable(&e.file_name().to_string_lossy())
        })
        .build()
        // Swallow, exactly as the Space walk does: a broken symlink is an ERROR
        // entry under `follow_links(true)`, so it is simply invisible (ADR
        // 0035). An `unwrap` here would panic inside `spawn_blocking` and empty
        // the listing.
        .filter_map(|e| e.ok());
    for entry in walker {
        // The directory itself is never a row. NOT `.min_depth(Some(1))`:
        // `collect_files` records that that configuration PANICKED under one
        // flag set and not under another, and a panic here would empty the
        // listing — the loop idiom is what the Space walk already relies on.
        if entry.depth() == 0 {
            continue;
        }
        // DIFFERENT from `collect_files`, which keeps files only: a DIRECTORY is
        // a row here, because selecting it descends. Under `follow_links(true)`
        // a symlink-to-file reports `is_file()` and a symlink-to-dir reports
        // `is_dir()`, which is the behaviour wanted (pinned by
        // `completion_dir_offers_symlinked_files_and_directories`).
        let is_dir = entry.file_type().is_some_and(|t| t.is_dir());
        if !is_dir && !entry.file_type().is_some_and(|t| t.is_file()) {
            continue; // fifos, sockets, devices: not completable
        }
        // NO EARLY `break` ANY MORE, and the reason is the finding this shape
        // fixes: cutting the WALK at `cap` decided which rows existed by WHERE
        // the alphabet put the directories, so a directory whose files sort
        // first returned a capped listing holding NO directory at all — 45 of 199
        // directories silently lost in `/usr/lib64`, measured — and the picker
        // then said "keep typing", which cannot recover a row that was never
        // sent. Truncation is therefore a decision made over the WHOLE directory
        // after the loop, where the dirs-first claim below can actually be
        // honoured.
        //
        // AND THE FULL SCAN IS NOT FREE. Both axes, because they differ:
        //   * PEAK MEMORY is unchanged by the `break` — sorted mode materialises
        //     and sorts the directory before entry #1 (the doc comment, the
        //     `walkdir-2.5.0` source, and RSS all agree), so this loop was never
        //     the memory cliff.
        //   * WALL CLOCK is not: the `break` stopped the iteration, the entry
        //     classification and the row build for everything past the cap, and the
        //     sort it could not skip is only about a tenth of the cost. Measured on
        //     a 55,000-entry directory: ~37 ms WITH the old break-at-cap loop
        //     against ~108 ms for this full scan (about 3x, and 3-5x across sizes
        //     and profiles), with ~280 ms for the full scan at 100,000 entries. The
        //     multiplier is machine- and profile-dependent — a debug build on the
        //     dev box measured ~110 ms with the break against ~228 ms full at
        //     55,000, and ~200 ms against ~420 ms at 100,000 — the direction is not.
        //
        // NAME THE USER-VISIBLE HALF: since `560f610` the composer HOLDS `Enter`
        // while a completion fetch is in flight, and this loop is inside that
        // fetch, so a huge directory is now also a longer held-keystroke window.
        // The `break` was still the wrong trade — it bought that time by deciding
        // which rows EXISTED by where the alphabet put the directories — but the
        // trade is a trade, and "costs nothing" is the sentence this one replaces.
        let path = entry.path();
        let insert = abs_to_slash(path);
        // The abbreviation is DISPLAY-only, and computed from the PATH rather
        // than by string-trimming `insert`, so a home of `/home/uu` cannot be
        // mistaken for a prefix of `/home/u…`. What it therefore CANNOT do is
        // recognise a home reached through a SYMLINK: the walk's path is not
        // canonicalised (nothing here is), so `…/link-home/notes` does not strip to
        // `~/notes` and that row shows its full path — see `CompletionEntryDto::display`.
        let display = home
            .and_then(|h| path.strip_prefix(h).ok())
            .filter(|rel| !rel.as_os_str().is_empty())
            .map(|rel| format!("~/{}", abs_to_slash(rel)))
            .unwrap_or_else(|| insert.clone());
        let row = CompletionEntryDto {
            name: entry.file_name().to_string_lossy().into_owned(),
            insert,
            display,
            is_dir,
        };
        // The per-group bound: `cap` rows of each kind is the most the
        // composition below can ever keep (directories keep at most `cap`; files
        // at most the room the directories leave, which is <= `cap`), so the
        // surplus entries of a huge directory are COUNTED and skipped instead of
        // each becoming three `String`s. It is an ALLOCATION bound and NOT a
        // behaviour rule: mutation-checked, deleting it leaves every test here
        // GREEN, which is why the `min(cap)` on `dir_keep` below is written even
        // though this bound already implies `dirs.len() <= cap` — the subtraction
        // must not depend on a bound no test enforces. The counters ARE
        // load-bearing for behaviour, and
        // `completion_dir_keeps_every_directory_when_the_directory_exceeds_the_cap`
        // is where a wrong count shows up as a missing row.
        if is_dir {
            dir_total += 1;
            if dirs.len() < cap {
                dirs.push(row);
            }
        } else {
            file_total += 1;
            if files.len() < cap {
                files.push(row);
            }
        }
    }
    // DIRECTORIES FIRST, each group in the walker's name order, TOTAL bounded by
    // `cap`. A directory is not an answer, so it is offered ahead of the files a
    // user would rather see: descending is the gesture that gets you there.
    // Pinned by `completion_dir_sorts_directories_before_files`.
    //
    // `sort_by_file_name` on the builder is what makes BOTH halves of that
    // deterministic (the same two-sorts, two-jobs reasoning as `collect_files`):
    // it decides WHICH entries the directory holds in sorted order, so "the
    // alphabetically first `cap`" means the same thing on every filesystem
    // instead of being `readdir` luck, and it makes each group's order stable.
    //
    // The directories' claim comes first and the files get the remainder, which
    // is the whole of the drop policy stated in the doc comment.
    let dir_keep = dirs.len().min(cap);
    let file_room = cap - dir_keep;
    let file_keep = files.len().min(file_room);
    // TRUE iff this listing left something behind — a dropped file, or a dropped
    // directory when the directories alone exceeded the cap. BOTH terms are
    // load-bearing: with only the files term the whole suite stayed green, because
    // every fixture that drops a directory also drops a file; the directories term
    // is pinned on its own by
    // `completion_dir_reports_truncation_when_only_directories_are_dropped`, whose
    // directory holds nothing but directories. Never true for a listing that is
    // actually complete (pinned by
    // `completion_dir_at_exactly_the_cap_is_not_truncated`).
    let truncated = dir_keep < dir_total || file_keep < file_total;
    dirs.truncate(dir_keep);
    files.truncate(file_keep);
    let mut entries = dirs;
    entries.extend(files);
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

/// `?[~/…]q`, `?/…q` or `?C:/…q` — list ONE directory for an out-of-Space
/// completion (ADR 0035). `None` query = no active token = empty, and NO
/// filesystem access at all.
///
/// The contract is `list_space_files`'s, deliberately, including the part that
/// looks like a bug: **never an `Err`**, always `Ok`, degrading to an EMPTY
/// listing, because a `?` picker that errors is worse than one that shows
/// nothing. That is also what makes a failure INDISTINGUISHABLE to the Client
/// from a directory that legitimately holds no files, so the one arm that CAN
/// fail logs WHICH arm fired — `eprintln!` being this crate's convention for
/// command-level diagnostics (compare `list_space_files` above and
/// `commands/spaces.rs`).
///
/// ONE failure arm, not `list_space_files`' two. That command has an `Ok(Err(_))`
/// arm because `canonicalize` can fail there; `completion_dir` is INFALLIBLE (it
/// returns a tuple, not a `Result` — an unreadable directory, an unformable query
/// and a `~` with no home are all `(empty, false)` INSIDE it), so there is no
/// error to log on the `Ok` side, and an `Ok`-side `eprintln!` would fire on
/// EVERY directory fetch and bury the one message that matters. Do not "restore
/// symmetry" with `list_space_files` — the shapes differ because the fallibility
/// does.
///
/// The two properties below are stated with what actually guards them, because
/// both are load-bearing and ONE of them is not guarded by a test.
///
/// * **A `None` query touches no filesystem at all** — held BY CONSTRUCTION (the
///   early return below is the only thing that makes it true), NOT by a test.
///   MUTATION-CHECKED: deleting that early return so `None` falls through into
///   `completion_dir("", …)` leaves the WHOLE suite GREEN, because the engine
///   returns `(empty, false)` for a query it cannot expand — the two paths are
///   OBSERVABLY IDENTICAL from here, so no test can distinguish them and none
///   should be written to pretend otherwise (an assertion would have to inspect
///   the filesystem, which is not this command's contract).
///   `list_completion_entries_degrades_to_empty_without_a_query` pins the empty
///   listing and NOTHING more — do not read its name as a no-`read_dir` proof.
///   The early return is still the right shape: without it the property would be
///   an accident of `expand_completion_root` refusing to expand `""` (which is why
///   the fall-through is silent today), i.e. a filesystem guarantee owned by a
///   pure string function one refactor away from changing, instead of a line here
///   that cannot be wrong.
/// * **No code path walks further than one directory** — held by
///   `list_completion_entries_lists_one_directory`, which asserts the WHOLE row
///   list of a two-level tree; `max_depth(Some(2))` in `completion_dir` reddens
///   it (mutation-checked), so the one-directory claim is a test and not a
///   comment.
///
/// WHY THERE IS NO BOUNDARY GATE ON THIS COMMAND, recorded because it will
/// otherwise be proposed again: listing ANY path from the webview is NOT a new
/// class of access in this app, and a gate here would protect nothing while
/// costing the feature its point. `list_space_files` walks ANY path recursively,
/// `read_file_bytes` reads any allowlisted-extension file, `set_space_trusted`
/// flips the Boundary flag from the webview, and `test_mcp_server` spawns an
/// arbitrary process — all pre-existing, all ungated. A curated root set (`/`,
/// `$HOME`, nothing else) was considered and refused for the same reason: it would
/// be a policy this app does not enforce anywhere else, and the moment the agent
/// goes to READ the path the Boundary decides, which is the enforcement point ADR
/// 0033/0035 put there. The same reasoning keeps `follow_links(true)`: a symlink
/// adds no reach a user lacks (typing the target works), and dropping it would
/// break `~/.local/bin` and most of macOS `~/Library`.
#[tauri::command]
pub async fn list_completion_entries(query: Option<String>) -> Result<CompletionDirDto, String> {
    let empty = || CompletionDirDto {
        entries: vec![],
        truncated: false,
    };
    let Some(query) = query else {
        // The ONLY thing guaranteeing the no-filesystem-access property above —
        // and the reason it is a construction rather than a test is in the doc
        // comment, not here.
        return Ok(empty());
    };
    // Listing a directory is blocking (disk I/O), so it runs on a worker thread
    // — the SAME `tauri::async_runtime::spawn_blocking` this file already uses
    // above, not `tokio::task::spawn_blocking`, which also compiles but would
    // make one file speak two dialects. `home_dir()` is resolved INSIDE the
    // closure so the pure engine never reads the environment itself (see
    // `expand_completion_root` for why that refusal exists).
    let query_for_log = query.clone();
    let listed = tauri::async_runtime::spawn_blocking(move || {
        completion_dir(
            &query,
            crate::skills::home_dir().as_deref(),
            MAX_PICKER_ENTRIES,
        )
    })
    .await;

    Ok(match listed {
        // The `Ok` arm logs NOTHING — see the doc comment.
        Ok((entries, truncated)) => CompletionDirDto { entries, truncated },
        Err(join) => {
            // UNTESTABLE, stated rather than implied: nothing a caller can pass
            // makes that closure panic (`completion_dir` has no `unwrap`,
            // `expect` or `debug_assert` a hostile path can trip — that is what
            // every "never unwrap in here" note in `completion_dir` and
            // `collect_files` exists to prevent), so reaching this arm would mean
            // INSERTING a panic into this file. Same reason `list_space_files`'
            // JoinError arm has no test, and the same reason neither is left
            // silent: a panic here is a BUG in this file, and without this line
            // the only symptom is "the picker quietly went empty". It was
            // VERIFIED BY INJECTION rather than pinned: a `panic!` placed at the
            // top of the closure above made
            // `list_completion_entries_lists_one_directory` fail on an EMPTY row
            // list (so still `Ok`, still the degraded shape) with this line on
            // stderr naming the query; the injected panic was removed again, and
            // no permanent test asserts it, because a test that inserts its own
            // panic would only be testing the test.
            eprintln!(
                "list_completion_entries: the directory listing PANICKED for query {query_for_log} ({join}); degrading to an empty listing"
            );
            empty()
        }
    })
}

#[cfg(test)]
mod tests {
    use crate::agent::MAX_IMAGE_BYTES;
    use crate::commands::files::{
        abs_to_slash, collect_files, completion_dir, expand_completion_root, image_within_cap,
        is_supported_image_path, list_completion_entries, list_space_files, name_is_completable,
        read_file_bytes, CompletionEntryDto, MAX_PICKER_ENTRIES,
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
        // such: dropping `depth > 0` leaves EVERY test in this module green
        // (mutation-checked — deliberately not quoted as a count, which goes stale
        // the moment a test is added; it was quoted as 39 for a while and the module
        // has outgrown that number several times over).
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
        // Space — Space-only being the SCOPE OF THIS WALK (ADR 0033), which ADR
        // 0035 left untouched: Directory completion is a SEPARATE engine that
        // deliberately sets `follow_links(true)` for its one-directory read, and
        // `completion_dir_offers_symlinked_files_and_directories` is its pin. Do
        // not read this line as "the picker can only see the Space"; it says the
        // Space WALK cannot be walked out of the Space.
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
            // AVOID READING THIS LINE AS FULL COVERAGE, but do not read it as
            // UNFAILABLE either — the second version of this note said that, and it
            // was false. What this assertion cannot do on Linux is catch a
            // `Component::RootDir` whose `as_os_str()` is `\`: that VALUE cannot
            // occur on this host (mutation-checked — replacing `collect_files`'s
            // `components()`-joined construction with `to_string_lossy()`, which is
            // the Windows-`\` bug it was written for, leaves every test here green).
            // What it CAN do, and a reviewer demonstrated by creating
            // `weird\name.rs`, is fail on a perfectly legal Linux filename byte: a
            // `\` in an entry's own name reaches the listing verbatim, because the
            // representability filter admits it (the absolute-token charset allows
            // `\`). That is why the fixture below holds no such file — it pins the
            // SEPARATOR this construction emits, which is its whole job.
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

    // ---- out-of-Space Directory completion (ADR 0035) ---------------------

    /// The rows as `(name, is_dir)`, in order — the shape every assertion below
    /// reads, so a test states the WHOLE expected listing rather than `any()`
    /// over it (an `any()` assertion passes when an entry is ADDED, which is
    /// exactly how a walk deeper than one directory would sneak in).
    fn rows(entries: &[CompletionEntryDto]) -> Vec<(&str, bool)> {
        entries
            .iter()
            .map(|e| (e.name.as_str(), e.is_dir))
            .collect()
    }

    /// A `completion_dir` query meaning "list `dir` itself": an absolute path
    /// plus the trailing separator the grammar puts there (no trailing `/` means
    /// "list the parent", which is a different test).
    fn list_query(dir: &std::path::Path) -> String {
        format!("{}/", dir.to_string_lossy())
    }

    #[test]
    fn expand_completion_root_expands_tilde_only_in_first_position() {
        let home = tmp_dir("expand-home");
        assert_eq!(
            expand_completion_root("~/.config/x", Some(&home)),
            Some(home.join(".config/x"))
        );
        // `~/` alone is the home directory itself — the `?/` case of ADR 0035.
        assert_eq!(
            expand_completion_root("~/", Some(&home)),
            Some(home.clone())
        );
        // Everything below must NEVER reach the filesystem: `~` is expanded in
        // the FIRST position only, and `~user` is a documented refusal (it would
        // invite a walk of `/home`).
        assert_eq!(expand_completion_root("a~b", Some(&home)), None);
        assert_eq!(expand_completion_root("~", Some(&home)), None);
        assert_eq!(expand_completion_root("~user/x", Some(&home)), None);
        assert_eq!(
            expand_completion_root("/etc/pas", Some(&home)),
            Some(std::path::PathBuf::from("/etc/pas"))
        );
        // No home resolved → a `~` query is nothing, not a failure.
        assert_eq!(expand_completion_root("~/x", None), None);
        // `..` is just a component: no normalisation, no rejection. The
        // Boundary canonicalises at gate time (ADR 0035), so the engine having
        // an opinion would only make the two disagree.
        assert_eq!(
            expand_completion_root("/home/u/../secrets", Some(&home)),
            Some(std::path::PathBuf::from("/home/u/../secrets"))
        );
    }

    #[test]
    fn expand_completion_root_never_returns_a_relative_path() {
        // THE invariant the whole row shape rests on. `CompletionEntryDto::insert`
        // documents itself ABSOLUTE, and it is built from a path UNDER this
        // function's answer — so a relative answer makes every row under it
        // relative, and the draft gets a path that resolves against the app's CWD
        // rather than the directory the user typed. Measured before the guard, with
        // CWD `/tmp/probeI` holding a literal `C:` directory under it: a query of
        // `C:/secret/` returned ONE row with `insert = "C:/secret/credentials"`
        // (a file under the CWD, not on any drive), and `C:/../../../etc/` returned
        // 200 rows of `/etc` with insert `C:/../../../etc/.java` — the `..` having
        // escaped the CWD while the row still read like an absolute path. One root
        // cause: the drive-root arm was an ungated byte test, and on Unix
        // `PathBuf::from("C:/x")` is NOT absolute.
        //
        // Asserted as the INVARIANT so it passes on Linux, macOS AND Windows CI:
        // every input is either `None` or an ABSOLUTE path. Which platforms take
        // which side of the drive-root case is pinned by
        // `expand_completion_root_accepts_a_windows_drive_root`, which is
        // `cfg`-gated for exactly that reason.
        let home = tmp_dir("expand-invariant");
        // WHY THERE IS NO COMMAND-LEVEL PIN FOR THE CWD WALK, stated so the gap
        // reads as a decision: the measured symptom (a `C:/…` query listing the
        // CWD's contents) is observable only if the process's CURRENT directory
        // holds a literal `C:` entry, and both ways of arranging that —
        // `env::set_current_dir`, or creating `C:` in the repo — mutate state the
        // other tests in this threaded binary share, the same refusal recorded at
        // the top of this module for `git_global(false)`. The invariant asserted
        // below is what makes that walk impossible in the first place, and it
        // holds on every platform.
        for query in [
            "/",
            "/etc/pas",
            "~/",
            "~/notes/x",
            "C:/x",
            "C:/Users/you/",
            "c:/x",
            // The escaping shape: absolute on Windows (where it is a drive path
            // and the Boundary canonicalises it at gate time), meaningless — so
            // REFUSED — everywhere else.
            "C:/../../../etc/",
        ] {
            let Some(path) = expand_completion_root(query, Some(&home)) else {
                continue; // a refusal is always allowed; it is what never touches the filesystem
            };
            // ONE `cfg`-gated exception, stated rather than hidden: on WINDOWS a
            // `/`-rooted query has no drive prefix, so Rust's `is_absolute()` is
            // false for it even though the platform resolves it against the
            // CURRENT DRIVE (which is what a Windows user means by `/x`). That
            // drive-relative spelling is the grammar's own `?/…` token and is
            // unchanged here; what the guard refuses is a path that resolves
            // against the CWD DIRECTORY, which is the `C:/…` shape off Windows.
            // On Linux and macOS — the platforms that run this suite — `/x` IS
            // absolute and IS asserted.
            if cfg!(windows) && query.starts_with('/') {
                continue;
            }
            assert!(
                path.is_absolute(),
                "{query} expanded to the RELATIVE path {path:?}; every `insert` under it would resolve against the CWD"
            );
        }
        // And the refusal half of the same contract: a query this function cannot
        // expand must produce `None` rather than a guess, because `None` is what
        // makes `~user`, a bare `~` and EVERY relative query never reach the
        // filesystem at all.
        for query in [
            "", "~", "~user/x", "a~b", "Notes/x", "C:", "c:\\x", "./x", "x",
        ] {
            assert_eq!(
                expand_completion_root(query, Some(&home)),
                None,
                "{query} is not an absolute query and must not expand"
            );
        }
        // The THIRD way the contract could leak: `~/x` expands against the `home`
        // the CALLER resolved, so a relative `home` would hand back a relative
        // root — and `home` comes from `dirs::home_dir()`, i.e. from `$HOME`,
        // process state this module does not control. Refusing it is the same rule
        // as everywhere else, and a relative path is relative on every platform, so
        // this needs no `cfg` gate.
        assert_eq!(
            expand_completion_root("~/x", Some(std::path::Path::new("relative/home"))),
            None,
            "a relative `home` must not produce a relative root"
        );
    }

    #[test]
    fn expand_completion_root_accepts_a_windows_drive_root() {
        // A drive root is an ABSOLUTE path on the platform that owns the syntax,
        // and NOTHING off it. That split is the `cfg` gate below — asserted
        // without it this test would be a lie on two of the three platforms this
        // suite can run on, because `PathBuf::from("C:/x")` is a meaningless
        // RELATIVE path on Unix (the defect the guard in `expand_completion_root`
        // refuses: a reviewer measured it listing files from the CWD under a
        // literal `C:` directory, and `/etc` through `C:/../../../etc/`).
        //
        // WINDOWS takes the parse arm: a directory row there inserts
        // `C:/Users/you/…`, and if that stopped parsing the kept-`?` token would
        // die after one level of descent (ADR 0035) — a failure CI can never see,
        // which is why the arm is written as a byte test and why this half of the
        // pin is `cfg`-gated rather than skipped.
        #[cfg(windows)]
        {
            assert_eq!(
                expand_completion_root("C:/Users/you/", None),
                Some(std::path::PathBuf::from("C:/Users/you/")),
                "the Windows descent depends on this"
            );
            // Any ASCII letter, and `home` is irrelevant to a drive root.
            assert_eq!(
                expand_completion_root("c:/x", Some(std::path::Path::new("/home/u"))),
                Some(std::path::PathBuf::from("c:/x"))
            );
            assert!(expand_completion_root("C:/x", None).is_some());
        }
        // UNIX and macOS: the syntax names nothing, so it is REFUSED — which is
        // also what keeps the walk away from the CWD.
        #[cfg(not(windows))]
        {
            assert_eq!(expand_completion_root("C:/Users/you/", None), None);
            assert_eq!(
                expand_completion_root("c:/x", Some(std::path::Path::new("/home/u"))),
                None,
                "`home` is irrelevant here too: nothing expands a drive root off Windows"
            );
        }
        // EVERYWHERE: `:/` is required, so a bare `C:` stays a relative query, and
        // so is anything else that is not `/`- or `~/`-rooted.
        assert_eq!(expand_completion_root("C:", None), None);
        assert_eq!(expand_completion_root("Notes/x", None), None);
    }

    #[test]
    fn completion_dir_lists_directories_as_well_as_files() {
        let root = tmp_dir("completion-list");
        touch(&root, "sub/f.txt");
        touch(&root, "top.txt");
        let (entries, truncated) = completion_dir(&list_query(&root), None, MAX_PICKER_ENTRIES);
        // THE one-directory claim, pinned by exact names rather than by a
        // comment: `f.txt` is two levels down, so any `max_depth` other than
        // `Some(1)` puts it in this listing. MUTATION-CHECKED, and the SHAPE of
        // the failure is worth writing down because the first version of this note
        // had it wrong: `max_depth(Some(2))` does not APPEND the grandchild, it
        // INSERTS it in walk order. Measured on this machine the actual is
        // `[("sub", true), ("f.txt", false), ("top.txt", false)]` — `f.txt` lands
        // BETWEEN the child directory and the root-level file, because the walker
        // yields it while descending `sub` and the directories-before-files sort
        // only re-groups, never re-orders within a group. The command-level
        // `list_completion_entries_lists_one_directory` measures the same diff one
        // layer up. That is why this assertion states the WHOLE list: "the last row
        // is `top.txt`" would have passed under the mutant.
        assert_eq!(
            rows(&entries),
            vec![("sub", true), ("top.txt", false)],
            "one directory: the child directory AND the file, never the grandchild"
        );
        assert!(!truncated);
        for entry in &entries {
            // `insert` is ABSOLUTE even though the user may have typed `~`
            // (nothing in this app expands a tilde — ADR 0035).
            let path = std::path::Path::new(&entry.insert);
            assert!(path.is_absolute(), "{} is not absolute", entry.insert);
            assert_eq!(path, root.join(&entry.name));
        }
    }

    /// Names the composer's `?` grammar cannot carry, so neither walk may offer
    /// them: `?` (a second `?` annihilates the token instead of ending it) and a
    /// newline (which also terminates the message).
    const UNGRAMMATABLE: [&str; 2] = ["we?ird.md", "bad\nname.md"];

    /// The same fixture both representability tests build: two ungrammatical
    /// NAMES (a file each) plus an ungrammatical DIRECTORY holding a CLEAN-NAMED
    /// child — the recursive walk's real hazard, since its rows are RELATIVE PATHS
    /// and a poisoned ancestor poisons them all — and THREE ordinary rows around
    /// them so a filter that is too broad cannot pass.
    fn ungrammatical_fixture(tag: &str) -> std::path::PathBuf {
        let root = tmp_dir(tag);
        touch(&root, "keep.md");
        touch(&root, "normal.rs");
        std::fs::create_dir_all(root.join("docs")).unwrap();
        touch(&root, "docs/ok.md");
        touch(&root, "a file with spaces.md");
        touch(&root, UNGRAMMATABLE[0]);
        touch(&root, UNGRAMMATABLE[1]);
        touch(&root, "wei?rd/inner.md");
        root
    }

    #[test]
    fn name_is_completable_refuses_exactly_the_two_grammar_bytes() {
        // The charset table, pinned so the filter cannot quietly widen. The
        // MUTATION that proves this test is not decoration: adding `||
        // name.contains('\\')` to `name_is_completable` reddens the `\` case below
        // (and `completion_insert_never_contains_a_backslash`'s fixture does NOT,
        // which is why the table is asserted here rather than inferred from a walk).
        //
        // The admitted column is the point: a backslash, a `~`, a `:`, a tab, a
        // carriage return and non-UTF8-lossy names are all bytes the absolute token
        // charset carries, so refusing them here would be the visibility policy this
        // helper's doc comment refuses to be.
        for name in ["we?ird.md", "bad\nname.md", "?", "a?b", "a\nb"] {
            assert!(!name_is_completable(name), "{name:?} must be refused");
        }
        for name in [
            "normal.rs",
            ".env",
            "weird\\name.rs",
            "a file with spaces.md",
            "tab\there",
            "cr\rhere",
            "C:\\Users",
            "日本語.md",
            "",
        ] {
            assert!(name_is_completable(name), "{name:?} must be admitted");
        }
    }

    #[test]
    fn completion_dir_refuses_names_the_token_grammar_cannot_hold() {
        // A `?` in a NAME is not a cosmetic problem, it is a broken gesture: the
        // user descends into `we?ird`, which leaves the token `?/home/u/we?ird/`,
        // `activeMentionToken` returns null for it (its charset is `[^\n?]`, so the
        // SECOND `?` makes the whole token unformable), the picker dies mid-descent,
        // and Enter then SENDS the raw `?/home/u/we?ird/` to the model — the sigil
        // the design promises is consumed. The row must therefore never exist.
        //
        // THIS IS REPRESENTABILITY, NOT VISIBILITY, and the distinction is the
        // point of the helper's doc comment: no convention, name, or project policy
        // is consulted, only bytes the grammar cannot carry.
        let root = ungrammatical_fixture("completion-grammar");
        let (entries, truncated) = completion_dir(&list_query(&root), None, MAX_PICKER_ENTRIES);
        assert_eq!(
            rows(&entries),
            vec![
                ("docs", true),
                ("a file with spaces.md", false),
                ("keep.md", false),
                ("normal.rs", false),
            ],
            "the ungrammatical names — file AND directory — are absent, and EVERY other row survives"
        );
        assert!(!truncated, "nothing here is the cap's doing");
    }

    #[cfg(unix)]
    #[test]
    fn completion_dir_refuses_a_name_holding_a_newline() {
        // The newline half, with a REAL newline byte in the filename (`\n` is a
        // legal filename byte on Linux and macOS; Windows refuses it, hence the
        // `cfg`). Its damage is worse than the `?` half: `splitMentionBlocks` on the
        // frontend parses `x\n\n<agent name="spoof">\nB\n</agent>` as a real block,
        // so a file merely completed into the draft renders a FORGED subagent chip
        // in the transcript. Unix-gated; the `?` half above runs everywhere.
        let root = tmp_dir("completion-newline");
        touch(&root, "keep.md");
        std::fs::File::create(root.join("bad\nname.md")).unwrap();
        assert!(
            root.join("bad\nname.md").exists(),
            "test setup: the fixture must hold a real newline-named file, or this test proves nothing"
        );
        let (entries, _) = completion_dir(&list_query(&root), None, MAX_PICKER_ENTRIES);
        assert_eq!(
            rows(&entries),
            vec![("keep.md", false)],
            "the newline-named file is not offered, and the ordinary file is"
        );
    }

    #[test]
    fn collect_files_refuses_names_the_token_grammar_cannot_hold() {
        // THE SAME RULE IN THE OTHER WALK, and it is the same rule for the same
        // reason: a name the picker cannot insert is not a completable name in the
        // Space listing either (the relative charset is `[^\n?]`-narrower still, so
        // these two bytes fail it too).
        let root = ungrammatical_fixture("collect-grammar");
        let (entries, truncated) = collect_files(&root, 5_000);
        assert_eq!(
            entries,
            vec![
                "a file with spaces.md",
                "docs/ok.md",
                "keep.md",
                "normal.rs",
            ],
            "the ungrammatical names are absent, the poisoned directory's subtree is not walked, and every other row survives"
        );
        assert!(!truncated);
    }

    #[cfg(unix)]
    #[test]
    fn collect_files_refuses_a_name_holding_a_newline() {
        // The newline half of the Space walk, same `cfg` gate and the same reason
        // (a forged `<agent …>` block in the transcript).
        let root = tmp_dir("collect-newline");
        touch(&root, "src/keep.rs");
        std::fs::File::create(root.join("bad\nname.md")).unwrap();
        let (entries, _) = collect_files(&root, 5_000);
        assert_eq!(
            entries,
            vec!["src/keep.rs"],
            "the newline-named file is not offered, and the ordinary file in its subdirectory is"
        );
    }

    #[cfg(unix)]
    #[test]
    fn collect_files_does_not_descend_into_a_directory_whose_name_the_grammar_cannot_hold() {
        // The recursive walk needs the rule at the DIRECTORY level too, for a
        // reason that only exists there: the row it would insert is the RELATIVE
        // PATH, so a `?` in an ANCESTOR's name poisons a child whose own name is
        // clean — `we?ird/inner.md` is no more insertable than `we?ird`. Checking
        // only the entry's own name (which is the rule for a FILE) would list it.
        let root = tmp_dir("collect-grammar-dir");
        touch(&root, "we?ird/inner.md");
        touch(&root, "docs/ok.md");
        let (entries, _) = collect_files(&root, 5_000);
        assert_eq!(
            entries,
            vec!["docs/ok.md"],
            "the poisoned subtree is not walked, and the sibling directory is"
        );
    }

    #[test]
    fn completion_dir_lists_the_parent_while_a_last_component_is_being_typed() {
        // THE OTHER HALF of the directory rule ADR 0035 states in one breath: the
        // queried directory is the expanded root when the query ENDS WITH `/`
        // (the token is inside it), and the root's PARENT otherwise (a last
        // component is still being typed). Every other test in this module asks
        // through `list_query`, which appends the `/`; the one exception is
        // `completion_degrades_to_empty_when_the_parent_is_a_file`, which reaches
        // the arm but asserts emptiness, so it passes either way. The parent arm
        // therefore had NO witness of its actual behaviour: MUTATION-CHECKED — replacing
        // `if query.ends_with('/')` with a constant `true` (always take the root
        // arm) left EVERY other test in this module GREEN, because a nonexistent
        // last component simply reads as a missing directory and degrades to the
        // empty listing this shape already pins. That this path is not what the
        // CLIENT sends (`dirPrefix` always ends in `/`, so the shipped picker only
        // ever takes the root arm) is exactly why the engine's own contract is
        // pinned here: ADR 0035 and `CONTEXT.md` describe the rule, and a rule
        // stated in a document has to be reddened by something.
        let root = tmp_dir("completion-parent-arm");
        touch(&root, "notes/todo.md");
        touch(&root, "notes/other.md");
        // `?~/notes/too` in the shape the engine takes it. The trailing `too` is
        // what the RENDERER filters on; the engine returns the whole directory,
        // so the assertion below expects both files and says nothing about `too`.
        let query = format!("{}/notes/todo", root.to_string_lossy());
        let (entries, truncated) = completion_dir(&query, None, MAX_PICKER_ENTRIES);
        assert_eq!(
            rows(&entries),
            vec![("other.md", false), ("todo.md", false)],
            "without a trailing `/` the listed directory is the query's parent, not the query"
        );
        assert!(
            !truncated,
            "two rows under a cap of 5,000 is a complete listing"
        );

        // The control that makes the assertion above the parent arm's and not a
        // fixture accident: the SAME tree asked with the trailing `/` lists the
        // directory the query NAMED. Without this half, a mutant that simply
        // emptied the parent arm would still see a non-empty listing fail the
        // assertion above for the wrong reason.
        let (slash, _) = completion_dir(
            &format!("{}/notes/", root.to_string_lossy()),
            None,
            MAX_PICKER_ENTRIES,
        );
        assert_eq!(
            rows(&slash),
            vec![("other.md", false), ("todo.md", false)],
            "the trailing `/` names the directory itself"
        );
    }

    #[test]
    fn completion_dir_sorts_directories_before_files() {
        // The picker descends, so directories go first even when the file sorts
        // earlier by name — the fixture is built so a plain name sort would put
        // `a_file.txt` first.
        let root = tmp_dir("completion-sort");
        touch(&root, "a_file.txt");
        std::fs::create_dir_all(root.join("z_dir")).unwrap();
        let (entries, _) = completion_dir(&list_query(&root), None, MAX_PICKER_ENTRIES);
        assert_eq!(
            rows(&entries),
            vec![("z_dir", true), ("a_file.txt", false)],
            "directories first, each group in the walker's name order"
        );
    }

    #[test]
    fn completion_dir_displays_home_relative_and_inserts_absolute() {
        let home = tmp_dir("completion-home");
        touch(&home, "notes.md");
        let (entries, _) = completion_dir(&list_query(&home), Some(&home), MAX_PICKER_ENTRIES);
        let row = entries.first().expect("the fixture holds one entry");
        assert_eq!(row.display, "~/notes.md", "the display is abbreviated");
        assert_eq!(
            std::path::Path::new(&row.insert),
            home.join("notes.md"),
            "…while the insert is the real path"
        );
        assert_ne!(row.display, row.insert);

        // Outside the home directory: `display == insert`, both absolute. The
        // `home` here is ANOTHER temp dir, deliberately not `temp_dir()` — every
        // fixture lives under `temp_dir()`, so passing that as `home` would make
        // this row look home-relative for the wrong reason.
        let other = tmp_dir("completion-not-home");
        touch(&other, "elsewhere.txt");
        let (entries, _) = completion_dir(&list_query(&other), Some(&home), MAX_PICKER_ENTRIES);
        let row = entries.first().expect("the fixture holds one entry");
        assert_eq!(row.display, row.insert);
        assert!(
            std::path::Path::new(&row.display).is_absolute(),
            "{} is not absolute",
            row.display
        );
    }

    #[test]
    fn completion_dir_never_offers_the_vcs_directories() {
        // The prune is REUSED (`is_skipped_dir`), not reimplemented, so one rule
        // prunes machine state in someone else's repository too.
        let root = tmp_dir("completion-vcs");
        git_repo(&root);
        std::fs::create_dir_all(root.join(".hg")).unwrap();
        std::fs::create_dir_all(root.join(".svn")).unwrap();
        touch(&root, "keep.rs");
        let (entries, _) = completion_dir(&list_query(&root), None, MAX_PICKER_ENTRIES);
        assert_eq!(
            rows(&entries),
            vec![("keep.rs", false)],
            "`.git`/`.hg`/`.svn` are never rows; got {entries:?}"
        );
    }

    #[test]
    fn completion_dir_lists_a_directory_named_git_when_it_is_the_one_queried() {
        // THE ROOT RULE, pinned for this walk so the doc comment's claim is a test:
        // naming `.git` directly LISTS it. The VCS prune is an ENTRY rule at
        // `depth > 0` — about what a directory contains — and never a veto on what
        // may be asked about, exactly as a Space opened at `.git` is walked by
        // `collect_files` and exactly as `is_skipped_dir`'s `depth > 0` gate reads.
        // The sibling test above pins the other half (`.git` is not a ROW among its
        // parent's entries), so the two together say where the line is.
        let root = tmp_dir("completion-git-root");
        touch(&root, ".git/config");
        touch(&root, ".git/HEAD");
        touch(&root, ".git/objects/pack.idx");
        let (entries, _) =
            completion_dir(&list_query(&root.join(".git")), None, MAX_PICKER_ENTRIES);
        assert_eq!(
            rows(&entries),
            vec![("objects", true), ("HEAD", false), ("config", false)],
            "the queried `.git` is listed; a nested one would be pruned"
        );
    }

    #[test]
    fn completion_dir_hides_ignored_entries_in_another_repository() {
        // THE test that proves reuse rather than reimplementation (ADR 0035
        // rejects raw `read_dir` for exactly this reason): the repo's own
        // `.gitignore` decides what a completion listing of THAT repo shows,
        // even though it is nobody's Space.
        let root = tmp_dir("completion-ignored");
        git_repo(&root);
        write(&root, ".gitignore", "node_modules\n");
        touch(&root, "node_modules/pkg/index.js");
        touch(&root, "package.json");
        let (entries, _) = completion_dir(&list_query(&root), None, MAX_PICKER_ENTRIES);
        assert!(
            !entries.iter().any(|e| e.name == "node_modules"),
            "a gitignored directory stays hidden out of the Space; got {entries:?}"
        );
        assert!(
            entries.iter().any(|e| e.name == "package.json"),
            "vacuity guard: the listing is not empty for the wrong reason"
        );
    }

    #[cfg(unix)]
    #[test]
    fn completion_dir_offers_symlinked_files_and_directories() {
        // Out of the Space `follow_links(true)` (ADR 0035): symlinks are the
        // norm (`/etc/os-release`, `~/.local/bin`, most of macOS `~/Library`),
        // one level deep there is no cycle to follow and nothing to contain.
        // The Space walk keeps `follow_links(false)` — see the two
        // `collect_files_does_not_*_symlinks` tests above.
        use std::os::unix::fs::symlink;
        let root = tmp_dir("completion-symlink");
        touch(&root, "real/target.txt");
        touch(&root, "realdir/inner.txt");
        symlink(root.join("real/target.txt"), root.join("link.txt")).unwrap();
        symlink(root.join("realdir"), root.join("linkdir")).unwrap();
        let (entries, _) = completion_dir(&list_query(&root), None, MAX_PICKER_ENTRIES);
        let dir_row = entries
            .iter()
            .find(|e| e.name == "linkdir")
            .expect("a symlinked directory is offered as a directory row");
        assert!(dir_row.is_dir, "a symlink to a directory descends");
        let file_row = entries
            .iter()
            .find(|e| e.name == "link.txt")
            .expect("a symlinked file is offered");
        assert!(!file_row.is_dir, "a symlink to a file finishes");
        assert!(
            !entries.iter().any(|e| e.name == "inner.txt"),
            "following the link does not walk into it; got {entries:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn completion_dir_hides_a_broken_symlink() {
        // A dangling link is an ERROR entry from the walker (`follow_links(true)`
        // stats the target), and errors are swallowed, so it is simply invisible
        // — and one broken link must not empty the rest of the listing.
        use std::os::unix::fs::symlink;
        let root = tmp_dir("completion-dangling");
        touch(&root, "keep.rs");
        symlink(root.join("no-such-target"), root.join("dangling")).unwrap();
        let (entries, truncated) = completion_dir(&list_query(&root), None, MAX_PICKER_ENTRIES);
        assert_eq!(
            rows(&entries),
            vec![("keep.rs", false)],
            "the dangling link is absent and the rest of the listing survives"
        );
        assert!(!truncated);
    }

    #[test]
    fn completion_dir_keeps_every_directory_when_the_directory_exceeds_the_cap() {
        // THE descent must survive a full directory. A directory with more
        // entries than the cap used to be cut off MID-WALK, in walk order, so
        // which rows the user could see was decided by where the alphabet
        // happened to put the directories: here the eight `f*.txt` files sort
        // before every `z-*` directory, so a cap of 4 returned FOUR FILES AND NO
        // DIRECTORY AT ALL — the rows this feature exists to provide (a
        // reviewer measured 45 of 199 directories lost in `/usr/lib64`), and the
        // frontend then advised "keep typing", which cannot recover a row that
        // was never sent. DIRECTORIES ARE NEVER DROPPED TO MAKE ROOM FOR FILES.
        //
        // MUTATION-CHECKED, both directions:
        //  * composing in walk order instead of directories-first → this test
        //    fails with `[("f0.txt", false), … ("f3.txt", false)]` (the old
        //    behaviour) — and so does the pre-existing
        //    `completion_dir_sorts_directories_before_files`;
        //  * giving files the WHOLE cap instead of what the directories leave
        //    (i.e. dropping the dirs' contribution to the budget) → 7 rows for a
        //    cap of 4.
        // No `read_dir_order_is_sorted` guard is needed here (unlike the Space
        // cap test): `sort_by_file_name` fixes the WALK order regardless of
        // `readdir`, so what matters is which end of that fixed order survives.
        let root = tmp_dir("completion-cap-dirs");
        for i in 0..8 {
            touch(&root, &format!("f{i}.txt"));
        }
        for name in ["z-a", "z-b", "z-c"] {
            std::fs::create_dir_all(root.join(name)).unwrap();
        }
        let (entries, truncated) = completion_dir(&list_query(&root), None, 4);
        assert_eq!(
            rows(&entries),
            vec![
                ("z-a", true),
                ("z-b", true),
                ("z-c", true),
                ("f0.txt", false),
            ],
            "all three directories, then as many of the alphabetically first files as the cap leaves"
        );
        assert!(
            truncated,
            "eleven entries under a cap of 4 were trimmed, so the listing is not complete"
        );
        assert!(
            entries.iter().any(|e| e.is_dir),
            "vacuity guard: a capped listing must still be descendable"
        );
    }

    #[test]
    fn completion_dir_keeps_the_first_cap_directories_when_directories_alone_exceed_the_cap() {
        // THE documented decision for the pathological shape — MORE directories
        // than the cap — which is a real one (`/usr/share`, `node_modules`):
        // keep the alphabetically first `cap` directories, drop EVERY file, and
        // report truncation. Files lose because a file row ends the gesture
        // while a directory row continues it; the cap is still honoured, so this
        // is a bounded answer and not a refusal. Pinning the SURVIVORS as the
        // first `cap` (rather than "some directories") is the point: the order
        // the user reads is the order that was kept.
        //
        // The ONE file is named to sort BEFORE the directories on purpose, so the
        // walk order and the composed order disagree: the pre-fix code (cut the
        // walk at the cap, then group) yields
        // `[("b", true), ("c", true), ("a.txt", false)]` and this test goes red
        // for the same reason as the one above, while a mutant that lets files
        // keep the whole cap yields `[("b", true), ("c", true), ("a.txt", false), …]`.
        let root = tmp_dir("completion-cap-dirsonly");
        for name in ["b", "c", "d", "e"] {
            std::fs::create_dir_all(root.join(name)).unwrap();
        }
        touch(&root, "a.txt");
        let (entries, truncated) = completion_dir(&list_query(&root), None, 3);
        assert_eq!(
            rows(&entries),
            vec![("b", true), ("c", true), ("d", true)],
            "the alphabetically first `cap` directories, and no file"
        );
        assert!(
            truncated,
            "a dropped file and a dropped directory is a capped listing"
        );
    }

    #[test]
    fn completion_dir_reports_truncation_when_only_directories_are_dropped() {
        // THE MISSING FIXTURE: the DIRECTORIES half of `truncated` had no witness
        // at all. `truncated` is `dir_keep < dir_total || file_keep < file_total`,
        // and every fixture above that drops a directory ALSO drops a file, so
        // the second disjunct was true wherever the first one was and the whole
        // suite stayed green under the mutation `truncated = file_keep <
        // file_total` (measured: all 798 tests green with the directories term
        // mutated away). Here the directory holds NOTHING BUT directories, so a
        // dropped file is not available to carry the claim: this test is the only
        // one whose `truncated` can be produced by `dir_keep < dir_total` alone.
        //
        // MUTATION-CHECKED: `truncated = file_keep < file_total` reddens THIS
        // test and NOTHING else in the suite (the failure is this assertion's own
        // message, `truncated` having become `false` because `files` is empty so
        // `file_keep == file_total == 0`) — which is exactly the hole this fixture
        // exists to close.
        //
        // The row assertion is the other half of the point: when the directories
        // alone exceed the cap the survivors are the alphabetically FIRST `cap`
        // of them, which `sort_by_file_name` makes deterministic regardless of
        // `readdir` (the same reasoning as
        // `completion_dir_keeps_the_first_cap_directories_when_directories_alone_exceed_the_cap`,
        // which differs from this one only in holding a single file).
        let root = tmp_dir("completion-cap-dirsonly-nofiles");
        for name in ["e", "d", "c", "b", "a"] {
            std::fs::create_dir_all(root.join(name)).unwrap();
        }
        let (entries, truncated) = completion_dir(&list_query(&root), None, 3);
        assert_eq!(
            rows(&entries),
            vec![("a", true), ("b", true), ("c", true)],
            "the alphabetically first `cap` directories, all of them directories"
        );
        assert!(
            truncated,
            "two directories were dropped and no file exists, so only the \
             directories' own term can report the truncation"
        );
    }

    #[test]
    fn completion_dir_at_exactly_the_cap_is_not_truncated() {
        // The truthfulness half of `truncated`: a listing that dropped NOTHING
        // must not claim it did, or the picker shows a cap note over a complete
        // list. The fixture is MIXED (a directory and files) so the count is the
        // composed total and not one group's length — a `dirs + files >= cap`
        // style off-by-one (or a `>` written where the boundary is `>`) reddens
        // this arm while the cap+1 arm in
        // `completion_dir_keeps_every_directory_when_the_directory_exceeds_the_cap`
        // reddens the other way.
        let root = tmp_dir("completion-cap-exact");
        std::fs::create_dir_all(root.join("z-dir")).unwrap();
        touch(&root, "a.txt");
        touch(&root, "b.txt");
        let (entries, truncated) = completion_dir(&list_query(&root), None, 3);
        assert_eq!(
            rows(&entries),
            vec![("z-dir", true), ("a.txt", false), ("b.txt", false)]
        );
        assert!(!truncated, "exactly `cap` rows means nothing was dropped");
    }

    #[test]
    fn completion_dir_honours_an_ignore_file_in_a_directory_that_is_not_a_repository() {
        // THE PIN FOR `.ignore(true)`, which the doc comment has always claimed and
        // no test held: flipping the flag to `false` left the WHOLE SUITE green
        // (mutation-checked; deliberately not quoted as a test count, which rots),
        // so "it honours `.ignore`" was an unenforced claim. It matters out here
        // precisely because of the case this
        // command exists for — a peek into SOMEONE ELSE'S tree, most of which are
        // not the user's Space: `.ignore` is the override knob that works with no
        // git at all, so it is also the documented remedy for a directory that is
        // not a repository.
        let root = tmp_dir("completion-dot-ignore");
        write(&root, ".ignore", "build.txt\n");
        touch(&root, "build.txt");
        touch(&root, "keep.rs");
        let (entries, _) = completion_dir(&list_query(&root), None, MAX_PICKER_ENTRIES);
        assert_eq!(
            rows(&entries),
            vec![(".ignore", false), ("keep.rs", false)],
            "the `.ignore`listed file is hidden and the ordinary file is not"
        );
    }

    #[test]
    fn completion_dir_honours_another_repositorys_git_info_exclude() {
        // THE PIN FOR `.git_exclude(true)` — same situation as `.ignore`: claimed,
        // unpinned, green when flipped. `.git/info/exclude` is the per-clone list,
        // so this is the rule that hides a collaborator's local scratch files when
        // the peek lands in their checkout.
        let root = tmp_dir("completion-exclude");
        write(&root, ".git/info/exclude", "secret.txt\n");
        touch(&root, ".git/config");
        touch(&root, "secret.txt");
        touch(&root, "keep.rs");
        let (entries, _) = completion_dir(&list_query(&root), None, MAX_PICKER_ENTRIES);
        assert_eq!(
            rows(&entries),
            vec![("keep.rs", false)],
            "the exclude-listed file is hidden, `.git` is pruned, and the ordinary file survives"
        );
    }

    #[test]
    fn completion_dir_reads_the_nearest_ancestor_repositorys_exclude_and_stops_above_it() {
        // THE TWO HALVES OF THE REACH RULE A PEEK ACTUALLY RUNS, pinned together
        // because the two are usually stated separately and each one alone reads
        // like the other. `completion_dir_inherits_an_ancestor_repositorys_gitignore`
        // pins an ancestor's `.gitignore`; nothing here pinned an ancestor's
        // `.git/info/exclude` (the existing exclude test puts `.git` IN the
        // queried directory, which is a different claim: the nearest repository
        // ABOVE the peek is the one whose git rules are read), and nothing pinned
        // the BARRIER — that the nearest ancestor holding `.git` is the git root
        // of the read and no git rule from ABOVE it is consulted. Both matter for
        // the layout this feature meets most often: `$HOME` as a dotfiles repo,
        // where `~/.gitignore` and `~/.git/info/exclude` between them decide every
        // `?~/…` peek, while a `.gitignore` above a nested repository does not.
        // Neither half is visible in the payload — `truncated` stays false and no
        // note names the file that decided it — so a doc claim about this has no
        // witness but this one.
        //
        // THREE MUTATIONS, all measured. The first two fail OPPOSITE assertions,
        // which is what makes this one test rather than two — the barrier and the
        // nearest-repository rule are the same rule seen from each side:
        //  * `.git_exclude(false)` → `secret_excluded.txt` comes back as a row;
        //  * `.require_git(false)` (the refusal `collect_files` documents) → the
        //    `.gitignore` ABOVE the ancestor `.git` reaches in and
        //    `secret_top.txt` disappears, which is the machine/home reach ADR 0034
        //    refuses;
        //  * `.parents(false)` → both files come back, since nothing above the
        //    queried directory is read at all (the sibling test above is that
        //    flag's own pin; this one reddens under it too).
        let root = tmp_dir("completion-nearest-repo");
        let top = root.join("top");
        // ABOVE the repository: inert for git rules, because the nearest `.git`
        // is the git root of the read.
        write(&top, ".gitignore", "secret_top.txt\n");
        let repo = top.join("repo");
        git_repo(&repo);
        write(&repo, ".git/info/exclude", "secret_excluded.txt\n");
        touch(&repo, "inner/pkg/keep.rs");
        touch(&repo, "inner/pkg/secret_excluded.txt");
        touch(&repo, "inner/pkg/secret_top.txt");
        let (entries, truncated) = completion_dir(
            &list_query(&repo.join("inner/pkg")),
            None,
            MAX_PICKER_ENTRIES,
        );
        assert_eq!(
            rows(&entries),
            vec![("keep.rs", false), ("secret_top.txt", false)],
            "the ancestor repository's `.git/info/exclude` hides its file, and the `.gitignore` above that repository hides nothing"
        );
        assert!(
            !truncated,
            "a rule read from outside the queried directory leaves no trace in the payload"
        );
    }

    #[test]
    fn completion_dir_inherits_an_ancestor_repositorys_gitignore() {
        // THE REACH AXIS OF A BRAND-NEW FILESYSTEM SURFACE, pinned at last. This is
        // THE ONE case where a rule that decides what the user sees lives OUTSIDE
        // the directory they queried: `parents(true)` walks the ancestors for
        // ignore files, and the git matchers are gated by the nearest `.git` above
        // rather than by the queried directory being a repository. Measured, and it
        // is the common layout — `$HOME` as a dotfiles repo — where `~/.gitignore`
        // silently decides every `?~/…` peek (control `["config",
        // "credentials.txt"]` → with the ignore file `["config"]`), with
        // `truncated = false` and no note in the UI. Nothing ABOVE the nearest
        // ancestor `.git` reaches in, and `core.excludesFile` never does (the
        // `git_global(false)` refusal).
        //
        // THE MUTATION THIS IS: `.parents(true)` → `.parents(false)` left the whole
        // suite green before this fixture existed; with the flag off, the fixture
        // below lists `credentials.txt` and this assertion fails. The whole repo and
        // its `.gitignore` live INSIDE one tempdir, so no test touches the real
        // `$HOME`.
        //
        // And the shape of the answer is worth reading as a fact about the feature:
        // a peek into a monorepo PACKAGE — the case `parents(true)` exists for — is
        // filtered by the rules of the repo that package belongs to.
        let root = tmp_dir("completion-ancestor-repo");
        let outer = root.join("outer");
        git_repo(&outer);
        write(&outer, ".gitignore", "credentials.txt\n");
        touch(&outer, "inner/pkg/keep.rs");
        touch(&outer, "inner/pkg/credentials.txt");
        let (entries, truncated) = completion_dir(
            &list_query(&outer.join("inner/pkg")),
            None,
            MAX_PICKER_ENTRIES,
        );
        assert_eq!(
            rows(&entries),
            vec![("keep.rs", false)],
            "the ancestor repository's `.gitignore` decides what a peek into its package shows"
        );
        assert!(
            !truncated,
            "the rule is invisible in the payload too: a filtered listing is still 'complete'"
        );
    }

    #[test]
    fn completion_dir_treats_a_gitignore_as_inert_without_an_ancestor_repository() {
        // THE OTHER SIDE OF THE SAME REACH RULE, and the common case out here: a
        // directory that is not inside ANY repository. `.gitignore` is a REPO-scoped
        // rule (`require_git` at its crate default), so with no `.git` among the
        // ancestors it decides nothing and everything is offered — which is why the
        // documented remedy for a directory whose listing needs narrowing is an
        // `.ignore` file (no git needed, see
        // `completion_dir_honours_an_ignore_file_in_a_directory_that_is_not_a_repository`),
        // never "run `git init`". Same fixture as the test above minus the `.git`
        // directory, so the ONLY difference between the two outcomes is the
        // repository.
        let root = tmp_dir("completion-no-ancestor-repo");
        let outer = root.join("outer");
        write(&outer, ".gitignore", "credentials.txt\n");
        touch(&outer, "inner/pkg/keep.rs");
        touch(&outer, "inner/pkg/credentials.txt");
        let (entries, _) = completion_dir(
            &list_query(&outer.join("inner/pkg")),
            None,
            MAX_PICKER_ENTRIES,
        );
        assert_eq!(
            rows(&entries),
            vec![("credentials.txt", false), ("keep.rs", false)],
            "no ancestor `.git` means the `.gitignore` is inert"
        );
    }

    #[test]
    fn completion_dir_offers_dot_named_files_and_directories() {
        // THE PIN FOR `.hidden(false)`, and the most load-bearing of the four flag
        // pins: flipping it to `true` left the WHOLE SUITE green (measured at the
        // time — not quoted as a count, which rots), and it is the flag
        // the feature's headline use cases run on — `~/.ssh`, `~/.config`, `~/.aws`
        // are all dot-directories, so the flip would have made the feature quietly
        // stop working everywhere with CI green. Both halves are pinned: a dot-FILE
        // (`.env`, the ADR 0033 case) and a dot-DIRECTORY (the ADR 0035 descent).
        let root = tmp_dir("completion-hidden");
        touch(&root, ".env");
        touch(&root, ".config/settings.json");
        touch(&root, "notes.md");
        let (entries, _) = completion_dir(&list_query(&root), None, MAX_PICKER_ENTRIES);
        assert_eq!(
            rows(&entries),
            vec![(".config", true), (".env", false), ("notes.md", false)],
            "a dot-file and a dot-directory are both offered"
        );
    }

    #[test]
    fn completion_dir_truncates_at_the_cap() {
        // Same boundary shape as `collect_files_at_exactly_the_cap_is_not_
        // truncated`, both arms: the cap+1'th entry is what trips `truncated`,
        // the survivors are the alphabetically first, and `>= ` <-> `>` would
        // flip one arm or the other.
        let root = tmp_dir("completion-cap");
        for i in 0..5 {
            touch(&root, &format!("f{i}.txt"));
        }
        // THE VACUITY GUARD its Space-walk sibling
        // (`collect_files_truncation_keeps_the_alphabetically_first_entries_not_readdir_order`)
        // already carried and this test did not: the "survivors are the
        // alphabetically first" half below only discriminates if this filesystem's
        // raw `readdir` order is NOT already alphabetical. Creating the files in
        // ascending order is what makes the two orders disagree on the tmpfs/ext4
        // directories these run on (`readdir` comes back reversed); if a
        // filesystem ever returns them sorted, this assertion says so instead of
        // letting the test pass for the wrong reason.
        assert!(
            !read_dir_order_is_sorted(&root),
            "test setup: this filesystem already returns readdir in alphabetical order, so the layout cannot discriminate"
        );
        let (entries, truncated) = completion_dir(&list_query(&root), None, 3);
        assert!(truncated, "more entries than the cap must be reported");
        assert_eq!(
            rows(&entries),
            vec![("f0.txt", false), ("f1.txt", false), ("f2.txt", false)]
        );

        let (entries, truncated) = completion_dir(&list_query(&root), None, 5);
        assert_eq!(entries.len(), 5);
        assert!(
            !truncated,
            "exactly `cap` rows is a complete listing, not a capped one"
        );
    }

    #[test]
    fn completion_degrades_to_empty_for_a_missing_directory() {
        let (entries, truncated) = completion_dir("/no/such/directory/", None, MAX_PICKER_ENTRIES);
        assert!(entries.is_empty());
        assert!(!truncated, "an empty listing is complete, not capped");
    }

    #[test]
    fn completion_degrades_to_empty_when_the_parent_is_a_file() {
        // `?~/notes/x` where `notes` is a FILE: the directory taken from the
        // query is a file, whose only walk entry is depth 0 — which is skipped.
        let root = tmp_dir("completion-parent-file");
        touch(&root, "f.txt");
        let query = format!("{}/f.txt/inner", root.to_string_lossy());
        let (entries, truncated) = completion_dir(&query, None, MAX_PICKER_ENTRIES);
        assert!(
            entries.is_empty(),
            "a file has no children to list; got {entries:?}"
        );
        assert!(!truncated);
    }

    #[test]
    fn completion_degrades_to_empty_when_tilde_has_no_home() {
        // `dirs::home_dir()` returning `None` is a real state (a service
        // account with no `HOME`), and the engine takes it as a parameter
        // precisely so this arm is testable — see the `git_global` refusal note
        // at the top of this module for what a process-global `HOME` costs.
        let (entries, truncated) = completion_dir("~/notes/", None, MAX_PICKER_ENTRIES);
        assert!(entries.is_empty());
        assert!(!truncated);
    }

    #[test]
    fn completion_insert_never_contains_a_backslash() {
        // What this pins is the SEPARATOR: on Windows `Component::RootDir`'s
        // `as_os_str()` is `\`, and a `\` in `insert` is fatal twice over (outside
        // the token charset, and it breaks the `[A-Za-z]:/` root shape), so
        // `abs_to_slash` exists to emit `/` instead. See the same distinction in
        // `list_space_files_returns_relative_slash_separated_entries`: what is
        // unfailable HERE is catching that Windows VALUE, not catching a backslash.
        // A `\` IS a legal Linux filename byte, so an entry NAMED `weird\name.rs`
        // does reach `insert` with a backslash in it (measured) — admitted on
        // purpose, because the absolute-token charset carries `\` and
        // `name_is_completable` therefore refuses only `?` and a newline. That case
        // is pinned by `completion_dir_offers_a_name_containing_a_backslash`; this
        // fixture holds no such file, so what fails here would be a separator bug.
        let root = tmp_dir("completion-backslash");
        touch(&root, "sub/f.txt");
        let (entries, _) = completion_dir(&list_query(&root), None, MAX_PICKER_ENTRIES);
        assert!(!entries.is_empty(), "vacuity guard");
        for entry in &entries {
            assert!(
                !entry.insert.contains('\\'),
                "{} has a backslash, which is outside the token charset",
                entry.insert
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn completion_dir_offers_a_name_containing_a_backslash() {
        // THE OTHER HALF OF THE BACKSLASH STORY, pinned so the sentence above is a
        // test and not a claim: `\` is a legal Linux filename byte (Windows forbids
        // it, hence the `cfg`), the representability filter admits it, and so the
        // row is offered with the backslash in `insert` exactly as the file is
        // named. Two things follow and neither is a defect: the row is only
        // REACHABLE by typing the name (a Windows-style separator is not something
        // this listing can produce, since it inserts `/`), and the byte is inside
        // the charset `FILE_ABSOLUTE_TOKEN_RE` allows, so the token still forms.
        let root = tmp_dir("completion-backslash-name");
        touch(&root, "weird\\name.rs");
        touch(&root, "keep.rs");
        let (entries, _) = completion_dir(&list_query(&root), None, MAX_PICKER_ENTRIES);
        let row = entries
            .iter()
            .find(|e| e.name == "weird\\name.rs")
            .expect("a backslash in a name is not a reason to hide the file");
        assert!(
            row.insert.contains('\\'),
            "the name is carried verbatim; got {}",
            row.insert
        );
        assert!(
            row.insert.starts_with('/') && row.insert.matches('/').count() >= 2,
            "the SEPARATORS are still `/` even in this row; got {}",
            row.insert
        );
    }

    #[cfg(unix)]
    #[test]
    fn completion_dir_offers_a_non_utf8_name_with_a_lossy_insert() {
        // THE OTHER HALF of the representability story, and the half that is a
        // REAL gap rather than a harmless one. A name that is not valid UTF-8 is
        // admitted by `name_is_completable` (it carries neither `?` nor a
        // newline), and both walks build their strings with `to_string_lossy()`,
        // so the invalid byte reaches `name` and `insert` as U+FFFD — a path that
        // is NOT the file's name. The row is therefore offered, inserted, and
        // unreadable: the agent's `read` of the inserted path fails, with nothing
        // in the listing saying so. Pinned as a gap rather than fixed, because
        // every fix is a worse trade: dropping such a file hides it for a reason
        // the user cannot see (a visibility decision, which ADR 0034 assigns to
        // the repo), and carrying raw bytes would need the DTO to stop being a
        // `String`, i.e. a byte-level path API across the IPC seam. What the app
        // DOES refuse is the two bytes that break the token grammar
        // (`?`, a newline); this is the residue the charset happens to carry.
        //
        // MUTATION-CHECKED against the tempting fix: widening
        // `name_is_completable` to also refuse a name whose
        // `to_string_lossy()` is lossy removes the row and reddens this test
        // (measured by adding `|| name.contains('\u{FFFD}')` — the whole row
        // disappears). The byte value itself is asserted, not the row's
        // existence, so the lossy conversion is the thing under test.
        use std::os::unix::ffi::OsStrExt;
        let root = tmp_dir("completion-non-utf8");
        let bad = std::ffi::OsStr::from_bytes(b"bad\xffname.md");
        std::fs::File::create(root.join(bad)).unwrap();
        assert!(
            root.join(bad).exists(),
            "test setup: the fixture must hold a file whose name is not valid UTF-8"
        );
        touch(&root, "keep.md");
        let (entries, _) = completion_dir(&list_query(&root), None, MAX_PICKER_ENTRIES);
        let row = entries
            .iter()
            .find(|e| e.name.contains('\u{FFFD}'))
            .unwrap_or_else(|| panic!("the non-UTF-8 name is offered; got {entries:?}"));
        assert!(
            row.insert.contains('\u{FFFD}'),
            "the lossy byte reaches the inserted path, which is the gap: {}",
            row.insert
        );
        assert!(
            !std::path::Path::new(&row.insert).exists(),
            "vacuity guard: the inserted path must NOT name the file, or this is not the gap described"
        );
        assert!(
            entries.iter().any(|e| e.name == "keep.md"),
            "the ordinary file survives"
        );
    }

    #[cfg(unix)]
    #[test]
    fn completion_dir_displays_a_symlinked_home_in_full() {
        // THE PIN FOR THE ONE DISPLAY CASE THAT IS NOT ABBREVIATED, so the
        // `CompletionEntryDto::display` caveat is a test rather than an
        // acknowledgement: the abbreviation is `strip_prefix` on the path the walker
        // produced, and nothing here canonicalises, so a home reached through a
        // symlink (`link-home -> $HOME`) does not strip. Measured before this test
        // existed: `display = "<tmp>/…/link-home/notes"` where the same file under
        // `$HOME` directly reads `~/notes`. Not a defect to fix — canonicalising here
        // would put a second path identity in charge of what the user sees, and the
        // Boundary owns canonicalisation — but the UI shows the long form for such a
        // home, and that is what this pins.
        use std::os::unix::fs::symlink;
        let home = tmp_dir("display-link-home");
        touch(&home, "notes.md");
        let outer = tmp_dir("display-link-outer");
        symlink(&home, outer.join("link-home")).unwrap();
        let (linked, _) = completion_dir(
            &list_query(&outer.join("link-home")),
            Some(&home),
            MAX_PICKER_ENTRIES,
        );
        let row = linked.first().expect("the fixture holds one entry");
        assert_eq!(
            row.display, row.insert,
            "a home reached through a symlink is displayed in full"
        );

        // The control: the SAME home named directly does abbreviate, so the
        // assertion above fails for the symlink and not for the fixture.
        let (direct, _) = completion_dir(&list_query(&home), Some(&home), MAX_PICKER_ENTRIES);
        assert_eq!(
            direct.first().expect("the fixture holds one entry").display,
            "~/notes.md",
            "vacuity guard: the abbreviation works when the home is named directly"
        );
    }

    #[test]
    fn abs_to_slash_round_trips_a_slash_rooted_path() {
        // The `RootDir` arm RUNS here for every path asserted below (on Linux
        // `as_os_str()` is simply `/`), so a `RootDir` arm that emits anything
        // other than a single leading `/` reddens this test — e.g. building the
        // string with `components().join("/")` would produce `//home/u/x.md`.
        // What does NOT fail here is the Windows VALUE (`\`), which cannot occur
        // on this host; see `completion_insert_never_contains_a_backslash`.
        assert_eq!(
            abs_to_slash(std::path::Path::new("/home/u/notes.md")),
            "/home/u/notes.md"
        );
        assert_eq!(abs_to_slash(std::path::Path::new("/")), "/");
        assert_eq!(
            abs_to_slash(std::path::Path::new("/home/u/../x")),
            "/home/u/../x",
            "`..` survives untouched — the Boundary canonicalises at gate time"
        );
    }

    // ---- the command itself (ADR 0035) ------------------------------------

    #[tokio::test]
    async fn list_completion_entries_degrades_to_empty_without_a_query() {
        // `None` = no active `?` token (a Space-less window, a draft with no
        // token): the SAME shape as `list_space_files_none_is_empty` — `Ok`,
        // empty, not truncated — and the command must not fail an IPC
        // round-trip over it.
        //
        // WHAT THIS DOES NOT PROVE, said plainly: that no filesystem access
        // happened. The command's `None` arm returns before the walk, but the
        // engine it would otherwise fall into ALSO returns `(empty, false)` for
        // an unformable query, so deleting the early return leaves this test
        // GREEN (mutation-checked — see the command's doc comment). The
        // no-filesystem-access property holds BY CONSTRUCTION (the early return),
        // not by test; this test pins the observable contract, which is the
        // empty listing.
        let dto = list_completion_entries(None).await.unwrap();
        assert!(dto.entries.is_empty());
        assert!(!dto.truncated);
    }

    #[tokio::test]
    async fn list_completion_entries_lists_one_directory() {
        // THE `done-when` pin for ADR 0035, at the COMMAND boundary rather than
        // the engine's: a tree TWO levels deep, and only the immediate children
        // come back. MUTATION-CHECKED — `max_depth(Some(2))` in `completion_dir`
        // reddens this assertion; the measured diff is
        // `[("sub", true), ("f.txt", false), ("top.txt", false)]` vs the two rows
        // expected below (the grandchild appears in WALK order, i.e. between the
        // directory and the file, which is also why the assertion states the
        // WHOLE list rather than checking membership). So the "no code path walks
        // more than one directory" claim is a test and not a comment — the
        // engine's own `completion_dir_lists_directories_as_well_as_files` pins
        // the same thing one layer below.
        let root = tmp_dir("completion-cmd");
        touch(&root, "sub/f.txt");
        touch(&root, "top.txt");
        let dto = list_completion_entries(Some(list_query(&root)))
            .await
            .unwrap();
        assert_eq!(
            rows(&dto.entries),
            vec![("sub", true), ("top.txt", false)],
            "one directory: the child directory AND the file, never the grandchild"
        );
        assert!(!dto.truncated);
        for entry in &dto.entries {
            // The command returns the engine's ABSOLUTE `insert` unchanged —
            // nothing here re-bases it on the Space (ADR 0035: nothing in this
            // app expands or re-roots a path at this layer).
            assert!(
                std::path::Path::new(&entry.insert).is_absolute(),
                "{} is not absolute",
                entry.insert
            );
        }
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
