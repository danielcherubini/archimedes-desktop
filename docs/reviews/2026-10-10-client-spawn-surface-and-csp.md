# Client Spawn Surface and CSP — 2026-10-10

## Summary

One finding, **high severity but latent**: `test_mcp_server` reaches `Command::new` with a program, arguments, environment and working directory supplied by the Client, and the app serves **no Content-Security-Policy at all**, so any script that runs in the app's own window can reach every registered command. Nothing in the app currently requires the command to accept what it accepts.

Two options are worth more than the one this document originally recommended, and the ranking below reflects that. No decision has been made. **This is a report, not a plan**: it records evidence and options so that one decision can be taken instead of rediscovered every review.

**Provenance.** Found while reviewing the out-of-Space `?` completion (ADR 0035, squash `c02ae68`). That feature is not the cause and adds no new class of access; it is the change that made someone enumerate the registered commands. Everything here predates it.

**What this replaces.** An earlier version lived at `docs/roadmap/client-spawn-surface-and-csp.md` and was wrong in four ways, all corrected below: it claimed the Test action needs to test a **draft** entry (it does not — nothing does); it called the command ACL no help, which reads as a Tauri limitation rather than our current choice; it said images arrive over the asset protocol (they do not); and it promised that an enumerated command list "cannot grow silently", which a hand-edited list cannot deliver.

## Evidence provenance

Every file:line below was re-read against this tree. Three claims came from a review pass and could **not** be re-checked here, so they are marked and should be measured before anything depends on them:

- ✅ **verified in this session**: the spawn chain and its absence of gates; `csp: null`; the capability file's contents; the absence of `__app-acl__` and of `src-tauri/permissions/`; the ACL gate condition and its reject path; the unconditional injection of `__TAURI_INTERNALS__` and its `invoke`; the invoke-key's scope; Shiki's inline `style` output (19 on a four-line snippet) and its escaping of `<`; images being `blob:` and `data:`; the browser-opening spawn; `AccessPolicy::default() == Allow`; the single `Sandbox::install` call site; the absence of any test pinning the command list.
- ⚠️ **not re-checked here**: that `react-style-singleton` and `motion` inject `<style>` elements at runtime and would interact with `style-src`; that a ~20-line Shiki transformer drives the inline-style count to zero; whether `csp` is applied when the document comes from `devUrl`; whether `style-src-attr` is supported by all three webviews.

---

## What is true today

| Fact | Where |
|---|---|
| `test_mcp_server(name: Option<String>, entry: Value)` | `src-tauri/src/commands/settings.rs:126` |
| the spawn: `Command::new(&def.command)`, `.args(&def.args)`, `.envs(&def.env)` (**ADDITIVE** — its own comment says the inherited env plus the extras), `.current_dir(def.cwd.unwrap_or(cwd))` with `cwd = Path::new(".")` | `src-tauri/src/agent/mcp/stdio.rs:57-63`, `commands/settings.rs:130` |
| no gate anywhere on the path: no allowlist, no saved-config lookup, no trust flag, no rate limit, no dev-mode guard | `agent/mcp/types.rs` (`classify_server`) → `agent/mcp/manager.rs:306-334` → `stdio.rs:57-68`; `build.rs` is `tauri_build::build()` |
| `auth_mcp_server` does not spawn a server, but it **does** spawn: `open` / `cmd /c start "" <url>` / `xdg-open`, with a URL taken from the caller's entry | `commands/settings.rs:137`, `agent/mcp/manager.rs:339-353, 365-375` |
| `"csp": null`; the only capability is `windows: ["main"]` with **no `remote` block** | `src-tauri/tauri.conf.json` (`app.security`), `src-tauri/capabilities/default.json` |
| no `__app-acl__` in `gen/schemas/acl-manifests.json`; no `src-tauri/permissions/` directory | verified directly |
| no test pins the registered command list: `src-tauri/tests/ipc.rs:100` builds **its own** `generate_handler![…]`, independent of `src-tauri/src/lib.rs:162` | verified directly |

**Nothing in the app needs this shape.** There is one Test affordance in the whole UI — `McpSection.tsx:158` — and it calls `testMcpServer(entry, name)` with an entry taken from `settings.mcpServers`, i.e. **a saved server**. The add/edit form saves immediately and has no Test action, and the repo's own test says so out loud: `SettingsPage.test.tsx:1219`, *"The one-shot test (the entry as saved)"*. ADR 0019 asks for exactly that — a per-row Test action — and never asks for draft testing. **The command takes a whole entry because it was written against the entry shape, not because a feature requires it.**

## The threat model, stated precisely

- `withGlobalTauri` is unset and controls **only** the plugin API on `window.__TAURI__`. Regardless of it, every main frame gets `window.isTauri` and `window.__TAURI_INTERNALS__` as an initialisation script (`tauri-2.11.5/src/manager/webview.rs:166-181`), and `__TAURI_INTERNALS__.invoke` is defined by that injected script (`scripts/core.js:81`). **So script injected into the app's own document can call the IPC entry point without importing anything.**
- There is a per-run secret: `generate_invoke_key()` — 16 random bytes, Z85 (`tauri/src/lib.rs:1250-1254`), templated into a closure in `scripts/ipc-protocol.js:12` and required on every request (`webview/mod.rs:1747-1758`). It authenticates the **transport**, not the caller — `invoke` closes over the key, so a caller that cannot read the key still sends it by calling `invoke`. What it bars is a caller that cannot run the page's own scripts, which is the remote-frame case.
- **Remote origins cannot reach custom commands.** The ACL is checked for plugin commands, for remote content, and for local content only when the app defines an ACL manifest (`webview/mod.rs:1819-1826`); with none defined, remote content is rejected — in release builds with `Command <x> not allowed by ACL` (`:1848-1850`).
- **There is no known injection sink**, which is why this is latent. One `dangerouslySetInnerHTML` in the tree (`MessageBubble.tsx:329`), fed by Shiki, which escapes `<` to `&#x3C;` (measured); `react-markdown` v10 with `remarkGfm` and **no `rehype-raw`**, so raw HTML arriving in agent output renders as text. Exploiting the surface needs a sink that does not exist yet, which is what makes it cheap to close now.

