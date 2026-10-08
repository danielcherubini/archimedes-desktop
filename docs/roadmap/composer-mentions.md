---
status: committed
done-when: Typing `$` / `#` / `@` in the composer opens a prefix-filtered picker (skills / effective MCP servers / user+space agent definitions); selecting inserts the token; on send, matched tokens expand into blocks (skill hard, agent/mcp soft-hint) sent + persisted with the user text, unmatched tokens pass through byte-identically; the bubble renders user text + chips (skill chip; distinct "named `<name>`" soft-hint chips for `@`/`#`); all AGENTS.md validation green.
---

# Composer Mentions (skills / agents / MCP servers) — Plan

**Goal:** Let the user name a Skill (`$`), an Agent definition (`@`), or an MCP server (`#`) in a message; the composer autocompletes the token and expansion injects the corresponding block (hard for skills, soft-hint for agents/MCP) into the sent + persisted text.

**Architecture:** Composer-side only. A new mention mechanism in `src/lib/skills.ts` (generalized in place: one unified matching/expansion pass over all three prefixes) + two new read-only Tauri commands (agent definitions for a Space; the effective three-layer MCP set) + a catalog hook mirroring `useSkillCatalog`. The picker (`ComposerSkills` → `ComposerMentions`), the send path (`expandMentions`), and the display (`splitMentionBlocks` + a soft-hint chip) are the three frontend integration points. Agent-side discovery (the `<skills>` prompt section, `list_agents`, the `mcp` tool) is UNCHANGED.

**Tech Stack:** Tauri 2 commands (Rust, `src-tauri/`), React 19 + TypeScript + Vitest (repo root), the existing `useSkillCatalog` module-level promise-cache pattern.

