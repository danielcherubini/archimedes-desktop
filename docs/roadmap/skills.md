---
status: approved
done-when: In a Space with skills (or with user-level skills): the left pane lists them (name + description, re-discovered on active-Space change); typing $ in the composer opens a filterable picker and selection inserts $name; sending a message containing $name delivers the skill's full content to the agent in the standard <skill> block format in BOTH native and external sessions (visible in the recorded transcript); an unmatched $token passes through verbatim; a skill with a >1024-char description is not listed; a malformed SKILL.md is skipped without a crash.
---

# Skills (v1)

**Goal** — the Client browses and invokes **Skills** (Agent Skills spec: a directory with a `SKILL.md`). The desktop discovers skills from the standard locations, lists them in the left pane, and expands `$name` mentions from the composer into the skill's full content on send — so **native and external sessions behave identically** (desktop-owned, ADR 0013; the agent's own native skill support is untouched and composes with this).

## 1. Skill discovery (Rust)

- **Roots** (the same roots pi scans):
  - *Space scope*: from the Space's path, walk up to the repository root (a `.git` marker; if none, the Space path itself is the only level) — at each level, collect `<dir>/.agents/skills` and `<dir>/.pi/skills` (innermost first).
  - *User scope*: `~/.agents/skills` and `~/.pi/agent/skills`.
- **Walk**: each root is walked recursively for `SKILL.md` files — bounded (max depth ~4), skips `node_modules`, symlinks resolved via realpath for dedupe.
- **Frontmatter parsing** (best-effort):
  - `SKILL.md` = optional YAML frontmatter (`name`, `description` — description may be multi-line/block style) + a markdown body.
  - YAML parse fails → loose fallback (single-line `name:`/`description:` scan); no frontmatter at all → `name` = the skill's directory name, `description` = "" (a hand-rolled skill is still listed, not dropped).
  - `description` > 1024 chars → the skill is skipped.
  - A skill that fails to read/parse is skipped, never a crash.
