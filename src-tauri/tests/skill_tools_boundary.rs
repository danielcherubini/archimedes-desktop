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
//! so no test here ever reads the real `~/.agents/skills` — EXCEPT the two
//! ADR 0030 cases at the bottom, which pin `$HOME` to a temp dir (the
//! `test_support` `ENV_LOCK` + `RestoreHome` pattern) because they are
//! precisely the tests about what the boundary derives FROM `$HOME`.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::json;
use tokio_util::sync::CancellationToken;

use archimedes_lib::agent::boundary;
use archimedes_lib::agent::policy::{AccessPolicy, FilePolicy};
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
        // HERMETIC: a single-root boundary (the `cwd`) and an empty deny-list
        // — never `boundary::read_roots` / `protected_dirs`, which read
        // `$HOME` (the same trap `skill_roots` exists to avoid).
        boundary: vec![cwd.to_path_buf()],
        file_policy: archimedes_lib::agent::policy::FilePolicy::default(),
        protected: Vec::new(),
        skill_roots: Some(
            roots
                .iter()
                .map(|r| (r.to_path_buf(), SkillScope::User))
                .collect(),
        ),
    }
}

/// The same ctx with an explicit READ boundary (the multi-root ADR 0030
/// cases). The policy pins `Sandboxed` for reads: these tests are about the
/// boundary the executor ENFORCES.
fn ctx_roots(cwd: &Path, roots: &[&Path], boundary: &[PathBuf]) -> ToolCtx {
    ToolCtx {
        boundary: boundary.to_vec(),
        file_policy: FilePolicy {
            reads: AccessPolicy::Sandboxed,
            writes: AccessPolicy::Sandboxed,
            shell: AccessPolicy::Allow,
        },
        ..ctx(cwd, roots)
    }
}

