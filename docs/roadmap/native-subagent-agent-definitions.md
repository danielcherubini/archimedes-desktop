---
status: committed
done-when: In a native session, `subagent({ task, agentName })` resolves `agentName` against a discovered Agent definition and the child runs with its frontmatter model/thinking/tools + system-prompt body (layered under explicit params); `list_agents` lists the discovered definitions; a user's existing `~/.pi/agent/agents/*.md` files resolve with zero config; ADR 0020 + CONTEXT.md committed.
---

# Native subagent Agent-definition resolution — Plan

**Goal:** The native harness resolves `subagent`'s `agentName` against user-authored Agent definitions (markdown + frontmatter, discovered like skills) and applies the frontmatter to the child; a `list_agents` tool advertises the definitions.
**Architecture:** A new pure `src-tauri/src/agents.rs` discovery module (mirror of `skills.rs`: total, hand-rolled frontmatter parser, first-wins dedupe); resolution in the native `AgentLoop::dispatch_subagent` (layered `LaunchConfig`: explicit params > frontmatter > parent defaults); a `list_agents` `ToolSpec` + handler + system-prompt rule addendum; the child tool filter extended to also strip `list_agents`.
**Tech Stack:** Rust (Tauri 2 backend), std fs only (no new crates), existing `ModelCatalog` / `Provider` / `AgentLoop` machinery.

**Reference spec:** the approved design is in git history (commit `65cc82f`, `docs: add native-subagent-agent-definitions spec`) — this plan replaces it in the same file.

**Conventions (all tasks):** Rust backend lives in `src-tauri/`. Validation from `src-tauri/`: `cargo test`, `cargo clippy --all-targets` (0 warnings), `cargo fmt --check`. TDD: failing test first, confirm it fails, then implement. Each task is independently commitable.

---

### Task 1: The `agents.rs` discovery module

