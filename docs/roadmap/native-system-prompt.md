---
status: committed
done-when: A native session starts with a persisted system prompt (preamble + <tools> + <rules> + <project_context> + <skills> + <cwd>) at seq 0, replays it verbatim on resume, and survives compaction; a native subagent child starts with [launch systemPrompt, if any] + the todo guidance line (when it has the manage_todo_list tool); the subagent tool's agentName is a free-form UI label (no lookup).
---

# Native Session System Prompt Plan

**Goal:** Give native sessions a minimal, pi-shaped system prompt (authored by the desktop, persisted with the transcript) and give native subagent children a reduced role prompt.
**Architecture:** A new `prompt.rs` module in the native harness builds the prompts (pure functions). `AgentLoop` persists the leading system message at seq 0 and preserves it through compaction. `build_native_session` (new sessions) and `dispatch_native_inner` (children) call it once at session start; resumes replay the stored transcript verbatim (no rebuild).
**Tech Stack:** Rust (Tauri 2 backend, tokio), SQLite (the existing `native_messages` table). No frontend changes (the UI already renders `agentName` + `task`).

**Spec reference:** the approved spec was this file's previous content (committed as `docs: add native-system-prompt spec`) + ADR 0017 (`docs/decisions/0017-native-session-system-prompt.md`). **Spec corrections in this plan (reviewer-vetted):** (1) the spec said "the compactor is unchanged" — that is WRONG: `run_compaction` currently DROPS the leading system message (it lands in `older` and is replaced by the summary, which is itself a `System` message). Task 2 fixes it, mirroring pi's compaction ("System messages are prompt state, not conversation; the compaction entry carries their replay"). (2) The `native_messages` table FKs `sessions` (`PRAGMA foreign_keys = ON`), and the callers `record_session` AFTER `build_native_session` returns — so the seq-0 persist (Task 2) would silently fail (FK violation) unless `build_native_session` records the row first. Task 3 handles it (early `record_session`; `record_session` is an idempotent upsert that preserves the stored `archived` flag — db.rs test `record_session_preserves_the_archived_flag`). (3) The `<project_context>` walk bound is a DELIBERATE deviation from pi (repo root, not the filesystem root) — see Task 1.

**Validation commands** (per AGENTS.md, run from `src-tauri/` unless noted): `cargo test` · `cargo clippy --all-targets` (must be 0 warnings) · `cargo fmt --check`. Frontend (repo root): `pnpm test` · `pnpm build` — run once at the end (no frontend changes expected).

---

### Task 1: `prompt.rs` — the prompt builder module

**Context:**
The native harness has no prompt machinery. This task creates the pure prompt builder: the main-session prompt (preamble + `<tools>` + `<rules>` + `<project_context>` + `<skills>` + `<cwd>`, ADR 0017) and the subagent child's reduced system message. Pure functions, no I/O beyond the documented fs reads — everything is unit-testable without a loop. The `<tools>` section takes the ADVERTISED tool specs (the `AgentLoop`'s filter output — Task 2 adds the accessor) so the prompt and the `tools[]` API param can never disagree. The skills section mirrors pi's `formatSkillsForPrompt` output byte-for-byte (metadata only, progressive disclosure — ADR 0013's `discover_skills` is the data source). The project context mirrors pi's `loadProjectContextFiles` (candidate list + walk-up + global file).

**Files:**
- Create: `src-tauri/src/agent/harness/prompt.rs`
- Modify: `src-tauri/src/agent/harness/mod.rs` (add `pub mod prompt;` after `pub mod provider;` + a `pub use prompt::{build_child_system_message, build_main_prompt, load_project_context, PromptContext};` line in the existing `pub use` block area)

**What to implement:**

The module, with these exact constants and signatures (note the two DELIBERATE deviations from pi, documented below the function):

```rust
/// The one-sentence preamble (pi's, with the app name swapped — ADR 0017).
const PREAMBLE: &str = "You are an expert coding assistant operating inside Archimedes Desktop, a desktop app that connects to coding agents. You help users by reading files, executing commands, editing code, and writing new files.";

/// The manage_todo_list guidance line (shared by the main prompt's <rules>
/// and the child's system message).
const TODO_GUIDANCE: &str = "Use manage_todo_list to track multi-step work — write the plan before starting, mark items completed as you go";

/// The subagent guidance line (main prompt only — a child has no subagent tool).
const SUBAGENT_GUIDANCE: &str = "Delegate independent subtasks with subagent — give each a systemPrompt describing its role and constraints (e.g. a read-only researcher, a focused reviewer)";

/// The context-file candidates (pi's order — first found wins per dir).
const CONTEXT_CANDIDATES: &[&str] = &["AGENTS.override.md", "AGENTS.md", "AGENTS.MD", "CLAUDE.md", "CLAUDE.MD"];

pub struct PromptContext<'a> {
    /// The Space's folder (canonical).
    pub cwd: &'a Path,
    /// The global context-file dir (`~/.pi/agent` — the caller computes it
    /// via `home_dir().join(".pi/agent")`; an empty/nonexistent path = no
    /// global file). Injected for testability.
    pub agent_dir: &'a Path,
    /// The tool specs ADVERTISED to the model (the `AgentLoop`'s
    /// `advertised_specs` — single source of truth: the prompt's <tools>
    /// section matches the `tools[]` API param exactly).
    pub tools: &'a [ToolSpec],
    /// The discovered skills (the existing `crate::skills::discover_skills`).
    pub skills: &'a [SkillInfo],
}

pub fn build_main_prompt(ctx: &PromptContext) -> String
pub fn build_child_system_message(system_prompt: Option<&str>, has_todo_tool: bool) -> Option<String>
pub fn load_project_context(cwd: &Path, agent_dir: &Path) -> Vec<(PathBuf, String)>
```