/// `HOME` restore guard (a copy of the pattern private to `loop.rs`'s test
/// module — the restore happens even when an assertion panics mid-test).
struct RestoreHome(Option<std::ffi::OsString>);
impl Drop for RestoreHome {
    fn drop(&mut self) {
        match self.0.take() {
            // SAFETY: the `ENV_LOCK` is still held at drop time (this guard
            // is declared after the `_lock` guard and outlives it in
            // reverse); no other thread mutates HOME concurrently.
            Some(v) => unsafe { std::env::set_var("HOME", v) },
            // SAFETY: as above.
            None => unsafe { std::env::remove_var("HOME") },
        }
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

// ── ADR 0030: the same attacks against a MULTI-ROOT boundary ───────────────

/// The ADR 0029 attack matrix, re-run where the session's READ boundary has
/// an EXTRA root (the real-world `~/.agents/skills` — outside the `cwd`, in
/// the boundary). Every case must still be rejected: adding a root widens
/// WHICH trees are in bounds, it does not weaken the canonicalize-then-check
/// logic for any of them. The contrast (a plain file inside the extra root IS
/// readable) keeps this from passing because the tool is simply broken.
#[tokio::test]
async fn the_attack_matrix_still_holds_with_an_extra_boundary_root() {
    let cwd = Tmp::new("mr-cwd");
    let extra = Tmp::new("mr-extra"); // the second boundary root
    let outside = Tmp::new("mr-outside");

    // In bounds: a plain file inside the extra root.
    fs::create_dir_all(extra.path().join("sub")).unwrap();
    fs::write(extra.path().join("IN-ROOT.txt"), "IN-BYTES").unwrap();
    fs::write(extra.path().join("sub/in-root.txt"), "IN-BYTES").unwrap();
    // Out of bounds, and the honey the attacks must never print.
    let outside_file = outside.path().join("HONEY.txt");
    fs::write(&outside_file, "HONEY-OUTSIDE").unwrap();
    fs::write(outside.path().join("sub"), "HONEY-SUB").unwrap();
    // Symlinks INSIDE the extra root — each looks in-bounds by NAME.
    symlink(&outside_file, &extra.path().join("link-out"));
    symlink(
        &outside.path().join("nope.txt"),
        &extra.path().join("link-dangling"),
    );
    symlink(outside.path(), &extra.path().join("dir-out"));
    // A sibling of the extra root: reachable by `..`, and NOT a root.
    let sibling = Tmp::new("mr-sibling");
    fs::write(sibling.path().join("SECRET.txt"), "HONEY-SIBLING").unwrap();

    // The boundary: the cwd FIRST, then the extra root.
    let b = vec![
        cwd.path().canonicalize().unwrap(),
        extra.path().canonicalize().unwrap(),
    ];
    let c = ctx_roots(cwd.path(), &[], &b);

    // The contrast: the extra root really is readable.
    let r = execute_tool(
        &c,
        "read",
        &json!({ "path": extra.path().join("IN-ROOT.txt") }),
    )
    .await;
    assert!(
        !r.is_error,
        "the extra root must be readable: {}",
        text_of(&r)
    );
    assert_eq!(text_of(&r), "IN-BYTES");

    let outside_str = outside_file.to_string_lossy().into_owned();
    let extra_str = extra.path().to_string_lossy().into_owned();
    let sibling_str = sibling.path().to_string_lossy().into_owned();
    let cases: Vec<(&str, String)> = vec![
        ("abs-outside", outside_str.clone()),
        ("abs-passwd", "/etc/passwd".into()),
        ("symlink-to-file", format!("{extra_str}/link-out")),
        ("symlink-dangling", format!("{extra_str}/link-dangling")),
        (
            "symlink-through-dir",
            format!("{extra_str}/dir-out/HONEY.txt"),
        ),
        (
            "dotdot-into-non-root",
            format!("{extra_str}/../{}/SECRET.txt", sibling_name(&sibling)),
        ),
        (
            "dotdot-via-subdir",
            format!(
                "{extra_str}/sub/../../{}/SECRET.txt",
                sibling_name(&sibling)
            ),
        ),
        ("abs-sibling", format!("{sibling_str}/SECRET.txt")),
        (
            "deep-traversal",
            format!("{extra_str}/sub/../../../etc/passwd"),
        ),
    ];

    for (label, path) in &cases {
        let r = execute_tool(&c, "read", &json!({ "path": path })).await;
        let text = text_of(&r);
        assert!(r.is_error, "{label}: must be rejected, got {text}");
        for honey in ["HONEY-OUTSIDE", "HONEY-SIBLING", "root:x:"] {
            assert!(!text.contains(honey), "{label}: leaked {honey:?} in {text}");
        }
    }

    // A write, though — the extra root is NOT a write root (the write
    // boundary is the `cwd` only), so a file inside the extra root is
    // refused even though it is readable.
    let w = execute_tool(
        &c,
        "write",
        &json!({ "path": extra.path().join("IN-ROOT.txt"), "content": "PWNED" }),
    )
    .await;
    assert!(w.is_error, "a read root must not be a write root");
    assert_eq!(
        fs::read_to_string(extra.path().join("IN-ROOT.txt")).unwrap(),
        "IN-BYTES",
        "the readable file must be untouched"
    );
}

fn sibling_name(t: &Tmp) -> String {
    t.path().file_name().unwrap().to_string_lossy().into_owned()
}

/// The seeded roots when `$HOME` is ITSELF a symlink (a real layout: cloud
/// dotfiles tools replace `~` with a link into a synced tree). The boundary
/// is canonicalized, so the derived roots are the REAL paths — and a write
/// addressed through the symlinked spelling is still caught by the deny-list
/// (both spellings canonicalize to the same dir).
/// `#[allow(clippy::await_holding_lock)]` is INTENTIONAL: the boundary reads
/// `HOME`, so the guard must stay held across the `.await`s.
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn a_symlinked_home_yields_canonical_roots_and_still_denies_writes() {
    let _lock = archimedes_lib::test_support::ENV_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let original_home = std::env::var_os("HOME");
    let _restore = RestoreHome(original_home);

    // The real home + a symlink that points at it; `HOME` = the symlink.
    let real = Tmp::new("symhome-real");
    let link_parent = Tmp::new("symhome-link");
    let home_link = link_parent.path().join("home");
    symlink(real.path(), &home_link);
    let skill_dir = real.path().join(".agents/skills/alpha");
    fs::create_dir_all(&skill_dir).unwrap();
    let skill_md = skill_dir.join("SKILL.md");
    fs::write(
        &skill_md,
        "---
name: alpha
---
THE-SKILL-BODY",
    )
    .unwrap();
    fs::write(real.path().join(".agents/skills/SECRET.txt"), "HONEY-HOME").unwrap();
    // SAFETY: the `ENV_LOCK` is held for the whole set→assert→restore span
    // (the `_lock` guard); no other thread mutates HOME concurrently.
    unsafe { std::env::set_var("HOME", &home_link) };

    let cwd = Tmp::new("symhome-cwd");
    let roots = boundary::read_roots(cwd.path());
    let real_canon = real.path().canonicalize().unwrap();
    // Premise: the symlinked home is genuinely NOT its canonical target.
    assert_ne!(home_link.canonicalize().unwrap(), home_link);
    // The derived roots are the CANONICAL (real) paths — never the symlinked
    // spelling, and never `$HOME` itself.
    let want = real.path().join(".agents/skills").canonicalize().unwrap();
    assert!(
        roots.contains(&want),
        "the symlinked home's skill ROOT must be a root: {roots:?}"
    );
    assert_eq!(want, skill_dir.canonicalize().unwrap().parent().unwrap());
    assert!(
        !roots.contains(&real_canon),
        "the boundary must never be $HOME itself: {roots:?}"
    );
    assert!(
        roots.iter().all(|r| !r.starts_with(&home_link)),
        "a root kept the symlinked spelling: {roots:?}"
    );

    // The deny-list is canonical too, and a write addressed through the
    // SYMLINKED spelling is still refused (the resolution happens first).
    let protected = boundary::protected_dirs();
    assert!(
        protected.contains(&real.path().join(".agents/skills").canonicalize().unwrap()),
        "the skill dir must be protected: {protected:?}"
    );
    let c = ToolCtx {
        boundary: roots,
        protected,
        file_policy: FilePolicy {
            reads: AccessPolicy::Allow,
            writes: AccessPolicy::Allow,
            shell: AccessPolicy::Allow,
        },
        skill_roots: None,
        cwd: cwd.path().to_path_buf(),
        cancel: CancellationToken::new(),
    };
    let via_link = home_link.join(".agents/skills/alpha/SKILL.md");
    let r = execute_tool(
        &c,
        "write",
        &json!({ "path": via_link, "content": "PWNED" }),
    )
    .await;
    assert!(
        r.is_error,
        "a protected write via the symlink must be refused"
    );
    assert!(
        text_of(&r).contains("protected"),
        "the refusal must name the reason: {}",
        text_of(&r)
    );
    assert_eq!(
        fs::read_to_string(&skill_md).unwrap(),
        "---
name: alpha
---
THE-SKILL-BODY",
        "the skill file must be untouched"
    );
    // A NEW file under the protected dir is refused through the link too.
    let r = execute_tool(
        &c,
        "write",
        &json!({ "path": home_link.join(".agents/skills/evil/SKILL.md"), "content": "x" }),
    )
    .await;
    assert!(
        r.is_error,
        "a new protected file via the symlink is refused"
    );
    assert!(
        !real.path().join(".agents/skills/evil").exists(),
        "nothing is created under the real home"
    );

    // And the READS the boundary grants: the skill body is reachable, its
    // sibling `SECRET.txt` is not (it is not under the skill dir, and the
    // `read_skill` mini-sandbox is still exactly one skill directory).
    let listed = execute_tool(&c, "list_skills", &json!({})).await;
    assert!(
        text_of(&listed).starts_with("alpha (user)"),
        "{}",
        text_of(&listed)
    );
    let r = execute_tool(&c, "read_skill", &json!({ "name": "alpha" })).await;
    assert!(!r.is_error, "{}", text_of(&r));
    assert_eq!(text_of(&r), "THE-SKILL-BODY");
    let r = execute_tool(
        &c,
        "read_skill",
        &json!({ "name": "alpha", "path": "../SECRET.txt" }),
    )
    .await;
    assert!(r.is_error, "the mini-sandbox is still ONE skill dir");
    assert!(!text_of(&r).contains("HONEY-HOME"), "{}", text_of(&r));
}