**Load-bearing invariants (every task must preserve):**
1. **Byte-identity with no matched token:** `expandMentions(text, { empty catalogs }) === text` (byte-identical) — the `mergeDedupeKey` resume-merge guarantee (the live bubble, the persisted record, and the agent input carry the SAME text; the key is content-based).
2. **Determinism:** expansion is a pure function of text + catalogs (same inputs → same blocks; re-expansion on resume is stable).
3. **`$` behavior unchanged:** the `$` regex stays `/\$([a-z0-9]+(?:-[a-z0-9]+)*)/g` (ZCode parity, boundary-free on the LEFT). `#`/`@` require a leading whitespace or line-start (`(^|\s)`). ASYMMETRIC ON PURPOSE (the spec's decision — a future reader must not "unify" it).
4. **Case policy:** tokens are lowercase-only; catalog matching is case-insensitive on the resource-name side.
5. **Unknown token → verbatim** (no error, no stripping). Deduped across the whole run; blocks in first-mention (text-position) order.
6. **Tag-safety:** the token char class `[a-z0-9-]` means a token can never carry a `"` or `<` — no escaping is needed for the interpolated `name` attribute (the same argument as `skills.rs`' `is_tag_safe_name`).

**The block literals (exact — the expansion AND the split regex must agree):**

Skill (UNCHANGED — `buildBlock` in `src/lib/skills.ts`):
```
<skill name="NAME" location="PATH">
References are relative to DIR.

BODY
</skill>
```

Agent (NEW):
```
<agent name="NAME">
NAME — DESCRIPTION
The user has explicitly named the agent definition "NAME" for this
request. Dispatch a subagent with agentName "NAME" to handle it
(the definition's frontmatter defines its model, tools, and system
prompt; your explicit tool params layer over the definition).
</agent>
```
(`NAME` = the definition name VERBATIM (not lowercased); `DESCRIPTION` = the frontmatter description verbatim; the `—` is an EM DASH. If the description is `""`, the first line is `NAME` alone.)

MCP (NEW):
```
<mcp name="NAME">
The user has explicitly named the MCP server "NAME" for this
request. Connect to it via the mcp tool (mcp({ connect: "NAME" }))
and use its tools. (Server summary: SUMMARY.)
</mcp>
```
(`SUMMARY` = the server's one-line summary: the `url` for HTTP, `command + args` for stdio. `NAME` verbatim.)

**Copy policy (the hard/soft split):** `$` blocks are HARD (the injected text IS the task); `@`/`#` blocks are SOFT POINTERS (the agent may reason the resource isn't needed). Picker/chip copy must read "the user named this resource" — never "force-loaded".

**Task order (each is one commit; tasks 5–6 build on 4, 4 on 3, 3's wire types on 1+2's shapes):**

---

### Task 1: Rust — `list_agent_definitions_for_space` command

**Context:** The `@`-mention picker needs the agent definitions a Space can actually dispatch — user-level + space-level (the same set the harness's `agentName` resolution honors, ADR 0020). The existing `list_agent_definitions` command is USER-level only (the Settings page is app-global) and STAYS BYTE-IDENTICAL — this is a NEW command. The discovery core already exists: `crate::agents::discover_agents(space_path: Option<&Path>)` (a `None` → user-level only; `Some(path)` → space roots walked to the repo root + user-level). The command is a thin wrapper over it (the established pattern in `commands/agents.rs`).

**Files:**
- Modify: `src-tauri/src/commands/agents.rs`
- Modify: `src-tauri/src/lib.rs` (register the command)

**What to implement:**
- In `src-tauri/src/commands/agents.rs`, after `list_agent_definitions`, add:
  ```rust
  /// The user-level + space-level Agent definitions (the `@`-mention
  /// picker — the SAME set the harness's `agentName` resolution honors,
  /// ADR 0020). `cwd: None` → user-level only (the app-scope case).
  /// Never fails: discovery is best-effort (a missing root / unreadable
  /// file / malformed frontmatter is skipped — `agents.rs`'s total
  /// contract).
  #[tauri::command]
  pub async fn list_agent_definitions_for_space(
      cwd: Option<String>,
  ) -> Result<Vec<AgentDefinitionDto>, String> {
      Ok(crate::agents::discover_agents(
          cwd.as_deref().map(std::path::Path::new),
      )
      .into_iter()
      .map(AgentDefinitionDto::from)
      .collect())
  }
  ```
  REUSE the existing `AgentDefinitionDto` (name / description / model / scope) — do NOT create a new wire shape.
- In `src-tauri/src/lib.rs`, add `commands::agents::list_agent_definitions_for_space,` to the `tauri::generate_handler![ … ]` list (immediately after the `commands::agents::list_agent_definitions,` line).
- DO NOT change `list_agent_definitions`, `discover_agents`, or the `AgentDefinitionDto` mapping.

**Steps:**
- [ ] Write failing test(s) in `src-tauri/src/commands/agents.rs` `mod tests`:
  - `list_agent_definitions_for_space_a_space_level_definition_is_listed`: WITH `crate::test_support::env_lock()` held + `RestoreHome` guard (the same set→assert→restore span as the existing tests, incl. `#[allow(clippy::await_holding_lock)]` + the SAFETY comment — `user_roots()` walks the real `~/.agents/agents` otherwise), a temp `HOME` with NO agent dirs + a temp dir (the module's `scratch()`-style dir) with `.agents/agents/space-agent.md` (`---\nname: space-agent\ndescription: A space-level agent.\n---\nBody.\n`); call `list_agent_definitions_for_space(Some(dir.to_string_lossy().into_owned()))` (the command takes `Option<String>` — NOT a `PathBuf`); assert the space-level definition is present with `scope == "space"`.
  - `list_agent_definitions_for_space_includes_user_level_definitions`: same `env_lock`/`RestoreHome` span, a temp `HOME` with `.agents/agents/user-agent.md` + a separate temp space dir with `.agents/agents/space-agent.md`; call with `Some(space.to_string_lossy().into_owned())`; assert BOTH appear (`scope "user"` + `scope "space"`).
  - `list_agent_definitions_for_space_none_is_user_level_only`: same HOME-pin pattern, call with `None`; assert only the user-level definition appears.
- [ ] Run `cargo test --lib commands::agents` (from `src-tauri/`)
  - Did the new tests fail (function not found / missing definitions)? If they passed unexpectedly, stop and investigate why.
- [ ] Implement the command + the `lib.rs` registration.
- [ ] Run `cargo test --lib commands::agents` (from `src-tauri/`)
  - Did all pass? If not, fix and re-run.
- [ ] Run `cargo clippy --all-targets` + `cargo fmt` (from `src-tauri/`)
  - 0 warnings? `cargo fmt --check` clean? Fix and re-run.
- [ ] Run `cargo test` (from `src-tauri/`) — full suite green.
- [ ] Commit with message: `feat: list agent definitions for a space (the @-mention picker)`

**Acceptance criteria:**
- [ ] `list_agent_definitions_for_space(Some(space))` returns user-level + space-level definitions (both scopes correct).
- [ ] `list_agent_definitions_for_space(None)` returns user-level only.
- [ ] The existing `list_agent_definitions` command and its test are byte-identical.
- [ ] `cargo test` + `cargo clippy --all-targets` (0 warnings) + `cargo fmt --check` green.

---

### Task 2: Rust — `list_mcp_servers_effective` command + `server_infos` helper

**Context:** The `#`-mention picker needs the EFFECTIVE MCP server set — the three-layer merge (pi-global `~/.pi/agent/mcp.json` + desktop `settings.json` `mcpServers` + pi-project `<cwd>/.pi/mcp.json`, precedence project > desktop > pi-global, ADR 0019) — as a name → (type, one-line summary) list. The merge already exists: `crate::agent::mcp::config::load_servers(home, project_cwd, desktop)` → `BTreeMap<String, ServerDef>` (the `McpManager::new` uses exactly this). This task adds a pure mapping helper next to `load_servers` (testable without a Tauri `State`) + a thin command. **Config read only — NO live connect** (the per-session `McpManager` rule, ADR 0018/0019, is untouched). The Settings page surface is UNCHANGED (ADR 0019's "effective view is a later extension" stands for Settings; this picker is not the Settings surface).

**Files:**
- Modify: `src-tauri/src/agent/mcp/config.rs` (helper + its tests)
- Create: `src-tauri/src/commands/mcp.rs` (the command)
- Modify: `src-tauri/src/commands/mod.rs` (`pub mod mcp;`)
- Modify: `src-tauri/src/lib.rs` (register the command)

**What to implement:**
- In `src-tauri/src/agent/mcp/config.rs`, add (after `load_servers`):
  ```rust
  /// One effective MCP server for the `#`-mention picker (a config read
  /// only — NO live connect; the per-session `McpManager` rule,
  /// ADR 0018/0019, is untouched).
  #[derive(Debug, Clone, PartialEq, Eq, Serialize)]
  #[serde(rename_all = "camelCase")]
  pub struct McpServerInfo {
      pub name: String,
      /// `"http" | "stdio"` (the `ServerDef` classification).
      pub kind: String,
      /// HTTP: the `url`. Stdio: `command` + `args` joined with a space
      /// (e.g. `npx -y x-mcp`; the command alone when there are no args).
      pub summary: String,
  }

  /// Map the effective set (`load_servers`' output) to the picker shape.
  /// Deterministic (a `BTreeMap` input → name-sorted output).
  pub fn server_infos(servers: &BTreeMap<String, ServerDef>) -> Vec<McpServerInfo> {
      servers
          .iter()
          .map(|(name, def)| {
              let (kind, summary) = match def {
                  ServerDef::Http(h) => ("http".to_string(), h.url.clone()),
                  ServerDef::Stdio(s) => (
                      "stdio".to_string(),
                      if s.args.is_empty() {
                          s.command.clone()
                      } else {
                          // A SPACE separator between the command and the
                          // args — the `+` operator concatenates with NO
                          // separator (`"npx" + "-y x-mcp"` would be
                          // `"npx-y x-mcp"`).
                          format!("{} {}", s.command, s.args.join(" "))
                      },
                  ),
              };
              McpServerInfo { name: name.clone(), kind, summary }
          })
          .collect()
  }
  ```
  (`use serde::Serialize;` is needed — add it to the imports.)
- Create `src-tauri/src/commands/mcp.rs`:
  ```rust
  //! The `#`-mention picker's MCP surface: the effective three-layer set
  //! (ADR 0019) as a read-only name → (kind, summary) list. NO live
  //! connect (the per-session `McpManager` rule, ADR 0018/0019, is
  //! untouched) — the settings read is best-effort (a missing / corrupt
  //! `settings.json` = the `load_settings` defaults = an empty desktop
  //! layer).

  use std::path::{Path, PathBuf};
  use std::sync::Arc;

  use tauri::State;

  use crate::agent::mcp::config::server_infos;
  use crate::agent::mcp::config::load_servers;
  use crate::agent::SessionManager;
  use crate::config::load_settings;

  /// The EFFECTIVE MCP server set for `cwd` (the `#`-mention picker —
  /// ADR 0019's three-layer merge, project > desktop > pi-global).
  /// `cwd: None` = app-scope: the project layer degrades via `read_layer`'
  /// best-effort read of `/`/`.pi/mcp.json` (absent in practice on every
  /// platform — the mechanism is the best-effort degradation, not a
  /// guaranteed skip).
  #[tauri::command]
  pub async fn list_mcp_servers_effective(
      state: State<'_, Arc<SessionManager>>,
      cwd: Option<String>,
  ) -> Result<Vec<crate::agent::mcp::config::McpServerInfo>, String> {
      let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
      let desktop = load_settings(state.config_dir()).mcp_servers;
      let project = cwd
          .as_deref()
          .map(Path::new)
          .unwrap_or_else(|| Path::new("/"));
      Ok(server_infos(&load_servers(&home, project, Some(&desktop))))
  }
  ```
  (Match the existing `McpManager::new` home-dir pattern: `dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))` — see `agent/harness/loop.rs`. NOTE: `load_settings` on a MISSING `settings.json` WRITES the defaults file — the existing `get_settings` command has the same side effect; this is established behavior, not a new one to introduce.)
- `src-tauri/src/commands/mod.rs`: add `pub mod mcp;` AFTER `pub mod history;` (the list is alphabetical: `agents, clipboard, files, history, sessions, settings, skills, spaces` — `mcp` slots between `history` and `sessions`; keep the `pub mod` spelling the file uses).
- `src-tauri/src/lib.rs`: add `commands::mcp::list_mcp_servers_effective,` to the `generate_handler!` list (after the `commands::settings::auth_mcp_server,` line).
- DO NOT change `load_servers`, `McpManager`, the `mcp` tool, or the Settings page.

**Steps:**
- [ ] Write failing tests in `src-tauri/src/agent/mcp/config.rs` `mod tests`:
  - `server_infos_maps_http_to_url_summary`: a `BTreeMap` with one `ServerDef::Http(HttpDef { url: "https://a".into(), headers: Default::default(), auth: AuthSpec::None })`; assert field-by-field against `McpServerInfo { name: "a".into(), kind: "http".into(), summary: "https://a".into() }` (Rust struct literals need `.into()` / `to_string()` for the `String` fields — `&str` literals do not compile as struct-literal fields; the codebase's own test pattern, e.g. `types.rs`).
  - `server_infos_maps_stdio_to_command_plus_args_summary`: one `ServerDef::Stdio(StdioDef { command: "npx".into(), args: vec!["-y".into(), "x-mcp".into()], env: Default::default(), cwd: None })`; assert `summary == "npx -y x-mcp"` + `kind == "stdio"`; a second case with `args: vec![]` asserts the command alone.
  - `server_infos_is_name_sorted`: two entries (`b`, `a`); assert output order `a`, `b`.
  - (Import the needed types in the test module: `use crate::agent::mcp::types::{AuthSpec, HttpDef, ServerDef, StdioDef};`.)
- [ ] Run `cargo test --lib agent::mcp::config` (from `src-tauri/`)
  - Did the new tests fail (`server_infos` not found)? If they passed unexpectedly, stop and investigate.
- [ ] Implement `server_infos` + `McpServerInfo` in `config.rs`, the `commands/mcp.rs` file, the `mod.rs` line, the `lib.rs` registration.
- [ ] Run `cargo test --lib agent::mcp` (from `src-tauri/`) — all pass.
- [ ] Run `cargo clippy --all-targets` + `cargo fmt` (from `src-tauri/`) — 0 warnings, fmt clean.
- [ ] Run `cargo test` (full suite) — green.
- [ ] Commit with message: `feat: list the effective MCP servers (the #-mention picker)`

**Acceptance criteria:**
- [ ] `server_infos` maps the `load_servers` output to name-sorted `McpServerInfo`s with correct `kind` / `summary` (the stdio summary has a SPACE between the command and the args — `"npx -y x-mcp"`).
- [ ] The command is a thin wrapper (the tested `load_servers` + `server_infos`; `State`-carrying commands in this codebase are tested via their pure helpers — no `State`-construction pattern exists, so the command itself is not directly tested).
- [ ] `cargo test` + clippy + fmt green.

---

### Task 3: Frontend core — generalize the mention mechanism in `src/lib/skills.ts` + wire types in `src/lib/tauri.ts`

**Context:** The mention mechanism lives in `src/lib/skills.ts` (the module-level `MENTION_RE`, `expandSkillMentions`, `splitSkillBlocks`, `activeSkillToken` + `src/lib/skills.test.ts`). This task generalizes it IN PLACE (the file keeps its name — it IS the mention mechanism; `SkillInfo` is one of the three catalogs). The existing `$` behavior stays BYTE-IDENTICAL (invariant 3) — the existing tests keep passing. Wire types for the two new commands (Task 1 / Task 2) land in `src/lib/tauri.ts` so the hook (Task 4) and the picker (Task 5) have typed inputs.

**Files:**
- Modify: `src/lib/skills.ts`
- Modify: `src/lib/skills.test.ts`
- Modify: `src/lib/tauri.ts`

**What to implement:**

In `src/lib/skills.ts` (keep ALL existing exports working; the call sites migrate in Tasks 5–6 — after Tasks 5–6, `activeSkillToken` / `expandSkillMentions` / `splitSkillBlocks` have no PRODUCTION callers (tests only); do NOT delete them in this feature — a follow-up may retire them):

The file's type import becomes `import type { AgentDefinitionDto, McpServerInfo, SkillInfo } from "./tauri";` (the two new names are needed by `MentionCatalogs` / `expandMentions`).

```ts
export type MentionKind = "skill" | "agent" | "mcp";

// The `$` regex is UNCHANGED (ZCode parity — invariant 3).
// The `#` / `@` regexes REQUIRE a leading whitespace or line-start
// (the asymmetric boundary — invariant 3; a future reader must not
// "unify" them with the `$` one).
export const AGENT_MENTION_RE = /(^|\s)@([a-z0-9]+(?:-[a-z0-9]+)*)/g;
export const MCP_MENTION_RE = /(^|\s)#([a-z0-9]+(?:-[a-z0-9]+)*)/g;

export interface MentionCatalogs {
  skills: SkillInfo[];
  agents: AgentDefinitionDto[]; // the wire DTO (name/description/model/scope) — name + description are all the block needs
  mcpServers: McpServerInfo[]; // the wire DTO (name/kind/summary)
}
```

`activeMentionToken` (generalizes `activeSkillToken`; keep `activeSkillToken` exported for compatibility):
```ts
export function activeMentionToken(
  value: string,
  caret: number,
): { prefix: "$" | "#" | "@"; remainder: string; start: number } | null {
  const before = value.slice(0, caret);
  // One shared shape: the span between the nearest preceding whitespace
  // (or start) and the caret starts with a prefix + a token char class.
  // (`$` keeps the SAME live-caret boundary as `activeSkillToken` — the
  // boundary-free-left applies to the EXPANSION regex only.)
  const m = before.match(/(^|\s)([$#@][a-z0-9-]*)$/);
  if (!m) return null;
  // The regex guarantees `m[2]` starts with one of the three glyphs —
  // narrow explicitly (`charAt` returns `string`, which does NOT satisfy
  // the union return type under `strict` TS).
  const first = m[2]!.charAt(0) as "\$" | "#" | "@";
  return {
    prefix: first,
    remainder: m[2]!.slice(1),
    start: (m.index ?? 0) + m[1]!.length,
  };
}
```

`expandMentions` (generalizes `expandSkillMentions`; the `$` path must produce the SAME output the old function produced for a skills-only catalog — verify with a test):
```ts
export function expandMentions(text: string, c: MentionCatalogs): string {
  // Collect hits from all three regexes in one stateless pass.
  // `matchAll` is used deliberately (NO shared-state `exec`/`test` loop —
  // the module's stale-`lastIndex` warning in `MENTION_RE`'s doc applies
  // to a manual loop; `matchAll` resets per call).
  const hits: { index: number; kind: MentionKind; token: string }[] = [];
  for (const m of text.matchAll(MENTION_RE)) hits.push({ index: m.index, kind: "skill", token: m[1] });
  for (const m of text.matchAll(MCP_MENTION_RE)) hits.push({ index: m.index + m[1].length, kind: "mcp", token: m[2] });
  for (const m of text.matchAll(AGENT_MENTION_RE)) hits.push({ index: m.index + m[1].length, kind: "agent", token: m[2] });
  hits.sort((a, b) => a.index - b.index); // first-mention (text-position) order

  // Case-insensitive name lookup per kind (the skill-side `byName` map,
  // generalized — catalog order within a name group).
  // … walk `hits`; a token matching NO catalog entry is skipped (verbatim,
  // invariant 5); one block per matched resource, deduped ACROSS THE WHOLE
  // RUN (a `Set` keyed by the catalog entry, like the existing code);
  // NO hits matching any catalog → return `text` UNCHANGED (invariant 1).
  // … blocks appended after the user's text, blank-line separated, joined
  // with "\n\n" (the existing `expandSkillMentions` joiner — the split
  // shape check depends on it).
}
```
The agent block is the exact literal above (the `—` EM DASH; `NAME — DESCRIPTION` line; description `""` → the line is `NAME` alone). The MCP block is the exact literal above with the server's `summary` interpolated. Tag-safety: the token char class `[a-z0-9-]` means the matched `token` can never break the `name="…"` attribute (invariant 6); the interpolated `NAME` is the CATALOG name (verbatim, like the skill's frontmatter name) — catalog names are checked tag-safe at discovery (Rust `is_tag_safe_name` for skills; agent names come from the same tolerant frontmatter reader; MCP names from a JSON object key) — do NOT add escaping (the existing skill block interpolates unescaped for exactly this reason).

`splitMentionBlocks` (generalizes `splitSkillBlocks`; keep `splitSkillBlocks` exported):
```ts
export interface MentionBlock {
  kind: MentionKind;
  /** The `name` attribute VERBATIM. */
  name: string;
  /** skill: the block's BODY (between the `References…` note and `</skill>`).
   *  agent / mcp: the block's hint content (between the tag lines). */
  body: string;
}

export function splitMentionBlocks(
  text: string,
): { text: string; blocks: MentionBlock[] } {
  // Parse the three shapes (each a known-shape regex, NOT a full HTML
  // parser — the existing skill shape unchanged):
  //   skill: <skill name="([^"]*)" location="[^"]*">\nReferences are relative to[^\n]*\n\n([\s\S]*?)\n<\/skill>
  //   agent: <agent name="([^"]*)">\n([\s\S]*?)\n<\/agent>
  //   mcp:   <mcp name="([^"]*)">\n([\s\S]*?)\n<\/mcp>
  // Robustness (the EXISTING policy, generalized): count the opening tags
  // PER KIND (`<skill[ >/]`, `<agent[ >/]`, `<mcp[ >/]`); the per-kind
  // counts must equal the per-kind parsed-block counts (a stray tag that
  // isn't a valid block makes them disagree → the whole text is
  // verbatim). The matches (all kinds, index-sorted) must be CONTIGUOUS
  // from the first (each `index` === the previous `end + 2` — the `\n\n`
  // joiner) AND nothing may follow the last match (NO-LOSS). Any
  // disagreement → return `{ text, blocks: [] }` (verbatim, byte-identical).
}
```

In `src/lib/tauri.ts` (add next to the existing `AgentDefinitionDto` / the commands section):
```ts
/** One effective MCP server (camelCase over IPC — the Rust `McpServerInfo`). */
export interface McpServerInfo {
  name: string;
  kind: "http" | "stdio";
  /** The `url` (HTTP) or `command + args` (stdio). */
  summary: string;
}

export async function listAgentDefinitionsForSpace(spacePath: string | null): Promise<AgentDefinitionDto[]> {
  return invoke<AgentDefinitionDto[]>("list_agent_definitions_for_space", { cwd: spacePath });
}

export async function listMcpServersEffective(spacePath: string | null): Promise<McpServerInfo[]> {
  return invoke<McpServerInfo[]>("list_mcp_servers_effective", { cwd: spacePath });
}
```
(`spacePath ?? null` is implicit — `invoke` sends `null` for `Option<String>` when the value is `null`, mirroring `listSkills`'s `spacePath: spacePath ?? null`.)

**Steps:**
- [ ] Extend `src/lib/skills.test.ts` with FAILING tests first (run them, confirm they fail on the missing exports / wrong behavior):
  - `$` regression (must keep passing after the change): the existing skill expansion / split / token tests.
  - `@` expansion: a catalog agent (a FULL `AgentDefinitionDto` — `satisfies AgentDefinitionDto`, incl. `model: null` + `scope: "user"` — NOT the partial `{ name, description, … }` shorthand) named `scout` with `description: "Fast recon."` → `@scout do X` expands to the exact literal block (assert the full string, incl. the EM DASH + the `NAME — DESCRIPTION` line); `@SCOUT` does NOT expand (verbatim); `x@scout` does NOT expand (the left boundary — a non-whitespace before the `@`); `@scout` at line-start DOES expand; ` hello @scout` (leading whitespace) DOES expand. The MCP fixture is a full `McpServerInfo` (`{ name, kind, summary }` — all three fields, no `…` shorthand).
  - `#` expansion: a catalog MCP `{ name: "postgres", kind: "stdio", summary: "npx -y x-mcp" }` → `#postgres` expands to the exact literal block (the `Server summary: npx -y x-mcp.` line); `#include` with NO server named `include` → verbatim; `#include` WITH a server named `include` → expands (the catalog-gating test); `user@host` (an email) does NOT expand.
  - Mixed: `"$skillA and @scout and #postgres"` → three blocks in text-position order; a token appearing twice → one block (deduped); no matched token → `expandMentions(text, emptyCatalogs) === text` BYTE-identical (the invariant-1 test — assert `===`, not `toEqual`).
  - `activeMentionToken`: `$`, `#`, `@` at line-start / after a space; `x@scout` → `null`; a bare `@` → `{ prefix: "@", remainder: "" }`; a mid-word `$` (`foo$bar`) → `null` (the live-caret boundary).
  - `splitMentionBlocks`: round-trip — `splitMentionBlocks(expandMentions(text, c))` returns the user text verbatim (trimmed of the trailing blank line, the existing policy) + the blocks in order with the right `kind`; a malformed agent/mcp tag (a stray `<agent>` that doesn't parse) → verbatim, byte-identical.
- [ ] Run `pnpm test src/lib/skills.test.ts`
  - Did the new tests fail (missing exports)? If they passed unexpectedly, stop and investigate.
- [ ] Implement the module changes + the `tauri.ts` types/wrappers.
- [ ] Run `pnpm test src/lib/skills.test.ts` — all pass (existing + new).
- [ ] Run `pnpm build`
  - Did the type-check pass? (The `tauri.ts` wrappers type-check against `AgentDefinitionDto` / `McpServerInfo`.) If not, fix and re-run.
- [ ] Commit with message: `feat: generalize the composer mention mechanism (@ / # alongside $)`

**Acceptance criteria:**
- [ ] All existing `$` tests pass UNCHANGED (byte-identical behavior).
- [ ] The exact block literals (incl. the EM DASH) round-trip through `expandMentions` → `splitMentionBlocks`.
- [ ] No matched token → byte-identical return (asserted with `===`).
- [ ] `pnpm test` + `pnpm build` green.

---

### Task 4: Frontend — `useMentionCatalogs` hook

**Context:** The composer needs all three catalogs (skills + agents + MCP servers) for the active Space, with the SAME freshness guarantees `useSkillCatalog` has (module-level promise cache keyed by Space; `[]` on key change so a stale Space's rows never expand; a failed fetch degrades to `[]`). This hook reuses `useSkillCatalog` verbatim for the skills row and mirrors its cache pattern for the other two.

**Files:**
- Create: `src/hooks/useMentionCatalogs.ts`
- Test: `src/hooks/useMentionCatalogs.test.ts`

**What to implement:**
```ts
import { useEffect, useRef, useState } from "react";
import {
  listAgentDefinitionsForSpace,
  listMcpServersEffective,
  type AgentDefinitionDto,
  type McpServerInfo,
  type SkillInfo,
} from "../lib/tauri";
import { useSkillCatalog } from "./useSkillCatalog";

/**
 * The three mention catalogs for a Space (or `null` = user-level /
 * app-scope only). Mirrors `useSkillCatalog`'s module-level promise
 * cache (the in-flight PROMISE is cached — the two-consumers-same-key
 * in-flight dedupe keeps working; a key change evicts the LEFT key; a
 * key change IMMEDIATELY serves `[]` until the new fetch resolves — a
 * stale Space's rows must never expand; a failed fetch degrades to `[]`
 * with a `console.error`). Two module-level caches (agents + mcp), keyed
 * the same way (`spacePath ?? "__global__"`).
 *
 * `null` key: `listAgentDefinitionsForSpace(null)` = user-level only
 * (the `cwd: None` command case); `listMcpServersEffective(null)` =
 * the desktop + pi-global layers (the `cwd: None` project-layer skip).
 */
export function useMentionCatalogs(
  spacePath: string | null,
): { skills: SkillInfo[]; agents: AgentDefinitionDto[]; mcpServers: McpServerInfo[] } {
  const skills = useSkillCatalog(spacePath);
  // … the agents + mcpServer rows via the same `{ key, rows }` state
  // pattern as `useSkillCatalog` (the two effects, two caches).
}

/**
 * TEST-ONLY: clear the agents + mcp module-level caches (the
 * `clearSkillCatalogCache` pattern from `useSkillCatalog` — the skills
 * cache itself is cleared by `clearSkillCatalogCache()`, which the
 * consuming test files' `beforeEach` already calls). Without the clear,
 * a key cached by an earlier test in the file serves WARM rows and the
 * gap-state / called-once assertions fail.
 */
export function clearMentionCatalogsCache(): void {
  // … `agentsCache.clear(); mcpCache.clear();`
}
```

**Steps:**
- [ ] Write failing tests in `src/hooks/useMentionCatalogs.test.ts` (the `src/hooks/useSkillCatalog.test.ts` pattern: `vi.mock("../lib/tauri", …)` — mock ALL THREE wrappers (`listSkills` + `listAgentDefinitionsForSpace` + `listMcpServersEffective`) + `renderHook` + `act`/`waitFor`; the `beforeEach` calls BOTH `clearSkillCatalogCache()` (the file's existing convention) AND the new `clearMentionCatalogsCache()`):
  - a space fetch: `listAgentDefinitionsForSpace("s")` + `listMcpServersEffective("s")` are called ONCE per key (the in-flight dedupe — a second `renderHook` with the same key must NOT re-invoke; the `useSkillCatalog.test.ts` "once-queue" assertion pattern).
  - a key change: the rows IMMEDIATELY serve `[]` until the new fetch resolves (assert the gap state) + the previous key is evicted (re-mounting the old key re-fetches).
  - a rejected fetch: the rows stay `[]` + `console.error` is called (spy on it; the `useSkillCatalog` test's pattern).
  - the `null` key: both wrappers are called with `null`.
- [ ] Run `pnpm test src/hooks/useMentionCatalogs.test.ts`
  - Did it fail (module missing)?
- [ ] Implement `src/hooks/useMentionCatalogs.ts` (incl. the exported `clearMentionCatalogsCache()` test-only helper).
- [ ] Run `pnpm test src/hooks/useMentionCatalogs.test.ts` — all pass.
- [ ] Run `pnpm build` — type-check green.
- [ ] Commit with message: `feat: useMentionCatalogs — the three mention catalogs for the composer`

**Acceptance criteria:**
- [ ] One IPC call per catalog per key (the in-flight promise cache).
- [ ] `[]`-on-key-change (a stale Space's rows never expand).
- [ ] Failed fetch → `[]` + `console.error`.
- [ ] `clearMentionCatalogsCache()` is exported + the test file's `beforeEach` clears all three caches (the `useSkillCatalog.test.ts` / `ChatStream.test.tsx` convention).
- [ ] `pnpm test` + `pnpm build` green.

---

### Task 5: Frontend — the picker: `ComposerMentions` + the composer wiring

**Context:** `ComposerSkills` (the floating row list above the textarea) + the picker state in `ChatStream` / `ComposerRow` are skill-specific. Generalize to all three prefixes: one picker, filtered to the active prefix's catalog (a `$`/`#`/`@` never mixes in one open list), rows with a prefix badge, and a `selectMention` that splices the token. The `archimedes:insert-skill` listener + `SkillsDialog` are UNCHANGED (the dialog still inserts `$name ` — `activeMentionToken` detects the `$` prefix).

**Files:**
- Create: `src/components/chat/ComposerMentions.tsx` (replaces `ComposerSkills.tsx` — delete the old file)
- Modify: `src/components/chat/ComposerRow.tsx`
- Modify: `src/components/ChatStream.tsx`
- Modify: `src/components/ChatStream.test.tsx` (the `skill-picker` testids + the picker interaction tests)

**What to implement:**
- `ComposerMentions.tsx`: the `ComposerSkills` component generalized. Props: `{ open: boolean; filtered: MentionRow[]; activeIndex: number; onSelect: (row: MentionRow) => void }` where
  ```ts
  export interface MentionRow {
    key: string; // the catalog entry's identity (name + kind is unique per kind)
    prefix: "$" | "#" | "@";
    name: string;
    description: string; // one-line (the `title` tooltip — the existing pattern)
  }
  ```
  Each row: a prefix BADGE (a small `text-ui-xs text-foreground-subtlest` span with the glyph `$` / `@` / `#` — the only visual change vs the old component) + name (primary, `text-ui-base`) + description (secondary, truncated, `title` tooltip — unchanged). The `data-testid` becomes `mention-picker` (update the `ChatStream.test.tsx` assertions from `skill-picker`). Everything else (the floating container, the keyboard-highlight `bg-surface-hover`, the `onMouseDown` preventDefault) is unchanged.
- `ComposerRow.tsx`: replace the `activeSkillToken` import/uses with `activeMentionToken` (the two call sites — `onChange` + the keydown re-evaluation — get the same shape plus the `prefix`); the `picker` prop type gains `prefix: "$" | "#" | "@"`; the `setPicker` PROP'S FUNCTION TYPE gains the same `prefix` field (it is `(picker: { query: string; index: number } | null) => void` today — a setter for the widened state); the `filtered: SkillInfo[]` prop becomes `MentionRow[]` (import `MentionRow` from `./ComposerMentions`); the `selectSkill` prop becomes `selectMention: (row: MentionRow) => void`; the `ComposerSkills` import becomes `ComposerMentions`.
- `ChatStream.tsx`:
  - Replace `const skills = useSkillCatalog(spacePath);` with `const { skills, agents, mcpServers } = useMentionCatalogs(spacePath);` (the import swap — `useSkillCatalog` is no longer imported in this file; the `spacePath` derivation is UNCHANGED).
  - The `picker` state gains `prefix`: `useState<{ prefix: "$" | "#" | "@"; query: string; index: number } | null>(null)`.
  - The `filtered` `useMemo`: build `MentionRow[]` from the ACTIVE prefix's catalog only —
    ```ts
    const rows =
      picker?.prefix === "#" ? mcpServers : picker?.prefix === "@" ? agents : skills;
    // map to MentionRow (prefix from the picker; description: skills/agents
    // use `description`; mcpServers use `summary`); then the EXISTING
    // case-insensitive name-substring filter on `picker?.query ?? ""`
    // (the existing null-safe `picker?.query` pattern — keep the
    // TS18047-avoiding `?.` exactly as-is).
    ```
    (When `picker` is `null`, `rows` is the skills list — harmless, the UI is gated on `picker && filtered.length > 0`, the existing comment's reasoning.)
  - The `filtered` `useMemo` DEPS become `[skills, agents, mcpServers, picker]` (the current `[skills, picker]` is INSUFFICIENT: a catalog fetch resolving while the picker is open — exactly what the new `findByText`-awaiting tests do — changes `agents`/`mcpServers` identity; without them in the deps the memo does not recompute and the picker stays empty until the next keystroke).
  - `selectSkill` → `selectMention(row: MentionRow)`: the existing splice logic, `inserted = `${row.prefix}${row.name.toLowerCase()} `` (the lowercase policy — the inserted token must be expandable; the regex is lowercase-only). SWAP the `activeSkillToken(draft, caret)` call at the top of that function to `activeMentionToken(draft, caret)` (the file's THIRD `activeSkillToken` call site — `activeSkillToken`'s regex is `$`-only, so it returns `null` for an `@`/`#` token and selecting an agent/MCP row would be a SILENT NO-OP if this swap is missed). The `skills.ts` import line (today `import { activeSkillToken, expandSkillMentions } from "../lib/skills"`) becomes `import { activeMentionToken, expandSkillMentions } from "../lib/skills"` in THIS task (the `expandSkillMentions` → `expandMentions` half is Task 6's). If `SkillInfo` becomes an unused import after the swap, remove it (`noUnusedLocals` is ON — `pnpm build` is the safety net; fix any type errors the widened `MentionRow` / `picker` / `setPicker` shapes surface — the mechanical swaps `tsc` will name are exactly: the `picker` state + setter, the `filtered` memo (body + deps), the `selectMention` signature, the `ComposerMentions` prop pass-through).
  - The `archimedes:insert-skill` listener: UNCHANGED (it inserts `$name ` — `activeMentionToken` sees the `$`).
  - The `send()` call (Task 6 changes the expansion line — this task must NOT touch it yet; keep `expandSkillMentions(rawText, skills)` until Task 6).
  - The `ComposerSkills` → `ComposerMentions` import/usage swap + the `picker` prop pass-through (add `prefix`).

**Steps:**
- [ ] Write/extend FAILING tests in `src/components/ChatStream.test.tsx` (the existing picker interaction tests — the `skill-picker` testid assertions + the type/Enter/Tab flows — are the template; the file's `beforeEach` already does `vi.clearAllMocks()` + `clearSkillCatalogCache()` — ADD `clearMentionCatalogsCache()` to that `beforeEach`, the established convention, or the module-level caches leak mock results across tests):
  - typing `#` opens the picker with the MCP rows (mock `listMcpServersEffective` via the existing `vi.mock("../lib/tauri")` — add the two new wrappers to the mock); the rows show the `#` badge.
  - typing `@` opens it with the agent rows (the `@` badge).
  - typing `$` opens it with the skill rows (the existing behavior — regression).
  - a mixed draft (`$a #b @c`) — only the prefix of the token at the CARET is listed.
  - Enter/Tab select splices `#name ` / `@name ` / `$name ` (assert the textarea value).
  - an unknown token (no matching catalog entry) → the picker stays closed / the token is untouched.
- [ ] Run `pnpm test src/components/ChatStream.test.tsx`
  - Did the new tests fail?
- [ ] Implement `ComposerMentions.tsx` (delete `ComposerSkills.tsx`), the `ComposerRow` changes, the `ChatStream` wiring.
- [ ] Run `pnpm test src/components/ChatStream.test.tsx` — all pass.
- [ ] Run `pnpm test` (the full frontend suite — `SkillsDialog.test.tsx` must still pass: the dialog is unchanged) + `pnpm build`.
- [ ] Commit with message: `feat: the composer picker lists skills, agents, and MCP servers`

**Acceptance criteria:**
- [ ] Typing `$` / `#` / `@` opens ONE picker, filtered to that prefix's catalog (never mixed).
- [ ] Rows show a prefix badge + name + description (tooltip).
- [ ] Enter/Tab/mouse select splices the token; the existing keyboard model (↑/↓ wrap, Escape close, caret-leave closes without swallowing keys) is preserved.
- [ ] `SkillsDialog` + the `archimedes:insert-skill` flow are unchanged and their tests pass.
- [ ] `pnpm test` + `pnpm build` green.

---

### Task 6: Frontend — the send path + the display (`splitMentionBlocks` + the soft-hint chip)

**Context:** The last two integration points: `send()` expands via `expandMentions` (replacing `expandSkillMentions` — the placement/TDZ comment in `send()` stays), and `MessageBubble` renders the expanded text via `splitMentionBlocks` (replacing `splitSkillBlocks`). The skill block renders the EXISTING `SkillBlockCard` (unchanged); the agent/mcp blocks render a NEW compact single-line chip (the "named `<name>`" soft-hint — the copy policy: the user NAMED the resource, the agent may still reason it isn't needed).

**Files:**
- Modify: `src/components/ChatStream.tsx` (the `send()` line + the import)
- Modify: `src/components/MessageBubble.tsx`
- Modify: `src/components/MessageBubble.test.tsx`
- Modify: `src/components/ChatStream.test.tsx` (the send-expansion assertions)

**What to implement:**
- `ChatStream.tsx` `send()`: replace `const text = expandSkillMentions(rawText, skills);` with `const text = expandMentions(rawText, { skills, agents, mcpServers });` — KEEP the placement comment exactly (the TDZ trap warning + the `!text` equivalence note — update the function name in it). The import swap: `expandSkillMentions` → `expandMentions` (from `../lib/skills` — same module).
- `MessageBubble.tsx` (the `case "user"` branch):
  - `splitSkillBlocks` → `splitMentionBlocks` (import swap; the destructuring `{ text, blocks }` is unchanged).
  - `SkillBlockCard` is UNCHANGED and renders for `kind === "skill"` blocks (the `MentionBlock` shape is a superset — `kind` + `name` + `body`; `SkillBlockCard` receives `{ block: { name, body } }` — pass a structural subset or a small adapter; do NOT change the card's DOM / `WandSparkles` icon / collapse behavior).
  - Add `NamedResourceChip({ block }: { block: MentionBlock })` for `kind === "agent" | "mcp"` — a compact SINGLE-LINE chip (NO collapse — the hint is short): a `rounded-lg bg-input px-3 py-2` row (the card's container classes) with the kind icon (`BotIcon` for agent, `ServerIcon` for mcp — `lucide-react`, the `Icon`-suffix naming the file's existing lucide imports use, e.g. `WandSparklesIcon`), a `Agent` / `MCP` label span (the card's label styling), the name (the card's name styling), and the suffix text `named by the user` (`text-ui-xs text-foreground-subtlest`). No chevron, no expand.
  - The block list render: `blocks.map((b, i) => b.kind === "skill" ? <SkillBlockCard key={i} block={…} /> : <NamedResourceChip key={i} block={b} />)`.
- DO NOT change: the image grid, the `agent-text` / `agent-thought` / `tool-call` branches, the persistence (the sent text is the expanded text — the `mergeDedupeKey` invariant is unchanged).

**Steps:**
- [ ] Write/extend FAILING tests in `src/components/MessageBubble.test.tsx` (the existing skill-block-card tests are the template):
  - a user message with an `<agent>` block (the exact literal from Task 3) renders the chip: the `Agent` label + the name + `named by the user`; NO collapse affordance (no chevron; the hint body is not shown expanded).
  - an `<mcp>` block → the `MCP` label + the name.
  - a `<skill>` block still renders the existing `SkillBlockCard` (regression — the existing tests keep passing).
  - a mixed message (skill + agent + mcp blocks) renders all three in order.
  - a message with no blocks renders verbatim (regression).
  - `ChatStream.test.tsx`: the send-path assertions update to the three-catalog `expandMentions` (the existing "the sent text is the expanded text" assertions; a `#`/`@` token in the sent text; an unmatched `#` token passes through byte-identically into the sent text). The `MessageBubble.tsx` `case "user"` display comment that references `expandSkillMentions` ("`expandSkillMentions` appends `<skill>` blocks…") updates to `expandMentions` in the same cleanup.
- [ ] Run `pnpm test src/components/MessageBubble.test.tsx src/components/ChatStream.test.tsx`
  - Did the new tests fail?
- [ ] Implement the `MessageBubble` + `ChatStream` changes.
- [ ] Run `pnpm test` (full frontend suite) — all pass.
- [ ] Run `pnpm build` — type-check + build green.
- [ ] Run the FULL validation matrix (AGENTS.md): `pnpm test`, `pnpm build` (repo root); `cargo test`, `cargo clippy --all-targets` (0 warnings), `cargo fmt --check` (`src-tauri/`).
- [ ] Commit with message: `feat: expand @ / # mentions on send + the soft-hint chip display`

**Acceptance criteria:**
- [ ] On send, `#`/`@`/`$` tokens expand per the spec (hard skill block; soft agent/mcp blocks); unmatched tokens pass through byte-identically (the `mergeDedupeKey` invariant).
- [ ] The bubble renders the user text + the chips (the skill card unchanged; the distinct `named by the user` chip for `@`/`#`).
- [ ] The full AGENTS.md validation matrix is green.
- [ ] The `done-when` (front-matter) is observable end-to-end: type a prefix → the picker → select → send → the chip.
