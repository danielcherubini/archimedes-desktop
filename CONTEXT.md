# Archimedes Desktop

A cross-platform (Windows/macOS/Linux) desktop app, built with Tauri 2, that connects to coding agents over the Agent Client Protocol (ACP). Pi is the first-class agent in v1; other ACP agents follow.

## Language

**Client**:
The Archimedes Desktop application itself — the ACP *client* role: it spawns agent processes, renders the conversation, and provides file/terminal/permission backends.
_Avoid_: App, frontend, IDE

**Agent**:
An external ACP-speaking coding agent process (pi, Claude Code, Codex, …), spawned by the Client as a subprocess and communicated with over stdio JSON-RPC.
_Avoid_: Subagent, worker, bot, assistant. (Note: in the pi-archimedes project "Agent" means a subagent configuration — different meaning, different project.)

**Session**:
One live conversation between the Client and one agent process, backed by exactly one spawned subprocess. The unit of process lifecycle, history, and permission state.
_Avoid_: Conversation, chat, thread, run

**Agent registry**:
The Client's list of known agents — each entry is a spawn command (program + args) plus metadata (name, capabilities, defaults). v1 ships with pi only.
_Avoid_: Agent list, agent config, agent profile

**Permission prompt**:
The Client's UI response to an ACP `session/request_permission` request from an agent — the user approves or denies a tool call.
_Avoid_: Approval dialog, consent prompt, confirm