- **Dedupe**: by lowercased name, first discovered wins — scan order: Space roots (innermost first) then user roots, so a Space skill shadows a same-named global skill (project-specific overrides global; mirrors pi's "first discovered" collision rule).
- **Surface**: one new Tauri command `list_skills(space_path) -> Vec<SkillInfo>` where `SkillInfo = { name, description, path (the SKILL.md file), dir (the skill directory), scope: "space" | "user" }`. Discovered on app start (active Space) and whenever the active Space changes; re-discovered at every `send_prompt` (cheap, always fresh).

## 2. Left pane — Skills section

- **Placement**: a new top-level section in the left pane (`SpacesList`), between the "New Session" / "Open Space" button row and the "Sessions" list (the ZCode placement — Skills is a top-level item above the task list, not nested under a Space).
- **Contents**: the `list_skills` result for the ACTIVE Space — its skills (all ancestor levels) + the user-level skills, one row each:
  - the skill's **name** (primary line) + **description** (secondary line, truncated to one line with an ellipsis; full text in a `title` tooltip),
  - a subtle scope cue: user-level rows get a small "global" hint; Space-level rows get none,
  - rows sorted by name (deterministic).
- **Interaction**: clicking a row inserts `$name ` at the caret (same as the picker would) and focuses the composer. No expand/collapse, no per-skill actions (v1 has no enable/disable or management).
- **States**: empty → a single muted line "No skills found." No loading spinner (discovery is a local disk walk); the section re-renders when the result arrives. No refresh button (re-discovered on active-Space change + at app start).
- **Data plumbing**: a small Zustand store slice — `skills[spacePath]` cache, invalidated on active-Space change. No new Tauri events — a plain command call.

## 3. Composer — `$` trigger + picker

- The composer stays a plain `<textarea>` (no contenteditable rewrite).
- **Trigger detection** (on every keystroke, while the composer is enabled and not locked): the text between the caret and the nearest preceding whitespace or start-of-line; if it starts with `$`, it is an active skill token — the picker opens, filtered by the remainder. A bare `$` (empty remainder) opens the picker with the full list. The token must not contain whitespace; any other text → picker closed.
- **Picker**: a popup rendered ABOVE the composer (anchored to the textarea), listing the active Space's skills (the same `list_skills` data as the left pane): each row = name + one-line-truncated description. Filter = case-insensitive substring match on the name (v1: name only). Keyboard: ↑/↓ move the highlight, Enter selects, Esc closes; click selects. While the picker is open, the textarea's key handling defers to the picker for those keys.
- **On select**: the active `$…` token is replaced with `$name ` (trailing space, caret after it); the picker closes; focus stays in the textarea. The token is PLAIN TEXT in v1 — no chip rendering (a styled chip needs the contenteditable rewrite; follow-up polish).
- **Send semantics**: the text is sent as-is (the `$name` tokens are NOT stripped client-side) — expansion happens in the desktop (Section 4), so the recorded message is identical to what the agent sees.

## 4. Expansion on send (Rust, harness-agnostic)

- **Where**: inside the `send_prompt` Tauri command handler, BEFORE the user message is recorded and BEFORE it is dispatched to the harness (native or external). Consequence: the RECORDED user message = what the agent saw — for external sessions pi records the same expanded text in its own session file (desktop and agent transcripts agree); for native sessions the SQLite store is the only store and a resume re-reads the expanded message (no consistency hole). A collapsed rendering of the injected block in the transcript is a possible UI polish, out of v1 scope.
- **Scan**: `/\$([a-z0-9]+(?:-[a-z0-9]+)*)/g` (ZCode's exact regex) over the user text. Each captured name is matched CASE-INSENSITIVELY against the freshly re-discovered skills. Multiple occurrences of the same token → one block. A token matching no skill is left verbatim (no error, no stripping).
- **Injected block** — appended after the user's text, separated by a blank line, one per matched skill (in the order first mentioned). The format mirrors pi's own native `/skill:name` expansion VERBATIM, so the agent sees the same shape whether it loaded the skill itself or the desktop injected it:

  ```
  <skill name="NAME" location="/abs/path/to/SKILL.md">
  References are relative to /abs/path/to/skill-dir.

  BODY
  </skill>
  ```

  where `BODY` = the SKILL.md content minus the frontmatter. (pi's format also supports `args` after `/skill:name` — the mention model has no args: the token is a pure trigger, the rest of the message is the user's request.)
- **Images/attachments**: untouched — expansion is text-only and composes with the existing image path. **Steer/follow-up**: the same `send_prompt` path covers them, so queued messages get the same expansion.

## 5. Out of scope (v1)

1. **Per-skill enable/disable** — no toggle, no persisted state; a skill you don't want is just not mentioned (or moved out of a skills root).
2. **Skill management in the UI** (create/edit/delete/copy-to-common) — skills are edited in the filesystem / with the `write-skills` skill; the desktop is a reader, never a writer (v1: zero write access to skill directories — also a safety property).
3. **Native-harness skill catalog** (injection of the skill list into the native session's system prompt, so a native agent can auto-discover skills without a mention) — the native harness has no prompt-construction surface yet; explicit `$name` invocation is the only native path until one exists. (External sessions already get pi's native auto-discovery for free.)
4. **Rich mention chips in the composer** (a styled pill instead of plain `$name` text) — needs the contenteditable rewrite; the plain token is functional today.
5. **Collapsed rendering of injected skill blocks in the transcript** — the transcript shows the injected content inline (long, but honest: it is what the agent saw).
6. **Drift from pi package/extension-added skills** — the desktop scans the standard roots only; a skill added by a pi package is visible to the agent but absent from the left pane until the desktop gains package awareness.
7. **Skill sync / marketplace / remote skills** — no network dimension in v1; skills are local files.
8. **Auto-refresh of the skill list** — re-discovered on active-Space change + app start + every send; no watch, no timer.