`build_main_prompt` — the EXACT rendered format (sections joined with a BLANK LINE, `"\n\n"` — pi's actual rendering: pi-ai's `getSystemMessageText` (`node_modules/@earendil-works/pi-ai/dist/utils/text.js`) joins the non-empty parts with `"\n\n"`, and each section value is `<name>\ncontent\n</name>`; the frozen format below is authoritative):

```
{PREAMBLE}

<tools>
- {name}: {description}        (one line per advertised spec, in order)
</tools>

<rules>
- {TODO_GUIDANCE}             (only when a spec named "manage_todo_list" is present)
- {SUBAGENT_GUIDANCE}         (only when a spec named "subagent" is present)
- Be concise in your responses
- Show file paths clearly when working with files
</rules>

<project_context>             (omitted entirely when load_project_context returns none)
Project-specific instructions and guidelines:

<project_instructions path="{path}">
{content}
</project_instructions>
</project_context>

<skills>                    (omitted when no skills, or when neither a "read" nor a "bash" spec is present)
The following skills provide specialized instructions for specific tasks.
Use the read tool to load a skill's file when the task matches its description.
When a skill file references a relative path, resolve it against the skill directory (parent of SKILL.md / dirname of the path) and use that absolute path in tool commands.

<available_skills>
  <skill>
    <name>{escaped name}</name>
    <description>{escaped description}</description>
    <location>{escaped path}</location>
  </skill>
</available_skills>
</skills>

<cwd>
{cwd with backslashes replaced by /}
</cwd>
```

