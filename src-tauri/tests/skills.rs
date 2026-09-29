//! Skill discovery (skills feature, Task 1): a pure, dependency-free
//! module that finds and parses skills on disk. Mirrors the roots pi
//! scans (so the left pane matches what pi advertises in the common
//! case) and ZCode's bounded-walk + best-effort frontmatter rules.

use std::fs;
use std::path::{Path, PathBuf};

use archimedes_lib::skills::{discover_in_roots, discover_skills, space_roots, SkillScope};

/// Write one file into `dir` (creating the file's parent dirs first).
fn write_file(dir: &Path, name: &str, contents: &str) {
    let path = dir.join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// A skill dir with a `SKILL.md` at `<root>/<name>`.
fn write_skill(root: &Path, name: &str, content: &str) {
    write_file(root, &format!("{name}/SKILL.md"), content);
}

/// The skill's row, or panic with context.
fn find_row<'a>(
    rows: &'a [archimedes_lib::skills::SkillInfo],
    name: &str,
) -> &'a archimedes_lib::skills::SkillInfo {
    rows.iter()
        .find(|s| s.name == name)
        .unwrap_or_else(|| panic!("skill {name:?} not found in {rows:?}"))
}

/// `name: alpha` + a multi-line `description: >` block (folded).
const ALPHA: &str =
    "---\nname: alpha\ndescription: >\n  a folded\n  description line\n---\nBody text here.\n";

#[test]
fn discover_in_roots_empty_and_missing_roots() {
    assert!(discover_in_roots(&[]).is_empty());
    let missing = PathBuf::from("/nonexistent/definitely/does/not/exist/skills");
    assert!(discover_in_roots(&[(missing, SkillScope::User)]).is_empty());
}

#[test]
fn discovers_a_skill_with_frontmatter() {
    let tmp = tempfile::tempdir().unwrap();
    write_skill(tmp.path(), "alpha", ALPHA);
    let root = tmp.path().to_path_buf();

    let rows = discover_in_roots(&[(root.clone(), SkillScope::User)]);
    assert_eq!(rows.len(), 1);
    let s = &rows[0];
    assert_eq!(s.name, "alpha");
    assert_eq!(s.description, "a folded description line");
    assert_eq!(s.scope, SkillScope::User);
    // BOTH `path` and `dir` are canonical (a `tempdir()` path can be a
    // symlink prefix on some platforms — canonicalize the EXPECTED too).
    assert_eq!(
        s.path,
        fs::canonicalize(root.join("alpha/SKILL.md")).unwrap()
    );
    assert_eq!(s.dir, fs::canonicalize(root.join("alpha")).unwrap());
    assert_eq!(s.body, "Body text here.");
}

#[test]
fn no_frontmatter_uses_the_directory_name() {
    let tmp = tempfile::tempdir().unwrap();
    write_skill(
        tmp.path(),
        "gamma",
        "Just a plain body, no frontmatter at all.\n",
    );
    let root = tmp.path().to_path_buf();

    let rows = discover_in_roots(&[(root.clone(), SkillScope::User)]);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].name, "gamma");
    assert_eq!(rows[0].description, "");
    assert_eq!(rows[0].body, "Just a plain body, no frontmatter at all.");
}

#[test]
fn quoted_and_block_values() {
    let tmp = tempfile::tempdir().unwrap();
    write_skill(
        tmp.path(),
        "beta",
        "---\nname: \"beta\"\ndescription: |\n  line one\n  line two\n---\nBeta body.\n",
    );
    write_skill(
        tmp.path(),
        "delta",
        "---\nname: delta\ndescription: >-\n  chomped\n  block\n---\nDelta body.\n",
    );
    let root = tmp.path().to_path_buf();

    let rows = discover_in_roots(&[(root.clone(), SkillScope::User)]);
    assert_eq!(rows.len(), 2);
    // Quoted `name` → the unquoted value.
    assert_eq!(find_row(&rows, "beta").name, "beta");
    // `|` joins with `\n`.
    assert_eq!(find_row(&rows, "beta").description, "line one\nline two");
    // `>-` (the most common real-world chomping form) keeps the `>`
    // fold semantics: joined with a single space, trimmed.
    assert_eq!(find_row(&rows, "delta").description, "chomped block");
}