**Read plainly: a bug in the renderer is a bug on the machine, and the app currently claims no containment on the far side of that.** That may be the right answer — it is the answer Tauri's architecture nudges toward — but it is not written down anywhere, so every feature infers it independently.

## Two facts that bound the severity

1. **The agent can already run arbitrary commands with no prompt.** `bash` is policed in the `Shell` direction (`agent/harness/loop.rs:72-79`) and `AccessPolicy::default()` is `Allow` with `shell == Allow` (`agent/policy.rs:283-291`). This finding is a **second** door into a room the app opened deliberately, and "make a renderer bug stop being a code-execution bug" is not a goal any option below fully achieves.
2. **The MCP spawn is not covered by the sandbox the app already ships.** `Sandbox::install` has exactly one call site, `agent/tools/exec.rs:294`. A user who chose Sandboxed still gets an unconfined, full-environment child from `test_mcp_server`.

## Options, ranked

1. **Deny-by-default command ACL — config only, and it fixes the property the previous version of this document wanted.** Tauri checks the ACL for local content **if the app defines an app ACL manifest**, and the manifest exists as soon as `src-tauri/permissions/` yields at least one permission (`tauri-build-2.6.3/src/acl.rs:408-410` → `APP_ACL_KEY` → `has_app_manifest()` → `webview/mod.rs:1823`). A command that no capability names is then rejected in Rust, on all three platforms, and debug builds name the missing permission. **So "the surface cannot grow silently" becomes structural**: a newly registered command is dead until someone grants it in a security file a reviewer must read. `src-tauri/capabilities/default.json` becomes the enumerable surface; if a test is wanted, snapshot that file, because it moves only when someone deliberately grants something. This replaces the command-list test this document used to propose, which had no failure mode except being deleted.
2. **Serve a CSP.** Worth having; it is the only option whose whole mechanism is "prevent script from running". Three constraints that must be measured rather than guessed, plus two open questions:
   - `style-src`: Shiki emits inline `style` attributes into markup that arrives through `dangerouslySetInnerHTML`, and Tauri's automatic nonce/hash injection reaches only the built HTML asset. ⚠️ Whether a transformer to generated classes, or the `style-src-attr` split, is viable is **unmeasured** here and unverified against WebKitGTK, WKWebView and WebView2.
   - `img-src 'self' blob: data:` — images are object URLs (`chatAttachments.ts:58`) and base64 `data:` URLs (`MessageBubble.tsx:527`); **the asset protocol is not used anywhere in `src/`**. Omitting `blob:` blanks every staged thumbnail and every image in restored history.
   - `connect-src` must admit the IPC origin, or IPC **silently** degrades to `postMessage` (`scripts/ipc-protocol.js:55-68`, whose comment names a CSP error as a cause). The consequence cuts both ways: a CSP can neither secure nor break the IPC door, and getting it wrong is invisible.
   - ⚠️ Whether `csp` is even applied when the document comes from `devUrl` is unverified, and CI is ubuntu-only — so the environment that would catch a mistake may not exist.
3. **Confine the spawn, and stop handing it the whole environment.** Reuse the Landlock ruleset the app already owns (`agent/tools/sandbox.rs`) at the MCP spawn, and pass an explicit environment instead of inheriting. Linux-only, but it survives every other failure above and it is the option that shrinks the blast radius rather than the entry points.
4. **Narrow the command to a name and look the entry up server-side.** Costs **no UI change** today, since the Test action already passes the saved entry. It is a **speed bump, not a boundary**: a caller that reaches this command also reaches `save_settings` (`commands/settings.rs:26`), so planting an entry and testing it by name is two calls instead of one. Its residual value is that the program name leaves the wire, which makes the call loggable.
5. **Write the trust model down**, whatever else is chosen. A draft decision sentence, containing nothing unverified: *"The Client is trusted content. Every command registered in `src-tauri/src/lib.rs` is reachable from the app's own window, and no command is reachable from a remote origin. **A renderer bug is treated as a machine compromise, not as a sandbox escape**: the app claims no containment beyond that point, and closes script-injection bugs as bugs. What the app bounds is the **agent's** reach — the Boundary and the per-direction policy of ADR 0030. If containment of the renderer ever becomes a requirement, the mechanism is a confirmation drawn by the operating system, which the renderer can neither observe nor synthesise — not a policy the renderer enforces."*

## Open questions

- Is the renderer trusted or untrusted, in writing? Every answer is defensible; the silence is not.
- If a CSP is served, who keeps it green on macOS and Windows when CI is ubuntu-only and it may not apply in dev at all?
- Should the agent's shell default stay `Allow`? It is the fact that bounds everything above, and it is a policy question this report does not answer.
