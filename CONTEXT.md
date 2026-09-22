# Archimedes Desktop

A cross-platform (Windows/macOS/Linux) desktop app, built with Tauri 2, that connects to coding agents over the Agent Client Protocol (ACP). Pi is the first-class agent in v1; other ACP agents follow.

## Language

**Client**:
The Archimedes Desktop application itself — the ACP *client* role: it spawns agent processes, renders the conversation, and provides file/terminal/permission backends.
_Avoid_: App, frontend, IDE

**Agent**:
An external ACP-speaking coding agent process (pi, Claude Code, Codex, …), spawned by the Client as a subprocess and communicated with over stdio JSON-RPC.
_Avoid_: Subagent, worker, bot, assistant. (Note: in the pi-archimedes project "Agent" means a subagent configuration — different meaning, different project. "Subagent session" is the desktop's term for a desktop-spawned delegated ACP session — see its entry below.)

**Space**:
A single on-disk folder the Client can open — the workspace in which a conversation and its file access happen. Identified by the folder's canonical path, not a user-supplied name; the display label is the folder's base name. v1: at most one live **Session** per Space, and at most one app-wide (the one-live policy); stored conversations of a Space survive.
_Avoid_: Project, workspace, folder, directory, environment

**Session**:
One live conversation between the Client and one agent process, backed by exactly one spawned subprocess. The unit of process lifecycle, history, and permission state. A Session lives inside one **Space**: its `cwd` (and fs sandbox root) is the Space's folder; a Space's active conversation is its most recent Session.
_Avoid_: Conversation, chat, thread, run

**Subagent session**:
A desktop-spawned ACP session that runs a task delegated by the main agent via the bridge (bridge mode only). Unlike a **Session**, it is not a user-facing conversation — it exists to complete the delegated task, and its progress renders in the Client through the same ACP pipeline as a Session. The subagent's suite runs in bridge mode, so its interactive tools (ask, sudo_exec) go directly to the Client without relaying through the main agent.
_Avoid_: Worker, delegated task, child session, background session

**Agent registry**:
The Client's list of known agents — each entry is a spawn command (program + args) plus metadata (name, capabilities, defaults). v1 ships with pi only.
_Avoid_: Agent list, agent config, agent profile

**Permission prompt**:
The Client's UI response to an ACP `session/request_permission` request from an agent — the user approves or denies a tool call.
_Avoid_: Approval dialog, consent prompt, confirm

**Bridge**:
The mechanism by which the archimedes suite (running inside an Agent process managed by the Client) routes its interactive UI primitives (ask picker, confirmations, masked password input) and ambient state (todos, cost, subagent streams, agent state) to the Client over a local channel. Gated by process spawn: the Client sets the bridge env vars on the Agent it spawns; the suite is inert when they are absent. See the pi-archimedes glossary for the suite-side view.
_Avoid_: Side channel, socket bridge, client mode, host mode