#[test]
fn description_over_1024_chars_is_skipped() {
    let tmp = tempfile::tempdir().unwrap();
    let long = "x".repeat(1025);
    write_skill(
        tmp.path(),
        "toolong",
        &format!("---\nname: toolong\ndescription: {long}\n---\nBody.\n"),
    );
    let root = tmp.path().to_path_buf();

    let rows = discover_in_roots(&[(root, SkillScope::User)]);
    assert!(
        rows.iter().all(|s| s.name != "toolong"),
        "1025-char description must be skipped: {rows:?}"
    );
}

#[test]
fn unicode_whitespace_in_block_indent_does_not_panic() {
    // Regression: the block dedent measured the leading whitespace in
    // BYTES but `trim_start` trims UNICODE whitespace (e.g. U+00A0
    // NBSP) — slicing at that byte offset can land mid-character and
    // panic. The discovery must be TOTAL: this skill is parsed (or
    // skipped), never a panic.
    let tmp = tempfile::tempdir().unwrap();
    write_skill(
        tmp.path(),
        "nb",
        "---\nname: nb\ndescription: |\n \u{00A0}x\n  y\n---\nB.\n",
    );
    let root = tmp.path().to_path_buf();

    let rows = discover_in_roots(&[(root, SkillScope::User)]);
    // The skill is parsed with a description derived from the block
    // (the NBSP is content, not indent — only the ASCII space is).
    assert_eq!(rows.len(), 1, "skill must be listed: {rows:?}");
    assert_eq!(rows[0].name, "nb");
    assert_eq!(rows[0].description, "x\n y");
}

#[test]
fn empty_inline_name_falls_back_to_the_directory_name() {
    // Regression: `name: ""` parsed to `Some("")` and the fallback only
    // fired on `None` — an empty name broke the left-pane row.
    let tmp = tempfile::tempdir().unwrap();
    write_skill(
        tmp.path(),
        "gamma",
        "---\nname: \"\"\ndescription: d\n---\nB.\n",
    );
    let root = tmp.path().to_path_buf();

    let rows = discover_in_roots(&[(root, SkillScope::User)]);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].name, "gamma", "empty name must fall back: {rows:?}");
}

#[test]
fn a_name_containing_a_quote_is_skipped() {
    // An unquoted inline `name: a"b` keeps the quote (the quote-strip only
    // removes ONE pair of MATCHING surrounding quotes) — and the name is
    // interpolated UNESCAPED into the injected `<skill name="…">` attribute,
    // so a `"` there would produce a malformed tag the agent mis-parses
    // → the skill is skipped, never listed.
    let tmp = tempfile::tempdir().unwrap();
    write_skill(
        tmp.path(),
        "q",
        "---\nname: a\"b\ndescription: d\n---\nB.\n",
    );
    let root = tmp.path().to_path_buf();

    let rows = discover_in_roots(&[(root, SkillScope::User)]);
    // Each fixture holds exactly ONE skill → the skip contract is that
    // the result is EMPTY (a fallback to the directory name would keep
    // a row and fail this).
    assert!(
        rows.is_empty(),
        "a name containing a quote must be skipped: {rows:?}"
    );
}

#[test]
fn a_directory_name_containing_a_quote_is_skipped() {
    // A `SKILL.md` with NO frontmatter in a directory named `we"ird`
    // (a legal Unix directory name): the directory-name fallback carries
    // the quote → the skill is skipped, never listed.
    let tmp = tempfile::tempdir().unwrap();
    write_skill(
        tmp.path(),
        "we\"ird",
        "Just a plain body, no frontmatter at all.\n",
    );
    let root = tmp.path().to_path_buf();

    let rows = discover_in_roots(&[(root, SkillScope::User)]);
    assert!(
        rows.is_empty(),
        "a fallback name containing a quote must be skipped: {rows:?}"
    );
}

