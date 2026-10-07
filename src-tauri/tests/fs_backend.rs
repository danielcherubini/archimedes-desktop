//! Integration test for the sandboxed file backend.
//!
//! The [`FsBackend`] is the client-side handler for the ACP `fs/read_text_file`
//! and `fs/write_text_file` methods. The critical property under test is the
//! sandbox: every path the agent asks for is canonicalized and must remain
//! under one of the backend's `roots`, including escapes via `..` and
//! symlinks.
//!
//! `roots` is a SET (ADR 0030): a session's read boundary is `cwd` PLUS the
//! skill/agent discovery roots, so a file under the SECOND root must be
//! readable while a path under NO root stays rejected.

use std::path::{Path, PathBuf};

use archimedes_lib::agent::{FsBackend, FsError};

/// Create a fresh temp directory to act as the sandbox root.
fn temp_root() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("fs-backend-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A single-root backend (the pre-ADR-0030 shape: `roots == vec![cwd]`).
fn single(root: &Path) -> FsBackend {
    FsBackend {
        roots: vec![root.to_path_buf()],
    }
}

#[test]
fn read_write_round_trip_inside_root() {
    let root = temp_root();
    let backend = single(&root);

    let file = root.join("hello.txt");
    backend
        .write(&file, "hello world\n")
        .expect("write inside root should succeed");

    let content = backend
        .read(&file)
        .expect("read inside root should succeed");
    assert_eq!(content, "hello world\n");

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn write_new_file_under_root_succeeds() {
    let root = temp_root();
    let backend = single(&root);

    // A file that does not exist yet, in a not-yet-existing subdirectory.
    let new_file = root.join("nested/dir/new.txt");
    backend
        .write(&new_file, "fresh content")
        .expect("writing a new file under root should succeed");

    let content = backend
        .read(&new_file)
        .expect("reading the new file should succeed");
    assert_eq!(content, "fresh content");

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn path_escape_via_dotdot_is_rejected() {
    let root = temp_root();
    let backend = single(&root);

    // `../etc/passwd` relative to root escapes the sandbox.
    let escape = root.join("../etc/passwd");
    let err = backend
        .read(&escape)
        .expect_err("reading ../etc/passwd must be rejected");
    assert!(
        matches!(err, FsError::PathEscape { .. }),
        "dotdot escape should be PathEscape, got {err:?}"
    );

    // Same for a write.
    let write_escape = root.join("../evil.txt");
    let err = backend
        .write(&write_escape, "nope")
        .expect_err("writing ../evil.txt must be rejected");
    assert!(
        matches!(err, FsError::PathEscape { .. }),
        "dotdot write escape should be PathEscape, got {err:?}"
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn path_escape_via_symlink_is_rejected() {
    let root = temp_root();
    let backend = single(&root);

    // A symlink inside root that points to a file outside root.
    let link = root.join("sneaky_link");
    std::os::unix::fs::symlink("/etc/passwd", &link).unwrap();

    let err = backend
        .read(&link)
        .expect_err("reading through an escaping symlink must be rejected");
    assert!(
        matches!(err, FsError::PathEscape { .. }),
        "symlink escape should be PathEscape, got {err:?}"
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn validate_resolves_valid_path() {
    let root = temp_root();
    let backend = single(&root);

    let file = root.join("v.txt");
    std::fs::write(&file, "x").unwrap();
    let resolved = backend
        .validate(&file)
        .expect("a path inside the root validates");
    assert_eq!(resolved, file.canonicalize().unwrap());

    // A not-yet-existing file under the root still validates (write target).
    let new_file = root.join("nested/dir/new.txt");
    backend
        .validate(&new_file)
        .expect("a new file under the root validates");

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn validate_rejects_escaping_path() {
    let root = temp_root();
    let backend = single(&root);

    let err = backend
        .validate(&root.join("../etc/passwd"))
        .expect_err("an escaping path must be rejected");
    assert!(
        matches!(err, FsError::PathEscape { .. }),
        "an escaping path should be PathEscape, got {err:?}"
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn read_bytes_round_trip_and_rejects_escape() {
    let root = temp_root();
    let backend = single(&root);

    let bytes: &[u8] = &[1, 2, 3, 4, 0, 255];
    let file = root.join("b.bin");
    std::fs::write(&file, bytes).unwrap();
    assert_eq!(
        backend.read_bytes(&file).expect("read_bytes inside root"),
        bytes
    );

    let err = backend
        .read_bytes(&root.join("../etc/passwd"))
        .expect_err("read_bytes must reject an escaping path");
    assert!(
        matches!(err, FsError::PathEscape { .. }),
        "an escaping path should be PathEscape, got {err:?}"
    );

    let _ = std::fs::remove_dir_all(&root);
}

// ── the root SET (ADR 0030) ───────────────────────────────────────────────

/// A file under a root that is NOT `roots[0]` is readable (the whole point of
/// the root set: `/repo/.agents/skills` is outside the session `cwd` yet must
/// be reachable).
#[test]
fn a_file_under_a_non_first_root_is_readable() {
    let a = temp_root();
    let b = temp_root();
    let backend = FsBackend {
        roots: vec![a.clone(), b.clone()],
    };

    let file = b.join("inside-b.txt");
    std::fs::write(&file, "B").unwrap();
    assert_eq!(backend.read(&file).expect("root B is in bounds"), "B");
    // And writable.
    let new_file = b.join("nested/new.txt");
    backend
        .write(&new_file, "fresh")
        .expect("writing under root B should succeed");
    assert_eq!(backend.read(&new_file).unwrap(), "fresh");

    let _ = std::fs::remove_dir_all(&a);
    let _ = std::fs::remove_dir_all(&b);
}

/// A path under NEITHER root is still a `PathEscape` — adding roots does not
/// make the backend a no-op.
#[test]
fn a_path_under_no_root_at_all_is_rejected() {
    let a = temp_root();
    let b = temp_root();
    let other = temp_root();
    let file = other.join("secret.txt");
    std::fs::write(&file, "secret").unwrap();
    let backend = FsBackend {
        roots: vec![a.clone(), b.clone()],
    };

    let err = backend
        .read(&file)
        .expect_err("a path outside BOTH roots must be rejected");
    assert!(
        matches!(err, FsError::PathEscape { .. }),
        "expected PathEscape, got {err:?}"
    );

    let _ = std::fs::remove_dir_all(&a);
    let _ = std::fs::remove_dir_all(&b);
    let _ = std::fs::remove_dir_all(&other);
}

/// `..` from a root-B-rooted path into a directory that is NOT a root is
/// rejected: every root keeps its own containment, the set is a union of
/// sandboxes, not a sandbox-free pass.
#[test]
fn dotdot_from_a_root_into_a_non_root_is_rejected() {
    let a = temp_root();
    let b = temp_root();
    let sibling = temp_root();
    std::fs::write(sibling.join("secret.txt"), "secret").unwrap();
    let backend = FsBackend {
        roots: vec![a.clone(), b.clone()],
    };

    let err = backend
        .read(Path::new("../secret.txt"))
        .expect_err("`..` out of every root must be rejected");
    assert!(
        matches!(err, FsError::PathEscape { .. }),
        "expected PathEscape, got {err:?}"
    );

    let _ = std::fs::remove_dir_all(&a);
    let _ = std::fs::remove_dir_all(&b);
    let _ = std::fs::remove_dir_all(&sibling);
}

/// A symlink INSIDE a non-first root that points outside every root is
/// rejected (the ADR 0029 symlink rule now applies to every root, not only
/// `roots[0]`).
#[cfg(unix)]
#[test]
fn a_symlink_inside_a_non_first_root_pointing_outside_is_rejected() {
    let a = temp_root();
    let b = temp_root();
    let outside = temp_root();
    let target = outside.join("HONEY.txt");
    std::fs::write(&target, "HONEY").unwrap();
    std::os::unix::fs::symlink(&target, b.join("link")).unwrap();
    let backend = FsBackend {
        roots: vec![a.clone(), b.clone()],
    };

    let err = backend
        .read(&b.join("link"))
        .expect_err("an escaping symlink in root B must be rejected");
    assert!(
        matches!(err, FsError::PathEscape { .. }),
        "expected PathEscape, got {err:?}"
    );

    let _ = std::fs::remove_dir_all(&a);
    let _ = std::fs::remove_dir_all(&b);
    let _ = std::fs::remove_dir_all(&outside);
}

/// An EMPTY root set is a hard error, NOT "unrestricted": fail closed. (The
/// `Ask`/`Allow` floor is an explicit `UNRESTRICTED` root, never an empty
/// vec.)
#[test]
fn an_empty_root_set_fails_closed() {
    let backend = FsBackend { roots: Vec::new() };
    let err = backend
        .validate(Path::new("a.txt"))
        .expect_err("no roots must never mean no limits");
    assert!(
        matches!(err, FsError::Io { .. }),
        "an empty root set must be an Io error, got {err:?}"
    );
}

/// Relative paths join to `roots[0]` (the ordering requirement: `roots[0]`
/// is always the canonical `cwd` — the join base).
#[test]
fn relative_paths_join_to_the_first_root() {
    let a = temp_root();
    let b = temp_root();
    let backend = FsBackend {
        roots: vec![a.clone(), b],
    };
    std::fs::write(a.join("base.txt"), "A").unwrap();
    assert_eq!(backend.read(Path::new("base.txt")).unwrap(), "A");
    let _ = std::fs::remove_dir_all(&a);
}

#[test]
fn absolute_path_outside_root_is_rejected() {
    let root = temp_root();
    let backend = single(&root);

    // An absolute path that has nothing to do with the sandbox.
    let outside = Path::new("/etc/hostname");
    let err = backend
        .read(outside)
        .expect_err("absolute path outside root must be rejected");
    assert!(
        matches!(err, FsError::PathEscape { .. }),
        "absolute outside path should be PathEscape, got {err:?}"
    );

    let _ = std::fs::remove_dir_all(&root);
}