**Context:**
The native harness has no way to find Agent definition files. This task creates the discovery layer: a pure, total module (fs reads only — never panics, never errors; a missing root, unreadable file, or malformed frontmatter is silently skipped) that finds flat `*.md` files in a bounded set of roots and parses their YAML frontmatter. It mirrors `src-tauri/src/skills.rs` (ADR 0013) structurally: the same `space_roots` walk-up, the same `home_dir` reuse, the same first-wins dedupe and deterministic sort — but with a flat file layout (pi's agent layout: `*.md` directly in the root, no subdir walk) and a 5-key frontmatter shape.

**Files:**
- Create: `src-tauri/src/agents.rs`
- Modify: `src-tauri/src/lib.rs` (add `mod agents;` — follow the existing `mod skills;` line's placement)
- Modify: `src-tauri/src/test_support.rs` (add `pub static ENV_LOCK: std::sync::Mutex<()>` — the shared lock serializing the HOME-mutating tests in Tasks 1–3; `Mutex` is std, no new crate)

**What to implement:**

In `src-tauri/src/agents.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentScope {
    Space,
    User,
}

impl std::fmt::Display for AgentScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            AgentScope::Space => "space",
            AgentScope::User => "user",
        })
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentDefinition {
    /// Frontmatter `name`, else the file's STEM name (`scout.md` → `scout`).
    pub name: String,
    /// Frontmatter `description`, else `""`.
    pub description: String,
    /// Frontmatter `model` (`"provider/id"` or `"provider/id:<level>"`), verbatim.
    pub model: Option<String>,
    /// Frontmatter `thinking` (a native extension beyond pi's format).
    pub thinking: Option<String>,
    /// Frontmatter `tools` (both the `read, bash` string and the `[read, bash]`
    /// array spellings normalize to this).
    pub tools: Option<Vec<String>>,
    /// The file body (minus the frontmatter), trimmed. `""` when the body is
    /// empty (no system prompt).
    pub system_prompt: String,
    pub scope: AgentScope,
    /// Absolute (canonical) path to the file.
    pub path: String,
}

/// Discover Agent definitions for `space_path` (its roots + the user-level
/// roots). `space_path: None` → user-level definitions only.
pub fn discover_agents(space_path: Option<&Path>) -> Vec<AgentDefinition>

/// The core, testable without env manipulation: discover over EXPLICIT roots.
/// `roots` order defines first-wins dedupe precedence.
pub fn discover_in_roots(roots: &[(PathBuf, AgentScope)]) -> Vec<AgentDefinition>

/// Walk UP from `space_path` to the repository root: the first ancestor
/// (including `space_path` itself) that contains a `.git` entry is the repo
/// root; if none exists, `space_path` alone is the only level. At each level
/// (innermost first) collect `<level>/.agents/agents` (first) and
/// `<level>/.pi/agents` (second), both tagged `AgentScope::Space`.
/// Mirror `skills::space_roots` verbatim (the walk logic), swapping only the
/// two dir names. Do NOT canonicalize the input before walking.
pub fn space_roots(space_path: &Path) -> Vec<(PathBuf, AgentScope)>

/// `home_dir().join(".agents/agents")` (first) and
/// `home_dir().join(".pi/agent/agents")` (second), both tagged
/// `AgentScope::User`. `home_dir()`: REUSE `crate::skills::home_dir`
/// (it is `pub(crate)`) — do NOT duplicate it.
pub fn user_roots() -> Vec<(PathBuf, AgentScope)>
```

`discover_agents`: build `roots` = `space_roots(p)` (when `Some`) then `user_roots()`, call `discover_in_roots` (the skills.rs pattern).

`discover_in_roots` — the core:
- `seen: HashSet<PathBuf>` shared across ALL roots (a symlink in a later root pointing at an earlier root's file collapses by canonical path, like skills).
- For each `(root, scope)` in order: `if !root.is_dir() { continue; }`; `fs::read_dir(&root)` (error → continue); collect the qualifying entries (`entry.file_name().to_string_lossy().ends_with(".md")` AND `(entry.file_type().is_file() || entry.file_type().is_symlink())`), **sort them by file name** (deterministic first-wins even for a same-root case-colliding pair like `a.md` + `A.md` — `read_dir` order is OS-dependent, and the final `sort_by_cached_key` sorts the OUTPUT, not the insertion order), then for each entry in sorted order: `fs::canonicalize` (error → skip); `if !seen.insert(canon) { continue; }`; `fs::read_to_string` (error → skip); `let stem = file_name with the `.md` suffix stripped`; `parse_agent_file(&content, &stem)` (→ `None` = skip); **first-wins by lowercased name** (a same-named later root is ignored — a Space root shadows a user root; an innermost ancestor shadows an outer one); insert with `scope` + `path = canonical path (lossy)`.
- Return `sort_by_cached_key(|d| (d.name.to_lowercase(), d.path.clone()))` (the skills.rs single-allocation-per-element pattern).

`parse_agent_file(content: &str, fallback_name: &str) -> Option<(String, String, Option<String>, Option<String>, Option<Vec<String>>, String)>` (returns `(name, description, model, thinking, tools, body)`):
- Normalize `\r\n` and lone `\r` to `\n` (skills.rs step 1).
- Line 1 not exactly `---` → no frontmatter: `(fallback_name, "", None, None, None, normalized.trim())`.
- Otherwise: the closing `---` line is REQUIRED — EOF before it → `None` (malformed).
- Top-level (non-indented) keys only, each parsed with a **copy of skills.rs's `parse_value`** (the tolerant value reader: inline value trimmed + one pair of surrounding quotes stripped; block values `|`/`>`/`|-`/`|+`/`>-`/`>+` = following indented lines, dedented, joined with `\n` (`|`) or a single space (`>`), trimmed; an unquoted inline value containing `: ` → `None`). Copy the function verbatim from `skills.rs` into `agents.rs` (it is private there; do NOT change `skills.rs`'s visibility — the two frontmatter shapes differ, so the modules stay independent). Keys: `name`, `description`, `model`, `thinking`, `tools` (first occurrence of each wins, like skills).
- `name`: empty/whitespace-only after parsing → `fallback_name` (the stem). NO tag-safety skip (unlike skills: an agent name is never interpolated unescaped into a prompt tag — it appears only in the `list_agents` tool RESULT, which the wire JSON-escapes).
- `description`: left as parsed, default `""`. NO 1024-char cap (unlike skills: a description is not prompt-injected — it appears only in `list_agents` output on demand).
- `model` / `thinking`: empty after parsing → `None`.
- `tools`: take the parsed raw string; `trim()`; if it starts with `[` AND ends with `]` → strip both brackets and split on `,`; else split on `,` directly; trim each element; drop empty elements; if the result is empty → `None`.
- `body`: the content after the closing `---`, trimmed (skills.rs step 6).
- Any other failure mode → `None` (total — never a crash).

`#[cfg(test)] mod tests` (helpers: copy the `scratch()` / `write_file` / `canon` pattern from `prompt.rs`'s test module — a scratch root with an EMPTY `.git/` dir so the context walk is bounded; `canon` for path comparisons). Tests:
1. `a_flat_agent_file_is_discovered_with_all_fields` — a root with `scout.md` (frontmatter `name: scout`, `description: Fast recon.`, `model: fake/m2`, `thinking: low`, `tools: read, bash`, body `You are a scout.`) → one `AgentDefinition` with every field populated, `scope` as tagged, `path` canonical.
2. `a_file_without_frontmatter_uses_the_stem_name` — `worker.md` with body only → `name: "worker"`, `description: ""`, `system_prompt` = the trimmed body.
3. `a_missing_name_key_falls_back_to_the_stem` — frontmatter with `description` but no `name`.
4. `a_malformed_frontmatter_is_skipped` — an opening `---` with no closing `---` → the file is skipped (no entry, no panic).
5. `tools_accepts_string_and_array_spellings` — two files: `tools: read, bash` and `tools: [read, bash]` → both `Some(vec!["read", "bash"])`.
6. `tools_empty_list_is_none` — `tools: []` (and `tools: ""`) → `None`.
7. `a_missing_root_yields_an_empty_vec` — `discover_in_roots(&[(nonexistent, User)])` → `vec![]`.
8. `a_non_md_file_is_ignored` — `notes.txt` in the root → no entry.
9. `first_wins_dedupe_space_shadows_user` — the same name in a Space-scope root and a User-scope root (Space root FIRST in the slice) → the Space one wins.
10. `innermost_level_shadows_outer` — `space_roots` over a scratch with `root/.git/` + `root/proj/` (the space) + `root/.agents/agents/a.md` + `root/proj/.agents/agents/a.md` (different bodies) → `discover_agents(Some(proj))` CONTAINS an entry named `a` whose `body` is the `proj`-level one and `scope` is `Space` (assert by FINDING the entry by name + asserting its body/scope — NOT exact-vec equality: the user-level roots may contribute unrelated entries on a developer machine, and the `ENV_LOCK`+HOME isolation below is not applied here since a find-by-name assertion is immune to extra entries).
11. `a_symlink_collapses_across_roots` — root B contains a symlink to root A's file (A first) → one entry, A's `path`.
12. `a_model_with_a_level_suffix_is_preserved_verbatim` — `model: fake/m2:high` → `Some("fake/m2:high")` (the suffix is NOT stripped at discovery — it is a thinking-level candidate handled downstream).
13. `output_is_deterministically_sorted` — two files → sorted by lowercased name.
14. `user_roots_reads_home` — set `HOME` to a scratch dir via `unsafe { std::env::set_var("HOME", tmp) }` (Rust ≥1.81: `set_var` is `unsafe` — call it inside an `unsafe` block), assert the two exact roots, then restore the ORIGINAL `HOME` value: capture it FIRST with `std::env::var_os("HOME")`; if it was `Some(v)` restore via `unsafe { std::env::set_var("HOME", v) }`, if it was `None` restore via `unsafe { std::env::remove_var("HOME") }`; do BOTH in a drop guard (a scope-exit struct implementing `Drop`) so the restore happens even when an assertion panics mid-test; hold `crate::test_support::ENV_LOCK` (a `static Mutex<()>`) for the WHOLE set→assert→restore span so the parallel test harness cannot interleave another HOME-reading test (a `cargo test --lib` runs all module tests concurrently).
15. `space_roots_walks_up_to_the_repo_root` — scratch with `.git` at the root, space at `root/a/b/` → levels `root/a/b`, `root/a`, `root` (innermost first), each contributing `.agents/agents` then `.pi/agents`.

**Steps:**
- [ ] Write the failing tests 1–15 in `src-tauri/src/agents.rs`'s `#[cfg(test)]` module (the module exists with the function STUBS returning `Vec::new()` / `None` so the tests compile and fail)
- [ ] Run `cargo test --lib agents::` from `src-tauri/`
  - Did tests 1–6 and 8–15 fail? Test 7 (`a_missing_root_yields_an_empty_vec`) PASSES against the `Vec::new()` stub by construction (the expected empty vec is exactly what the stub returns) — that is EXPECTED, not a signal to stop. If any OTHER test passed unexpectedly, stop and investigate why.
- [ ] Implement `discover_in_roots` + `parse_agent_file` (+ the copied `parse_value`) in `src-tauri/src/agents.rs`
- [ ] Implement `space_roots` / `user_roots` / `discover_agents` (mirroring `skills.rs`)
- [ ] Add `mod agents;` to `src-tauri/src/lib.rs`
- [ ] Run `cargo test --lib agents::` from `src-tauri/`
  - Did all 15 tests pass? If not, fix the failures and re-run before continuing.
- [ ] Run `cargo clippy --all-targets` from `src-tauri/`
  - 0 warnings? If not, fix and re-run.
- [ ] Run `cargo fmt` from `src-tauri/`
- [ ] Commit with message: "feat(agents): agent-definition discovery module (ADR 0020, Task 1)"

**Acceptance criteria:**
- [ ] `discover_agents` is pure + total: all 15 tests pass; a missing root / unreadable file / malformed frontmatter never panics or errors
- [ ] `cargo clippy --all-targets` is warning-free; `cargo fmt --check` passes
- [ ] `skills.rs` is UNCHANGED (the `parse_value` is a copy, not a refactor)

---

### Task 2: The `list_agents` harness tool + child tool filter

**Context:**
The parent model needs to DISCOVER which Agent definitions exist (it cannot guess names). This task adds the `list_agents` tool to the native harness (a no-arg tool whose result is one line per discovered definition), advertises it in the system prompt's `<rules>` via the existing `SUBAGENT_GUIDANCE` line, and extends the subagent-child tool filter so a child (which cannot dispatch) never receives `list_agents` either. The Tauri IPC command `list_agents` in `src-tauri/src/commands/spaces.rs:23` (the process registry) is a DIFFERENT thing and is NOT touched.

**Files:**
- Modify: `src-tauri/src/agent/harness/loop.rs` (the `tool_specs()` spec, the `dispatch_tool` match arm, the `list_agents_tool` handler, the `advertised_tool_specs` child filter, the `subagent` spec's description)
- Modify: `src-tauri/src/agent/harness/prompt.rs` (the `SUBAGENT_GUIDANCE` constant)
- Modify: `src-tauri/src/agent/subagent.rs` (the `dispatch_native_inner` child-tools filter — 3 sites)

**What to implement:**

In `loop.rs`:

1. In `tool_specs()` (line ~1567, after the `subagent` spec at ~1712), add:

```rust
ToolSpec {
    name: "list_agents".into(),
    description: "List available subagent configurations (name, description, source, model/tools overrides). Call before dispatching if unsure which agents exist or which fits the task.".into(),
    parameters: json!({ "type": "object" }),
},
```

2. In `dispatch_tool`'s `match tc.name.as_str()` (line ~1022), add the arm (before the `"mcp"` arm):

```rust
"list_agents" => self.list_agents_tool().await,
```

3. New private method on `AgentLoop` (near `dispatch_subagent`):

```rust
/// The `list_agents` tool result: one line per discovered Agent definition
/// (ADR 0020), or `"none"`.
async fn list_agents_tool(&self) -> ToolResult {
    let agents = crate::agents::discover_agents(Some(&self.space_cwd));
    let text = if agents.is_empty() {
        "none".to_string()
    } else {
        agents
            .iter()
            .map(|a| {
                let mut line = format!("{} ({}): {}", a.name, a.scope, a.description);
                let mut notes: Vec<String> = Vec::new();
                if let Some(m) = &a.model {
                    notes.push(format!("model: {m}"));
                }
                if let Some(t) = &a.thinking {
                    notes.push(format!("thinking: {t}"));
                }
                if let Some(t) = &a.tools {
                    notes.push(format!("tools: {}", t.join(", ")));
                }
                if !notes.is_empty() {
                    line.push_str(&format!(" [{}]", notes.join(", ")));
                }
                line
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    ToolResult {
        content: vec![ContentBlock::Text { text }],
        details: None,
        is_error: false,
    }
}
```

4. In `advertised_tool_specs` (line ~1377), the child filter currently drops `subagent` only:

```rust
let not_child_subagent = !is_child || spec.name != "subagent";
```

Change it to also drop `list_agents` (a child cannot dispatch, so listing dispatch targets is pointless token burn):

```rust
let not_child = !is_child || (spec.name != "subagent" && spec.name != "list_agents");
```

and use `not_child` in the filter (rename the variable; update the surrounding comment to "dropping `subagent` AND `list_agents` when the session is a subagent child").

5. In the `subagent` `ToolSpec` (line ~1712), replace the description with:

```rust
description: "Delegate tasks to subagents. `task` is required. Optional: `agentName` (a discovered Agent definition — its frontmatter model/thinking/tools + system-prompt body apply, layered under any explicit params), `model`, `systemPrompt`, `tools`. Omit `agentName` for a config-less dispatch (parent model, all tools, no system prompt). Model override is rarely needed.".into(),
```

In `prompt.rs` (line 17), replace the constant:

```rust
const SUBAGENT_GUIDANCE: &str = "Delegate independent subtasks with subagent — give each a systemPrompt describing its role and constraints (e.g. a read-only researcher, a focused reviewer), or a discovered agent (list_agents)";
```

(The existing golden tests in `prompt.rs` interpolate `{SUBAGENT_GUIDANCE}` into their golden strings (lines ~381, ~477), so they track the constant change automatically; the `!prompt.contains("Delegate independent subtasks")` absence-assertions (prompt.rs ~447 and session.rs ~7095 — session.rs does NOT interpolate the constant, it only asserts the absence of that PREFIX) still hold as long as the new text keeps the `Delegate independent subtasks` prefix — it does. Do NOT rewrite that prefix in future edits.)

In `subagent.rs` — `dispatch_native_inner` (line ~984), the child-tools `match &launch.tools` has THREE filter sites, each currently `!= "subagent"`. Change ALL THREE to exclude both:

```rust
Some(tools) => tools
    .iter()
    .filter(|t| !matches!(t.as_str(), "subagent" | "list_agents"))
    .cloned()
    .collect(),
None => {
    if parent_enabled_tools.is_empty() {
        tool_specs()
            .into_iter()
            .map(|t| t.name)
            .filter(|t| !matches!(t.as_str(), "subagent" | "list_agents"))
            .collect()
    } else {
        parent_enabled_tools
            .iter()
            .filter(|t| !matches!(t.as_str(), "subagent" | "list_agents"))
            .cloned()
            .collect()
    }
}
```

Update the step-7 comment above the `match` ("MINUS `subagent` (the recursion guard)") to "MINUS `subagent` AND `list_agents` (the recursion guard — a child cannot dispatch, so it gets neither)".

Tests:

In `loop.rs`'s `mod tests`:
- `list_agents_is_in_the_default_tool_specs` — `tool_specs()` contains a spec named `list_agents` with the new description; `advertised_tool_specs(&None, false)` contains it.
- `a_child_does_not_advertise_subagent_or_list_agents` — `advertised_tool_specs(&None, true)` contains neither `subagent` nor `list_agents`, AND as a CONTRAST assertion `advertised_tool_specs(&None, false)` contains BOTH (without the contrast the test passes VACUOUSLY pre-implementation — `list_agents` is not in `tool_specs()` yet, so "contains neither" is trivially true; the contrast makes the missing spec fail the test). A new dedicated test — do NOT weaken the existing `tool_specs_advertise_exactly_the_executor_param_keys` test at line ~3437.
- `list_agents_tool_lists_discovered_agents` — build a loop with `build_loop` (its `space_cwd` is the temp dir); `write_file(&loop.space_cwd.join(".agents/agents/scout.md"), "---\nname: scout\ndescription: Fast recon.\nmodel: fake/m2\ntools: [read, bash]\n---\nYou are a scout.\n")`; **isolate the user-level roots first** — `discover_agents` includes `user_roots()`, and a developer's real `~/.agents/agents` / `~/.pi/agent/agents` files would add lines and break the exact assertion: hold `crate::test_support::ENV_LOCK`, capture the original `HOME` (`var_os`), `unsafe { std::env::set_var("HOME", empty_scratch) }` in a drop guard (the Task 1 test-14 pattern); call `loop_.list_agents_tool().await`; assert the text is exactly `scout (space): Fast recon. [model: fake/m2, tools: read, bash]`.
- `list_agents_tool_returns_none_when_empty` — same loop, no agent files written, same `ENV_LOCK`+`HOME`-to-empty-scratch isolation (restore via the drop guard) → text is exactly `none`.
- **Stub for the TDD gate:** before the tests, add a STUB `async fn list_agents_tool(&self) -> ToolResult { ToolResult { content: vec![ContentBlock::Text { text: "stub".to_string() }], details: None, is_error: false } }` so the two handler tests COMPILE and fail (calling a nonexistent method is a compile error for the whole test binary — unlike Task 3's `resolve_launch`, the `list_agents_tool` tests need an explicit stub to "fail").

In `prompt.rs`'s `mod tests`:
- `the_subagent_guidance_mentions_list_agents` — a prompt built with a `subagent` spec advertised contains `or a discovered agent (list_agents)`.

In `subagent.rs`'s `mod tests`:
- `a_native_child_excludes_subagent_and_list_agents_tools` — `make_native_manager` (the existing helper, line ~2262 — it calls `SubagentSessionManager::new(config_dir.to_path_buf(), None).expect("subagent manager should build")`; mirror it verbatim when copying) + `dispatch_native_force_temp_file` (or the plain `dispatch_native` — follow the existing native dispatch tests' pattern) with `parent_enabled_tools = vec!["read", "list_agents"]` and a `default_native_launch()`; the `subagent-session-started` payload is captured by subagent.rs's existing `TestSink` (verified: its `emit` records EVERY `(event, payload)` pair — use it with a LIVE `std::sync::mpsc` channel: `let (tx, rx) = std::sync::mpsc::channel(); let sink: Arc<dyn EventSink> = Arc::new(TestSink { tx });` — do NOT reuse the `native_rec_sink()` helper, which builds `let (tx, _rx) = …channel()` and DROPS the receiver, and do NOT add a redundant `RecordingSink` duplicate); **await the dispatch outcome BEFORE asserting** — `dispatch_native` returns immediately and the `subagent-session-started` emit happens inside the `tokio::spawn`ed driver task, so `rx.try_iter()` called immediately can observe an empty channel: `let _ = tokio::time::timeout(Duration::from_secs(10), dispatch_rx).await;` (the existing native dispatch tests' pattern, e.g. the temp-file cleanup test at line ~2397 — the emit happens-before the outcome resolves, so the frame is guaranteed present after the await), THEN iterate `rx.try_iter()` for the `("subagent-session-started", payload)` frame and assert `payload["enabledTools"] == ["read"]`.

**Steps:**
- [ ] Write the 6 failing tests above (the `list_agents_tool` tests compile against the `"stub"` stub and fail on the stub text; `list_agents_is_in_the_default_tool_specs` fails on the missing spec; `a_child_does_not_advertise_subagent_or_list_agents` fails on the contrast assertion — the parent side expects `list_agents` present; the prompt.rs test fails on the missing addendum; the subagent.rs test fails because `list_agents` is still in the child's tools)
- [ ] Run `cargo test --lib` from `src-tauri/`
  - Did the new tests fail for the expected reasons (NOT compile errors — the `list_agents_tool` stub exists; NOT vacuous passes — the contrast assertion is in place)? If any passed unexpectedly, stop and investigate why.
- [ ] Implement items 1–5 in `loop.rs` (REPLACE the `"stub"` `list_agents_tool` stub with the real handler) + the `prompt.rs` constant + the 3 `subagent.rs` filter sites
- [ ] Run `cargo test --lib` from `src-tauri/`
  - Did ALL tests pass (the 6 new + the existing golden tests, which track the `SUBAGENT_GUIDANCE` change — the prompt.rs goldens auto-track via interpolation; the session.rs:~7095 absence-prefix assertion holds because the new text keeps the `Delegate independent subtasks` prefix)? If not, fix the failures and re-run before continuing.
- [ ] Run `cargo clippy --all-targets` from `src-tauri/`
  - 0 warnings? If not, fix and re-run.
- [ ] Run `cargo fmt` from `src-tauri/`
- [ ] Commit with message: "feat(harness): list_agents tool + child tool filter (ADR 0020, Task 2)"

**Acceptance criteria:**
- [ ] `list_agents` is advertised to a native parent, absent from a subagent child (both the `tools[]` API param and the prompt's `<tools>` section), and its handler output matches the spec'd format exactly
- [ ] The `subagent` ToolSpec description is the new text; `SUBAGENT_GUIDANCE` ends with `or a discovered agent (list_agents)`
- [ ] The `subagent-session-started` payload's `enabledTools` never contains `subagent` or `list_agents` for a native child
- [ ] `commands/spaces.rs`'s Tauri `list_agents` command is UNCHANGED

---

### Task 3: `agentName` resolution in `dispatch_subagent`

**Context:**
Tasks 1–2 make definitions discoverable and listable; this task makes the `subagent` dispatch USE them. When the tool call's `agentName` matches a discovered definition (case-insensitive exact match), the frontmatter is layered UNDER the explicit launch params (the approved precedence: explicit > frontmatter > parent defaults) and the child is built from the layered `LaunchConfig` through the UNCHANGED `dispatch_native` machinery (catalog model resolution with the `:<level>` suffix handling, thinking validation, tools filtering, the `subagent-session-started` payload). Two degrade rules keep stale files from breaking dispatches: a frontmatter `model` that resolves to nothing in the catalog is dropped (falling through to the explicit param, else the parent model — an explicit `model` param that is unknown still FAILS the dispatch, unchanged); unknown tool names in a frontmatter `tools` list are dropped, and a list that empties out is treated as absent (the child is never zeroed out by a stale file).

**Files:**
- Modify: `src-tauri/src/agent/harness/loop.rs` (the `resolve_launch` method, the `dispatch_subagent` call site, the `LaunchConfig` import, tests)
- Modify: `src-tauri/src/agent/subagent.rs` (ONE change: add `#[derive(Clone, Debug, PartialEq)]` to `pub struct LaunchConfig` at line ~79 — the struct currently has NO derives, so `resolve_launch`'s `launch.clone()` and the tests' `assert_eq!` on `LaunchConfig` values would not compile; `NativeDeps` adjacent already derives `Clone`, so this is consistent)

**What to implement:**

In `loop.rs`, add a private method on `AgentLoop` (near `dispatch_subagent`):

(Prerequisite — the `LaunchConfig` type is NOT currently named in `loop.rs`: line 50 is `use crate::agent::subagent::SubagentSessionManager;` and `dispatch_subagent` only ever *binds* the `dispatch_params` result by inference. Change line 50 to `use crate::agent::subagent::{LaunchConfig, SubagentSessionManager};` — without it the signature below does not resolve.)

```rust
/// Resolve `agentName` against the discovered Agent definitions and layer
/// the frontmatter UNDER the explicit launch params (ADR 0020 — explicit >
/// frontmatter > parent defaults). `agentName` empty / no match → the
/// `launch` is returned VERBATIM (the label-only, config-less behavior —
/// never an error).
fn resolve_launch(&self, launch: &LaunchConfig, agent_name: &str) -> LaunchConfig {
    let name = agent_name.trim();
    if name.is_empty() {
        return launch.clone();
    }
    let Some(def) = crate::agents::discover_agents(Some(&self.space_cwd))
        .into_iter()
        .find(|d| d.name.eq_ignore_ascii_case(name))
    else {
        return launch.clone();
    };
    // A frontmatter `model` that resolves to NOTHING (not in the catalog —
    // a stale file) degrades to the next layer (the explicit param, else the
    // parent model) — a stale file must not fail the dispatch. The `:<level>`
    // suffix is stripped ONLY for the resolvability check (mirroring
    // `dispatch_native_inner`'s `rsplit_once(':')`); the stored value is the
    // VERBATIM frontmatter string (the suffix is a thinking-level candidate
    // handled downstream). An explicit `model` param is NEVER degraded here
    // (it is the model's current intent — `dispatch_native` still fails it
    // when unknown, unchanged).
    let model = launch.model.clone().or_else(|| {
        def.model.as_ref().and_then(|m| {
            let bare = m
                .rsplit_once(':')
                .map(|(b, _)| b.to_string())
                .unwrap_or_else(|| m.clone());
            crate::agent::session::resolve_composed_model(&self.catalog, &bare)
                .is_some()
                .then_some(m.clone())
        })
    });
    // Unknown tool names in the frontmatter are DROPPED (a file hint, not
    // precise intent); a list that empties out is treated as ABSENT (the
    // child is never zeroed out by a stale file). An explicit `tools` param
    // is NEVER filtered here (its semantics — verbatim, `dispatch_native`
    // minus `subagent`/`list_agents` — are unchanged). `then_some` (NOT
    // `then`) — the codebase precedent is `bridge.rs:1393`.
    let tools = launch.tools.clone().or_else(|| {
        def.tools.as_ref().and_then(|t| {
            let known: Vec<String> = t
                .iter()
                .filter(|name| tool_specs().iter().any(|s| s.name == **name))
                .cloned()
                .collect();
            (!known.is_empty()).then_some(known)
        })
    });
    // An EMPTY frontmatter body means "no system prompt" (the `or_else`
    // must not turn it into `Some("")` — `dispatch_native` would prepend an
    // empty message).
    let system_prompt = launch
        .system_prompt
        .clone()
        .or_else(|| (!def.system_prompt.is_empty()).then(|| def.system_prompt.clone()));
    let thinking = launch.thinking.clone().or_else(|| def.thinking.clone());
    LaunchConfig {
        system_prompt,
        model,
        thinking,
        tools,
    }
}
```

(`resolve_composed_model` is `pub(crate)` in `session.rs:3171` — `crate::agent::session::resolve_composed_model`. `LaunchConfig` is imported per the prerequisite above — line 50 becomes `use crate::agent::subagent::{LaunchConfig, SubagentSessionManager};` — and the `#[derive(Clone, Debug, PartialEq)]` on `LaunchConfig` (subagent.rs) is what makes `launch.clone()` + the tests' `assert_eq!` compile.)

In `dispatch_subagent` (line ~1084), between the `agent_name` extraction and the `manager.dispatch_native(...)` call, insert:

```rust
// ADR 0020: resolve `agentName` against the discovered Agent definitions
// (layered — explicit params win; no match / empty = config-less, never an
// error).
let launch = self.resolve_launch(&launch, &agent_name);
```

Do NOT change anything else in `dispatch_subagent` (the `agent_name` still rides along as the UI label; `dispatch_native` is untouched).

Tests (in `loop.rs`'s `mod tests`). The unit tests build a loop with `build_loop` (its `space_cwd` temp dir is the Space root — write agent files to `space_cwd/.agents/agents/`; the catalog is the single `fake/m1` model `build_loop_with_db` builds, so a frontmatter `model: fake/m2` is STALE by construction — exactly the degrade case) and call `resolve_launch` directly:

1. `resolve_launch_no_agent_name_returns_the_launch_verbatim` — `agent_name: ""` → the returned `LaunchConfig` equals the input (all fields).
2. `resolve_launch_unknown_agent_name_returns_the_launch_verbatim` — `agent_name: "nope"` (no file) → verbatim. **Isolate the user-level roots first** (the same `ENV_LOCK` + `HOME`-to-empty-scratch drop-guard pattern as Task 2's `list_agents_tool` tests — a developer's real `~/.pi/agent/agents/nope.md` would match and break the "no match" case; tests 3–11 are immune to user-level files by first-wins, but this one is not).
3. `resolve_launch_case_insensitive_match` — file `scout.md`, `agent_name: "Scout"` → the frontmatter is applied.
4. `resolve_launch_frontmatter_fills_gaps` — an all-`None` launch + a file with `model: fake/m1` (IN the catalog), `thinking: low`, `tools: [read, bash]`, body `You are a scout.` → all four fields populated from the file.
5. `resolve_launch_explicit_params_win_over_frontmatter` — a launch with ALL FOUR fields set to values X + a file with DIFFERENT values Y, EXCEPT `launch.thinking` is left `None` → the three set fields keep X (the explicit params win) AND `thinking` becomes Y (the `None` field is filled from the file — this second assertion is what makes the test fail against the verbatim stub, which would leave `thinking` `None`).
6. `resolve_launch_stale_frontmatter_model_degrades_to_explicit_param` — file `model: fake/m2` (NOT in the catalog) + `thinking: low` + launch `model: Some("fake/m1"), thinking: None` → `model == Some("fake/m1")` (the explicit param is the next layer) AND `thinking == Some("low")` (filled from the file — fails under the stub).
7. `resolve_launch_stale_frontmatter_model_degrades_to_none` — file `model: fake/m2` + `tools: [read]` + launch `model: None, tools: None` → `model == None` (the parent model applies downstream) AND `tools == Some(vec!["read"])` (fails under the stub).
8. `resolve_launch_stale_model_with_level_suffix_degrades` — file `model: fake/m2:high` + `system_prompt` body `You are a scout.` + launch `system_prompt: None` → `model == None` (the bare `fake/m2` is not in the catalog — the suffix does not save it) AND `system_prompt == Some("You are a scout.")` (fails under the stub).
9. `resolve_launch_unknown_tool_names_are_dropped` — file `tools: [read, bogus_tool]` → `Some(vec!["read"])` (known names kept, unknown dropped).
10. `resolve_launch_tools_emptied_out_degrades_to_none` — file `tools: [bogus_tool]` + `model: fake/m1` (IN the catalog) + launch `model: None` → `tools == None` (never an empty allowlist) AND `model == Some("fake/m1")` (fails under the stub).
11. `resolve_launch_empty_body_means_no_system_prompt` — file with frontmatter only (empty body) + `thinking: low` + launch `thinking: None` → `system_prompt: None` AND `thinking == Some("low")` (fails under the stub).

The end-to-end test (the integration proof — `dispatch_subagent` actually calls `resolve_launch` and the child receives the layered config):

12. `a_native_subagent_with_a_matching_agent_name_runs_the_frontmatter_config` —
    - Scratch Space: a temp dir (the loop's `space_cwd`) + `write_file(&cwd.join(".agents/agents/scout.md"), "---\nname: scout\ndescription: Fast recon.\nmodel: fake/m2\nthinking: low\ntools: [read]\n---\nYou are a scout.\n")`.
    - A TWO-model catalog: `fake/m1` (the parent) + `fake/m2` (the frontmatter target) — `build_loop_with_db` builds a single-model catalog, so add a small test helper `build_loop_with_subagent(provider, events, turn_cancel, settle_tx, retry, sink, subagent_manager, models: Vec<Model>) -> AgentLoop` in the test module (**returns the `AgentLoop` directly, NOT `build_loop_with_db`'s `(AgentLoop, Arc<Db>)` tuple** — test 12 needs no `Db` assertions; a copy of `build_loop_with_db`'s body with the `Db` construction dropped, the `models` vec in the `ModelCatalog`, the `subagent` `AgentLoop::new` arg (position 18, `None` in `build_loop_with_db`) set to `subagent_manager`, and the `sink` parameter passed through instead of the fixed `TestSink`; the PARENT's `model` arg is `models[0].clone()` — the `fake/m1` entry — and BOTH the parent's `ModelCatalog` and the manager's `NativeDeps.catalog` get the full `models` vec, so the child's `resolve_composed_model` finds `fake/m2`).
    - The manager: a `make_native_manager`-style helper in the loop.rs test module (mirror `subagent.rs`'s, line ~2262 — including its `.expect("subagent manager should build")` on `SubagentSessionManager::new(scratch_config_dir, None)`, which returns a `Result`): `set_native_deps(NativeDeps { provider_factory, catalog: ModelCatalog { models, ..Default::default() }, todo_store, sudo: SudoDeps::default(), settle_timeout: Duration::from_secs(30), trust_db: None, config_dir: None })`. The `provider_factory` returns a `RequestRecordingProvider` (a NEW test-module struct below) for ANY model.
    - The parent's provider: `ScriptedProvider::new(vec![Some(vec![ProviderEvent::ToolCall(ToolCall { id: "t1".into(), name: "subagent".into(), arguments: json!({ "task": "recon the auth code", "agentName": "scout" }) }), ProviderEvent::Done(FinishReason::ToolCalls)], Some(vec![ProviderEvent::TextDelta("done".into()), ProviderEvent::Done(FinishReason::Stop)])])` (turn 1 dispatches the subagent, turn 2 settles the parent).
    - `RequestRecordingProvider` (new, in the test module): `struct RequestRecordingProvider { recorded: Arc<StdMutex<Vec<RecordedRequest>> }` where `struct RecordedRequest { model: String, system: Option<String>, tool_names: Vec<String> }`; `impl Provider for RequestRecordingProvider` — `complete` records `(req.model.clone(), the first System-role message's text (match `MessageContent::Text`), req.tools' names)` and returns a fixed single-response stream: `futures_util::stream::iter(vec![ProviderEvent::TextDelta("scout done".into()), ProviderEvent::Done(FinishReason::Stop)]).boxed()` (mirror `ScriptedProvider`'s stream construction — `Provider` is `#[async_trait]`, so the impl is annotated `#[async_trait::async_trait]` FULLY QUALIFIED — the loop.rs test module has no `use async_trait` import, and every impl there spells it fully qualified, like `ArcBoxProvider` in `subagent.rs`).
    - The sink: a NEW test-module struct `RecordingSink { events: Arc<StdMutex<Vec<(String, Value)>>} }` implementing `EventSink::emit` by pushing `(event.to_string(), payload)` for EVERY event (the existing `TestSink` captures only `session-update` — it cannot see `subagent-session-started`).
    - Drive: `loop_.handle_prompt(&text_prompt("go")).await` (the module's dominant drive pattern — the finding-13b test at line ~3312 and the tests at 2273/2428/2506 all use exactly this; do NOT use `run()` + `send_prompt` — `run(mut self)` consumes the loop, and `send_prompt(&self, text: &str, images: &[ImageRef])` takes a string + image slice, not a `Prompt`), then:
      - Find the `subagent-session-started` frame in `RecordingSink.events` → assert `payload["model"] == "fake/m2"` AND `payload["enabledTools"] == ["read"]`.
      - Assert the `RequestRecordingProvider`'s recorded requests: the CHILD's request (the one whose `model == "m2"` — the parent's requests are `m1`) has `system == Some("You are a scout.")` AND `tool_names == ["read"]`.
      - Assert the parent turn settled (a `done` text frame arrives / the settle watch fires — bounded wait, no hang).

**Steps:**
- [ ] Write the failing tests 1–11 (the `resolve_launch` unit tests — the method does not exist yet: add a STUB `fn resolve_launch(&self, launch: &LaunchConfig, _agent_name: &str) -> LaunchConfig { launch.clone() }` so the tests compile — the stub needs the `LaunchConfig` derives + import from the Files list — and the non-verbatim ones fail) + the test helpers (`build_loop_with_subagent`, the loop.rs `make_native_manager` mirror, `RequestRecordingProvider`, `RecordingSink`)
- [ ] Write the failing end-to-end test 12
- [ ] Run `cargo test --lib harness::loop` from `src-tauri/`
  - Did tests 3–11 + 12 fail (the stub returns the launch verbatim / the child runs on the parent model)? Did tests 1–2 pass (the verbatim cases hold under the stub)? If a test in 3–11 PASSED, stop and investigate — the strengthened assertions (each such test carries a frontmatter-derived expectation alongside its precedence assertion) are what make the stub fail them. If 1 or 2 failed, stop and investigate.
- [ ] Implement `resolve_launch` (replace the stub with the full body above) + the `dispatch_subagent` call-site line
- [ ] Run `cargo test --lib harness::loop` from `src-tauri/`
  - Did ALL tests pass (12/12, including the end-to-end)? If not, fix the failures and re-run before continuing.
- [ ] Run `cargo test --lib` from `src-tauri/`
  - Did the FULL suite pass (no regressions in `subagent.rs` / `session.rs` / `prompt.rs`)? If not, fix and re-run.
- [ ] Run `cargo clippy --all-targets` from `src-tauri/`
  - 0 warnings? If not, fix and re-run.
- [ ] Run `cargo fmt` from `src-tauri/`
- [ ] Commit with message: "feat(harness): subagent agentName resolution against Agent definitions (ADR 0020, Task 3)"

**Acceptance criteria:**
- [ ] The full resolution matrix holds (tests 1–11): verbatim on empty/unknown; case-insensitive match; layered precedence on all four fields (each test asserts BOTH the precedence AND a frontmatter-derived fill, so the verbatim stub cannot pass them); stale-model degrade (with and without the `:<level>` suffix); unknown-tool drop; empty-after-drop degrade; empty-body → `None`
- [ ] The end-to-end test proves the integration: the child's `subagent-session-started` payload carries `fake/m2` + `["read"]`, and the child's first model request carries the frontmatter body as its system message with exactly the `read` tool
- [ ] `dispatch_native` (subagent.rs) is UNCHANGED by this task EXCEPT the `#[derive(Clone, Debug, PartialEq)]` on `LaunchConfig`; an explicit unknown `model` param still fails the dispatch (existing behavior, covered by existing tests)

---

### Task 4: ADR 0020 + CONTEXT.md

**Context:**
The feature supersedes ADR 0017's "nameless subagents" decision, which explicitly warns: "a future reader must not 'fix' it into a lookup without superseding this decision." This task records the decision (ADR 0020) and the new terminology (CONTEXT.md's **Agent definition** term) so the next reader finds the "why" without archaeology. The ADR is append-only per the decisions convention; ADR 0017's BODY is not edited (its front-matter `superseded-by` field is set — that is the sanctioned change mechanism).

**Files:**
- Create: `docs/decisions/0020-native-subagent-agent-definitions.md`
- Modify: `docs/decisions/0017-native-session-system-prompt.md` (front-matter ONLY — set `superseded-by:`; do NOT touch the body, do NOT change `status` — the supersession is PARTIAL: 0017's main-session prompt decisions stand, only the "nameless subagents" decision is superseded, so `status` stays `accepted`)
- Modify: `CONTEXT.md` (add the **Agent definition** term; update the **Subagent session** entry)

**What to implement:**

`docs/decisions/0020-native-subagent-agent-definitions.md`:

```md
---
status: accepted
date: <TODAY — fill in the actual date>
superseded-by:
---

# The native harness resolves `subagent`'s `agentName` against user-authored Agent definitions

A native session's `subagent` tool took `agentName` as a free-form UI label only (ADR 0017) — a subagent's model/tools/system-prompt had to be hand-typed by the parent model, and the agent-definition files users already write (markdown + YAML frontmatter — the format pi's subagent extension uses) were ignored. We decided: the native harness DISCOVERS Agent definitions (flat `*.md` files; space-level `.agents/agents` + `.pi/agents` walked up to the repo root, user-level `~/.agents/agents` + `~/.pi/agent/agents`) like it discovers skills (ADR 0013), resolves `agentName` against them (case-insensitive exact match), and layers the frontmatter (`model` / `thinking` / `tools`) + body (system prompt) UNDER the explicit tool-call params (explicit > frontmatter > parent defaults). A `list_agents` tool advertises the definitions; a subagent child gets neither `subagent` nor `list_agents`.

**Supersedes** ADR 0017's "nameless subagents" decision (its "the desktop ships NO named subagent presets" consequence + the "a future reader must not 'fix' it into a lookup" warning). The supersession is PARTIAL — 0017's main-session prompt decisions (persist-and-replay, the reduced child message, no per-call rebuild) stand untouched. The distinction that makes it defensible: 0017 rejected the desktop SHIPPING named presets (a static `general`/`researcher`/`reviewer` table — "a product-level concept the user rejected"); this feature ships NOTHING — the user writes their own definition files, the desktop only discovers them (the ADR 0013 skills model). A user's existing `~/.pi/agent/agents/*.md` files work with zero configuration (ADR 0012's "just works with your pi setup" consumer contract).

**Considered Options**

- **Authoritative frontmatter** (a matched definition fully wins; explicit tool params are ignored): rejected — the model loses the ability to partially override ("use scout but with the fast model"); the layered precedence is strictly more flexible and degrades to the same config when no params are passed.
- **Fail the dispatch on a stale frontmatter `model`** (unknown model → `Failed`, like an explicit param): rejected — a frontmatter value is a USER FILE that can go stale (the provider renamed the model); a stale file must not break a dispatch. Explicit params stay strict (they are the model's current intent). The effective model stays observable via the `subagent-session-started` `model` field.
- **Gate space-level agent files behind the Trusted Space flag (ADR 0010) / a confirm prompt (pi's extension does this for project agents)**: rejected — a repo-controlled agent file is the SAME trust class as the Space's `AGENTS.md` / skills, which the desktop already injects ungated (the user opened the Space); adding a second gate for a subset of repo-controlled prompts would be inconsistent.
- **Native-only scope** (the external pi path's `dispatch_params` untouched): the fix targets the native harness only — the pi suite's `subagent` tool spec lives in the separate pi-archimedes repo and its own dispatch path is unchanged.

**Consequences**

- The desktop gains a second consumer of pi's config dir layout (`~/.pi/agent/agents/` — next to `settings.json` / `auth.json` / `models-store.json`, ADR 0012): a pi format change breaks discovery (mitigated: best-effort, a missing/unparseable file is skipped, never a crash — the skills.rs total contract).
- `agentName` is no longer "a no-op for behavior" (ADR 0017's consequence is superseded) — but an UNKNOWN or omitted `agentName` is still a no-op for behavior (label-only, config-less, never an error): the feature is additive, not a breaking change to existing tool calls.
- The `list_agents` NAME collides with the Tauri IPC command `list_agents` (the process registry, `commands/spaces.rs`) — different layers (model tool vs Client IPC), no runtime collision; future readers should not "unify" them.
- A frontmatter `thinking` field is a NATIVE extension beyond pi's agent format (`name` / `description` / `tools` / `model`); pi ignores unknown frontmatter fields, so the file format stays pi-compatible in both directions.
```

`docs/decisions/0017-native-session-system-prompt.md` — front-matter ONLY:

```diff
 ---
 status: accepted
 date: 2026-10-01
-superseded-by:
+superseded-by: 0020-native-subagent-agent-definitions.md
 ---
```

(`status` STAYS `accepted` — the supersession is partial; the body is NOT edited — the decisions directory is append-only.)

`CONTEXT.md` — add this term to the Language section (place it after the **Agent registry** entry, keeping the file's format — bold term, definition paragraph, `_Avoid_:` line):

> **Agent definition:**
> A user-authored markdown file (flat, in a standard agents dir) with YAML frontmatter (`name`, `description`, `model`, `thinking`, `tools`) + a system-prompt body — discovered by the desktop like a **Skill** (space-level `.agents/agents` + `.pi/agents` walked to the repo root; user-level `~/.agents/agents` + `~/.pi/agent/agents`). Selectable in a **Subagent session** via the `subagent` tool's `agentName` (layered under explicit params — ADR 0020); advertised by the `list_agents` tool. Distinct from **Agent** (the conversation partner) and **Agent registry** (the Client's spawn-command list).
> _Avoid_: Agent config, agent preset, subagent preset

And in the existing **Subagent session** entry, replace the parenthetical "(native-native: the parent's model/tools minus `subagent`, plus optional `launch` overrides; its `agentName` is a free-form UI label only, ADR 0017)" with "(native-native: the parent's model/tools minus `subagent` and `list_agents`, plus optional `launch` overrides; its `agentName` resolves against discovered **Agent definitions** — ADR 0020; an unknown/omitted name is a config-less label-only dispatch)".

**Steps:**
- [ ] Create `docs/decisions/0020-native-subagent-agent-definitions.md` with the content above (fill in the actual date)
- [ ] Edit ADR 0017's front-matter `superseded-by` field (front-matter ONLY — verify the body is byte-identical afterwards: `git diff docs/decisions/0017-native-session-system-prompt.md` shows exactly one changed line)
- [ ] Add the **Agent definition** term to `CONTEXT.md` + update the **Subagent session** entry
- [ ] Verify: `ls docs/decisions/ | grep 0020` (the file exists with the correct sequential number — 0019 is the highest existing); `grep -c "Agent definition" CONTEXT.md` ≥ 2 (the term entry + the cross-reference in the Subagent session entry)
- [ ] Commit with message: "docs: ADR 0020 (native subagent Agent-definition resolution) + CONTEXT.md term"

**Acceptance criteria:**
- [ ] ADR 0020 exists, is numbered sequentially after 0019, and explicitly names what it supersedes (0017's "nameless subagents" decision) and why the supersession is defensible (shipped presets vs user-authored files)
- [ ] ADR 0017's body is UNCHANGED (one-line front-matter diff only)
- [ ] `CONTEXT.md` defines **Agent definition** and the **Subagent session** entry no longer says `agentName` is "a free-form UI label only, ADR 0017"