#[test]
fn a_path_containing_a_quote_is_skipped() {
    // A skill at `<root>/we"ird/SKILL.md` WITH a valid frontmatter name:
    // the name check passes, but the frontend ALSO interpolates `path`
    // UNESCAPED into the injected `<skill location="…">` attribute (and
    // `dir` into the `References are relative to DIR.` line) — the same
    // malformed-tag class the name check guards against → the skill is
    // skipped, never listed.
    let tmp = tempfile::tempdir().unwrap();
    write_skill(
        tmp.path(),
        "we\"ird",
        "---\nname: foo\ndescription: d\n---\nB.\n",
    );
    let root = tmp.path().to_path_buf();

    let rows = discover_in_roots(&[(root, SkillScope::User)]);
    assert!(
        rows.is_empty(),
        "a skill whose path contains a quote must be skipped: {rows:?}"
    );
}

#[test]
fn a_block_value_name_with_a_newline_is_skipped() {
    // A `|` (literal) block `name` joined with `\n`: the joined name
    // contains a control character → the skill is skipped, never listed.
    // (The `>` style would FOLD to spaces and stay clean — only `|`
    // preserves newlines.)
    let tmp = tempfile::tempdir().unwrap();
    write_skill(
        tmp.path(),
        "bl",
        "---\nname: |\n  line one\n  line two\n---\nB.\n",
    );
    let root = tmp.path().to_path_buf();

    let rows = discover_in_roots(&[(root, SkillScope::User)]);
    assert!(
        rows.is_empty(),
        "a block name containing a newline must be skipped: {rows:?}"
    );
}

#[test]
fn a_whitespace_only_name_falls_back_to_the_directory_name() {
    // `name: "   "` parses to `Some("   ")` (non-empty) — a whitespace-only
    // name would yield a blank dialog row and an inert `$   ` insert token;
    // it must fall back to the directory name, same as an empty one.
    let tmp = tempfile::tempdir().unwrap();
    write_skill(
        tmp.path(),
        "gamma",
        "---\nname: \"   \"\ndescription: d\n---\nB.\n",
    );
    let root = tmp.path().to_path_buf();

    let rows = discover_in_roots(&[(root, SkillScope::User)]);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(
        rows[0].name, "gamma",
        "whitespace-only name must fall back: {rows:?}"
    );
}

#[test]
fn description_1024_cjk_chars_is_still_listed() {
    // Regression: the 1024 limit was measured in BYTES, dropping a
    // CJK description (≈3 bytes/char) at ~341 CHARS. The spec is
    // chars — 1024 CJK chars (3072 bytes) must still be listed, while
    // the 1025-ASCII-char test above proves the char limit still drops
    // over-limit descriptions.
    let tmp = tempfile::tempdir().unwrap();
    let long = "你".repeat(1024);
    write_skill(
        tmp.path(),
        "cjk",
        &format!("---\nname: cjk\ndescription: {long}\n---\nBody.\n"),
    );
    let root = tmp.path().to_path_buf();

    let rows = discover_in_roots(&[(root, SkillScope::User)]);
    assert!(
        rows.iter().any(|s| s.name == "cjk"),
        "1024-CHAR (3072-byte) description must be listed: {rows:?}"
    );
    // Sanity: the description is stored in full (3072 bytes of content).
    assert_eq!(find_row(&rows, "cjk").description.len(), 3072);
}

