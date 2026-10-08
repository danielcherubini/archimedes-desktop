---
status: approved
done-when: Typing `$` / `#` / `@` in the composer opens a prefix-filtered picker (skills / effective MCP servers / user+space agent definitions); selecting inserts the token; on send, matched tokens expand into blocks (skill hard, agent/mcp soft-hint) sent + persisted with the user text, unmatched tokens pass through byte-identically; the bubble renders user text + chips (skill chip; distinct "named `<name>`" soft-hint chips for `@`/`#`); all AGENTS.md validation green.
---

# Composer mentions for Agent definitions and MCP servers

## Problem

The composer's `$`-skill mention (ZCode parity, `src/lib/skills.ts`) is the only way to name a resource explicitly in a message. Agent definitions (ADR 0020) and MCP servers (ADR 0018/0019) are agent-*discovered* only. Extend the mention mechanism to all three resource types with a consistent grammar: `$` → Skill, `#` → MCP server, `@` → Agent definition.

## Scope

**Composer mention UX only.** Agent-side discovery (system-prompt `<skills>` section, `list_agents`, the `mcp` tool) is **untouched**. No agent-behavior change beyond what the injected text asks.

## 1. Mention grammar & matching

| Prefix | Resolves against | Match regex |
|--------|-----------------|-------------|
| `$` | Skill catalog | `/\$([a-z0-9]+(?:-[a-z0-9]+)*)/g` — **unchanged** (ZCode parity) |
| `#` | MCP server catalog | `/(^|\s)#([a-z0-9]+(?:-[a-z0-9]+)*)/g` |
| `@` | Agent definition catalog | `/(^|\s)@([a-z0-9]+(?:-[a-z0-9]+)*)/g` |

- **Asymmetric left boundary, on purpose**: `$` keeps its boundary-free-left ZCode-parity form (changing it alters documented behavior); `#`/`@` require start-of-line or a leading whitespace (the `activeSkillToken` live-caret policy). Kills `x@y`, emails, `#[derive`-adjacent tokens; `#include` still matches `include` but only expands if a server named `include` exists.
- **Case policy** mirrors skills: tokens lowercase-only (`#DEBUG` passes through verbatim); catalog matching case-insensitive on the resource-name side.
- **Unknown token → verbatim** (no error, no stripping), exactly as skills do.
- One unified pass over all three prefixes; blocks in **text position order**, deduped across the run, first-mention order.

## 2. Expansion semantics — the hard/soft split

Client-side, at send: matched tokens expand into blocks appended after the user's text (the existing mechanism). The live bubble, the persisted record, and the agent's input all carry the **same** text (the `mergeDedupeKey` load-bearing invariant); expansion is a pure function of text + catalogs (deterministic).

- **`$skill` — HARD (unchanged):** the existing `<skill name location>` block (frontmatter name verbatim, path, dir, body). The agent follows the injected instructions.
- **`@agent` — SOFT hint** (name + one-line description; full-definition injection was rejected):

  ```
  <agent name="NAME">
  NAME — DESCRIPTION
  The user has explicitly named the agent definition "NAME" for this
  request. Dispatch a subagent with agentName "NAME" to handle it
  (list_agents / the definition's frontmatter define its model, tools,
  and system prompt; explicit tool params layer over the definition —
  ADR 0020).
  </agent>
  ```

- **`#mcp` — SOFT hint** (no eager-connect, no tool-name injection — the tools list is per-session and unavailable at send-time, ADR 0018/0019):

  ```
  <mcp name="NAME">
  The user has explicitly named the MCP server "NAME" for this
  request. Connect to it via the mcp tool (mcp({ connect: "NAME" }))
  and use its tools. (Server summary: URL or command+args.)
  </mcp>
  ```

