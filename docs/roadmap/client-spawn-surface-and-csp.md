---
status: draft
done-when: an ADR states what the Client may make the Rust side execute and why that is the right boundary; `tauri.conf.json` serves a CSP that survives fonts, syntax colouring and the offline case on all three platforms; and a test enumerates the commands that take an unbounded path or a spawn from the Client so the list cannot grow silently.
---

# Client Spawn Surface and CSP — Plan

**Goal:** decide, once and in the open, what the Client is allowed to make the Rust side execute, and make a renderer bug stop being a code-execution bug.

**`status: draft` means the decision is NOT made.** Nothing below is approved. This document records what was measured and what each option costs so that the decision can be made once instead of rediscovered by the next reviewer.

**Provenance.** Found while reviewing the out-of-Space `?` completion (ADR 0035, squash `c02ae68`). That feature is not the cause and adds no new class of access; it is simply the change that made someone enumerate the registered commands. Everything below predates it and none of it was touched by it.

---

## What is true today (each line re-verified against the code, not against a prior report)

| Fact | Where | What the caller controls |
|---|---|---|
| `test_mcp_server(name: Option<String>, entry: Value)` | `src-tauri/src/commands/settings.rs:126` | **the program to run** |
| the spawn | `src-tauri/src/agent/mcp/stdio.rs:57-63` | `Command::new(&def.command)`, `.args(&def.args)`, `.envs(&def.env)` which is **ADDITIVE** (its own comment says so), `.current_dir(def.cwd.unwrap_or(cwd))` |
| the fallback cwd | `commands/settings.rs:130` passes `Path::new(".")` | — |

So the **program name, its arguments, its environment and its working directory all arrive from the JSON the Client passed.** No path check, no policy gate, no confirmation, no rate limit. The chain is `classify_server` → `manager::test_server` → `StdioClient::connect` → `Command::new`.

**Why the shape exists.** ADR 0019's per-row **Test** action must be able to test a **draft** entry before it is saved, so the command takes an `entry` rather than a name (`src/lib/tauri.ts:694` posts the form's values). ADR 0019 keeps the *UI* narrow — a structured form, "no raw-JSON editor in v1". **That narrowness is a form, not a boundary.** The command is the boundary, and it accepts anything.

**The sibling.** `auth_mcp_server` (`commands/settings.rs:137`) is HTTP-only (`agent/mcp/manager.rs:339-341` rejects a stdio def), so it does not spawn — but it opens a browser at an authorization URL taken from the caller's entry (`manager.rs:353`). Not code execution; the same ungated shape, one notch down in severity.

**The ACL is not a help here.** `src-tauri/capabilities/default.json` lists `windows: ["main"]` and **no `remote` block**. `src-tauri/gen/schemas/acl-manifests.json` contains **no `__app-acl__`** key and `src-tauri/permissions/` does not exist, so Tauri 2.11.5 skips the ACL check for local content and enforces it for remote content. Read plainly: **every app command is reachable from the first-party webview, and no command is reachable from a remote origin.** That is the right shape for the boundary and it is why a CSP is the only remaining layer between a renderer bug and `invoke("test_mcp_server", …)`.

**`"csp": null`** — `src-tauri/tauri.conf.json`, `app.security`. No Content-Security-Policy is served at all.

**Why this is latent and not an incident.** There is no known injection sink. `react-markdown` v10 with `remarkGfm` and **no `rehype-raw`**, so raw HTML arriving in agent output is inert, and the single `dangerouslySetInnerHTML` (`src/components/MessageBubble.tsx:329`) receives Shiki's own generated HTML. Exploiting the surface needs a sink that does not exist yet — which is exactly why this is cheap to close now and expensive to close after.

**Nothing pins the surface.** `src-tauri/tests/ipc.rs:100` builds **its own** `generate_handler![…]` list, independent of `src-tauri/src/lib.rs:162`. Adding a fourteenth ungated command reddens nothing anywhere.

**How to re-verify without spawning anything.** From devtools in a dev build, `invoke("list_space_files", { spacePath: "~/.ssh" })` — actually `"/home/<you>/.ssh"`, since nothing in the app expands a tilde — lists the directory. That is the same class of access, it has been there since ADR 0033, and it is read-only, which makes it the honest demonstration of the surface. Do not demonstrate the spawn itself.

---

## The options, with the costs found in THIS repo

1. **Serve a CSP.** The constraints are concrete, not hypothetical: `index.html:12-16` loads a stylesheet from `fonts.googleapis.com` with a preconnect to `fonts.gstatic.com`, so `style-src`/`font-src` must allow them; Shiki emits **inline `style` attributes**, so `style-src` needs `'unsafe-inline'`, and a nonce cannot reach markup injected through `dangerouslySetInnerHTML`; images arrive over the asset protocol; and the `index.html` comment at `:10` already reasons about the **offline** case, so the app must keep working when the font host is unreachable. A CSP that is too tight silently kills fonts or syntax colouring, on three platforms, and CI is ubuntu-only. This is a real cost, not a formality.
2. **Confirm before spawning, natively.** The only boundary a script cannot click through, because the dialog is drawn by the OS and the renderer can neither observe nor synthesise the click. Cost: ADR 0019's Test action becomes two clicks, and scoping the prompt to "a command that has not been approved before" needs a notion of *approved* that does not exist yet.
3. **Narrow the command** to a name, looking the entry up server-side. This breaks ADR 0019's test-a-draft flow, and a caller that can invoke the command can save an entry first — so **name it a speed bump, not a fix**, if it is chosen.
4. **Document the trust model and change nothing:** the Client is trusted, and the Boundary/policy machinery (ADR 0030) governs the **agent**, not the renderer. Whatever else gets chosen, this sentence should exist somewhere durable, because today the trust model is implicit and every new feature has to infer it.

The recommendation carried into the decision: **1 plus the surface-enumerating test**, with the trust model written down either way, and **2** revisited the day the app renders untrusted HTML for real.

---

## Invariants whatever is chosen must keep

- ADR 0019's Test action stays **one click** for a saved server, one-shot and bounded (~10 s), the child dropped (`kill_on_drop`).
- **No dialog storm.** One confirmation per user intent, never one per IPC.
- The renderer keeps working **offline**; a CSP that hard-requires a font host breaks the case `index.html:10` already accounts for with `font-display: swap`.
- Gates, unchanged: `pnpm test`, `pnpm build` from the root; `cargo test`, `cargo clippy --all-targets` at **0 warnings**, `cargo fmt --check` from `src-tauri/`.
- `docs/decisions/` is append-only: ADR 0019 and ADR 0030 bodies are not edited. A new ADR, or a dated appended note.

## Open questions

- Is the renderer trusted or untrusted, in writing? Every answer is defensible; the silence is not.
- If a CSP is served, who owns keeping it green on macOS and Windows when CI is ubuntu-only?
- Should the surface-enumerating test assert the **whole** command list against `lib.rs`, or only the subset that takes a path or spawns? The former is a stronger pin and a busier diff.