#[test]
fn malformed_frontmatter_is_skipped() {
    let tmp = tempfile::tempdir().unwrap();
    // Opening `---` but EOF before a closing `---` → malformed → skipped.
    write_skill(
        tmp.path(),
        "broken",
        "---\nname: broken\ndescription: oops\n(no closing dash line)\n",
    );
    let root = tmp.path().to_path_buf();

    let rows = discover_in_roots(&[(root, SkillScope::User)]);
    assert!(
        rows.iter().all(|s| s.name != "broken"),
        "malformed frontmatter must be skipped: {rows:?}"
    );
}

#[test]
fn walk_skips_node_modules_and_bounded_depth() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // `node_modules` is skipped entirely.
    write_skill(
        &root.join("node_modules/x"),
        "nm",
        "---\nname: nm\n---\nB.\n",
    );
    // Depth 5 → absent.
    write_skill(
        &root.join("b1/b2/b3/b4/b5"),
        "SKILL.md",
        "---\nname: deep5\n---\nB.\n",
    );
    // Depth 4 → present.
    write_file(
        &root.join("a1/a2/a3/a4"),
        "SKILL.md",
        "---\nname: kept4\n---\nB.\n",
    );
    // A `SKILL.md` directly at the root (depth 0) → absent.
    fs::write(root.join("SKILL.md"), "---\nname: atroot\n---\nB.\n").unwrap();

    let rows = discover_in_roots(&[(root.to_path_buf(), SkillScope::User)]);
    let names: Vec<&str> = rows.iter().map(|s| s.name.as_str()).collect();
    assert!(
        !names.contains(&"nm"),
        "node_modules must be skipped: {names:?}"
    );
    assert!(
        !names.contains(&"deep5"),
        "depth 5 must be skipped: {names:?}"
    );
    assert!(
        names.contains(&"kept4"),
        "depth 4 must be present: {names:?}"
    );
    assert!(
        !names.contains(&"atroot"),
        "depth 0 must be skipped: {names:?}"
    );
}

#[cfg(unix)]
#[test]
fn symlink_dedupe() {
    // WITHIN ONE root: `b/SKILL.md` is a symlink to `a/SKILL.md`, and
    // NEITHER has frontmatter (the fallback names `a`/`b` differ — the
    // name-dedupe CANNOT hide a broken path-dedupe). The skill appears
    // ONCE, via the canonical-path `HashSet`.
    let tmp = tempfile::tempdir().unwrap();
    write_skill(tmp.path(), "a", "Body of a (via link).\n");
    // `b/SKILL.md` is a symlink to `a/SKILL.md` (b must not pre-exist).
    let a = tmp.path().join("a/SKILL.md");
    let b = tmp.path().join("b/SKILL.md");
    fs::create_dir_all(b.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&a, &b).unwrap();

    let rows = discover_in_roots(&[(tmp.path().to_path_buf(), SkillScope::User)]);
    assert_eq!(
        rows.len(),
        1,
        "symlinked SKILL.md must collapse to one row: {rows:?}"
    );
    assert_eq!(rows[0].name, "a");
    assert_eq!(rows[0].body, "Body of a (via link).");
}

#[test]
fn first_wins_dedupe_and_order() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    write_skill(
        a.path(),
        "shared",
        "---\nname: shared\ndescription: from A\n---\nA body.\n",
    );
    write_skill(
        b.path(),
        "shared",
        "---\nname: shared\ndescription: from B\n---\nB body.\n",
    );
    write_skill(a.path(), "zeta", "---\nname: zeta\n---\nZ.\n");
    write_skill(b.path(), "Beta", "---\nname: Beta\n---\nB.\n");

    // Root A first → A's `shared` shadows B's. Mixed names sort
    // case-insensitively (Beta < shared < zeta).
    let rows = discover_in_roots(&[
        (a.path().to_path_buf(), SkillScope::Space),
        (b.path().to_path_buf(), SkillScope::User),
    ]);
    assert_eq!(rows.len(), 3, "{rows:?}");
    assert_eq!(
        find_row(&rows, "shared").description,
        "from A",
        "first-wins dedupe: {rows:?}"
    );
    let order: Vec<&str> = rows.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(order, vec!["Beta", "shared", "zeta"]);
    assert_eq!(rows[0].scope, SkillScope::User); // `Beta` lives in root B
    assert_eq!(rows[2].scope, SkillScope::Space); // `zeta` lives in root A
}

