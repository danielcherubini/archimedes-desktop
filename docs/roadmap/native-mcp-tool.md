---
status: in-progress
done-when: The native harness (`AgentLoop`) advertises and executes an `mcp` tool — a single gateway tool mirroring the `@pi-archimedes/mcp` proxy tool (same name, params, and search → describe → call workflow) — that loads MCP server definitions from the same `mcp.json` files pi reads and speaks to them over stdio and streamable HTTP, performing OAuth 2.1 (authorization_code + PKCE + DCR, client_credentials, refresh) for HTTP servers that require it.
---

# Native `mcp` tool Plan

**Progress:** ALL 8 TASKS DONE (config, types, rpc, stdio, http, oauth + callback, manager, tool, AgentLoop wiring — all green: 378 Rust tests + clippy 0 warnings + fmt clean).

**Goal:** A native-harness session (ADR 0011) loses the user's MCP servers today: the harness's tool set is built-ins only, and pi's built-in MCP (and the retired `@pi-archimedes/mcp` suite package) live on the pi-agent side. Give the harness the `mcp` tool so a native session can discover and call MCP-server tools exactly like a pi session.

**Architecture:** One new module tree `src-tauri/src/agent/mcp/` (sibling of `agent/harness/`):

- `config.rs` — load `~/.pi/agent/mcp.json` (global) + `<project>/.pi/mcp.json` (project override, highest precedence) — the SAME two files pi's built-in MCP reads (ADR 0024, pi-archimedes). `mcpServers` entries: HTTP = `{ url, headers?, auth?, bearerTokenEnv? }`, stdio = `{ command, args?, env?, cwd? }`; `disabled: true` skipped; `type: "sse"` (legacy) → not supported (pi ≥ 0.99 dropped it too). `imports` NOT supported (v1 — the desktop reads `mcpServers` directly; ADR 0012 precedent: the desktop is a read-only consumer of pi's config, best-effort — a missing/unparseable file degrades to "no MCP servers", never a crash).
- `types.rs` — `ServerDef` / `AuthSpec` (`None` | `Bearer { token | env_var }` | `OAuth { grant_type, client_id, client_secret, scope, redirect_uri, client_name, authorization_server_url }`) / `McpState` (Disconnected / Connecting / Connected / NeedsAuth / Error).
- `rpc.rs` — JSON-RPC 2.0 message build/parse (request, response, error, notification; batch not needed).
- `stdio.rs` — stdio client: spawn the process (`tokio::process`, `kill_on_drop`), newline-delimited JSON-RPC over stdin/stdout, `initialize` → `notifications/initialized` handshake, `tools/list`, `tools/call`; a per-request timeout (default 60 s, `requestTimeoutMs` overridable per server); cancellation via the turn token.
- `http.rs` — streamable-HTTP client (MCP spec): `POST {url}` with `Accept: application/json, text/event-stream`, a single JSON-RPC request per POST; the response is EITHER a JSON object OR a `text/event-stream` SSE stream (`event:`/`data:` lines — one `message` event carrying the JSON-RPC response); honor + resend the `Mcp-Session-Id` response header on subsequent requests; `initialize` → `notifications/initialized`; `tools/list`; `tools/call`; `DELETE` with the session id to terminate. Bearer token / static headers from the config; a 401 on an `auth: "oauth"` server triggers the OAuth path.
- `oauth.rs` — OAuth 2.1 for HTTP servers (ported from the suite's plan-026 design, ADR 0015 refresh strategy):
  - **Discovery**: a 401's `WWW-Authenticate: Bearer error="invalid_request_credentials", resource_metadata="…"` → RFC 9728 Protected Resource Metadata → `authorization_servers[0]` → RFC 8414 Authorization Server Metadata (`{url}/.well-known/oauth-authorization-server`, fallback `{url}/.well-known/openid-configuration`); or the config's `authorizationServerUrl` directly.
  - **DCR**: `POST {registration_endpoint}` with `client_name`, `redirect_uris` (the local callback), `grant_types`, `response_types: ["code"]`; a pre-registered `clientId` (+ `clientSecret`) skips DCR.
  - **authorization_code + PKCE**: random 32-byte verifier → S256 challenge (base64url); open the default browser (`xdg-open` / `open` / `cmd /c start`) at the authorization URL; wait for the local callback (`callback.rs`) on a bound 127.0.0.1 port, `state`-checked, 5-minute window, raced against the turn token; exchange the code at `token_endpoint` (client auth: `client_secret_basic` when a secret exists, `client_secret_post` otherwise).
  - **client_credentials** grant type: straight `token_endpoint` request (non-interactive).
  - **Refresh** (ADR 0015): an expired token with a `refresh_token` re-runs the refresh grant — EXCEPT a pre-registered PUBLIC client (config `clientId`, no `clientSecret`) is NEVER auto-refreshed (the auth server rejects the refresh grant with `invalid_client`); it degrades to a fresh interactive flow.
  - **Storage**: `~/.local/share/archimedes/mcp-auth.json` (the `dirs` app-data dir the rest of the desktop already uses, joined with `archimedes` — NOT the bundle identifier), `0600`, one entry per server: `{ client_id, client_secret?, token, refresh_token?, expires_at?, auth_server_url, scope? }`. (Not the OS keyring — no new system dependency; the file is the user's own home dir.)
- `callback.rs` — the local OAuth callback server: bind 127.0.0.1 (random free port), single-shot `/callback` handler, `?code=&state=` validation against the reserved state, a "success — you can close this tab" HTML page, timeout + cancellation; a bind failure is surfaced (the flow result carries the authorization URL so the user/model can complete it manually — the model's `ask` tool can relay the URL).
- `manager.rs` — `McpManager` (one per `AgentLoop`, constructed in `AgentLoop::new` with the space `cwd`): `server → (def, state, client)`; lazy connect on first use; `tools/list` cached per connection; `connect`/`status`/`close` actions; a failed connect is `Error { text }` / `NeedsAuth` (never silently dropped).
- `tool.rs` — the `mcp` tool handler: the `@pi-archimedes/mcp` proxy tool's param shape VERBATIM (`tool`, `args` (JSON string or object), `search`, `describe`, `connect`, `server`, `action`) and its result conventions (status lines, search matches with server + description, `describe` → full input schema, call → the server's `content[]` rendered as text lines + `(isError)` flag):
  - `mcp({})` / `action: "status"` → all servers + connection status
  - `mcp({ server })` → list that server's tools
  - `mcp({ search })` → tools matching name/description across servers
  - `mcp({ describe })` → one tool's full input schema
  - `mcp({ tool, args })` → call (raw name, or `server` to disambiguate)
  - `mcp({ connect })` → eagerly connect a server
  - `mcp({ action: "auth", server })` → run the interactive OAuth flow for that server (the native-harness extension — the suite had no TUI-less way to trigger auth; the model or a user instruction triggers it)
- **Wiring** (`harness/loop.rs`): a `dispatch_tool` match arm (`"mcp"` → the handler, raced against the turn token like the suite tools) + a `tool_specs()` entry (the pi-archimedes description verbatim).

**Tech Stack:** Rust (tokio process/net/io-util/time, reqwest rustls, serde, serde_json, uuid; NEW deps: `sha2` (PKCE S256), `rand` (PKCE verifier + state entropy)). No frontend work — the existing `mcp` tool-card rendering in `src/lib/toolOutput.ts` (verb "MCP", `PlugIcon`, primary-arg summary) already renders `mcp`-sourced tool calls.

**Out of scope (v1, documented):** `imports` (foreign client config import), legacy SSE transport, direct-tool exposure (`mcp__server__tool` expansion), `includeTools`/`excludeTools`/`toolPrefix` filtering, the metadata cache (offline search), the TUI panels / `/mcp` commands / config write-back (the desktop UI's MCP settings surface is a later feature), resource/prompt tools, `npx` command resolution.

**Global rules (every task):**
- TDD: failing test first, confirm it fails, then make it pass.
- Rust verification (run from `src-tauri/`): `cargo test` → `cargo clippy --all-targets` (0 warnings) → `cargo fmt --check`.
- The `mcp` tool is a SUITE tool (like `manage_todo_list`/`sudo_exec`): frame-free core + a `dispatch_tool` match arm — NOT a `tools/exec.rs` built-in.
- Cancellation: every blocking wait (connect, tool call, OAuth callback window) races the TURN token (`tokio::select!`), mirroring the suite-tool pattern in `dispatch_tool`.
- Best-effort config (ADR 0012 precedent): a missing/unparseable `mcp.json` = no servers (a `status` line says so), never an error for the whole tool.

---

### Task 1: `config.rs` — config load + merge

**Context:** No MCP config reading exists in the desktop. pi's built-in reads `~/.pi/agent/mcp.json` (global) and `<project>/.pi/mcp.json` (override) — the `mcpServers` shape (pi-archimedes ADR 0024: same two files, same shape, extra fields ignored).

**Files:**
- Create: `src-tauri/src/agent/mcp/mod.rs` (module decls + re-exports), `src-tauri/src/agent/mcp/config.rs`, `src-tauri/src/agent/mcp/types.rs`
- Modify: `src-tauri/src/agent/mod.rs` (`pub mod mcp;`)

**What to implement:**
1. `types.rs`: `ServerDef { http: Option<HttpDef>, stdio: Option<StdioDef> }` where `HttpDef { url, headers: BTreeMap<String,String>, auth: AuthSpec }` and `StdioDef { command, args: Vec<String>, env: BTreeMap<String,String>, cwd: Option<PathBuf> }`; `AuthSpec { None, Bearer { token: Option<String>, env_var: Option<String> }, OAuth { grant_type, client_id, client_secret, scope, redirect_uri, client_name, authorization_server_url — all Option } }` (the `OAUTH_CONFIG_FIELDS` set, validated: unknown fields dropped, a `{ token }` object = Bearer, `"oauth"` = default authorization_code, a non-`token`/non-OAuth object = `None`).
2. `config.rs`: `load_servers(home_dir: &Path, project_cwd: &Path) -> BTreeMap<String, ServerDef>` — read both files (a missing file = empty; a parse failure = that file's servers absent, never an error), project entries OVERRIDE global entries by name, `disabled: true` dropped, `type: "sse"` entries dropped (unsupported — a warning line is the tool's problem), `bearerTokenEnv` → `AuthSpec::Bearer { env_var }` (resolved at CONNECT time, not load time), `headers`/`auth` on a stdio def ignored.
3. Unit tests (temp dirs, real files): global-only, project-override-wins, disabled-dropped, sse-dropped, bearerTokenEnv, a parse failure degrades (global still loads), `auth` classification (the 5 shapes above).

### Task 2: `rpc.rs` — JSON-RPC 2.0

**Files:** Create `src-tauri/src/agent/mcp/rpc.rs`

**What to implement:** `RpcRequest { id, method, params }` → `to_json_line()`; `parse_response(line: &str) -> Result<RpcResponse, RpcError>` where `RpcResponse { id, result: Option<Value>, error: Option<RpcError { code, message, data }> }`; `parse_notification` (no `id`); a request-id counter helper. Unit tests: round-trips, an error response, a notification, a malformed line is an error.

### Task 3: `stdio.rs` — the stdio client

**Context:** MCP stdio transport = newline-delimited JSON-RPC 2.0 over a spawned process's stdin/stdout; `initialize` (params: `protocolVersion: "2025-06-18"`, `capabilities: {}`, `clientInfo { name: "archimedes", version }`) → the server's `initialize` response → the client's `notifications/initialized` → `tools/list` / `tools/call`.

**Files:** Create `src-tauri/src/agent/mcp/stdio.rs`; Test: `src-tauri/src/bin/fake_mcp_stdio.rs` (a dev-only binary: reads NDJSON from stdin, answers `initialize` / `tools/list` / `tools/call` (one canned tool `echo` returning its input as `content: [{type:"text",text}]`), `notifications/initialized` → 202-style no-response)

**What to implement:** `StdioClient::connect(def: &StdioDef, cwd: &Path, cancel: &CancellationToken, timeout: Duration) -> Result<StdioClient, String>` (spawn with `kill_on_drop`, the def's `env` ADDITIVE over the inherited env, the def's `cwd` over the session cwd; the initialize handshake with the timeout; an error response = `Err`); `list_tools(&self) -> Result<Vec<ToolInfo>, String>`; `call_tool(&self, name: &str, args: &Value) -> Result<ToolCallResult, String>` (`ToolCallResult { content: Vec<ContentBlock-shaped text items>, is_error: bool }` — `content[]` items: `text` joined, `image`/`resource` counted as `(+N non-text)`); a writer task (mpsc → stdin lines) + a reader task (stdout lines → oneshot/matching by `id`); every op races `cancel` (a cancel KILLS the process via `kill_on_drop` + an explicit `kill()`).
Integration tests (spawn the fake binary): connect + list + call round-trip; a request timeout (the fake hangs) → `Err`; a cancel mid-call kills the process (the child exits).

### Task 4: `http.rs` — the streamable-HTTP client

**Context:** The MCP streamable-HTTP transport (the suite's `server-client.ts` + the SDK's `streamableHttp.js`): one JSON-RPC request per `POST` to the server `url`; `Accept: application/json, text/event-stream`; the response is a JSON object OR an SSE stream (`event: message` / `data: <json>` lines; also `event: ping` — ignore); a `Mcp-Session-Id` response header is resent on every subsequent request; `initialize` → `notifications/initialized` (a notification = no response body expected, accept 202/200 empty); `DELETE` + the session header terminates.

**Files:** Create `src-tauri/src/agent/mcp/http.rs`

**What to implement:** `HttpClient::connect(def: &HttpDef, cancel, timeout) -> Result<HttpClient, String>` (the initialize handshake; the auth header from `AuthSpec`: `Bearer` token literal / env-resolved at connect time / `OAuth` → the `oauth.rs` token provider (Task 5 seam — a `fn get_token(&self) -> Option<String>` closure injected so this task doesn't depend on it); a 401 with `WWW-Authenticate` on an OAuth server → `Err("needs-auth: …")`); `list_tools` / `call_tool` (same shapes as Task 3); a `send_request` helper handling BOTH response media types (JSON parse / SSE line parse — reuse the SSE line-splitting discipline from `harness/provider.rs`); the session-id header; a per-request timeout (the `read_timeout`-style idle bound, NOT a total bound — a slow `tools/call` that dribbles SSE events stays alive).
Tests (raw `tokio::net::TcpListener` servers, the `provider.rs` `raw_http_server` pattern): a JSON response; an SSE response (dribbled `data:` lines); the session-id echo (the test server asserts the header on request 2); a 401 → `needs-auth`; a slow SSE stream completes (dribble under the timeout).

### Task 5: `oauth.rs` + `callback.rs` — OAuth 2.1

**Files:** Create `src-tauri/src/agent/mcp/oauth.rs`, `src-tauri/src/agent/mcp/callback.rs`; new Cargo deps: `sha2 = "0.10"`, `rand = "0.9"`

**What to implement:**
1. `callback.rs`: `CallbackServer::bind() -> Result<CallbackServer, String>` (127.0.0.1 random port, a `tokio` HTTP listener — a minimal single-shot request parser like the test servers, NO new HTTP dep); `wait(code_state: String, timeout, cancel) -> Result<CallbackResult, String>` (`CallbackResult { code, state_ok }`); a success HTML page; a bind failure = `Err` (the caller surfaces the manual-URL path).
2. `oauth.rs`:
   - `discover(auth_server_url: &str, client: &reqwest::Client) -> Result<ServerMetadata, String>` (RFC 8414: `{base}/.well-known/oauth-authorization-server` JSON: `authorization_endpoint`, `token_endpoint`, `registration_endpoint?`; fallback `{base}/.well-known/openid-configuration`).
   - `from_www_authenticate(header: &str) -> Option<String>` (the `resource_metadata="url"` value) + `resource_metadata(url) -> Result<Vec<String>, String>` (RFC 9728 `authorization_servers`).
   - `register_client(metadata, client_name, redirect_uri) -> Result<Credentials, String>` (DCR POST; the response `client_id`/`client_secret?`).
   - `pkce()` → `(verifier, challenge)` (32 random bytes → base64url; S256 = `base64url(SHA-256(verifier_ascii))` — a small base64url helper, the `base64` crate's URL-safe engine, padding stripped).
   - `exchange_code(metadata, creds, code, verifier) -> Result<StoredAuth, String>` (the token request; client auth basic/post per secret presence; parse `access_token`/`refresh_token?`/`expires_in?` → `expires_at`).
   - `client_credentials(metadata, creds) -> Result<StoredAuth, String>`.
   - `refresh(stored: &StoredAuth) -> Option<StoredAuth>`-ish: a refresh grant; the ADR 0015 guard (no `client_secret` + a pre-registered `client_id` → `None` = never auto-refresh).
   - `authenticate(def: &HttpDef, cancel) -> Result<StoredAuth, AuthError>` — the orchestrator: `client_credentials` grant type → straight token; `authorization_code` → DCR (unless pre-registered) → PKCE → open the browser (a `tokio::process` `Command` on the platform opener, fire-and-forget, a failure is non-fatal — the URL is in the result) → `CallbackServer` wait (5 min, raced against `cancel`) → `exchange_code` → store.
   - `load_credentials(path) / save_credentials(path, map)` — the `mcp-auth.json` file (0600 via `std::os::unix::fs::Permissions`; Windows: no chmod, the file lives in the user's profile).
   Tests (a mock OAuth server — a raw tokio HTTP handler serving the metadata JSON, the DCR endpoint, the token endpoint (asserting the `code`/`verifier`/client-auth it received), + a fake authorization endpoint that redirects to the callback URL with a `code`): the full authorization_code flow end-to-end (callback server real, browser-open stubbed via a seam); client_credentials; the refresh guard (public client → no refresh); a bad `state` is rejected; the storage round-trip (0600 assert on unix).

### Task 6: `manager.rs` — the server manager

**Files:** Create `src-tauri/src/agent/mcp/manager.rs`

**What to implement:** `McpManager { servers: BTreeMap<String, ServerEntry>, home_dir, project_cwd }` where `ServerEntry { def, state: McpState, tools: Option<Vec<ToolInfo>> }`; `McpManager::new(home, project_cwd)` (Task 1's `load_servers`); `status_lines(&self)` (the `mcp({})` output — `<name>: <state>[ (<error>)]`, `No MCP servers configured.` when empty); `ensure_connected(&mut self, name, cancel, timeout) -> Result<&Client, String>` (lazy connect; `NeedsAuth`/`Error` states are NOT auto-retried — `connect` action or a fresh `authenticate` is explicit); `list_tools` / `call_tool` (route to the connected client, cache `tools/list`); `connect(name)` (eager — the `connect` action; an OAuth server in `NeedsAuth` with `action: "auth"` → `authenticate`); `close_all()` (session teardown — kill stdio children, DELETE http sessions).
Tests: the status lines (no servers / a connected / an error / a needs-auth); the lazy connect (first `call` connects; a second call reuses — the fake server counts connections); `close_all` kills the stdio child (the process exits).

### Task 7: `tool.rs` — the `mcp` tool handler

**Files:** Create `src-tauri/src/agent/mcp/tool.rs`

**What to implement:** `pub async fn mcp_tool(manager: &mut McpManager, args: &Value, cancel: &CancellationToken) -> ToolResult` — the param dispatch EXACTLY as the `@pi-archimedes/mcp` proxy tool (its `buildProxyToolExecute`): the status default (no `tool`/`search`/`describe`/`connect`/`server`, or `action: "status"`); `server` → list that server's tools (name + description, one per line); `search` → matching tools across servers (`[server] name — description`); `describe` → the full input schema (JSON); `tool` (+ `args` string-or-object, a JSON-string parse failure is a tool error) → `call_tool` (the result text = the content items joined; `isError` → a `ToolResult` failure with the text); `connect` → eager connect (the outcome line); `action: "auth"` + `server` → `authenticate` (the result: `authenticated` / the authorization URL + manual-completion guidance on a bind failure / the error). Unknown `action` → an error listing the valid ones. A tool-name disambiguation: an exact raw-name match across servers wins; a tie → the `server` param required (an error naming the candidates).
Tests (a fake stdio server via the Task 3 binary + a `McpManager` over a temp config): every action; the ambiguity error; the `args` JSON-string path; the `isError` → failed `ToolResult` mapping.

### Task 8: wiring — `AgentLoop`

**Files:** Modify `src-tauri/src/agent/harness/loop.rs`

**What to implement:** `AgentLoop` gains a `mcp: McpManager` (constructed in `new` — `home_dir` from `dirs::home_dir`, `project_cwd` = `self.space_cwd`); a `dispatch_tool` match arm `"mcp"` → `mcp_tool(&mut self.mcp, &tc.arguments, &turn)` (the `&mut self` borrow is already the `dispatch_tool` signature; the suite-tool `select!`-against-`turn` pattern); a `tool_specs()` entry — the `@pi-archimedes/mcp` description VERBATIM (the workflow text) + the params schema (all optional — the tool is usable with no params = status); `close_session`/loop teardown → `self.mcp.close_all()`.
Tests: an `AgentLoop`-level test (the existing loop test harness pattern) — a session with a temp `mcp.json` (the fake stdio server) where the model (a canned provider response) emits an `mcp` tool call → the tool result round-trips into the transcript; a Stop mid-`mcp`-call cancels (the child is killed).
