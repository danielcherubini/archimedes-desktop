---
status: accepted
date: 2026-09-26
superseded-by:
---

# Trusted Space: trust is enforced desktop-side, not in the gate

A **Trusted Space** is a Space whose Sessions skip the Permission prompt for the gated tools (`bash`/`edit`/`write`): the desktop auto-approves them. Enforcement is **desktop-side** — on a `Confirm` request the desktop looks up the session's Space (by canonical `cwd`) and, when trusted, responds `confirmed: true` immediately without emitting a prompt; the bundled gate extension (`gate.ts`) is **unchanged**. `sudo_exec` (the desktop's own confirm + password modal) and `ask` (a `Select` request, not a `Confirm`) are unaffected by trust. Trust is stored per-Space (a `trusted` flag on the `spaces` row), defaults to off, is fail-closed (no Space row = untrusted), takes effect immediately (per-request lookup — no session restart), and Subagent Sessions inherit it through the same `handle_extension_ui_request` path.

**Considered Options**

- **Spawn-time gate bypass** (omit `-e` / an env var such as `PI_ARCHIMEDES_GATE=off`): rejected — it would take effect for *new* Sessions only (a mid-session toggle silently not working), the gate would carry a second code path, and "who decides permissions" would have two answers.
- **A per-Session flag**: rejected — trust would die with the session. The user's trust decision is about the *folder* (the Space), not a conversation.
- **A Config option advertised over RPC**: rejected — trust is a desktop concept, not an agent concept; the agent has no business knowing about it (the agent is the source of truth for config options; this is the desktop's source of truth).

**Consequences**

- The desktop is the **sole decider of permissions**; the gate is a pure requester. Future permission policies (allowlists, per-tool rules) belong desktop-side, not in `gate.ts`.
- The `Confirm` request type is the gate's by invariant (`ask` emits `Select`, `sudo_exec` is a desktop modal) — trust cannot accidentally suppress `ask` answers or `sudo_exec` confirms.
- Auto-approved calls emit **no `permission-request` event** — the transcript shows the tool call and its result as usual, with no prompt; the user's visibility into "what is auto-approved" is the trust flag on the Space itself.