#[test]
fn space_roots_walks_up_to_the_git_marker() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let space = repo.join("sub");
    // `repo` is the repo root (a `.git` entry); `repo/sub` is the space.
    write_file(&space, ".agents/skills/inner/SKILL.md", "inner\n");
    write_file(&repo, ".pi/skills/gamma/SKILL.md", "gamma\n");
    fs::create_dir_all(repo.join(".git")).unwrap();

    let roots = space_roots(&space);
    assert_eq!(roots.len(), 4, "{roots:?}");
    // Innermost (the space itself) first, both variants at each level.
    assert_eq!(roots[0].0, space.join(".agents/skills"));
    assert_eq!(roots[0].1, SkillScope::Space);
    assert_eq!(roots[1].0, space.join(".pi/skills"));
    assert_eq!(roots[2].0, repo.join(".agents/skills"));
    assert_eq!(roots[3].0, repo.join(".pi/skills"));
    assert!(roots.iter().all(|(_, scope)| *scope == SkillScope::Space));
    // The discovery actually walks them: the ancestor-level skill is found.
    let rows = discover_in_roots(&roots);
    assert!(rows.iter().any(|s| s.name == "gamma"), "{rows:?}");
    assert!(rows.iter().any(|s| s.name == "inner"), "{rows:?}");

    // No `.git` anywhere up the chain → only the space path itself is a level.
    let bare = tempfile::tempdir().unwrap();
    let bare_space = bare.path().join("sp");
    write_file(&bare_space, ".agents/skills/x/SKILL.md", "x\n");
    let bare_roots = space_roots(&bare_space);
    assert_eq!(bare_roots.len(), 2, "{bare_roots:?}");
    assert_eq!(bare_roots[0].0, bare_space.join(".agents/skills"));
    assert_eq!(bare_roots[1].0, bare_space.join(".pi/skills"));
}

#[test]
fn discover_skills_none_returns_only_user_scope_rows() {
    // `None` → user-level roots only. Deterministic for ANY real `HOME`
    // contents: every row must be `User`-scoped (no `Space` rows).
    let rows = discover_skills(None);
    assert!(rows.iter().all(|s| s.scope == SkillScope::User), "{rows:?}");
}

/// The `list_skills` Tauri command (Task 2): a THIN wrapper over
/// `discover_skills` — the command is `async` and takes NO `State` (a pure
/// fs read, no manager/db access), so the test is a plain direct call via
/// the `archimedes_lib` crate path (integration tests are a separate crate
/// — `crate::` would refer to the test crate, not the library).
#[tokio::test]
async fn list_skills_command_is_a_thin_wrapper() {
    // fixture: <dir>/.agents/skills/alpha/SKILL.md
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    write_skill(&dir.join(".agents/skills"), "alpha", ALPHA);

    // `Some(path)` → the Task 1 discovery for that Space: the skill is
    // present and `Space`-scoped (the space root shadows any same-named
    // user skill — it is walked first).
    let rows =
        archimedes_lib::commands::skills::list_skills(Some(dir.to_str().unwrap().to_string()))
            .await
            .unwrap();
    assert_eq!(
        find_row(&rows, "alpha").scope,
        SkillScope::Space,
        "the space's skill must be Space-scoped: {rows:?}"
    );

    // `None` → user-level skills only: Ok, and every row is `User`-scoped
    // (deterministic for ANY real `HOME` contents — no `Space` rows).
    let none_rows = archimedes_lib::commands::skills::list_skills(None)
        .await
        .unwrap();
    assert!(
        none_rows.iter().all(|s| s.scope == SkillScope::User),
        "None must return user-level skills only: {none_rows:?}"
    );
}
