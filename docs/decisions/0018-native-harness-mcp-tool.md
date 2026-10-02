---
status: accepted
date: 2026-10-02
superseded-by:
---

# The native harness speaks MCP itself: a single `mcp` proxy tool, pi's `mcp.json` as the config source, file-stored OAuth

A native-harness session (ADR 0011) has NO access to the user's MCP servers: the harness's tool set is built-ins + suite tools, and MCP lives on the pi-agent side (pi ≥ 0.99's built-in MCP; the `@pi-archimedes/mcp` suite package, deprecated per its ADR 0024). We decided: the harness gets ONE `mcp` tool — a **proxy** tool with the `@pi-archimedes/mcp` package's param shape VERBATIM (`tool` / `args` / `search` / `describe` / `connect` / `server` / `action`; the search → describe → call workflow) — that loads server definitions from the SAME two files pi's built-in MCP reads (`~/.pi/agent/mcp.json` + project `.pi/mcp.json`, `mcpServers` shape; project overrides global) and speaks to them over **stdio** (spawned process, newline-delimited JSON-RPC 2.0) and **streamable HTTP** (one JSON-RPC request per POST, JSON or SSE response, `Mcp-Session-Id` session header). HTTP servers authenticate with static headers / `Bearer` tokens (literal or env-var) or **OAuth 2.1**: `authorization_code` + PKCE + dynamic client registration (a pre-registered `clientId`/`clientSecret` skips DCR), `client_credentials`, and refresh with the ADR 0015 (pi-archimedes) config-stub guard — a pre-registered PUBLIC client (config `clientId`, no `clientSecret`) is NEVER auto-refreshed (the auth server rejects the refresh grant with `invalid_client`); it degrades to a fresh interactive flow. The interactive flow opens the default browser and waits on a 127.0.0.1 callback server (5-minute window, raced against the turn token; a bind failure surfaces the authorization URL for manual completion).

**Considered Options**

- **Expose each MCP server's tools as DIRECT tools** (`mcp__server__tool`, the suite's `directTools` mode + `toolPrefix` strategies): rejected for v1 — the proxy tool is the model's single entry point (one advertised spec instead of N×M), matches the suite's primary interface, and keeps the harness's tool list stable; direct exposure (with prefix/filter settings) is a later extension.
- **Reuse pi's built-in MCP by running a pi sidecar just for MCP**: rejected — a second process + an inference protocol (the ADR 0011 rejection rationale) for what is a plain JSON-RPC client; the desktop is the harness (ADR 0011) and speaks the wire itself.
- **A desktop-owned MCP config surface** (its own settings + UI): rejected for v1 — the first-generation contract: the desktop is a read-only CONSUMER of the user's existing pi setup (best-effort: a missing/unparseable `mcp.json` degrades to "no MCP servers", never a crash). A desktop MCP settings surface is a later feature (it needs the UI panel work the suite's TUI panels had).
- **OS-keyring credential storage** (the suite's `auth-storage.ts`): rejected — a new system dependency (keyring backends differ per platform); the credentials live in `~/.local/share/archimedes/mcp-auth.json` (the desktop's existing `dirs` app-data root, `0600`) — the user's own home dir.

**Consequences**

- **The desktop reads pi's `mcp.json` files** — a coupling to their shape (the `mcpServers` entries: `url`/`headers`/`auth`/`bearerTokenEnv` for HTTP, `command`/`args`/`env`/`cwd` for stdio). If pi changes the shape, the harness's MCP degrades (best-effort), it does not break.
- **`imports` is NOT supported** (the suite/pi built-in's foreign-client config import, e.g. from Cursor/Claude Code): v1 reads `mcpServers` only — a server must be in a `mcp.json` `mcpServers` to be visible. Legacy `type: "sse"` entries are dropped too (pi ≥ 0.99 dropped them).
- **One manager per session** (constructed at `AgentLoop` start with the space `cwd`): no cross-session connection sharing (a second session reconnects); `close_session` kills stdio children and DELETEs HTTP sessions.
- **OAuth is triggered explicitly** — a `NeedsAuth` server does NOT auto-start an interactive flow on a tool call (the suite's `autoAuth: false` default): the tool result says `needs-auth`; `mcp({ action: "auth", server })` (a native-harness extension — the suite's auth entry was the TUI `/mcp auth` command) runs the flow.
- The native `mcp` tool is a **suite tool** (frame-free core + a `dispatch_tool` match arm, raced against the turn token) — NOT a `tools/exec.rs` built-in; its tool-card rendering needs no frontend work (the existing `mcp` source case in `src/lib/toolOutput.ts`).
