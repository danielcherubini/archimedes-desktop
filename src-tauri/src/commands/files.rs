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
/// Client; past the cap the picker says the listing is capped rather than
/// implying it is complete.
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

/// Is a DIRECTORY named `name` one we never descend into / never list?
fn is_skipped_dir(name: &str, depth: usize) -> bool {
    // Depth 0 is the Space root itself, which may legitimately be a
    // dot-dir (a Space opened at `~/.dotfiles`) — never pruned.
    (depth > 0 && name.starts_with('.')) || PICKER_SKIP_DIRS.contains(&name)
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
    let mut entries: Vec<String> = Vec::new();
    let mut truncated = false;
    // `min_depth(1)`: the root itself is never an entry (and its own name is
    // never a skip decision). No depth LIMIT — a deeply-nested file is still
    // addressable; the walk is count-bound only.
    //
    // TWO sorts, two jobs. `sort_by_file_name` makes the WALK deterministic,
    // which is what decides WHICH entries survive when the walk is truncated
    // mid-iteration by the cap — without it the surviving set is `readdir`
    // order, i.e. filesystem luck. The `entries.sort()` at the end makes the
    // DISPLAY order deterministic, and cannot substitute for the builder
    // sort because it runs only after the `break`.
    let walker = walkdir::WalkDir::new(root)
        .min_depth(1)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|e| {
            let depth = e.depth();
            let name = e.file_name().to_string_lossy();
            // A directory whose own name is skipped: prune the whole subtree
            // (and it would never be listed anyway — only files are).
            !(e.file_type().is_dir() && is_skipped_dir(&name, depth))
        })
        .filter_map(|e| e.ok());
    for entry in walker {
        if !entry.file_type().is_file() {
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
        // `min_depth(1)` guarantees a non-empty relative path (a child's
        // stripped path is never empty) — asserted rather than skipped, so
        // the invariant keeps a tripwire without dead code.
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
    let walked = tauri::async_runtime::spawn_blocking(move || {
        let root = Path::new(&space_path)
            .canonicalize()
            .map_err(|e| e.to_string())?;
        Ok::<_, String>(collect_files(&root, MAX_PICKER_ENTRIES))
    })
    .await;

    let walked = match walked {
        Ok(Ok(w)) => w,
        // A missing/unsafe Space, or a blocking-task join error: no listing.
        Ok(Err(_)) | Err(_) => return Ok(empty()),
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
    fn collect_files_prunes_dot_directories_and_node_modules() {
        let root = tmp_dir("prune");
        touch(&root, ".git/config");
        touch(&root, "node_modules/pkg/index.js");
        touch(&root, ".hidden/z");
        touch(&root, "keep.rs");
        let (entries, truncated) = collect_files(&root, 5_000);
        assert_eq!(entries, vec!["keep.rs"]);
        assert!(!truncated);
    }

    #[test]
    fn collect_files_keeps_dot_named_files() {
        // The deliberate split: dot-DIRECTORIES are pruned, dot-FILES are
        // completion targets (`.` is in the token charset).
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
        let parent = tmp_dir("dotroot");
        let root = parent.join(".dotfiles");
        std::fs::create_dir_all(&root).unwrap();
        touch(&root, "a.txt");
        let (entries, _) = collect_files(&root, 5_000);
        assert_eq!(entries, vec!["a.txt"]);
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
            assert!(!entry.contains('\\'), "{entry} has a backslash");
        }
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
