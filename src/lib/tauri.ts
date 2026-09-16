/**
 * Typed wrappers over the Tauri IPC surface (commands + events) implemented
 * in Rust (Tasks 2–3).
 *
 * Case conventions on the wire:
 * - command arguments: camelCase here (Tauri maps them to snake_case Rust
 *   parameters),
 * - `SessionInfo` (the `start_session` return value): camelCase
 *   (`sessionId`, `agentId`),
 * - event payloads: camelCase (`sessionId`, `requestId`),
 * - ACP update discriminators: snake_case (`agent_message_chunk`, …).
 */

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

// ---------------------------------------------------------------------------
// Types mirroring the Rust side
// ---------------------------------------------------------------------------

/** Return value of the `start_session` command (camelCase). */
export interface SessionInfo {
  sessionId: string;
  agentId: string;
  cwd: string;
  capabilities: Record<string, unknown>;
}

/** Why the agent stopped a prompt turn (snake_case strings). */
export type StopReason =
  | "end_turn"
  | "max_tokens"
  | "refusal"
  | "max_turn_requests"
  | "cancelled";

/**
 * The user's decision on a permission prompt. Mirrors the Rust
 * `PermissionOutcome` serde shape: the unit variant serializes to the bare
 * string `"cancelled"`, the struct variant to `{"selected": {"option_id": …}}`.
 */
export type PermissionOutcome =
  | { selected: { option_id: string } }
  | "cancelled";

export interface PermissionOption {
  optionId: string;
  name: string;
  kind?: string;
}

/** The agent's `session/request_permission` request (camelCase). */
export interface PermissionRequest {
  sessionId: string;
  toolCall: { toolCallId?: string; title: string };
  options: PermissionOption[];
}

export type AcpToolCallStatus =
  | "pending"
  | "in_progress"
  | "completed"
  | "failed";

export interface ContentBlock {
  type: string;
  text?: string;
  [key: string]: unknown;
}

export interface ToolCallDiff {
  path: string;
  oldText?: string | null;
  newText: string;
}

export type ToolCallContent =
  | { type: "content"; content: ContentBlock }
  | { type: "diff"; path: string; oldText?: string | null; newText: string };

/**
 * A `session/update` notification body. The three update types the desktop
 * renders are modelled; anything else (unknown/future types) falls through
 * to the reducer's `default` branch and is ignored (forward-compat).
 */
export type AcpSessionUpdate =
  | {
      sessionUpdate: "agent_message_chunk";
      content?: ContentBlock;
      messageId?: string;
    }
  | {
      sessionUpdate: "tool_call";
      toolCallId: string;
      title?: string;
      status?: AcpToolCallStatus;
      content?: ToolCallContent[];
    }
  | {
      sessionUpdate: "tool_call_update";
      toolCallId: string;
      title?: string;
      status?: AcpToolCallStatus;
      content?: ToolCallContent[];
    };

export interface SessionUpdatePayload {
  sessionId: string;
  update: AcpSessionUpdate;
}

export interface SessionClosedPayload {
  sessionId: string;
  reason: "user" | "agent-exited" | "error";
}

export interface PermissionRequestPayload {
  sessionId: string;
  requestId: string;
  request: PermissionRequest;
}

/** A row from the app's SQLite `messages` table (camelCase over IPC). */
export interface MessageRow {
  id: number;
  sessionId: string;
  kind: "user" | "agent-text" | "tool-call" | "diff" | (string & {});
  /** ContentChunk.messageId for agent-text; the tool call id for tool-call. */
  messageKey: string | null;
  /** The message body, serialized (camelCase). */
  payloadJson: string;
  /** Unix milliseconds. */
  createdAt: number;
}

/** The persisted app settings (`settings.json` in the config dir). */
export interface AppSettings {
  theme: "dark" | "light";
  paneLayout: Record<string, unknown>;
}

// ---------------------------------------------------------------------------
// Commands (invoke)
// ---------------------------------------------------------------------------

export async function startSession(
  agentId: string,
  cwd: string,
): Promise<SessionInfo> {
  return invoke<SessionInfo>("start_session", { agentId, cwd });
}

export async function sendPrompt(
  sessionId: string,
  text: string,
): Promise<StopReason> {
  return invoke<StopReason>("send_prompt", { sessionId, text });
}

export async function closeSession(sessionId: string): Promise<void> {
  return invoke("close_session", { sessionId });
}

export async function respondPermission(
  sessionId: string,
  requestId: string,
  outcome: PermissionOutcome,
): Promise<void> {
  return invoke("respond_permission", { sessionId, requestId, outcome });
}

export async function resumeSession(
  agentId: string,
  sessionId: string,
  cwd: string,
): Promise<SessionInfo> {
  return invoke<SessionInfo>("resume_session", { agentId, sessionId, cwd });
}

// ---------------------------------------------------------------------------
// History (SQLite) + settings commands
// ---------------------------------------------------------------------------

/** All stored sessions, newest first. */
export async function listSessions(): Promise<SessionInfo[]> {
  return invoke<SessionInfo[]>("list_sessions");
}

/** A stored session's transcript, in insertion order. */
export async function loadHistory(sessionId: string): Promise<MessageRow[]> {
  return invoke<MessageRow[]>("load_history", { sessionId });
}

/** Delete a stored session (its messages cascade). */
export async function deleteSession(sessionId: string): Promise<void> {
  return invoke("delete_session", { sessionId });
}

export async function getSettings(): Promise<AppSettings> {
  return invoke<AppSettings>("get_settings");
}

export async function saveSettings(settings: AppSettings): Promise<void> {
  return invoke("save_settings", { settings });
}

// ---------------------------------------------------------------------------
// Events (listen) — register once in App.tsx and dispatch into the stores
// ---------------------------------------------------------------------------

export function listenSessionUpdate(
  callback: (payload: SessionUpdatePayload) => void,
): Promise<UnlistenFn> {
  return listen<SessionUpdatePayload>("session-update", (event) =>
    callback(event.payload),
  );
}

export function listenSessionClosed(
  callback: (payload: SessionClosedPayload) => void,
): Promise<UnlistenFn> {
  return listen<SessionClosedPayload>("session-closed", (event) =>
    callback(event.payload),
  );
}

export function listenPermissionRequest(
  callback: (payload: PermissionRequestPayload) => void,
): Promise<UnlistenFn> {
  return listen<PermissionRequestPayload>("permission-request", (event) =>
    callback(event.payload),
  );
}
