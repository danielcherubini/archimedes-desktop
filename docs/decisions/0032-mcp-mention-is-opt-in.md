---
status: accepted
date: 2026-10-07
superseded-by:
---

# The `#` MCP mention ships OFF behind a setting

ADR 0031's grammar is sound but `#` is the one prefix that collides with what a coding user actually pastes: `#include`, `#123` (an issue number), `#hashtag` all satisfy the token grammar `[a-z0-9]+(?:-[a-z0-9]+)*`, so an accidental expansion lands whenever a server happens to share such a name (ADR 0031 named this and leaned on catalog-gating to shrink it — but "shrunk" is not "gone" for a prefix this common in code). We decided: the whole `#` machinery stays, and a persisted `mcpMentionsEnabled: bool` (`#[serde(default)]` → **off** for every existing `settings.json`, opt-in from Settings → General) gates whether it is live; `$` and `@` remain unconditional.

**Supersedes ADR 0031 PARTIALLY.** Only availability changes — the grammar (the asymmetric `$` vs `#`/`@` left boundary), the hard/soft split, the exact-match emergent tag-safety invariant, the picker-boundary `is_mentionable_name` filter, and the 1024-code-point caps all STAND untouched. `list_mcp_servers_effective` / `server_infos` / `McpServerInfo` also stand, un-gated and still fetched regardless of the flag: they are a cheap config read and the natural base for the deferred tool-level `#` picker.

**Considered Options**

- **Delete `#` outright** (drop `MentionKind`'s `"mcp"`, the block builder, the split parse, the chip, the IPC): rejected — it destroys the tested machinery and the `list_mcp_servers_effective` foundation for a tool-level `#` picker we may still want; a default-off flag costs one field and keeps re-enabling free.
- **Gate by narrowing the token grammar** (reject digit-only tokens, require a non-code-looking name): rejected — it fights the shared `$`/`@` grammar for one prefix and still cannot distinguish `#include` from a legitimately-named server; the collision is semantic, not lexical.
- **Gate the MCP catalog FETCH on the flag**: rejected — it is a cheap config read, and gating it would make a live toggle require a remount to take effect. The flag is a dependency of the `filtered` memo precisely so flipping it re-derives rows immediately.

**Consequences**

- **The gate is expressed as an EMPTY MCP catalog, not a new code path.** Both the picker and the expansion are already catalog-gated (ADR 0031), so `mcpServers: []` makes a `#` token match nothing: `expandMentions` returns the text BYTE-IDENTICAL via the existing verbatim invariant, `filtered` yields no rows so no picker opens, and the keyboard branches (also gated on `filtered.length > 0`) leave **Enter still sending** rather than swallowing keys. `src/lib/skills.ts` gained no flag parameter and is unchanged.
- **The gate covers the TRIGGER only — the display is deliberately NOT gated.** A message persisted while `#` was on keeps parsing its `<mcp>` block and rendering the "named by the user" chip, so toggling the setting off never corrupts or re-flows history, and `mergeDedupeKey` stays byte-stable across the toggle. Already-sent messages are NOT retroactively expanded (the text is persisted as sent).
- **Fail-safe default is OFF while settings are still loading** (`settings?.mcpMentionsEnabled ?? false`), so a `#` typed in the boot window does not expand — the safe direction, mirroring the `spinnerStyle ?? "typing"` precedent. A `getSettings` failure therefore leaves `#` off for the session.