- **Framing (load-bearing for copy/UX):** `$skill` is a hard trigger (the injected text *is* the task); `@`/`#` are soft pointers (the agent may still reason the resource isn't needed). Picker and chip copy must read "the user named this resource," never "force-loaded."
- **No matched token → return the input unchanged** (byte-identical — the no-mention `mergeDedupeKey` guarantee).

## 3. Composer picker & display

- **Live token detection**: `activeSkillToken` generalizes to `activeMentionToken(value, caret)` → `{ prefix: '$'|'#'|'@', remainder, start }`; the span between the nearest preceding whitespace (or start) and the caret, with Section 1's per-prefix boundary policy.
- **Picker**: `ComposerSkills` → `ComposerMentions` — one list, filtered to the active prefix's catalog (so `$`/`#`/`@` never mix in one open list). Same keyboard model (↑/↓ wrap, Enter/Tab select, Escape close, caret-leave closes without swallowing keys). Rows: name (primary) + one-line description (secondary, `title` tooltip) + prefix badge.
- **Select** splices `#name ` / `@name ` / `$name ` into the draft at the token (existing insert path).
- **Send**: `expandSkillMentions` → `expandMentions(rawText, skills, mcpServers, agents)` — the unified pass of Sections 1/2.
- **Display**: `splitSkillBlocks` → `splitMentionBlocks` — parses `<skill>`/`<agent>`/`<mcp>` blocks back out (same robustness: contiguous-suffix shape check; malformed → verbatim, no-loss). `<skill>` renders the existing hard chip; `<agent>`/`<mcp>` render a **distinct "named `<name>`" soft-hint chip** (signals the hard/soft difference in the UI).

## 4. Catalogs & IPC

- **Skills**: unchanged (`useSkillCatalog`, module-level promise cache keyed by space).
- **Agents — new command `list_agent_definitions_for_space(cwd)`**: user-level + space-level definitions (`.agents/agents` + `.pi/agents` walked to the repo root, then user-level) — the same set the harness's `agentName` resolution honors (ADR 0020). The existing `list_agent_definitions` command stays byte-identical (Settings/subagent callers untouched). Returns `AgentDefinitionDto` (name + description; the picker doesn't need model/tools).
- **MCP — new command `list_mcp_servers_effective(cwd)`**: the three-layer merged set (pi-global `~/.pi/agent/mcp.json` + desktop `settings.json` + pi-project `<cwd>/.pi/mcp.json`, project > desktop > pi-global, ADR 0019) as `name → { type (http|stdio), summary (url | command+args) }`. A config read only — NO live connect (the per-session `McpManager` rule, ADR 0018/0019, is untouched). A new command — the Settings page surface is unchanged (ADR 0019's "effective view is a later extension" stands for Settings; the mention picker is not the Settings surface).
- **Freshness**: `useMentionCatalogs(spacePath)` reuses the `useSkillCatalog` pattern — module-level promise cache keyed by space, `[]`-on-key-change (a stale Space's rows never expand), failed fetches degrade to `[]`. Space-scoped rows (space-level agents/servers) are only nameable in that Space — matching the widened dispatch set.

## 5. Validation

Per AGENTS.md: `pnpm test` + `pnpm build` (repo root); `cargo test` + `cargo clippy --all-targets` (0 warnings) + `cargo fmt --check` (`src-tauri/`). TDD throughout: failing test first.

**Key new test surfaces:**

- Regex/matching: `#`/`@` left-boundary cases (`x@y`, `user@host`, `#include` with/without a server named `include`, `#DEBUG`, line-start, leading-tab), verbatim pass-through of unknown tokens, dedup, text-position ordering across all three prefixes.
- Expansion: byte-identity with no matched token (the `mergeDedupeKey` guarantee, incl. resume-merge), deterministic re-expansion, the exact block literals for `<agent>`/`<mcp>`.
- Split/display: round-trip (expand → split = user text verbatim + chips), malformed-block → verbatim no-loss, mixed skill+agent+mcp blocks.
- New Rust commands: `list_agent_definitions_for_space` (space-level + user-level, HOME-pin test pattern per `commands/agents.rs`; space-level definition in a temp dir appears, user-level via isolated `HOME`), `list_mcp_servers_effective` (three-layer merge precedence: project > desktop > pi-global, shadowing by name; no network — config read only).
