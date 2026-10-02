---
status: accepted
date: 2026-10-02
superseded-by:
---

# User-managed MCP servers in Settings (the config surface ADR 0018 deferred)

ADR 0018 gave the native harness its `mcp` proxy tool reading the SAME two files pi's built-in MCP reads (`~/.pi/agent/mcp.json` + `<project>/.pi/mcp.json`, `mcpServers` shape) and **rejected** a desktop-owned MCP config surface for v1 ("the ADR 0012 contract: the desktop is a read-only CONSUMER of the user's existing pi setup; a desktop MCP settings surface is a later feature"). That later decision is now: the desktop's **Settings page owns an MCP-servers list** — a `mcpServers` field in the desktop's own `settings.json`, using pi's entry shape VERBATIM (`url` / `headers` / `auth` / `bearerTokenEnv` for HTTP, `command` / `args` / `env` / `cwd` for stdio) so an entry copy-pastes straight between the two files. The effective server set is the existing merge (global `~/.pi/agent/mcp.json` + project `<cwd>/.pi/mcp.json`) with the desktop layer merged in at **global precedence** — project > desktop > pi-global (the most specific layer wins; a desktop entry with the same name OVERRIDES a pi-global entry, the ADR 0014 user-wins-on-clash pattern). pi's files stay READ-ONLY: the desktop never writes them.

**Scope (deliberately narrow):** the Settings MCP section lists the desktop's OWN entries only — pi's global / project entries are NOT shown (the user sees them where they always did: the `mcp.json` files). Each row: the name, a type badge (HTTP / stdio), a one-line summary (the url, or `command` + args), a per-row **Test** action (a one-shot bounded connect + `tools/list` — the tool count or the error, mirroring the provider refresh), and edit / remove. Add / edit is a **structured form dialog** (name + an HTTP/stdio toggle + the type's fields; `headers` / `env` as KEY=VALUE lines, `args` one-per-line) — no raw-JSON editor in v1.

**Why:** the desktop is the harness (ADR 0011) and pi is being phased out — a desktop-owned MCP store is the durable home for "which MCP servers does this app talk to", symmetric with the provider store (ADR 0014). The `settings.json` home (vs a separate file or writing pi's file) keeps ONE desktop config file, reuses the existing `Settings` serde plumbing (`#[serde(default)]` — a pre-feature file parses unchanged), and the verbatim entry shape means a user migrating from pi's file pastes entries as-is.

**Considered Options**

- **Write pi's `~/.pi/agent/mcp.json` directly** (one source of truth, pi would see the same entries): rejected — it breaks the ADR 0012 read-only-consumer contract and races with pi's own edits of the same file.
- **A separate `~/.local/share/archimedes/mcp.json`** (next to the ADR 0018 `mcp-auth.json`): rejected — a third config file the user has to know about for what `settings.json` already hosts (the providers).
- **The UI also shows pi's entries read-only (edit / remove desktop entries only):** rejected for v1 — the user chose a desktop-only list (simpler; pi's entries stay in their own files). A read-only "effective" view is a later extension.
- **A raw-JSON entry editor:** rejected — a structured form is approachable + validates the shape; a paste-JSON helper is a later extension.
- **Per-server live status columns** (connected / tools, auto-refreshed): rejected for v1 — the `McpManager` is per-session (one per `AgentLoop`, ADR 0018); a settings-page live view would need a background manager. The on-demand Test action gives the signal the user needs.

**Consequences**

- **`settings.json` gains `mcpServers`** — a `name → entry` map in pi's shape. A pre-feature file parses to `{}` (`#[serde(default)]` — no migration).
- **The harness's effective set is a THREE-layer merge** — pi-global < desktop < pi-project. A desktop entry shadows a same-named pi-global entry; a project entry shadows both. `McpManager::new` gains the desktop layer (loaded from `settings.json` via the session manager's `config_dir`, threaded onto the `AgentLoop`).
- **The Test action is one-shot + bounded** — a throwaway client (no manager entry, no cross-session sharing, ADR 0018's per-session rule is untouched): connect + `tools/list` under a ~10 s bound, the client is dropped (a stdio child is `kill_on_drop`). A `needs-auth` result surfaces as the error text (the `mcp` tool's `auth` action remains the way to complete OAuth in-session).
- **Desktop entries are GLOBAL** — a desktop entry applies to every Space (like the providers); a per-project override is still the project `mcp.json`'s job.
