---
status: committed
done-when: In a Space with skills (or with user-level skills): the left pane lists them (name + description, re-discovered on active-Space change); typing $ in the composer opens a filterable picker and selection inserts $name; sending a message containing $name delivers the skill's full content to the agent in the standard <skill> block format in BOTH native and external sessions (visible in the recorded transcript); an unmatched $token passes through verbatim; a skill with a >1024-char description is not listed; a malformed SKILL.md is skipped without a crash.
---

# Skills (v1) Plan

**Goal:** The Client browses and invokes Skills (a directory with a `SKILL.md`) — desktop-side discovery, a left-pane Skills section, and `$name` composer mentions expanded into the skill's full content on send, identically for native and external sessions.

**Architecture:** A new Rust module (`src-tauri/src/skills.rs`) discovers skills from the standard roots (Space's `.agents/skills` + `.pi/skills` walked up to the repo root; user-level `~/.agents/skills` + `~/.pi/agent/skills`), parses `SKILL.md` frontmatter (name/description/body) best-effort, and is exposed over one Tauri command (`list_skills`). The frontend fetches the catalog for the active Space into a small hook, renders a Skills section in the left pane, and — in the composer — a `$` trigger opens a filterable picker whose selection inserts a `$name ` token; `send()` expands the tokens into pi-format `<skill>` blocks BEFORE `addUserMessage`/`sendPrompt`, so the live bubble, the persisted record, and the agent's input are all identical.

**Tech Stack:** Rust (Tauri 2 commands, `std::fs`, `tempfile` dev-dep — NO new dependencies), React 19 + TypeScript (plain `<textarea>` composer, Zustand-adjacent hook, vitest + @testing-library/react).

**REFINEMENT vs the spec (read first):** spec Section 4 placed the expansion in the Rust `send_prompt` handler (before record/dispatch). The plan moves the expansion to the FRONTEND `send()` (before `addUserMessage` + `sendPrompt`) for one integration reason: the store's resume-merge dedupe key for user messages is CONTENT-BASED (`user|${text}|${images}` — `mergeDedupeKey` in `src/store/sessions.ts`). If the live bubble held the raw text and the persisted row held the expanded text, a send during a resume reload would render the message TWICE (raw + expanded), and the bubble's content would change across an app restart. Frontend-side expansion makes live bubble = persisted record = agent input (the spec's core invariant, preserved verbatim); Rust stays a dumb recorder (its `send_prompt`/`send_prompt_with_images` are UNCHANGED). Two follow-on consequences: (1) `SkillInfo` gains a `body` field (the skill's content minus frontmatter) so the frontend can build the injected block — Rust already reads + parses every `SKILL.md` for discovery, so this is free; (2) "re-discovered at every `send_prompt`" becomes "catalog fetched on active-Space change + app start, cached per Space" (a skill created on disk mid-session is picked up on the next Space switch — an accepted v1 trade for keeping the send path free of an extra round-trip). Everything else in the spec stands.

**Plan-level decisions beyond the spec (reviewer-confirmed, keep consistent across tasks):**
- **Case policy (one rule, three places):** mention TOKENS are lowercase-only (ZCode's exact regex; an uppercase char after `$` is not a token and closes the picker). Matching lowercases the SKILL NAME (`skill.name.toLowerCase()` vs the captured token) — a skill named `DeBuG` is invoked by `$debug`. Inserted tokens (picker + left-pane insert event) are therefore `$<name.toLowerCase()> ` so a picker-selected skill ALWAYS expands on send; the injected `<skill name="…">` block keeps the frontmatter name verbatim.
- **No YAML crate:** the frontmatter reader is hand-rolled and tolerant (the spec's "loose fallback" becomes the only parser — same outcome: an unparseable skill is skipped, never a crash).
- **Accepted edge:** a send before the catalog fetch resolves (app start / Space switch) sends any mention UNEXPANDED (the picker is not open either) — the fetch is a local disk walk and resolves quickly; the edge is accepted, not guarded.

---

### Task 1: Rust skill discovery module

**Context:** This task is the foundation: a pure, dependency-free Rust module that finds and parses skills on disk. It exists because the desktop (Client) is the owner of the skill layer (ADR 0013) — the left pane and the mention expansion both consume its output. It mirrors the roots pi scans (so the left pane matches what pi advertises in the common case) and ZCode's bounded-walk + best-effrontmatter robustness rules. NO new crate dependencies: frontmatter is parsed by a hand-rolled tolerant reader (the spec's "loose fallback" becomes the only parser — the outcome is identical: a skill that cannot be parsed is skipped, never a crash).

**Files:**
- Create: `src-tauri/src/skills.rs`
- Modify: `src-tauri/src/lib.rs` (add `pub mod skills;` to the module list at the top — required so the integration test in `src-tauri/tests/` can see the module)
- Test: `src-tauri/tests/skills.rs`

**What to implement:**

In `src-tauri/src/skills.rs`:

```rust
use std::path::{Path, PathBuf};
use serde::Serialize;

/// One discovered skill (camelCase over IPC; all fields are lowercase single words).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillInfo {
    pub name: String,        // frontmatter `name`, else the skill's directory name
    pub description: String, // frontmatter `description`, else ""
    pub path: String,        // absolute path to the SKILL.md file
    pub dir: String,         // absolute path to the skill's directory (parent of SKILL.md)
    pub scope: SkillScope,   // "space" | "user"
    pub body: String,        // SKILL.md content MINUS the frontmatter, trimmed (the injected block's BODY)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SkillScope {
    Space,
    User,
}

/// Discover skills for `space_path` (its roots + the user-level roots).
/// `space_path: None` → user-level skills only.
/// Pure (fs reads only) and total: never panics, never errors — a missing
/// root / unreadable file / unparseable SKILL.md is silently skipped.
pub fn discover_skills(space_path: Option<&Path>) -> Vec<SkillInfo>;

/// The core, testable without env manipulation: discover over EXPLICIT roots.
/// `roots` order defines first-wins dedupe precedence.
pub fn discover_in_roots(roots: &[(PathBuf, SkillScope)]) -> Vec<SkillInfo>;
```

Plus private helpers (all in the same file — EXCEPT `space_roots` and `user_roots`, which are `pub`: the integration test in `src-tauri/tests/skills.rs` calls them directly, and integration tests are a separate crate that cannot see `pub(crate)`/private items):

- `pub fn space_roots(space_path: &Path) -> Vec<(PathBuf, SkillScope)>` — walk UP from `space_path` to the repository root: the first ancestor (including `space_path` itself) that contains a `.git` entry is the repo root; if none exists, `space_path` alone is the only level. At each level (innermost first) collect `<level>/.agents/skills` (first) and `<level>/.pi/skills` (second), both tagged `SkillScope::Space`. Do NOT canonicalize the input before walking (the caller passes a canonical path; if `space_path` doesn't exist, return an empty vec).
- `pub fn user_roots() -> Vec<(PathBuf, SkillScope)>` — `home_dir().join(".agents/skills")` (first) and `home_dir().join(".pi/agent/skills")` (second), both tagged `SkillScope::User`. `home_dir()`: `std::env::var_os("HOME")` → else `USERPROFILE` (Windows) → else `std::env::home_dir()`; if none, an empty vec.
- `fn find_skill_files(root: &Path, seen: &mut HashSet<PathBuf>) -> Vec<PathBuf>` — recursive walk for files named exactly `SKILL.md`: ONLY depths 1..=4 below `root` (a `SKILL.md` directly at the root — depth 0 — is NOT a skill; depth 1 = `<root>/<name>/SKILL.md`); skip any directory named `node_modules`; skip files whose canonical (symlink-resolved via `std::fs::canonicalize`, errors → skip) path was already seen — `seen` is a `HashSet<PathBuf>` SHARED across all roots (threaded through `discover_in_roots`), so a symlink in a later root pointing at an earlier root's `SKILL.md` collapses by PATH, not just by name; return in deterministic (lexicographic) order.
- `fn parse_skill_file(content: &str, fallback_name: &str) -> Option<(name, description, body)>` — the tolerant reader:
  1. Normalize `\r\n` and lone `\r` to `\n`.
  2. Frontmatter = a block starting at the very beginning: line 1 is exactly `---`, then lines until a line that is exactly `---`. The closing `---` is REQUIRED — EOF before it means the frontmatter is malformed → `None` (step 7). If line 1 is not `---`: no frontmatter → `name = fallback_name`, `description = ""`, `body = the whole content (trimmed)`.
  3. Inside the frontmatter, read TOP-LEVEL (non-indented) `name:` and `description:` keys only (indented lines belong to the previous key's block value; any other top-level key is ignored). Value forms:
     - inline: the rest of the line, trimmed; strip one pair of matching surrounding quotes (`"…"` or `'…'`); an empty inline value means "no value".
     - block: the inline value is exactly `|` or `>` (optionally followed by ONE chomping/indent indicator — `-` or `+`, e.g. `>-`, `|-`, `|+` — a common real-world form) with any further trailing whitespace — the value is the following indented (space/tab-prefixed) lines until the next non-indented line or the closing `---`; `|` joins with `\n`, `>` joins with a single space; trim the result.
     - an unquoted inline value containing `: ` (a YAML mapping ambiguity) → treat as no value (tolerant, no crash).
  4. `name` empty after parsing → `fallback_name`. `description` left as parsed (may be `""`).
  5. `description` longer than 1024 chars → return `None` (the skill is skipped — ZCode's `skill_description_too_long` rule).
  6. `body` = the content after the closing `---` (trimmed); with no frontmatter, the whole content (trimmed).
  7. ANY failure mode (unreadable file, missing closing `---`, etc.) → `None`.
- `discover_in_roots`: for each root in order (skip non-existent roots): `find_skill_files` (sharing the one `HashSet`); for each file: read (error → skip), `parse_skill_file` (None → skip); dedupe by `name.to_lowercase()` FIRST-WINS across the whole run (a Space root shadows a same-named user skill; an innermost ancestor level shadows an outer one); sort the result case-insensitively by `name` (tie-break: `path`); fill `dir = std::fs::canonicalize(parent of SKILL.md)` and `path = the canonical file path` (BOTH canonical — the tests must canonicalize their expected values too, since `tempfile::tempdir()` paths can be symlink prefixes on some platforms).
- `discover_skills`: `discover_in_roots(&[space_roots(p) if Some] ++ user_roots())` — i.e. build `Vec<(PathBuf, SkillScope)>` = (when `Some`) `space_roots(p)`, then `user_roots()`, and delegate.

In `src-tauri/src/lib.rs`: add `pub mod skills;` after `pub mod config;` (keep the existing list order: `agent`, `commands`, `config`, `skills`, `storage`, `test_support`).

In `src-tauri/tests/skills.rs` (integration tests, mirroring the fixture style of `src-tauri/tests/catalog.rs` — `tempfile::tempdir()` + `std::fs::write`). Imports (integration tests are a SEPARATE crate — `crate::` refers to the test crate, NOT the library; the library is named `archimedes_lib` per `Cargo.toml`'s `[lib]`):

```rust
use archimedes_lib::skills::{discover_in_roots, discover_skills, SkillScope};
```

- `discover_in_roots_empty_and_missing_roots` — no roots / non-existent roots → an empty `Vec` (no panic; the function returns `Vec<SkillInfo>` directly — no `Result`).
- `discovers_a_skill_with_frontmatter` — `<root>/alpha/SKILL.md` with `name: alpha` + a multi-line `description: >` block → listed with the parsed name/description, `dir` = `std::fs::canonicalize(<the skill dir>)` (canonicalize the EXPECTED value too — a `tempdir()` path can be a symlink prefix on some platforms), `body` = content minus frontmatter.
- `no_frontmatter_uses_the_directory_name` — a `SKILL.md` with no `---` block → `name` = the directory name, `description` = `""`.
- `quoted_and_block_values` — `name: "beta"` (quoted) + `description: |` block → unquoted name, `\n`-joined description. Add a case for the chomping form `description: >-` (the most common real-world form) → same `>` join semantics (joined with a single space, trimmed).
- `description_over_1024_chars_is_skipped` — a 1025-char description → the skill is absent.
- `malformed_frontmatter_is_skipped` — a frontmatter with no closing `---` (EOF inside the block) → the skill is absent, no panic.
- `walk_skips_node_modules_and_bounded_depth` — a `SKILL.md` inside `<root>/node_modules/x/` → absent; a `SKILL.md` at depth 5 → absent; at depth 4 → present; a `SKILL.md` directly at the root (depth 0) → absent.
- `symlink_dedupe` — WITHIN ONE root: `<root>/a/SKILL.md` and `<root>/b/SKILL.md` where `b/SKILL.md` is a symlink to `a/SKILL.md`, and NEITHER has frontmatter (so the fallback names `a`/`b` differ — the name-dedupe CANNOT hide a broken path-dedupe) → the skill appears ONCE (the first root order's entry). This is the only design that actually exercises the canonical-path `HashSet` (a cross-root symlink with identical frontmatter would be collapsed by the name-dedupe even without the path-dedupe — a vacuous test). (Gate the test `#[cfg(unix)]` — symlink fixtures are unix-only, the project ships cross-platform.)
- `first_wins_dedupe_and_order` — the same name in root A then root B → root A's entry wins; mixed names → case-insensitive sort.
- `space_roots_walks_up_to_the_git_marker` — a temp repo: `repo/.git/` (an empty file or dir), `repo/sub/` as the space path, `repo/.pi/skills/gamma/SKILL.md` → `space_roots` includes `repo/.pi/skills` (the ancestor level) and `repo/sub/.agents/skills` (the space level, innermost first); and a temp dir WITHOUT `.git` anywhere up the chain → only the space path itself is a level. (The test calls `space_roots` directly — hence `pub` in the module.)
- `discover_skills_none_returns_only_user_scope_rows` — `discover_skills(None)` must return ONLY rows with `scope == SkillScope::User` (no `Space` rows) — deterministic for ANY real `HOME` contents (no env manipulation needed; the user roots from the real `HOME` may contribute skills, and every one of them is `User`-scoped).

**Steps:**
- [ ] Write the failing tests in `src-tauri/tests/skills.rs` (all the cases above; they fail to compile because `archimedes_lib::skills` doesn't exist yet — that is the "failing" state for this task)
- [ ] Run `cargo test --test skills` (from `src-tauri/`)
  - Did it fail (compile error: `skills` module not found)? If it passed unexpectedly, stop and investigate why.
- [ ] Implement `src-tauri/src/skills.rs` (`SkillInfo`, `SkillScope`, `discover_skills`, `discover_in_roots`, the private helpers) and add `pub mod skills;` to `src-tauri/src/lib.rs`
- [ ] Run `cargo test --test skills` (from `src-tauri/`)
  - Did all tests pass? If not, fix the failures and re-run before continuing.
- [ ] Run `cargo test` (from `src-tauri/` — the full suite, to catch regressions)
- [ ] Run `cargo fmt` then `cargo fmt --check` (from `src-tauri/`)
  - Did it succeed? If not, fix and re-run before continuing.
- [ ] Run `cargo clippy --all-targets` (from `src-tauri/`)
  - Did it succeed with 0 warnings? If not, fix and re-run before continuing.
- [ ] Commit with message: "feat(skills): Rust skill discovery (roots, bounded walk, tolerant frontmatter)"

**Acceptance criteria:**
- [ ] `cargo test` passes (full suite, from `src-tauri/`), including the new `tests/skills.rs` cases
- [ ] `cargo clippy --all-targets` reports 0 warnings; `cargo fmt --check` passes
- [ ] `discover_skills(None)` returns user-level skills only (every row `scope == User`); `discover_skills(Some(p))` returns Space roots (innermost first) + user roots, deduped first-wins, sorted case-insensitively by name
- [ ] A `SKILL.md` with a >1024-char description, malformed frontmatter, or an unreadable file is skipped without a panic or an error return

---

### Task 2: `list_skills` Tauri command + registration

**Context:** Task 1's module is a pure function; this task exposes it over IPC so the frontend (Tasks 3-5) can fetch the catalog. The command is deliberately a THIN wrapper — all the logic (and all the tests) live in Task 1's module. It takes `Option<String>` so the frontend can request "user-level skills only" (no active Space); it takes NO `State` parameter (pure fs read, no manager/db access).

**Files:**
- Create: `src-tauri/src/commands/skills.rs`
- Modify: `src-tauri/src/commands/mod.rs` (add `pub mod skills;`)
- Modify: `src-tauri/src/lib.rs` (add `commands::skills::list_skills,` to the `generate_handler!` list — insert after `commands::spaces::set_space_trusted,` to keep the spaces-related commands grouped)
- Test: `src-tauri/tests/skills.rs` (add the command test)

**What to implement:**

`src-tauri/src/commands/skills.rs` (module doc comment in the style of `commands/spaces.rs`):

```rust
//! The skill catalog over IPC (read-only — the desktop is a READER of skill
//! directories, never a writer; ADR 0013).
use std::path::Path;

use crate::skills::SkillInfo;

/// The skill catalog for a Space (its roots + the user-level roots).
/// `space_path: None` → user-level skills only. Never fails: discovery is
/// total (a missing root / unreadable file is skipped, Task 1) — the `Err`
/// arm is for the unexpected.
#[tauri::command]
pub async fn list_skills(space_path: Option<String>) -> Result<Vec<SkillInfo>, String> {
    Ok(crate::skills::discover_skills(
        space_path.as_deref().map(Path::new),
    ))
}
```

`src-tauri/src/commands/mod.rs`: add `pub mod skills;` BETWEEN `pub mod settings;` and `pub mod spaces;` (keep the file's alphabetical module list).

`src-tauri/src/lib.rs`: add `commands::skills::list_skills,` to the `generate_handler!` macro list (after `commands::spaces::set_space_trusted,`).

`src-tauri/tests/skills.rs` (append):
- `list_skills_command_is_a_thin_wrapper` — a temp space fixture (one skill in `.agents/skills/`); the command is `async` and takes NO `State` (pure fs read), so the test is a plain direct call with the `archimedes_lib` crate path (integration tests are a separate crate — `crate::` would refer to the test crate):
  ```rust
  #[tokio::test]
  async fn list_skills_command_is_a_thin_wrapper() {
      // fixture: <dir>/.agents/skills/alpha/SKILL.md
      let rows = archimedes_lib::commands::skills
          ::list_skills(Some(dir.path().to_str().unwrap().to_string()))
          .await
          .unwrap();
      // assert: contains the skill with scope "space"
      let none_rows = archimedes_lib::commands::skills::list_skills(None).await.unwrap();
      // assert: Ok, and every row has scope "user"
  }
  ```
  (Note the parameter is `Option<String>` — pass `Some(<path as String>)`, not a `PathBuf`; and `await` the call — an un-awaited `async` fn would assert on a `Future`.)

**Steps:**
- [ ] Write the failing test `list_skills_command_is_a_thin_wrapper` in `src-tauri/tests/skills.rs`
- [ ] Run `cargo test --test skills` (from `src-tauri/`)
  - Did it fail (compile error: `commands::skills` not found)? If it passed unexpectedly, stop and investigate why.
- [ ] Implement `src-tauri/src/commands/skills.rs`, the `commands/mod.rs` entry, and the `lib.rs` `generate_handler!` entry
- [ ] Run `cargo test --test skills` (from `src-tauri/`)
  - Did all tests pass? If not, fix the failures and re-run before continuing.
- [ ] Run `cargo test` (from `src-tauri/` — full suite)
- [ ] Run `cargo fmt` then `cargo fmt --check` (from `src-tauri/`)
- [ ] Run `cargo clippy --all-targets` (from `src-tauri/`)
  - Did it succeed with 0 warnings? If not, fix and re-run before continuing.
- [ ] Commit with message: "feat(skills): list_skills Tauri command"

**Acceptance criteria:**
- [ ] `list_skills(Some(path))` returns the Task 1 discovery for that Space; `list_skills(None)` returns user-level skills only
- [ ] The command is registered in `generate_handler!` (the app compiles and the existing `tests/ipc.rs` mock-runtime test still passes)
- [ ] `cargo test` + `cargo clippy --all-targets` (0 warnings) + `cargo fmt --check` all pass from `src-tauri/`

---

### Task 3: Frontend skill client + `expandSkillMentions` (pure lib)

**Context:** This task is the frontend's data + logic layer: the IPC wrapper for the Task 2 command, and the pure expansion function that turns `$name` mentions into pi-format `<skill>` blocks. The expansion lives HERE (not in Rust) per the plan's REFINEMENT note: the store's resume-merge dedupe key for user messages is content-based (`mergeDedupeKey` in `src/store/sessions.ts`), so the live bubble, the persisted record, and the agent's input must all carry the SAME text — the only way that holds is for the text to be expanded once, on the frontend, before both `addUserMessage` and `sendPrompt`. The injected block uses the pi skill-expansion format (the exact literal in the function doc below) so the agent sees the same shape whether it loaded the skill itself or the desktop injected it.

**Files:**
- Modify: `src/lib/tauri.ts` (add the `SkillInfo` interface + `listSkills` wrapper)
- Create: `src/lib/skills.ts`
- Test: `src/lib/skills.test.ts`

**What to implement:**

`src/lib/tauri.ts` (append, following the file's existing conventions — `invoke<T>(...)` wrappers with doc comments; the Rust arg is `space_path`, so the frontend passes `spacePath` — Tauri maps camelCase→snake_case automatically, the same as the existing `sendPrompt` → `session_id`):

```ts
/** One discovered skill (camelCase over IPC — the Rust `SkillInfo`). */
export interface SkillInfo {
  name: string;
  description: string;
  path: string;
  dir: string;
  scope: "space" | "user";
  /** The SKILL.md content minus the frontmatter (the injected block's BODY). */
  body: string;
}

/** The skill catalog for a Space (`null` = user-level skills only). */
export async function listSkills(spacePath: string | null): Promise<SkillInfo[]> {
  return invoke<SkillInfo[]>("list_skills", { spacePath: spacePath ?? null });
}
```

`src/lib/skills.ts`:

```ts
import type { SkillInfo } from "./tauri";

/**
 * The mention regex (ZCode's exact form): a `$` followed by a lowercase
 * [a-z0-9] run, hyphen-separated. Uppercase or other chars after `$` are
 * NOT a skill token (the match must reach the end of the scanned span).
 * Implementation note: use `text.match(MENTION_RE)` (with the `/g` flag it
 * returns ALL matches and resets `lastIndex`); do NOT loop `exec`/`test`
 * against the shared module-level regex (a stale `lastIndex` across calls
 * can drop mentions on repeat sends).
 */
const MENTION_RE = /\$([a-z0-9]+(?:-[a-z0-9]+)*)/g;

/**
 * Expand `$name` mentions in `text` into pi-format `<skill>` blocks.
 *
 * - Each captured name (lowercase, by the regex) is matched against
 *   `skill.name.toLowerCase()` — i.e. matching is CASE-INSENSITIVE on the
 *   SKILL-NAME side: a skill named `DeBuG` is invoked by `$debug`. (Tokens
 *   are lowercase-only: `$DEBUG` is NOT a mention — the regex cannot match
 *   it, and it passes through verbatim.)
 * - A token matching no skill is left VERBATIM (no error, no stripping).
 * - One block per matched skill, even if its token appears multiple times
 *   (deduped).
 * - Blocks are appended AFTER the user's text, separated by a blank line,
 *   in the order the skills are FIRST mentioned.
 * - The block format is the pi skill-expansion format (the exact literal
 *   below — the shape pi's native `/skill:name` expansion produces, so the
 *   agent sees the same shape whether it loaded the skill itself or the
 *   desktop injected it):
 *
 *     <skill name="NAME" location="PATH">
 *     References are relative to DIR.
 *
 *     BODY
 *     </skill>
 *
 *   where `NAME` = the skill's frontmatter name VERBATIM (not lowercased),
 *   `PATH` = `skill.path`, `DIR` = `skill.dir`, `BODY` = `skill.body`.
 *   (pi's format also supports `args` after `/skill:name` — the mention
 *   model has no args: the token is a pure trigger, the rest of the
 *   message is the user's request.)
 * - No matched token → return `text` UNCHANGED (the function is pure — no
 *   mutation of inputs).
 */
export function expandSkillMentions(text: string, skills: SkillInfo[]): string;
```

`src/lib/skills.test.ts` (vitest, following the existing `src/lib/*.test.ts` style):
- `no_tokens_returns_the_text_unchanged` — `"hello world"` with skills → identical string.
- `expands_a_single_mention_in_pi_format` — text `"fix $debug"` + a skill `{name: "debug", path: "/s/.agents/skills/debug/SKILL.md", dir: "/s/.agents/skills/debug", body: "Step 1. Step 2."}` → `fix $debug\n\n<skill name="debug" location="/s/.agents/skills/debug/SKILL.md">\nReferences are relative to /s/.agents/skills/debug.\n\nStep 1. Step 2.\n</skill>` (assert the EXACT string, including the blank line between the note and the body and the `</skill>` on its own line).
- `skill_names_match_case_insensitively` — a catalog holding a skill named `DeBuG` AND a skill named `DEBUG` (a SYNTHETIC array — a real discovered catalog can never hold both, since `discover_in_roots` dedupes by lowercased name): the text `"x $debug"` → exactly TWO blocks (one per matched skill), `toContain('<skill name="DeBuG"')` and `toContain('<skill name="DEBUG"')` — the block's `name` attribute stays the frontmatter name verbatim while the MATCHING was case-insensitive.
- `uppercase_tokens_are_not_mentions` — `"$DEBUG"` and `"$DeBuG"` (uppercase in the TOKEN) pass through UNCHANGED even when a `debug` skill exists (the regex is lowercase-only — ZCode parity).
- `unmatched_token_passes_through` — `$nope` with no such skill → the text is unchanged.
- `one_block_per_skill_even_when_mentioned_twice` — `$debug` twice → exactly ONE block.
- `multiple_skills_in_first_mention_order` — `$a` then `$b` then `$a` → blocks in order `a`, `b` (two blocks total).
- `hyphenated_names_match` — `$my-skill-x` matches a skill named `my-skill-x`.
- `tokens_embedded_in_words_still_expand_zcode_parity` — `a$debug` (no word boundary before the `$`) DOES expand (document the ZCode-parity choice: the regex is boundary-free on the left, so `$` mid-word is a mention — the name says so, and the assertion matches it).
- `empty_skills_list_returns_text_unchanged`.

Accepted edge (no test): a send before the catalog fetch resolves (app start / Space switch) sends any mention UNEXPANDED (the picker is not open either) — the fetch is a local disk walk and resolves quickly; the edge is accepted, not guarded.

**Steps:**
- [ ] Write the failing tests in `src/lib/skills.test.ts`
- [ ] Run `pnpm test -- src/lib/skills.test.ts` (from the repo root)
  - Did it fail (module not found / function not implemented)? If it passed unexpectedly, stop and investigate why.
- [ ] Implement `expandSkillMentions` in `src/lib/skills.ts` and the `SkillInfo` interface + `listSkills` wrapper in `src/lib/tauri.ts`
- [ ] Run `pnpm test -- src/lib/skills.test.ts` (from the repo root)
  - Did all tests pass? If not, fix the failures and re-run before continuing.
- [ ] Run `pnpm test` (from the repo root — the full frontend suite)
- [ ] Run `pnpm build` (from the repo root — type-check + build)
  - Did it succeed? If not, fix and re-run before continuing.
- [ ] Commit with message: "feat(skills): frontend skill client + mention expansion"

**Acceptance criteria:**
- [ ] `expandSkillMentions` passes all `src/lib/skills.test.ts` cases (exact pi-format block, case-insensitive match, dedupe, pass-through)
- [ ] `listSkills` is typed and registered in `src/lib/tauri.ts` following the file's existing wrapper conventions
- [ ] `pnpm test` + `pnpm build` both pass from the repo root

---

### Task 4: `useSkillCatalog` hook + left-pane Skills section

**Context:** This task wires the catalog into the UI: a small hook that fetches `listSkills` for the active Space (cached per Space so the left pane and the composer share one fetch), and the left-pane Skills section itself (the ZCode placement: a top-level item above the Sessions list). Clicking a skill row inserts `$name ` into the composer — the composer lives in a different subtree (`ChatStream`), so the insertion is a decoupled `window` CustomEvent that Task 5's composer listens for.

**Files:**
- Create: `src/hooks/useSkillCatalog.ts`
- Create: `src/components/SkillsSection.tsx`
- Modify: `src/components/SpacesList.tsx` (render the section between the button row and the "Sessions" label)
- Test: `src/components/SkillsSection.test.tsx`
- Test: `src/components/SpacesList.test.tsx` (extend the existing tauri mock)

**What to implement:**

`src/hooks/useSkillCatalog.ts`:

```ts
import { useEffect, useState } from "react";
import { listSkills, type SkillInfo } from "../lib/tauri";

/**
 * The skill catalog for a Space (or `null` = user-level skills only).
 *
 * Cached in a MODULE-LEVEL `Map<string, SkillInfo[]>` keyed by
 * `spacePath ?? "__global__"` so the left pane AND the composer (two
 * components, same key) share ONE fetch. Re-fetched when `spacePath`
 * changes (the active Space changed) or on first mount (app start).
 * No loading state (the spec: discovery is a local disk walk — the
 * section simply re-renders when the result arrives). A failed fetch
 * degrades to `[]` (logged via `console.error`, same convention as the
 * existing `closeSession` failure path in `SpacesList.tsx`).
 */
export function useSkillCatalog(spacePath: string | null): SkillInfo[];
```

Implementation notes: `const [rows, setRows] = useState<SkillInfo[]>([])` (initial state is `[]` — a cached promise can't be read synchronously; a SETTLED cache entry resolves in a microtask via the effect's `p.then` below, so a remounted consumer sees its rows on the next tick — the `two_simultaneous_consumers_share_one_fetch` and `a_second_consumer_mounted_after_resolve_does_not_refetch` tests in `useSkillCatalog.test.ts` pin this behavior); a `useEffect` on `[spacePath]` that, when the cache misses, gets-or-starts the fetch and re-renders on resolve. **The cache must store the IN-FLIGHT PROMISE, not just the resolved rows** (load-bearing: `App` mounts `SpacesList` AND `ChatStream` simultaneously, both call `useSkillCatalog` with the same key in the same commit — if the cache only populated on resolve, both effects would see a cache miss and BOTH call `listSkills` → 2 IPC calls per key, and the one-fetch property below would be false in the real app):

```ts
const cache = new Map<string, Promise<SkillInfo[]>>(); // key → in-flight OR settled promise

export function useSkillCatalog(spacePath: string | null): SkillInfo[] {
  const key = spacePath ?? "__global__";
  const [rows, setRows] = useState<SkillInfo[]>([]);
  useEffect(() => {
    let cancelled = false;
    let p = cache.get(key);
    if (!p) {
      p = listSkills(spacePath)
        .catch((err) => {
          // A failed fetch degrades to [] (logged — the same convention as the
          // existing `closeSession` failure path in `SpacesList.tsx`). A FAILED
          // promise is NOT cached (delete it) so a later effect retries — but
          // only if THIS promise is still the cached entry (a test's
          // `clearSkillCatalogCache()` + re-mount may have cached a FRESH one
          // under the same key; an unconditional delete would evict it).
          console.error("listSkills failed:", err);
          if (cache.get(key) === p) cache.delete(key);
          return [] as SkillInfo[];
        });
      cache.set(key, p);
    }
    void p.then((r) => {
      if (!cancelled) setRows(r); // stale-response guard: the Space changed while in flight
    });
    return () => {
      cancelled = true;
    };
  }, [spacePath]);
  return rows;
}

/** Test-only: clear the module-level catalog cache (call in `beforeEach`). */
export function clearSkillCatalogCache(): void {
  cache.clear();
}
```

`src/hooks/useSkillCatalog.test.ts` (new — `renderHook` from `@testing-library/react` (available in the project's `@testing-library/react` v16), `vi.mock("../lib/tauri")` with `listSkills: vi.fn().mockResolvedValue([...])`, `beforeEach` = `vi.clearAllMocks()` + `clearSkillCatalogCache()`):
- `two_simultaneous_consumers_share_one_fetch` — render TWO hook consumers in the SAME test (e.g. a tiny wrapper component rendering nothing but calling the hook twice, or two `renderHook` calls before either resolves) with the same `spacePath` → `await waitFor(() => expect(vi.mocked(listSkills)).toHaveBeenCalledTimes(1))` (the in-flight-promise cache — the property that is FALSE with a resolve-only cache, since both effects run in the same commit before the first resolve lands).
- `different_spaces_fetch_separately` — two consumers with different `spacePath` values → 2 calls.
- `a_second_consumer_mounted_after_resolve_does_not_refetch` — consumer A resolves, then consumer B mounts with the same key → still 1 total call (the settled cached PROMISE is reused — the effect's `p.then` delivers rows on the next microtask and `listSkills` is not called again). Pin the notes-line behavior: after the second mount, `await waitFor(() => expect(resultB.current).toHaveLength(1))`.

`src/components/SkillsSection.tsx`:

```tsx
/**
 * The left-pane Skills section (the ZCode placement: a top-level item
 * above the Sessions list). One row per skill: name (primary) +
 * description (secondary, one-line truncated, full text in a `title`
 * tooltip); user-level rows carry a subtle "global" cue; Space-level
 * rows get none. Clicking a row inserts `$name ` into the composer via
 * the `archimedes:insert-skill` CustomEvent (Task 5 listens) — the
 * composer is in a different subtree, so no prop drilling.
 */
export default function SkillsSection({ skills }: { skills: SkillInfo[] })
```

Render (matching the existing `SpacesList` styling conventions — `text-ui-base`, `text-foreground-subtlest`, `hover:bg-surface-hover`, `rounded-lg` rows):
- The section label: `<p className="px-2.5 py-2 text-ui-base text-foreground-subtlest">Skills</p>` (identical treatment to the existing "Sessions" label).
- Rows: a `div` per skill, `group flex flex-col gap-0.5 rounded-lg px-2.5 py-1 hover:bg-surface-hover cursor-pointer`, `role="button"` + `tabIndex={0}` + `onClick`/`onKeyDown` (Enter) — the same accessibility pattern as the existing `SessionRow`:
  - line 1: the name (`text-ui-base`, `truncate`) + — for `scope === "user"` — a small muted `global` badge (a `text-ui-xs text-foreground-subtlest` span, right-aligned);
  - line 2: the description (`text-ui-sm text-foreground-subtle`, `truncate`, `title={description}`) — omit the line entirely when the description is `""`.
- `onClick` handler: `window.dispatchEvent(new CustomEvent("archimedes:insert-skill", { detail: "$" + skill.name.toLowerCase() + " " }))` (lowercased — the case policy: the inserted token must be expandable, and the mention regex is lowercase-only).
- Empty state: `skills.length === 0` → a single muted line `<p className="p-3 text-ui-sm text-foreground-subtlest">No skills found.</p>` (the same treatment as the existing "No spaces yet" empty state).

`src/components/SpacesList.tsx` (modify):
- Derive the active Space's path: the active session's `cwd` is the Space's folder (CONTEXT.md: a Session's `cwd` IS the Space's folder). Live sessions first, then stored:
  ```ts
  const activeSession = sessions.find((s) => s.sessionId === activeSessionId);
  const activeHistory = activeSession
    ? undefined
    : historySessions.find((s) => s.sessionId === activeSessionId);
  const activeSpacePath = activeSession?.cwd ?? activeHistory?.cwd ?? null;
  ```
  (`sessions` / `historySessions` / `activeSessionId` are already selected at the top of the component.)
- Call `const skills = useSkillCatalog(activeSpacePath);`
- Render `<SkillsSection skills={skills} />` BETWEEN the closing `</div>` of the button row (the `flex flex-col gap-2 border-b ... p-3` div holding "New Session" / "Open Space") and the existing `<p ...>Sessions</p>` label.

`src/components/SkillsSection.test.tsx` (new — the `@testing-library/react` + `render` pattern of the existing component tests). NOTE: the section is PROP-DRIVEN (it receives `skills` as a prop — it does NOT fetch; the fetch lives in the hook, covered by `useSkillCatalog.test.ts`), so these tests pass the catalog as props and need NO `listSkills` mock (a `vi.mock("../lib/tauri")` here would be dead weight — the only tauri import is the type). `findByText` is used (it resolves immediately for synchronously-rendered content, and keeps the style consistent with the other test files):
- `renders_one_row_per_skill_with_name_and_description` — `skills` prop with 2 skills → `await screen.findByText(…)` 2 rows, name + description text present.
- `user_scope_rows_carry_the_global_cue` — TWO SEPARATE renders (a single render with both skills would make a global `queryByText("global")` ambiguous — the user row's badge matches): render the `scope: "user"` skill alone → `findByText("global")` present; render the `scope: "space"` skill alone → `queryByText("global")` is `null`.
- `clicking_a_row_dispatches_the_insert_event` — a skill named `Alpha` (note the uppercase — the case policy lowercases the INSERTED token): `await screen.findByText("Alpha")`, spy on `window.dispatchEvent` (or `addEventListener` a listener first); click the row → an `archimedes:insert-skill` event with `detail: "$alpha "` was dispatched (lowercased name + trailing space).
- `renders_the_empty_state` — `skills: []` → "No skills found."

`src/components/SpacesList.test.tsx` (extend the EXISTING `vi.mock("../lib/tauri", ...)` object — add `listSkills: vi.fn().mockResolvedValue([{ name: "alpha", description: "Alpha skill", path: "/p/.agents/skills/alpha/SKILL.md", dir: "/p/.agents/skills/alpha", scope: "space", body: "B" }])` to the mock, and add `listSkills` to the file's existing `../lib/tauri` import line; and add `beforeEach(clearSkillCatalogCache)` — REQUIRED: the hook's module-level cache is shared across tests in a file, so without clearing it, test 1's warm cache (keyed by the `seed()` fixture's `cwd`) makes test 2's fresh `listSkills` mock moot — the hook serves the cached value and never refetches. Note the file's existing `beforeEach` uses `vi.clearAllMocks()`, which CLEARS CALL COUNTS but PRESERVES the factory's `mockResolvedValue` implementation — so per-test overrides below are what actually change what each test sees):
- `renders_the_skills_section_above_the_sessions_list` — with the factory's 1-skill catalog → `await screen.findByText("Skills")` + `findByText("alpha")` (the catalog arrives via an async effect — the assertions MUST await it); then assert the one-fetch property: `expect(vi.mocked(listSkills)).toHaveBeenCalledTimes(1)`; the "Sessions" label still renders.
- `the_skills_section_shows_the_empty_state_when_there_are_no_skills` — override the factory inside the test: `vi.mocked(listSkills).mockResolvedValue([])` (REQUIRED — `clearAllMocks` keeps the factory's 1-skill implementation, so without the override this test would see `alpha` and fail); → `await screen.findByText("No skills found.")`. (This test is listed LAST so its override cannot leak into the other test: the `beforeEach` cache-clear makes each test refetch, and because `clearAllMocks` PRESERVES implementations, this test's `mockResolvedValue([])` override would leak to any test declared after it — declaration order is what keeps the two independent.)

**Steps:**
- [ ] Write the failing tests: `src/components/SkillsSection.test.tsx` (all 4 cases) + the 2 new `SpacesList.test.tsx` cases
- [ ] Run `pnpm test -- src/components/SkillsSection.test.tsx src/components/SpacesList.test.tsx` (from the repo root)
  - Did it fail (components not found / not rendered)? If it passed unexpectedly, stop and investigate why.
- [ ] Implement `src/hooks/useSkillCatalog.ts` and `src/components/SkillsSection.tsx`; wire the section into `src/components/SpacesList.tsx`
- [ ] Run `pnpm test -- src/components/SkillsSection.test.tsx src/components/SpacesList.test.tsx` (from the repo root)
  - Did all tests pass? If not, fix the failures and re-run before continuing.
- [ ] Run `pnpm test` (from the repo root — full suite)
- [ ] Run `pnpm build` (from the repo root)
  - Did it succeed? If not, fix and re-run before continuing.
- [ ] Commit with message: "feat(skills): left-pane Skills section (catalog hook + section + SpacesList wiring)"

**Acceptance criteria:**
- [ ] The left pane shows a "Skills" section between the button row and the "Sessions" label, with one row per skill (name + one-line description, `global` cue for user-scope rows) and the "No skills found." empty state
- [ ] Clicking a row dispatches `archimedes:insert-skill` with `detail` = `$<name.toLowerCase()> ` (lowercased — the case policy)
- [ ] The hook caches per Space AND dedupes in-flight fetches (two simultaneous consumers with the same key → ONE `listSkills` call — verified by `useSkillCatalog.test.ts`'s `two_simultaneous_consumers_share_one_fetch`; the `SpacesList` test's `toHaveBeenCalledTimes(1)` covers the single-consumer case)
- [ ] `pnpm test` + `pnpm build` pass from the repo root

---

### Task 5: Composer `$` trigger + picker + expansion in `send()`

**Context:** The final task: the composer's `$` trigger (a filterable picker above the textarea) and the send-path expansion. Per the plan's REFINEMENT note, `send()` expands the draft with Task 3's `expandSkillMentions` BEFORE both `addUserMessage` and `sendPrompt` — so the live bubble, the persisted record, and the agent's input are all the SAME text (the content-based dedupe key in `mergeDedupeKey` then matches across a resume reload). The composer stays a plain `<textarea>` (no contenteditable rewrite): the token is PLAIN TEXT in v1 (no chip rendering). The picker data is the SAME `useSkillCatalog` result as the left pane (same key → one shared fetch, Task 4).

**Files:**
- Modify: `src/components/ChatStream.tsx`
- Test: `src/components/ChatStream.test.tsx` (extend)

**What to implement:**

`src/components/ChatStream.tsx` — the composer region (the `div` with `className="m-3 rounded-2xl border border-input-border bg-input p-3 ..."` that holds the attachment strip + the `<textarea ref={composerRef} ...>` + the toolbar row):

1. **Catalog + derived Space path** (the same derivation as Task 4's `SpacesList`). PLACEMENT CONSTRAINT (enforced by the existing regression test "survives the no-session → active-session transition (all hooks are called unconditionally, before the early return)"): ALL new hooks added by this task — `useSkillCatalog` (item 1), the `picker` `useState` (item 3), the `filtered` `useMemo` (item 3), and the `archimedes:insert-skill` `useEffect` (item 6) — MUST be called UNCONDITIONALLY, in the top block of hooks, BEFORE the `if (!activeSessionId) return …` early return (alongside the existing `liveSession`/`historySession` derivations `useSkillCatalog` reuses). A hook placed in/after the composer JSX region (or after the early return) changes the hook count across the session/no-session transition and breaks that existing green test. Plain (non-hook) functions like `activeSkillToken` and `selectSkill` are placement-flexible (the file's own comment says so for the attachment handlers):
   ```ts
   const activeSession = useSessions((s) => s.sessions.find((x) => x.sessionId === s.activeSessionId));
   const activeHistory = useSessions((s) => (s.sessions.some((x) => x.sessionId === s.activeSessionId) ? undefined : s.historySessions.find((x) => x.sessionId === s.activeSessionId)));
   const spacePath = activeSession?.cwd ?? activeHistory?.cwd ?? null;
   const skills = useSkillCatalog(spacePath);
   ```
   (If `ChatStream` already computes `liveSession`/`historySession` for the active `activeSessionId` — it does, for the placeholder logic — REUSE those existing derivations instead of adding new selectors: `spacePath = liveSession?.cwd ?? historySession?.cwd ?? null`. The `useSkillCatalog` call itself stays in the unconditional top block regardless.)

2. **Trigger detection** — a pure helper (export it for the test, or keep it module-private and test through the UI):
   ```ts
   /**
    * The active skill token at the caret: the text between the nearest
    * preceding whitespace (or start-of-line) and the caret. Returns the
    * token's remainder (AFTER the `$`) when the span starts with `$` —
    * `""` for a bare `$` — else `null` (no active token: the span doesn't
    * start with `$`, or it contains a character that can't be part of a
    * skill name, e.g. uppercase — the regex `[a-z0-9-]*$` simply won't
    * reach the caret).
    */
   function activeSkillToken(value: string, caret: number): string | null {
     const before = value.slice(0, caret);
     const match = before.match(/(^|\s)(\$[a-z0-9-]*)$/);
     return match ? match[2]!.slice(1) : null;
   }
   ```

3. **Picker state**: `const [picker, setPicker] = useState<{ query: string; index: number } | null>(null);` (placed in the unconditional top hook block — item 1's constraint):
   - On the textarea `onChange` (real TS — `activeSkillToken` is called ONCE and its return value drives the picker state; a bare `$` (empty token remainder) opens the picker with the FULL list, any non-`$` span closes it):
   ```ts
   onChange={(e) => {
     const token = activeSkillToken(e.target.value, e.target.selectionStart ?? e.target.value.length);
     setDraft(e.target.value);
     setPicker(token !== null ? { query: token, index: 0 } : null);
   }}
   ```
   - `filtered` — NULL-SAFE (the `picker` state is `{…} | null` and this `useMemo` lives in the unconditional top hook block, so a bare `picker.query` is a TS18047 compile error under `strict: true` — `pnpm build` is `tsc && vite build` — and a `picker!.query` "fix" would crash at render whenever the picker is closed, i.e. nearly every render):
   ```ts
   const filtered = useMemo(
     () => skills.filter((s) => s.name.toLowerCase().includes((picker?.query ?? "").toLowerCase())),
     [skills, picker],
   );
   ```
   (case-insensitive substring on the NAME — v1: name only, not description; `picker?.query ?? ""` yields the full list while the picker is null — harmless, the picker UI is gated on `picker && filtered.length > 0`.)
   - **`activeIndex` — derive it, don't clamp it in place** (the raw `picker.index` can go stale: the catalog can change — a Space switch refetches `skills` — while the picker is open with a non-zero `index`, and `filtered[picker.index]!` with a stale index is a `selectSkill(undefined)` → `TypeError` crash on Enter). NULL-SAFE derivation (same nullable-`picker` constraint as `filtered` above):
   ```ts
   const activeIndex = picker
     ? Math.min(picker.index, Math.max(0, filtered.length - 1))
     : 0;
   ```
   Use `activeIndex` in EVERY place the index is consumed: the highlight (`i === activeIndex`), the arrow-key wrap arithmetic (base the next index on `activeIndex`), and the Enter selection (`selectSkill(filtered[activeIndex]!)`). The `Math.max(0, …)` is belt-and-braces: the picker UI and the keyboard branch are both guarded by `filtered.length > 0`, so `filtered.length - 1` is ≥ 0 there.
   - Asymmetry note (accepted, no action): the trigger regex (`[a-z0-9-]*` — allows a trailing hyphen, an in-progress token) is intentionally LOOSER than `MENTION_RE` (no hyphen at a token's end) — a token like `$a--b` opens the picker (filtered to nothing — no name contains `a--b`) and on send `MENTION_RE` would match the leftmost `$a` if a skill `a` exists (ZCode-parity behavior, accepted). The trigger ALSO requires `(^|\s)` before the `$` while `MENTION_RE` is left-boundary-free — so `a$debug` expands on send but never opens the picker while typed (accepted).

4. **Picker UI** — add `relative` to the composer wrapper div's `className`, and render INSIDE that wrapper (before the attachment strip):
   ```tsx
   {picker && filtered.length > 0 && (
     <div className="absolute left-3 right-3 -top-2 z-10 -translate-y-full rounded-lg border border-input-border bg-input p-1 shadow-lg">
       {filtered.map((s, i) => (
         <button
           key={s.name}
           type="button"
          onMouseDown={(e) => { e.preventDefault(); selectSkill(s); }}  // mousedown (not click): the textarea blur fires first on click
           className={`flex w-full flex-col gap-0.5 rounded-md px-2 py-1 text-left ${i === activeIndex ? "bg-surface-hover" : ""}`}
         >
           <span className="text-ui-base">{s.name}</span>
           {s.description !== "" && (
             <span className="truncate text-ui-sm text-foreground-subtle" title={s.description}>{s.description}</span>
           )}
         </button>
       ))}
     </div>
   )}
   ```
   (Positioning: the picker floats ABOVE the composer wrapper — `-top-2 -translate-y-full` puts it above the wrapper's top edge; `z-10` keeps it above the transcript. If the wrapper's existing `className` already positions children, adapt the anchor classes to the actual wrapper — the REQUIREMENT is "the picker renders above the composer, anchored to it, full-width minus the wrapper padding".)

5. **Keyboard handling** — extend the textarea's existing `onKeyDown` (currently: `Enter` + no-Shift → `send()`). The index used here is `activeIndex` (item 3's derivation — NOT the raw `picker.index`, which can be stale against a changed `filtered`):
   ```ts
   onKeyDown={(e) => {
     if (picker && filtered.length > 0) {
       if (e.key === "ArrowDown") { e.preventDefault(); setPicker({ ...picker, index: (activeIndex + 1) % filtered.length }); return; }
       if (e.key === "ArrowUp") { e.preventDefault(); setPicker({ ...picker, index: (activeIndex + filtered.length - 1) % filtered.length }); return; }
       if (e.key === "Enter") { e.preventDefault(); selectSkill(filtered[activeIndex]!); return; }
       if (e.key === "Escape") { e.preventDefault(); setPicker(null); return; }
     }
     if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); void send(); }
   }}
   ```
   `selectSkill(skill)`: compute the replacement — the token span runs from the `$` (the match's start in `before`) to the caret. Per the case policy, the inserted token is the LOWERCASED name (`` `$${skill.name.toLowerCase()} ` `` — so a skill named `DeBuG` selected from the picker is inserted as `$debug ` and WILL expand on send; the expansion itself keeps the frontmatter name verbatim in the block's `name` attribute):
   ```ts
   const selectSkill = (skill: SkillInfo) => {
     if (!picker) return;
     const el = composerRef.current;
     const caret = el?.selectionStart ?? draft.length;
     const before = draft.slice(0, caret);
     const m = before.match(/(^|\s)(\$[a-z0-9-]*)$/);
     if (!m) return;
     const tokenStart = m.index! + m[1]!.length; // the `$` position
     const inserted = `$${skill.name.toLowerCase()} `;
     const next = before.slice(0, tokenStart) + inserted + draft.slice(caret);
     setPicker(null);
     setDraft(next);
     requestAnimationFrame(() => {
       const el = composerRef.current;
       if (!el) return;
       el.focus();
       const pos = tokenStart + inserted.length;
       el.setSelectionRange(pos, pos);
     });
   };
   ```
   (The `requestAnimationFrame` re-focus + caret-set is REQUIRED: `setDraft` re-renders the controlled textarea, which would otherwise drop focus/caret.)

6. **The `archimedes:insert-skill` listener** (Task 4's left-pane rows dispatch it): a `useEffect` (deps: none — read through refs, matching the component's existing `draftRef` mirror pattern):
   ```ts
   useEffect(() => {
     const onInsertSkill = (e: Event) => {
       const text = (e as CustomEvent<string>).detail;
       if (typeof text !== "string") return;
       const el = composerRef.current;
       const draft = draftRef.current; // the LIVE value (the ref mirror already exists in this component)
       if (!el) { setDraft(draft + text); return; }
       const start = el.selectionStart ?? draft.length;
       const end = el.selectionEnd ?? start;
       const next = draft.slice(0, start) + text + draft.slice(end);
       setDraft(next);
       requestAnimationFrame(() => {
         el.focus();
         const pos = start + text.length;
         el.setSelectionRange(pos, pos);
       });
     };
     window.addEventListener("archimedes:insert-skill", onInsertSkill);
     return () => window.removeEventListener("archimedes:insert-skill", onInsertSkill);
   }, []);
   ```
   (If the component does NOT already have a `draftRef` mirror — it does: `send()`'s stale-draft guard uses `draftRef.current` — REUSE it; do not add a second ref.)

7. **`send()` — the expansion** (the REFINEMENT's core change). The current code starts `const text = draft.trim();` and uses `text` for the emptiness guard, the stale-draft guard comparison, `addUserMessage`, and `sendPrompt`. Change it so the RAW text drives the guards and the EXPANDED text drives the writes. PLACEMENT IS LOAD-BEARING (a TDZ trap): the existing images-empty normalization block (`if (images !== undefined && images.length === 0) { if (!text) return; images = undefined; }`) sits BETWEEN the stale-draft guard and `setDraft("")` and still references `text` — so `const text = expandSkillMentions(rawText, skills)` must be declared IMMEDIATELY AFTER the stale-draft guard, BEFORE that images-empty block (declaring it later would make the unchanged `if (!text)` a "Block-scoped variable 'text' used before its declaration" error; semantically the placement is safe — expansion maps `""`→`""`, so `!text` is identical to `!rawText`):
   ```ts
   const rawText = draft.trim();
   if ((!rawText && !hasImages) || composerLocked) return;
   ...
   // (the existing resume block: `if (!hasImages && text === "") return;`
   //  becomes `if (!hasImages && rawText === "") return;`)
   ...
   // (the existing stale-draft guard stays, compared against the RAW text —
   //  the draft may have gained/lost a mention while the image read was
   //  in flight; expansion is re-derived from the FRESH draft below, so
   //  the guard must NOT compare expanded texts)
   if (draftRef.current.trim() !== rawText) return;
   // ← EXPANSION HERE: immediately after the stale-draft guard, BEFORE the
   //   images-empty normalization block (which keeps its `if (!text)` as-is)
   const text = expandSkillMentions(rawText, skills);
   if (images !== undefined && images.length === 0) {
     if (!text) return;
     images = undefined;
   }
   ...
   setDraft("");
   addUserMessage(activeSessionId, text, imageRefs); // EXPANDED
   beginTurn(activeSessionId);
   const stopReason = imageRefs
     ? await sendPrompt(activeSessionId, text, imageRefs) // EXPANDED
     : await sendPrompt(activeSessionId, text);          // EXPANDED
   ```
   (Everything else in `send()` — the image read, the reconciliations, the attachment release — is UNCHANGED. `expandSkillMentions` is a cheap string op; no extra `await` is added to the send path. Accepted edge: a send before the catalog fetch resolves sends any mention UNEXPANDED — the catalog is `[]` until the fetch lands, and the picker is not open either; the fetch is a local disk walk and resolves quickly, so the edge is accepted, not guarded.)

`src/components/ChatStream.test.tsx` (extend — the existing setup mocks `../lib/tauri` with `sendPrompt: vi.fn().mockResolvedValue("end_turn")` and drives the real store; ADD `listSkills: vi.fn().mockResolvedValue([{ name: "debug", description: "Debug a failure", path: "/s/.agents/skills/debug/SKILL.md", dir: "/s/.agents/skills/debug", scope: "space", body: "Step 1. Step 2." }])` to the mock object, and add `beforeEach(clearSkillCatalogCache)` — the hook's module-level cache is shared across tests in a file, so each test must start with a COLD cache or the previous test's warm cache (same `cwd` key from the `seedLiveSession` fixture) makes the fresh `listSkills` mock moot. CONSEQUENCE: with a cold cache the catalog arrives via an ASYNC effect — every picker assertion must `await` (`findByText`/`waitFor`); a synchronous `getByText` runs before the fetch resolves and fails):
- `typing_$ opens the skill_picker` — render with the mocked session + catalog; `fireEvent.change` on the textarea with `value: "$"` → `await screen.findByText("debug")` (the picker renders once the catalog fetch resolves — the `await` covers both the fetch and the render).
- `the_picker_filters_as_the_token_is_typed` — `fireEvent.change` with `value: "$de"` → `await screen.findByText("debug")` (catalog ready); then `fireEvent.change` with `value: "$zzz"` → `expect(screen.queryByText("debug")).toBeNull()` (the picker is not visible — no matches; the catalog is already warm at this point, so no await is needed for the second change).
- `enter_selects_the_highlighted_skill_and_inserts_the_token` — `fireEvent.change` with `value: "$de"`, `await screen.findByText("debug")` (catalog ready), then `fireEvent.keyDown(enter)` on the textarea → the draft is `$debug ` (read back from the TEXTAREA's `value` on the next render — the draft is local `useState` in `ChatStream`, it is NOT in any store) and the picker is closed.
- `escape_closes_the_picker_without_inserting` — `value: "$de"` + `await findByText("debug")` + `Escape` → picker gone, draft still `$de`.
- `send_expands_the_mention_before_addUserMessage_and_sendPrompt` — set the draft to `fix $debug ` (WITH THE TRAILING SPACE — a trailing space ends the active token, so the picker is NOT open and Enter is NOT intercepted by the picker's `selectSkill` branch; `rawText = draft.trim()` still yields `fix $debug`, so every assertion below holds unchanged), press Enter to send → `expect(useSessions.getState().messages["s1"])` contains a user message whose `text` STARTS with `fix $debug` and CONTAINS `<skill name="debug" location="/s/.agents/skills/debug/SKILL.md">` + `References are relative to /s/.agents/skills/debug.` + `Step 1. Step 2.`; AND `expect(sendPrompt).toHaveBeenCalledWith("s1", <the same expanded text>)` (the 2-arg form — no images). (The synchronous part of `send()` — `addUserMessage` + the `sendPrompt` call — runs before the first `await`, so the assertions hold without an extra `waitFor`; if a flake appears, wrap in `waitFor`.)
- `an_unmatched_token_is_sent_verbatim` — draft `hi $nope ` (trailing space, picker closed) → the sent text is `hi $nope` unchanged (assert `sendPrompt` received exactly that — `rawText` is `hi $nope` and `expandSkillMentions` found no match).
- `the_left_pane_insert_event_appends_at_the_caret` — while the textarea is focused at the end of an existing draft `hello `, dispatch the event INSIDE act — `await act(async () => { window.dispatchEvent(new CustomEvent("archimedes:insert-skill", { detail: "$debug " })); })` — the `act` wrap is REQUIRED: a raw `dispatchEvent` does not flush React's state updates (the listener's `setDraft` would be invisible to a synchronous assertion; the file's own `fireEvent.paste` comment documents this convention) → the textarea's value becomes `hello $debug `. (No catalog dependency — the insert event is independent of `listSkills`.)

**Steps:**
- [ ] Write the failing tests in `src/components/ChatStream.test.tsx` (all 7 cases; add `listSkills` to the existing tauri mock)
- [ ] Run `pnpm test -- src/components/ChatStream.test.tsx` (from the repo root)
  - Did it fail (picker not rendered / expansion not applied)? If it passed unexpectedly, stop and investigate why.
- [ ] Implement the composer changes in `src/components/ChatStream.tsx` (items 1-7 above)
- [ ] Run `pnpm test -- src/components/ChatStream.test.tsx` (from the repo root)
  - Did all tests pass? If not, fix the failures and re-run before continuing.
- [ ] Run `pnpm test` (from the repo root — full suite; the existing `sendPrompt` 2-arg assertions in this file must STILL pass — the no-skill / no-mention sends are byte-identical to before)
- [ ] Run `pnpm build` (from the repo root)
  - Did it succeed? If not, fix and re-run before continuing.
- [ ] Commit with message: "feat(skills): composer $ trigger, picker, and send-path expansion"

**Acceptance criteria:**
- [ ] Typing `$` in the composer opens the picker (full list for a bare `$`, name-filtered otherwise); ↑/↓/Enter/Esc + click all work; selection inserts `$<name.toLowerCase()> ` as plain text with the caret after it (the case policy — a picker-selected skill always expands on send)
- [ ] Sending a message with a `$name` mention calls BOTH `addUserMessage` and `sendPrompt` with the EXPANDED text (the exact pi-format block), for both native and external sessions (the expansion is harness-agnostic — it happens before the Tauri boundary)
- [ ] A message with no (or no matching) mentions is sent byte-identical to the pre-feature behavior (the existing `sendPrompt` assertions in `ChatStream.test.tsx` pass unchanged)
- [ ] The `archimedes:insert-skill` event (left-pane rows, Task 4) inserts at the caret and focuses the composer
- [ ] `pnpm test` + `pnpm build` pass from the repo root

---

## Verification (run at the end of the whole feature, per AGENTS.md)

| What | Command | Where |
|------|---------|-------|
| Frontend unit tests | `pnpm test` | repo root |
| Frontend type-check + build | `pnpm build` | repo root |
| Rust tests | `cargo test` | `src-tauri/` |
| Rust lint | `cargo clippy --all-targets` | `src-tauri/` (0 warnings) |
| Rust formatting | `cargo fmt --check` | `src-tauri/` |

**Manual smoke (the done-when, end-to-end):** with a Space that has a skill in `.agents/skills/` (e.g. a minimal `SKILL.md` with `name` + `description` frontmatter): the left pane lists it; typing `$` in the composer offers it; selecting + sending shows the expanded `<skill>` block in the user bubble AND the agent acts on the skill's instructions — in BOTH a native and an external (pi) session.
