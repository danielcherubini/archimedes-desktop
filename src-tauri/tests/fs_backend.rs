//! Integration test for the sandboxed file backend.
//!
//! The [`FsBackend`] is the client-side handler for the ACP `fs/read_text_file`
//! and `fs/write_text_file` methods. The critical property under test is the
//! sandbox: every path the agent asks for is canonicalized and must remain
//! under the session's `cwd` (the backend's `root`), including escapes via
//! `..` and symlinks.

use std::path::{Path, PathBuf};

use archimedes_desktop_lib::agent::{FsBackend, FsError};

/// Create a fresh temp directory to act as the sandbox root.
fn temp_root() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("fs-backend-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn read_write_round_trip_inside_root() {
    let root = temp_root();
    let backend = FsBackend { root: root.clone() };

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
    let backend = FsBackend { root: root.clone() };

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
    let backend = FsBackend { root: root.clone() };

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
    let backend = FsBackend { root: root.clone() };

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
fn absolute_path_outside_root_is_rejected() {
    let root = temp_root();
    let backend = FsBackend { root: root.clone() };

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