Rules for the body:
- `<tools>` with ZERO specs renders the body as the single line `(none)`. (Deliberate native simplification: pi's tools body also carries a trailing sentence — "In addition to the tools above, you may have access to other custom tools depending on the project." — the native format omits it; the frozen format above is authoritative.)
- The `<rules>` order is EXACTLY: TODO_GUIDANCE (if present), SUBAGENT_GUIDANCE (if present), `Be concise in your responses`, `Show file paths clearly when working with files`.
- `<project_context>`: one `<project_instructions path="...">` block per file, in the order `load_project_context` returns them, separated by a blank line; the content verbatim (BOM already stripped).
- `<skills>`: the body above is pi's `formatSkillsForPrompt` output with its leading `"\n\n"` trimmed — the per-skill block is byte-identical to pi's (two-space indent, `  <skill>` … `</available_skills>`). The "Use the read tool…" line becomes "Use bash to load a skill's file when the task matches its description." when `read` is absent and `bash` present. XML-escape `name`/`description`/`location` (pi's `escapeXml`: `&` `<` `>` `"` `'` → `&amp;` `&lt;` `&gt;` `&quot;` `&apos;`). (Deliberate v1 divergence: pi's `formatSkillsForPrompt` also EXCLUDES skills with `disableModelInvocation = true` — `SkillInfo`/`discover_skills` (ADR 0013) has no such field, so no filtering in v1.)
- Omitted sections leave NO trace (no empty `<x></x>`, no dangling blank lines — the join only includes present sections).

`build_child_system_message`:
- `Some(sp)` + `has_todo_tool` → `format!("{sp}\n{TODO_GUIDANCE}")`
- `Some(sp)` + `!has_todo_tool` → `Some(sp.to_string())`
- `None` + `has_todo_tool` → `Some(TODO_GUIDANCE.to_string())`
- `None` + `!has_todo_tool` → `None`

`load_project_context(cwd, agent_dir)` — mirrors pi's `loadProjectContextFiles` candidate list + walk-up + global-file-first ordering, with TWO deliberate deviations (documented so a future reader doesn't "fix" them back to pi's):
1. The walk STOPS at the REPO ROOT — the first ancestor (inclusive) containing a `.git` entry (file OR dir); if none, the filesystem root. (Pi walks to the filesystem root — a project's context must not leak from the user's home dir / unrelated ancestors.)
2. Dedup is by CANONICAL path (`std::fs::canonicalize`, falling back to the raw path on failure). (Pi dedups by raw lexical path.)

1. The global file: `load_context_file_from_dir(agent_dir)` (the first candidate that is a file; BOM stripped) — pushed FIRST. `agent_dir` empty/missing → skip.
2. Walk up from `cwd`: collect dirs from `cwd` up to the bound in (1); for each dir, outermost first, `load_context_file_from_dir(dir)`; dedup per (2); skip unreadable files silently (best-effort, like the rest of the function).
3. Return the collected `(path, content)` pairs.

`load_context_file_from_dir(dir)` (private): for each candidate in `CONTEXT_CANDIDATES`, `dir.join(name)`; the FIRST candidate that `is_file()` AND reads successfully wins — a stat/read failure on an existing candidate CONTINUES to the next candidate (pi's `loadContextFileFromDir` behavior: the read `catch` warns and falls through; a non-file also continues); an unreadable candidate is skipped, and if none reads, `None`.

**Steps:**
- [ ] Create `prompt.rs` with the signatures above (`todo!()` bodies) + the failing tests in `#[cfg(test)] mod tests` (all use `std::env::temp_dir().join(uuid)` scratch dirs; `SkillInfo` literals — construct directly, all fields are pub):
  1. `main_prompt_full_shape` — a ctx with 3 fake `ToolSpec`s: EXACTLY `read` (description `"Read a file."`), `manage_todo_list`, `subagent` (the `read` spec is REQUIRED for the `<skills>` section to render per the rule above), 1 `SkillInfo`, a temp cwd with an `AGENTS.md`, a temp agent_dir with an `AGENTS.md`, and an EMPTY `.git/` dir in the scratch root (the scratch-dir rule below) → assert the EXACT full output (golden string, blank-line join, global file first in `<project_context>`). The golden MUST interpolate the scratch-dir paths (`cwd`/`agent_dir`/skill `location` differ per run) — build the expected string with `format!` using the actual paths, not a hardcoded literal.
  2. `main_prompt_rules_conditional` — tools WITHOUT `manage_todo_list`/`subagent` → both guidance lines absent, the two pi lines present; tools WITH both → all 4 lines in the exact order.
  3. `main_prompt_omitted_sections` — no context files anywhere + no skills → no `<project_context>`/`<skills>` sections (and no dangling blank lines — the `<rules>` block is immediately followed by `<cwd>`).
  4. `main_prompt_skills_requires_read_or_bash` — skills present, tools = `[manage_todo_list]` only → no `<skills>`; tools include `bash` (no `read`) → the "Use bash to load…" line; tools include `read` → the read line.
  5. `main_prompt_tools_section` — `<tools>` body = one `- {name}: {description}` line per spec in order; zero specs → `(none)`.
  6. `context_candidate_precedence` — a dir with `AGENTS.override.md` + `AGENTS.md` + `CLAUDE.md` → only `AGENTS.override.md`; a dir with only `CLAUDE.md` → `CLAUDE.md`; a dir where `AGENTS.md` exists but is UNREADABLE (chmod 000 — `#[cfg(unix)]`) and `CLAUDE.md` reads fine → `CLAUDE.md` (read-failure continues, per the spec above).
  7. `context_walk_up_repo_root_bound` (the DELIBERATE deviation from pi) — a temp repo `repo/` with an EMPTY `repo/.git/` dir, `repo/AGENTS.md`, cwd = `repo/sub/nested/` → result = `[repo/AGENTS.md]` only; a `repo_parent/AGENTS.md` above the repo root is NOT included.
  8. `context_walk_up_no_git` — `a/b/c` (cwd), `a/AGENTS.md`, no `.git` BELOW THE SCRATCH ROOT (the scratch-root `.git` from the scratch-dir rule bounds the walk at the scratch — the point of this test is that `a/`, a non-repo ancestor WITHIN the scratch, is still included) → `a/AGENTS.md` IS included.
  9. `context_order_outermost_first` — `a/AGENTS.md` + `a/b/AGENTS.md` (cwd = `a/b`) → order `[a/AGENTS.md, a/b/AGENTS.md]`.
  10. `context_dedup_symlink` (`#[cfg(unix)]`) — `a/AGENTS.md` + a symlink `a/b/AGENTS.md` → one entry.
  11. `context_bom_stripped` — an `AGENTS.md` starting with a UTF-8 BOM → content without it.
  12. `context_global_first` — `agent_dir/AGENTS.md` + `cwd/AGENTS.md` → `[global, cwd]`; an `agent_dir` with only `CLAUDE.md` → the global `CLAUDE.md` is included.
  13. `child_message_matrix` — the 4 cases of `build_child_system_message` (exact strings).
  14. `skills_format_matches_pi_golden` — 2 `SkillInfo`s, `read` selected → the `<skills>` body (from `The following skills…` through `</available_skills>`) equals a golden string copied from pi's `formatSkillsForPrompt` output (the pi source: `/home/daniel/.local/lib/node_modules/@earendil-works/pi-coding-agent/dist/core/skills.js` — trim its leading `"\n\n"`).
  15. `skills_xml_escaping` — a skill name/description containing `&`/`<`/`"` → escaped per the `escapeXml` table.

  **Scratch-dir rule (all fs tests, incl. 1, 4, 12, 13):** create an EMPTY `.git/` dir in the scratch ROOT so the walk is bounded at the scratch (a stray `AGENTS.md`/`CLAUDE.md` in `/tmp` or `/` — from another project's test run — must not break the exact-golden or absence assertions; on macOS `std::env::temp_dir()` is a symlinked path, so compare canonicalized paths wherever paths are compared). Tests 7–10 (walk/dedup) additionally assert CONTAINMENT/ORDER of the expected entries, not exact vector equality.
- [ ] Run `cargo test harness::prompt` — did the tests FAIL (`todo!()` panics / compile errors)? If they passed unexpectedly, stop and investigate.
- [ ] Implement `prompt.rs` per the spec above + the `mod.rs` registration.
- [ ] Run `cargo test harness::prompt` — did all tests pass? If not, fix and re-run.
- [ ] Run `cargo fmt` then `cargo clippy --all-targets` — 0 warnings.
- [ ] Commit with message: `feat(harness): prompt builder (main + child prompts, project context, skills)`

**Acceptance criteria:**
- [ ] `cargo test harness::prompt` — all 15 tests pass.
- [ ] `cargo clippy --all-targets` — 0 warnings; `cargo fmt --check` — clean.
- [ ] `build_main_prompt` output for a full context equals the golden string byte-for-byte (test 1).
- [ ] `build_child_system_message` matches the 4-case matrix (test 13).

---

### Task 2: `AgentLoop` — persist the system message + preserve it through compaction

**Context:**
Today `prepend_system` inserts a `System` message at index 0 but it is NEVER persisted (`persist_transcript_message` persists only the LAST message) — so a resumed session loses it. And `run_compaction` DROPS it: the leading `System` message lands in `older`, which is replaced by the summary (itself a `System` message) — a long session would silently lose its system prompt after the first compaction. Pi's compaction excludes system messages ("System messages are prompt state, not conversation; the compaction entry carries their replay"). This task adds the persist (inside `prepend_system`, so both call sites — session start and child dispatch — are covered and cannot forget) and the compaction preservation.

**Files:**
- Modify: `src-tauri/src/agent/harness/loop.rs`

**What to implement:**

1. A new private method beside `persist_transcript_message` (line ~1295):

```rust
/// Persist the LEADING system message (if any) at seq 0 (an idempotent
/// upsert — the transcript record of the session's system prompt;
/// ADR 0017). A resume replays it verbatim via `load_transcript`.
/// The `len == 1` guard makes `prepend_system`'s "call AT MOST ONCE"
/// contract explicit: a second call on a non-empty transcript would
/// otherwise clobber an existing seq-0 row.
fn persist_system_message(&self) {
    if self.messages.len() != 1 {
        return;
    }
    let Some(m) = self.messages.first() else { return };
    if !matches!(m.role, ChatRole::System) {
        return;
    }
    let content_json = serde_json::to_string(m).unwrap_or_default();
    if let Err(e) = self.store.insert_message(&self.session_id, 0, role_str(m.role), &content_json) {
        eprintln!("harness: system message persist failed: {e}");
    }
}
```

2. `prepend_system` (line ~291): add `self.persist_system_message();` as the LAST statement (after the `insert` + `compactor.reestimate`). Update its doc comment: append "+ persisted at seq 0 (a resume replays it; a compaction preserves it)".

3. A new `pub` instance method (the prompt builder's `<tools>` source — single source of truth with the `tools[]` API param):

```rust
/// The tool specs advertised to the model (the `enabled_tools` filter +
/// the subagent drop for a child) — the SAME value `model_request` sends
/// as `tools[]`, so the prompt's `<tools>` section can never disagree
/// with the API param (ADR 0017).
pub fn advertised_specs(&self) -> Vec<ToolSpec> {
    Self::advertised_tool_specs(&self.enabled_tools, self.subagent.is_none())
}
```

(`advertised_tool_specs` itself stays private; `model_request` is UNCHANGED.)

4. `run_compaction` (line ~1078) — preserve the leading system message. Replace the `let (older, recent) = split_for_compaction(&self.messages, keep);` line with:

```rust
// The leading system message (if any) is prompt state, not conversation
// (pi's compaction: "System messages are prompt state, not conversation;
// the compaction entry carries their replay"): EXCLUDE it from the
// compaction target and RE-PREPEND it to the compacted transcript.
let system_head = self
    .messages
    .first()
    .filter(|m| matches!(m.role, ChatRole::System))
    .cloned();
let compactable: &[ChatMessage] = match &system_head {
    Some(_) => &self.messages[1..],
    None => &self.messages[..],
};
let (older, recent) = split_for_compaction(compactable, keep);
```

And in the `Ok(summary)` arm, replace `let mut compacted = Vec::with_capacity(1 + recent.len()); compacted.push(summary_msg);` with:

```rust
let mut compacted = Vec::with_capacity(2 + recent.len());
if let Some(s) = system_head.clone() {
    compacted.push(s);
}
compacted.push(summary_msg);
```

The `if !older.is_empty()` guard is UNCHANGED (empty `older` → no summary → `self.messages` untouched → the system head stays in place naturally). The `replace_messages` rewrite (fresh seq run) then writes the preserved system message at seq 0 again.

**Steps:**
- [ ] Extend the test helper: `build_loop` (line ~1896) currently swallows the `Db`. Refactor it to delegate to a new `build_loop_with_db(provider, events, turn_cancel, settle_tx, retry) -> (AgentLoop, Arc<Db>)` — same body EXCEPT `SessionStore::new(db.clone())` (so the `Arc<Db>` can be returned; `SessionStore` takes an `Arc`), returning `(loop_, db)` — and have `build_loop` call it and drop the `db`. Existing tests are unchanged.
- [ ] Write the failing tests in `loop.rs`'s `#[cfg(test)] mod tests` (assert on the STORE via `loop_.store.load_messages("s1")` — it returns `Vec<ChatMessage>` in seq order, so `loaded[0]` IS the seq-0 row; do NOT assert on `Db::load_native_messages` directly — it returns content-JSON `Vec<String>` only, with no seq/role columns):
  1. `prepend_system_persists_at_seq_zero` — `build_loop_with_db` (a `ScriptedProvider` with one canned assistant event); `loop_.prepend_system("the prompt".into())`; `loop_.store.load_messages("s1").unwrap()` → one entry, a `ChatRole::System` message with content text `"the prompt"` (and, if asserting the stored JSON, the content string contains `"role":"system"` — `ChatRole` serializes lowercase).
  2. `compaction_preserves_leading_system_message` — a loop built via `build_loop_with_db`, then `loop_.catalog.compaction.keep_recent_tokens = 1;` (the `catalog` field is `pub` and `CompactionConfig`'s fields are `pub`; `run_compaction` reads `self.catalog.compaction.keep_recent_tokens` directly — line ~1082 — so the mutation takes effect; a direct `run_compaction` call never checks `enabled`); `loop_.load_transcript(vec![system "the prompt", user, assistant, user, assistant])` (5 messages; the `ScriptedProvider`'s first call returns a canned assistant text event for the summary, subsequent calls return a normal assistant event); call `loop_.run_compaction(&CancellationToken::new()).await` directly (same-module test — the private method is visible; `run_compaction` takes a bare `&CancellationToken`, NOT the `Arc<StdMutex<CancellationToken>>` the test passes into `build_loop_with_db` as `turn_cancel` — do not pass `&turn_cancel`, it won't type-check); assert: `loop_.messages[0]` is the ORIGINAL system message (`"the prompt"`), `loop_.messages[1]` is the summary (`"Summary of previous conversation:…"`), and the `replace_messages` rewrite kept the system at seq 0 (`loop_.store.load_messages("s1").unwrap()[0]` is the `System` `"the prompt"` message).
  3. `compaction_without_system_message_unchanged` — same setup but the seeded transcript has NO leading system message → `loop_.messages[0]` is the summary (today's behavior preserved).
  4. `resume_replays_system_message_verbatim` — a loop; `loop_.prepend_system("the prompt".into())`; `let loaded = loop_.store.load_messages("s1").unwrap();` `loop_.load_transcript(loaded.clone())` → `loop_.messages[0]` is byte-identical to the original (role `System`, same content).
- [ ] Run `cargo test harness::r#loop` — did the new tests FAIL before the implementation? (Test 1 fails — nothing persisted; test 2 fails — the system message is dropped.) If they passed unexpectedly, stop and investigate.
- [ ] Implement items 1–4.
- [ ] Run `cargo test harness::r#loop` — all tests pass (existing compaction tests included — they must not regress).
- [ ] Run `cargo fmt` + `cargo clippy --all-targets` — 0 warnings.
- [ ] Commit with message: `feat(harness): persist the system message at seq 0 + preserve it through compaction`

**Acceptance criteria:**
- [ ] A `prepend_system`'d message is in `native_messages` at seq 0 with role `"system"` (test 1).
- [ ] After `run_compaction`, the leading system message survives at index 0 and seq 0; the summary follows it (test 2).
- [ ] A transcript without a leading system message compacts exactly as today (test 3).
- [ ] `load_transcript` of the stored rows restores the system message byte-identically (test 4); all pre-existing `loop.rs` tests still pass.

---

### Task 3: main-session wiring — `build_native_session`

**Context:**
Wire the prompt into the native session lifecycle: a NEW session gets the built prompt (once, at session start, before the first prompt — `prepend_system` persists it at seq 0); a RESUME replays the stored transcript verbatim (the existing `load_transcript` path — NO rebuild, per the static-per-session decision: a changed AGENTS.md applies from the next new session). The `<tools>` section uses `loop_.advertised_specs()` (Task 2) so the prompt matches the `tools[]` param; skills come from the existing `discover_skills` (ADR 0013); the global context dir is `~/.pi/agent` (the desktop is a consumer of the user's pi setup — ADR 0012; best-effort: no home dir → no global file).

**Files:**
- Modify: `src-tauri/src/agent/session.rs` (`build_native_session`, line ~1883)
- Modify: `src-tauri/src/skills.rs` (make `home_dir()` `pub(crate)` — it is currently a private `fn home_dir() -> Option<PathBuf>` at line ~191)

**What to implement:**

**The FK fix (reviewer-corrected Critical):** `native_messages.session_id` references `sessions(id)` (`src-tauri/src/storage/db.rs` schema, `PRAGMA foreign_keys = ON` at db.rs line ~98), and the callers record the session row AFTER `build_native_session` returns (`start_native_session`: `let info = self.build_native_session(…).await?; self.record_session(&info);`). So the seq-0 persist (Task 2, inside `prepend_system`) would hit an FK violation and be silently dropped (`eprintln!`). Fix: `build_native_session` records the row BEFORE the prompt block. `self.record_session` is `fn record_session(&self, info: &SessionInfo)` returning `()` (swallows DB errors, `upsert_space`s idempotently, preserves the stored `archived` flag) — the callers' `record_session` stays (an idempotent refresh). The reorder is safe: `NativeHandle::new` only needs `prompt_tx`/`loop_.control_tx.clone()`/`cancel`/`turn_cancel` (all in scope — `AgentLoop::new` already got its clones); `handle.config_state()` just clones an `Arc` (nothing mutates it before `drive_native_session` registers the handle — reading it earlier is strictly safer); `set_loop_task` only stores the `AbortHandle` (spawn→set order preserved); `drive_native_session` works whether or not the loop task is running (a prompt sent pre-spawn just queues in the `mpsc(8)`; the handle is local until `drive`, so no prompt can arrive before spawn anyway); the `info` block's block-scoped `MutexGuard` still dies before the first await (`drive_native_session(…).await`) because the prompt block is entirely sync.

**The ONE new layout for the function tail** — replace everything from `let parent_enabled_tools = harness.enabled_tools.clone();` (session.rs ~1977) through `self.driver.drive_native_session(…).await?; Ok(info)` with EXACTLY this (every statement written in full — no `…` placeholders; the `info` block's existing text is kept verbatim in place, only its position moves, and it keeps its block scoping):

```rust
        // (existing, unchanged) the enabled_tools + thinking level setup
        let parent_enabled_tools = harness.enabled_tools.clone();
        loop_.set_enabled_tools(if parent_enabled_tools.is_empty() {
            None
        } else {
            Some(parent_enabled_tools)
        });
        if let Some(level) = &thinking_level {
            loop_.set_thinking_level(Some(level.clone()));
        }
        // (MOVED UP) the handle — the info block reads handle.config_state()
        let handle = NativeHandle::new(
            prompt_tx,
            loop_.control_tx.clone(),
            cancel,
            turn_cancel,
            model.clone(),
            thinking_level.clone(),
        );
        // (MOVED UP) the SessionInfo block — keep this block's EXISTING TEXT
        // verbatim in place (only its position moves), and keep the block
        // scoping: the MutexGuard must die before drive_native_session's await
        let info = {
            let state_guard = handle.config_state();
            let state = state_guard.lock().unwrap();
            SessionInfo {
                session_id: session_id.clone(),
                agent_id: entry.id.clone(),
                cwd: cwd.clone(),
                capabilities: native_capabilities(
                    &state.model,
                    state.thinking_level.as_deref(),
                ),
                config_options: synthesize_catalog_config_options(
                    &self.catalog,
                    &state.model,
                    state.thinking_level.as_deref(),
                ),
                // A native START mints a fresh session (ADR 0016); a
                // resume overrides `archived` with the stored row's flag
                // (`resume_native_session`).
                archived: false,
            }
        };
        // (NEW) the sessions row BEFORE the seq-0 persist (the FK)
        self.record_session(&info);
        // (NEW) the system prompt — NEW sessions only (a resume replays the
        // stored transcript verbatim — the load_transcript below restores the
        // system message at index 0; NO rebuild; the row was recorded above)
        if !is_resume {
            let agent_dir = crate::skills::home_dir()
                .map(|h| h.join(".pi/agent"))
                .unwrap_or_default();
            let skills = crate::skills::discover_skills(Some(&cwd));
            let specs = loop_.advertised_specs();
            let prompt = build_main_prompt(&PromptContext {
                cwd: &cwd,
                agent_dir: &agent_dir,
                tools: &specs,
                skills: &skills,
            });
            loop_.prepend_system(prompt);
        }
        // (existing, MOVED DOWN — from above the handle to just before the spawn)
        // the resume replay
        if is_resume {
            if let Ok(messages) = store.load_messages(&session_id) {
                loop_.load_transcript(messages);
            }
        }
        // (MOVED DOWN) spawn + the test seam
        let loop_handle = tokio::spawn(loop_.run());
        handle.set_loop_task(loop_handle.abort_handle());
        self.driver
            .drive_native_session(
                handle,
                events_rx,
                settle_rx,
                &entry.id,
                cwd,
                sink,
                info.clone(),
            )
            .await?;
        Ok(info)
```

Plus the import at the top of `session.rs: `use crate::agent::harness::{build_main_prompt, PromptContext};` (the `mod.rs` re-exports — Task 1). Do NOT touch `resume_native_session`, `drive_native_session`, the callers' `record_session`, or the `if is_resume` block's CONTENT (its position moves per the layout above; the early record is inside `build_native_session` — allowed).

**Steps:**
- [ ] Write the failing tests in `session.rs`'s `#[cfg(test)] mod tests` (setup: follow the `native_resume_carries_the_archived_flag` pattern (session.rs ~4734) — build the `SessionManager` manually: `open_db(dir)` → `attach_db(db.clone())` → `set_catalog` → `set_provider_factory` with a mock `Provider` that RECORDS the `ModelRequest`s it receives (push `req.clone()` into an `Arc<StdMutex<Vec<ModelRequest>>>` before returning the canned stream — the existing `HangingProvider`/`PacedProvider` mocks (session.rs tests ~4937/~5064) ignore `_req`, so add the recording mock; `subagent.rs`'s `CannedProvider` shows the canned-stream pattern), and KEEP the `db` (`Arc<Db>`) for the assertions; a temp Space dir — create an EMPTY `.git/` dir in it (the Task 1 scratch-dir rule: bounds the project-context walk at the Space), a native `AgentEntry` harness config; drive one turn via the existing prompt-sending test helpers. NOTE: the prompt assertions are `contains`-based precisely because the real `~/.pi/agent/` context file and discovered skills leak in via `home_dir()`/`discover_skills` (not injectable at this level) — assert on the controlled content, not the full prompt):
  1. `start_native_session_persists_system_prompt` — start a native session in a temp Space that contains an `AGENTS.md` (content `"project rules"`); trigger one prompt (the recording provider returns a canned assistant end event); assert the recorded `ModelRequest.messages[0]` is a `System` message whose text contains the PREAMBLE, a `<tools>` section with a `- read:` line, a `<project_context>` section containing `"project rules"`, and a `<cwd>` section containing the temp Space path; AND assert via the kept `db` that the session row exists AND a `native_messages` row for the session with the system content exists (the FK fix — proving the persist did NOT hit an FK violation: `db.load_native_messages(session_id)` non-empty, first entry's JSON contains `"role":"system"`).
  2. `start_native_session_prompt_respects_enabled_tools` — a native `AgentEntry` harness with `enabled_tools: ["read", "bash"]` (the rest of the harness config as the existing tests build it); assert `ModelRequest.messages[0]`'s `<tools>` section has EXACTLY the `read` + `bash` lines, NO `subagent` line, and the `<rules>` section contains NEITHER guidance line (only the two pi lines).
  3. `resume_native_session_replays_system_prompt_verbatim` — start a session in a temp Space with `AGENTS.md` = `"v1 rules"`; complete one turn (the prompt is persisted); OVERWRITE the Space's `AGENTS.md` with `"v2 rules"`; resume (the existing `resume_native_session` path — the stored row from the start); trigger one prompt; assert the recorded `ModelRequest.messages[0]` still contains `"v1 rules"` and does NOT contain `"v2 rules"` (no rebuild — the stored prompt is replayed verbatim; the Space's `.git` dir bounds the walk, so the overwrite is the only context change).
- [ ] Run `cargo test session` — did the new tests FAIL (no system message in the request today; test 1's `native_messages` assertion also fails — the FK violation / no persist today)? If they passed unexpectedly, stop and investigate.
- [ ] Implement the `build_native_session` reorder (the single layout above) + the `skills.rs` visibility change (the `home_dir()` at skills.rs line ~191 → `pub(crate) fn home_dir()`).
- [ ] Run `cargo test session` — all tests pass (existing native-session tests included — the reorder must not break them).
- [ ] Run `cargo fmt` + `cargo clippy --all-targets` — 0 warnings.
- [ ] Commit with message: `feat(harness): wire the system prompt into native sessions (build on start, replay on resume)`

**Acceptance criteria:**
- [ ] A new native session's first model request carries the system message at `messages[0]` (test 1), with project context + skills + cwd sections per the Task 1 format, AND the seq-0 `native_messages` row exists (FK fix verified in test 1).
- [ ] `enabled_tools` restricts the `<tools>` section AND the conditional guidance lines (test 2).
- [ ] A resume replays the stored prompt verbatim — a changed `AGENTS.md` does NOT leak into the resumed session's prompt (test 3).
- [ ] All pre-existing `session.rs` tests pass (the reorder is behavior-preserving).

---

### Task 4: subagent-child wiring — `dispatch_native_inner`

**Context:**
Replace the child's `if let Some(sp) = &launch.system_prompt { loop_.prepend_system(sp.clone()); }` (the seed at step 7 of `dispatch_native_inner`, line ~1008) with the ADR 0017 child message: `[launch.systemPrompt, if any]` + the `TODO_GUIDANCE` line when the child has the `manage_todo_tool` (its `child_tools` = the parent's minus `subagent`); `None` → no system message (today's behavior). The persist (Task 2, inside `prepend_system`) writes to the child's THROWAWAY `Db` — the child is ephemeral (never resumed; the throwaway Db is dropped at teardown), so the persist is the uniform code path, not a resume record. `agentName` is UNCHANGED — it stays a free-form UI label (no lookup is added anywhere; ADR 0017).

**Files:**
- Modify: `src-tauri/src/agent/subagent.rs` (`dispatch_native_inner`)

**What to implement:**

Replace:

```rust
if let Some(sp) = &launch.system_prompt {
    loop_.prepend_system(sp.clone());
}
```

with:

```rust
// The child's system message (ADR 0017): [launch.systemPrompt, if any]
// + the todo guidance line (when the child has the manage_todo_list
// tool — its tools = the parent's minus subagent); `None` → no system
// message (today's behavior). `prepend_system` persists it at seq 0
// (the child's throwaway Db — the child is ephemeral; the persist is
// the uniform code path, not a resume record).
let has_todo_tool = child_tools.iter().any(|t| t == "manage_todo_list");
if let Some(msg) = build_child_system_message(
    launch.system_prompt.as_deref(),
    has_todo_tool,
) {
    loop_.prepend_system(msg);
}
```

(`child_tools` is already computed a few lines above — the `let child_tools: Vec<String> = match &launch.tools { … }` at line ~993 — and `loop_.set_enabled_tools(Some(child_tools.clone()))` already uses it. Add the `build_child_system_message` import: `use crate::agent::harness::build_child_system_message;`.) Also update the step-10 comment two blocks below: `// 10. The task (the preflight — …). The `system_prompt` was already seeded in step 7 (BEFORE the task prompt).` → `// 10. The task (the preflight — …). The system message was already seeded in step 7 (BEFORE the task prompt).`

**Steps:**
- [ ] Write the failing tests in `subagent.rs`'s `#[cfg(test)] mod tests` (setup: the existing `dispatch_native` test pattern (tests E/F, subagent.rs ~2343+) — `make_native_manager(&config_dir, provider, settle_timeout)` + `native_rec_sink()` + `manager.dispatch_native_force_temp_file("parent-1", &config_dir, &native_test_model(), <parent_enabled_tools>, "tester", …)`; the parent's tools are the `parent_enabled_tools: Vec<String>` ARGUMENT (`Vec::new()` = all), NOT an `AgentLoop` field. The existing providers (`HangingProvider`, `CannedProvider`) ignore `_req` — add a NEW recording provider: a `CannedProvider` that pushes `req.clone()` into an `Arc<StdMutex<Vec<ModelRequest>>>` before returning the canned stream, so the test asserts on the child's `ModelRequest.messages`):
  1. `native_child_system_prompt_plus_todo_line` — a dispatch with `launch.system_prompt = Some("You are a careful reviewer.")`, `parent_enabled_tools = Vec::new()` (= all → the child HAS `manage_todo_list`); assert the child's recorded `ModelRequest.messages[0]` is a `System` message with EXACTLY `"You are a careful reviewer.\nUse manage_todo_list to track multi-step work — write the plan before starting, mark items completed as you go"`.
  2. `native_child_system_prompt_alone_without_todo_tool` — same dispatch but `parent_enabled_tools = vec!["read".into(), "bash".into()]` (the child's tools = those minus `subagent` → NO `manage_todo_list`); assert `messages[0]` is a `System` message with EXACTLY `"You are a careful reviewer."` (no todo line).
  3. `native_child_todo_line_only_when_no_system_prompt` (the DEFAULT dispatch — `launch.system_prompt = None`, `parent_enabled_tools = Vec::new()` = all → the child HAS `manage_todo_list`): assert `messages[0]` is a `System` message with EXACTLY the `TODO_GUIDANCE` line alone (the `has_todo_tool` derivation from `child_tools` in the `None` case — the common default).
  4. `native_child_no_system_message_when_neither` — a dispatch with `launch.system_prompt = None` and `parent_enabled_tools = vec!["read".into(), "bash".into()]` (no todo tool); assert `messages[0]` is the TASK `User` message (NO system message — today's behavior preserved). (The call sketch passes `"tester".to_string()` for `agent_name`, as the existing tests do; `child_tools` is computed at line ~982, before the seed at ~1008.)
- [ ] Run `cargo test subagent` — expected result per test (today's behavior vs the new assertion): test 1 FAILS (today a `Some`-prompt child gets only the `systemPrompt` — no todo line); test 2 PASSES (today's `prepend_system(sp)` already yields `sp` alone); test 3 FAILS (today a `None`-prompt child gets NO system message — the new behavior adds the todo line); test 4 PASSES (today's no-system-message behavior is preserved). If test 1 or 3 passed, or test 2 or 4 failed, stop and investigate.
- [ ] Implement the replacement.
- [ ] Run `cargo test subagent` — all tests pass. NOTE on the existing tests: the `system_prompt: Some(…)` cases in the file (subagent.rs ~1556/1584) are `subagent_pi_args` unit tests for the EXTERNAL pi CLI path (line ~252) — UNCHANGED by this task; NO existing `dispatch_native` test uses `system_prompt: Some`, so the new tests are the first dispatch-level coverage of the child system message.
- [ ] Run `cargo fmt` + `cargo clippy --all-targets` — 0 warnings.
- [ ] Run the FULL validation suite (the AGENTS.md gate): `cargo test` (src-tauri) · `cargo clippy --all-targets` · `cargo fmt --check` · `pnpm test` + `pnpm build` (repo root).
- [ ] Commit with message: `feat(harness): wire the child system message into native subagent dispatch`

**Acceptance criteria:**
- [ ] A child with a `launch.systemPrompt` + the todo tool gets exactly the two-line message (test 1); without the todo tool, the `systemPrompt` verbatim (test 2).
- [ ] The default dispatch (`None` prompt, full tools) gets the one-line todo message (test 3); `None` + no todo tool gets no system message (test 4).
- [ ] The full AGENTS.md validation suite is green (this is the last task — the branch is merge-ready when it is).
