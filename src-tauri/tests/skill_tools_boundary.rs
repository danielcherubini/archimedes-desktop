//! The `read_skill` boundary, attacked (ADR 0029).
//!
//! `read_skill` exists to read files OUTSIDE the session sandbox, so its
//! ONLY containment is the per-skill `FsBackend` rooted at the skill's own
//! directory. These tests are the adversarial proof that the boundary holds:
//! every case below names something that is NOT a file inside the skill dir
//! and must be rejected WITHOUT (a) returning foreign content or (b)
//! disclosing a path the model did not name (a resolved symlink target).
//!
//! The fixtures live in temp dirs and are pinned via `ToolCtx::skill_roots`,
//! so no test here ever reads the real `~/.agents/skills`.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::json;
use tokio_util::sync::CancellationToken;

use archimedes_lib::agent::tools::{execute_tool, ContentBlock, ToolCtx, ToolResult};
use archimedes_lib::skills::SkillScope;

/// A temp dir (removed on `Drop`).
struct Tmp(PathBuf);
impl Tmp {
    fn new(tag: &str) -> Self {
        let d = std::env::temp_dir().join(format!("skill-esc-{tag}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&d).unwrap();
        Self(d)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A `<name>/SKILL.md` skill under `root`, returning the skill's dir.
fn write_skill(root: &Path, name: &str, body: &str) -> PathBuf {
    let dir = root.join(name);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\n---\n{body}\n"),
    )
    .unwrap();
    dir
}

#[cfg(unix)]
fn symlink(target: &Path, link: &Path) {
    std::os::unix::fs::symlink(target, link).unwrap();
}
#[cfg(windows)]
fn symlink(target: &Path, link: &Path) {
    // Symlinks need privilege on Windows; a junction covers the DIRECTORY
    // cases (the file-symlink cases are skipped there).
    if target.is_dir() {
        std::os::windows::fs::symlink_dir(target, link).is_ok();
    } else {
        std::os::windows::fs::symlink_file(target, link).is_ok();
    }
}

fn ctx(cwd: &Path, roots: &[&Path]) -> ToolCtx {
    ToolCtx {
        cwd: cwd.to_path_buf(),
        cancel: CancellationToken::new(),
        skill_roots: Some(
            roots
                .iter()
                .map(|r| (r.to_path_buf(), SkillScope::User))
                .collect(),
        ),
    }
}

fn text_of(r: &ToolResult) -> String {
    match &r.content[0] {
        ContentBlock::Text { text } => text.clone(),
        ContentBlock::Image { .. } => "<image>".to_string(),
    }
}

/// The escape matrix: `path` values that must ALL be rejected, against a
/// skill dir holding symlinks that point out of it.
#[tokio::test]
async fn read_skill_rejects_every_path_that_leaves_the_skill_dir() {
    let cwd = Tmp::new("cwd");
    let root = Tmp::new("root");
    let outside = Tmp::new("outside");
    let skill = write_skill(root.path(), "alpha", "THE-BODY");

    // Files that must NEVER be reachable / whose bytes must never appear.
    fs::write(root.path().join("SECRET.txt"), "HONEY-ROOT").unwrap();
    let outside_file = outside.path().join("HONEY.txt");
    fs::write(&outside_file, "HONEY-OUTSIDE").unwrap();

    // Symlinks INSIDE the skill dir — each looks in-bounds by NAME.
    symlink(&outside_file, &skill.join("link-out"));
    symlink(&root.path().join("SECRET.txt"), &skill.join("link-sibling"));
    symlink(
        &outside.path().join("nope.txt"),
        &skill.join("link-dangling"),
    );
    fs::create_dir_all(skill.join("sub")).unwrap();
    symlink(outside.path(), &skill.join("dir-out"));

    let outside_str = outside_file.to_string_lossy().into_owned();
    let cases: Vec<(&str, String)> = vec![
        ("empty", String::new()),
        ("dot", ".".into()),
        ("slash", "/".into()),
        ("dotdot", "..".into()),
        ("dotdot-file", "../SECRET.txt".into()),
        ("deep-traversal", "././../../etc/passwd".into()),
        ("via-subdir", "sub/../../SECRET.txt".into()),
        ("absolute-outside", outside_str.clone()),
        ("absolute-passwd", "/etc/passwd".into()),
        ("symlink-to-file", "link-out".into()),
        ("symlink-to-sibling", "link-sibling".into()),
        ("symlink-dangling", "link-dangling".into()),
        ("symlink-through-dir", "dir-out/HONEY.txt".into()),
        ("nul-byte", "sub/../\u{0}SECRET".into()),
    ];

    for (label, path) in &cases {
        let r = execute_tool(
            &ctx(cwd.path(), &[root.path()]),
            "read_skill",
            &json!({ "name": "alpha", "path": path }),
        )
        .await;
        let text = text_of(&r);
        // 1. Rejected (a directory / a non-file is still a failure).
        assert!(r.is_error, "{label}: must be rejected, got {text}");
        // 2. No foreign content.
        for honey in ["HONEY-ROOT", "HONEY-OUTSIDE", "HONEY-PASSWD", "root:x:"] {
            assert!(!text.contains(honey), "{label}: leaked {honey:?} in {text}");
        }
        // 3. No path the model did not name (a resolved symlink target).
        //    Its own `path` argument is fair game; `atk-…outside` is only
        //    known to the model when it typed it.
        if !path.contains("skill-esc-outside") {
            assert!(
                !text.contains("skill-esc-outside"),
                "{label}: disclosed an unnamed path: {text}"
            );
        }
    }
}

/// The NAME is the allowlist: a name that is not exactly a discovered skill
/// resolves to nothing (no prefix match, no trimming, no NUL tricks).
#[tokio::test]
async fn read_skill_name_must_match_a_discovered_skill_exactly() {
    let cwd = Tmp::new("name-cwd");
    let r1 = Tmp::new("name-r1");
    let r2 = Tmp::new("name-r2");
    write_skill(r1.path(), "pub", "PUBLIC-BODY");
    write_skill(r2.path(), "secret-thing", "PRIVATE-BODY");

    // Only r1 is an allowlisted root.
    let c = ctx(cwd.path(), &[r1.path()]);
    for name in ["pu", "pub-x", " pub", "pub ", "a\u{0}lpha", "", "SECRET"] {
        let r = execute_tool(&c, "read_skill", &json!({ "name": name })).await;
        assert!(r.is_error, "{name:?}: must not resolve");
        assert!(
            !text_of(&r).contains("PRIVATE-BODY"),
            "{name:?}: reached a skill outside the allowlisted roots"
        );
    }
    // Case-insensitive EXACT match is the one intended leniency.
    let r = execute_tool(&c, "read_skill", &json!({ "name": "PUB" })).await;
    assert!(!r.is_error);
    assert_eq!(text_of(&r), "PUBLIC-BODY");
}

/// A skill reached THROUGH a symlinked root is still readable (canonicalized
/// `dir` — the boundary follows the real path, not the walked one).
#[tokio::test]
async fn a_skill_discovered_through_a_symlinked_root_is_still_readable() {
    let cwd = Tmp::new("symroot-cwd");
    let real = Tmp::new("symroot-real");
    let link_root = Tmp::new("symroot-link");
    write_skill(real.path(), "beta", "B-BODY");
    symlink(real.path(), &link_root.path().join("target"));

    let c = ctx(cwd.path(), &[&link_root.path().join("target")]);
    let list = execute_tool(&c, "list_skills", &json!({})).await;
    assert!(!list.is_error);
    assert!(
        text_of(&list).starts_with("beta (user)"),
        "{}",
        text_of(&list)
    );

    let r = execute_tool(&c, "read_skill", &json!({ "name": "beta" })).await;
    assert!(!r.is_error, "{}", text_of(&r));
    assert_eq!(text_of(&r), "B-BODY");
}

/// `list_skills` reflects ONLY the allowlisted roots (it cannot be steered
/// into printing another root's catalog).
#[tokio::test]
async fn list_skills_prints_only_the_allowlisted_roots() {
    let cwd = Tmp::new("list-cwd");
    let r1 = Tmp::new("list-r1");
    let r2 = Tmp::new("list-r2");
    write_skill(r1.path(), "shown", "S");
    write_skill(r2.path(), "hidden", "H");

    let only_r1 = execute_tool(&ctx(cwd.path(), &[r1.path()]), "list_skills", &json!({})).await;
    assert_eq!(text_of(&only_r1), "shown (user): ");
    assert!(!text_of(&only_r1).contains("hidden"));

    let both = execute_tool(
        &ctx(cwd.path(), &[r1.path(), r2.path()]),
        "list_skills",
        &json!({}),
    )
    .await;
    let text = text_of(&both);
    assert!(text.contains("shown") && text.contains("hidden"), "{text}");
}
