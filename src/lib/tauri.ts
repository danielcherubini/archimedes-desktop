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
import type { ImageRef } from "./chatAttachments";

// ---------------------------------------------------------------------------
// Types mirroring the Rust side
// ---------------------------------------------------------------------------

/** One entry of a config option's `options` list (camelCase). */
export interface SessionConfigSelectOption {
  value: string;
  name: string;
  description?: string | null;
}

/** A group in a config option's `options` list (camelCase). */
export interface SessionConfigSelectGroup {
  name: string;
  options: SessionConfigSelectOption[];
}

/**
 * A session config option the agent advertises (camelCase wire shape).
 * `category` is snake_case per the ACP spec (`"model"`,
 * `"thought_level"`); `type` discriminates the payload shape.
 */
export interface SessionConfigOption {
  id: string;
  name: string;
  description?: string | null;
  category?: string | null;
  type: "select" | "boolean";
  currentValue: string | boolean;
  options?: (SessionConfigSelectOption | SessionConfigSelectGroup)[]; // select kind only
}

/** Return value of the `start_session` command (camelCase). */
export interface SessionInfo {
  sessionId: string;
  agentId: string;
  cwd: string;
  capabilities: Record<string, unknown>;
  configOptions?: SessionConfigOption[];
  /** The desktop's archived flag (ADR 0016): `true` hides the session from its Space group into the Archived section (the transcript is kept). Always present over IPC. */
  archived: boolean;
}

/** A registry entry over IPC (camelCase). */
export interface AgentEntryDto {
  id: string;
  name: string;
}

/** A space bookkeeping row (camelCase over IPC). */
export interface SpaceRow {
  path: string;
  createdAt: number;
  lastOpenedAt: number;
  /** Whether the space is trusted (permission prompts for gated tools are auto-approved). */
  trusted: boolean;
}

/** The folder-check result (camelCase over IPC). */
export interface SpaceCheck {
  canonicalPath: string;
  isSpace: boolean;
}

/**
 * Why a session was closed (snake_case strings over the `session-closed`
 * event). The UI renders close reasons in the paused banner copy.
 * Recorded per session id in the store's `closeReasons`.
 */
export type CloseReasonStr = "user" | "agent-exited" | "error";

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
 * A `session/update` notification body. The five update types the desktop
 * handles are modelled; anything else (unknown/future types) falls through
 * to the reducer's `default` branch and is ignored (forward-compat).
 *
 * `rawOutput` is the tool's result (the RPC `AgentToolResult`), live-partial
 * while the tool runs, final on `tool_execution_end`.
 */
export type AcpSessionUpdate =
  | {
      sessionUpdate: "agent_message_chunk";
      content?: ContentBlock;
      messageId?: string;
    }
  | {
      sessionUpdate: "agent_thought_chunk";
      content?: ContentBlock;
      messageId?: string;
    }
  | {
      sessionUpdate: "tool_call";
      toolCallId: string;
      title?: string;
      status?: AcpToolCallStatus;
      rawInput?: unknown;
      rawOutput?: unknown;
      content?: ToolCallContent[];
    }
  | {
      sessionUpdate: "tool_call_update";
      toolCallId: string;
      title?: string;
      status?: AcpToolCallStatus;
      rawInput?: unknown;
      rawOutput?: unknown;
      content?: ToolCallContent[];
    }
  | { sessionUpdate: "config_option_update"; configOptions: SessionConfigOption[] };

export interface SessionUpdatePayload {
  sessionId: string;
  update: AcpSessionUpdate;
}

export interface SessionClosedPayload {
  sessionId: string;
  reason: CloseReasonStr;
}

export interface PermissionRequestPayload {
  sessionId: string;
  requestId: string;
  request: PermissionRequest;
}

// ---------------------------------------------------------------------------
// Bridge (the delegated interactive UI — Task 4's listener emits these)
// ---------------------------------------------------------------------------

/**
 * One question of the ask schema (the `ask` tool's `params.questions` entry,
 * mirrored structurally — the ask package owns the concrete type).
 */
export interface AskQuestionDto {
  id: string;
  question: string;
  description?: string;
  options: Array<{ label: string }>;
  multi?: boolean;
  recommended?: number;
}

/**
 * The user's answer to an `ask` bridge request — the `result` of the
 * response frame, verbatim (one result per question).
 */
export interface AskResponsePayload {
  cancelled: boolean;
  results: Array<{ id: string; selectedOptions: string[]; customInput?: string }>;
}

/**
 * A `bridge-request` event payload (camelCase over the wire — the Rust
 * listener builds it). `sessionId` is the ACP session id (the desktop sets
 * it on the listener once the ACP id is known; bridge requests only occur
 * mid-turn, so they always carry it). `params` = the method's params: the
 * ask schema (`{ questions }`) for `ask`, `{ command, reason }` for
 * `confirm`/`password`.
 */
export interface BridgeRequestPayload {
  sessionId: string;
  requestId: string;
  method: "ask" | "confirm" | "password";
  source: string;
  toolCallId?: string;
  params: Record<string, unknown>;
}

/**
 * A `bridge-event` push (camelCase over the wire). `event` is the wire name
 * (`todos_update` / `todos_clear` / `cost_update` / `state` / `session` —
 * the bus `COST_UPDATE` maps to `cost_update`, NOT `cost`); `payload` is the
 * bus payload verbatim.
 */
export interface BridgeEventPayload {
  sessionId: string;
  seq: number;
  event: string;
  payload: unknown;
}

/**
 * The `result` to send back via `respond_bridge_request` — for `ask`, the
 * `AskResponsePayload` verbatim; for `confirm`, `{ confirmed }`; for
 * `password`, `{ password }` (or `{ password: "" }` for a cancel).
 */
export type BridgeResponseDto =
  | AskResponsePayload
  | { confirmed: boolean }
  | { password: string };

/**
 * The `metrics` of the `subagent-closed` payload / the `dispatch_subagent`
 * response (same shape over the wire). v1: the suite does NOT push the
 * subagent's OWN usage, so the captured values are `{0, 0, 0, <real
 * durationMs>}` — `durationMs` is real (wall clock), the token/cost fields
 * are 0.
 */
export interface SubagentMetrics {
  inputTokens: number;
  outputTokens: number;
  cost: number;
  durationMs: number;
}

/**
 * A `subagent-session-started` event payload (camelCase over the wire —
 * emitted by the `SubagentSessionManager` once the subagent's ACP session
 * is ESTABLISHED, so `sessionId` is the ACP id, never the placeholder).
 */
export interface SubagentSessionStartedPayload {
  sessionId: string;
  parentSessionId: string;
  agentName: string;
  task: string;
}

/**
 * A `subagent-closed` event payload (camelCase over the wire). `error` is
 * present only for `failed`; `metrics` = the captured `cost_update` payload
 * + duration (see `SubagentMetrics`). CONCURRENT with the driver teardown's
 * `session-closed` for the same id (the worker task does not await it) —
 * the frontend stores the snapshot in the subagents store for exactly this
 * reason.
 */
export interface SubagentClosedPayload {
  sessionId: string;
  status: "completed" | "failed";
  error?: string;
  metrics?: SubagentMetrics;
}

/** A row from the app's SQLite `messages` table (camelCase over IPC). */
export interface MessageRow {
  id: number;
  sessionId: string;
  kind: "user" | "agent-text" | "agent-thought" | "tool-call" | "diff" | (string & {});
  /** ContentChunk.messageId for agent-text; the tool call id for tool-call. */
  messageKey: string | null;
  /** The message body, serialized (camelCase). */
  payloadJson: string;
  /** Unix milliseconds. */
  createdAt: number;
}

/** A user-configured model provider (`settings.json` `providers` entry). */
export interface ProviderConfig {
  id: string;
  name: string;
  baseUrl: string;
  apiKey: string;
}

/**
 * An MCP server entry (the `settings.json` `mcpServers` value — ADR 0019):
 * pi's `mcpServers` entry shape VERBATIM, so an entry copy-pastes between
 * the desktop's `settings.json` and pi's `mcp.json` files. An entry is an
 * HTTP server (`url` + optional `headers` / `auth` / `bearerTokenEnv`) or
 * a stdio server (`command` + optional `args` / `env` / `cwd`).
 */
export interface McpServerEntry {
  // HTTP:
  url?: string;
  headers?: Record<string, string>;
  /** `"oauth"` (the interactive flow) or the OAuth config object's fields. */
  auth?: string | Record<string, string>;
  bearerTokenEnv?: string;
  // stdio:
  command?: string;
  args?: string[];
  env?: Record<string, string>;
  cwd?: string;
  // common:
  disabled?: boolean;
}

/** The font settings (the settings UI's size slider + optional family overrides). */
export interface FontSettings {
  sizePx: number;
  uiFamily: string | null;
  codeFamily: string | null;
}

/**
 * The persisted app settings (`settings.json` in the config dir).
 * Mirrors the Rust `Settings` struct's camelCase shape exactly (a
 * `saveSettings` round-trip loses no field).
 */
export interface AppSettings {
  /** "system" follows the OS scheme live (the `matchMedia` listener in theme.ts). */
  theme: "system" | "dark" | "light";
  paneLayout: Record<string, unknown>;
  defaultAgent: string | null;
  defaultTrustNewSpaces: boolean;
  defaultModel: string | null;
  providers: ProviderConfig[];
  /** (ADR 0019) The user-managed MCP servers: `name → entry` (pi's entry shape). */
  mcpServers: Record<string, McpServerEntry>;
  font: FontSettings;
  /** Per-model remembered thinking level (ADR 0015): `"<provider>/<id>"` → the last level the user set for that model. */
  defaultThinkingLevels: Record<string, string>;
}

/** The effective catalog's model (the Default-model select + provider discovery status). */
export interface ModelDto {
  id: string;
  provider: string;
  contextWindow: number;
  supportsThinking: boolean;
  thinkingLevels: string[];
}

/** One discovered skill (camelCase over IPC — the Rust `SkillInfo`). */
export interface SkillInfo {
  name: string;
  description: string;
  path: string;
  dir: string;
  scope: "space" | "user";
  /** The SKILL.md content minus the frontmatter (the injected block's BODY). */
  body: string;
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
  images?: ImageRef[],
): Promise<StopReason> {
  return invoke<StopReason>("send_prompt", { sessionId, text, images: images ?? [] });
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

/** Cancel the session's in-flight prompt turn (Esc). The agent resolves the open prompt with `stopReason: "cancelled"`. */
export async function cancelSession(sessionId: string): Promise<void> {
  return invoke("cancel_session", { sessionId });
}

/** Set a session config option (model / thinking level); returns the agent's updated `configOptions`. */
export async function setSessionConfigOption(
  sessionId: string,
  configId: string,
  value: string,
): Promise<SessionConfigOption[]> {
  return invoke<SessionConfigOption[]>("set_session_config_option", {
    sessionId,
    configId,
    value,
  });
}

/**
 * The current system-clipboard image (if any) as PNG bytes.
 *
 * WebKitGTK's `paste` event does not expose clipboard images as
 * `DataTransfer` file items (`items`/`files` are empty for a pasted image —
 * verified on webkit2gtk-4.1 2.52.5 on Wayland), so the composer reads the
 * image from the system clipboard directly (Rust/arboard) as a fallback.
 * `null` when the clipboard has no image (text-only or empty).
 */
export async function readClipboardImage(): Promise<number[] | null> {
  return invoke<number[] | null>("read_clipboard_image");
}

// ---------------------------------------------------------------------------
// History (SQLite) + settings commands
// ---------------------------------------------------------------------------

/** All stored sessions, newest first. `includeArchived` includes the archived rows (their `archived` flag is set); `undefined` → the Rust default `false`. */
export async function listSessions(
  includeArchived?: boolean,
): Promise<SessionInfo[]> {
  return invoke<SessionInfo[]>("list_sessions", { includeArchived });
}

/** A stored session's transcript, in insertion order. */
export async function loadHistory(sessionId: string): Promise<MessageRow[]> {
  return invoke<MessageRow[]>("load_history", { sessionId });
}

/** Delete a stored session (its messages cascade). */
export async function deleteSession(sessionId: string): Promise<void> {
  return invoke("delete_session", { sessionId });
}

/** Archive (or unarchive) a stored session (ADR 0016): sets the `sessions.archived` flag; the transcript is NOT touched. `false` when no row matched. */
export async function setSessionArchived(
  sessionId: string,
  archived: boolean,
): Promise<boolean> {
  return invoke("set_session_archived", { sessionId, archived });
}

export async function getSettings(): Promise<AppSettings> {
  return invoke<AppSettings>("get_settings");
}

export async function saveSettings(settings: AppSettings): Promise<void> {
  return invoke("save_settings", { settings });
}

/** The effective catalog's models (the Default-model select + provider discovery status). `forceRefresh` = a provider id whose discovery cache is bypassed. */
export async function listModels(forceRefresh?: string): Promise<ModelDto[]> {
  return invoke("list_models", { forceRefresh: forceRefresh ?? null });
}

/** Test ONE MCP server entry (the Settings page's Test action, ADR 0019): a one-shot bounded connect + `tools/list`. Resolves the tool count; rejects with the error text (a `needs-auth` / a network failure). */
export async function testMcpServer(entry: McpServerEntry): Promise<number> {
  return invoke("test_mcp_server", { entry });
}

// ---------------------------------------------------------------------------
// Spaces + agents commands (boot, the new-session dialog, "forget this space")
// ---------------------------------------------------------------------------

/** All configured agents (the dialog's dropdown). */
export async function listAgents(): Promise<AgentEntryDto[]> {
  return invoke<AgentEntryDto[]>("list_agents");
}

/** All spaces, most recently opened first. */
export async function listSpaces(): Promise<SpaceRow[]> {
  return invoke<SpaceRow[]>("list_spaces");
}

/** "Forget this space": delete the bookkeeping row (conversations stay stored). */
export async function deleteSpace(path: string): Promise<void> {
  return invoke("delete_space", { path });
}

/** Flip a space's trusted flag (no-op for a missing row). */
export async function setSpaceTrusted(path: string, trusted: boolean): Promise<void> {
  return invoke("set_space_trusted", { path, trusted });
}

/** Canonicalize a folder and say whether a space row already exists for it. */
export async function spaceForPath(path: string): Promise<SpaceCheck> {
  return invoke<SpaceCheck>("space_for_path", { path });
}

/** The skill catalog for a Space (`null` = user-level skills only). */
export async function listSkills(spacePath: string | null): Promise<SkillInfo[]> {
  return invoke<SkillInfo[]>("list_skills", { spacePath: spacePath ?? null });
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

export function listenBridgeRequest(
  callback: (payload: BridgeRequestPayload) => void,
): Promise<UnlistenFn> {
  return listen<BridgeRequestPayload>("bridge-request", (event) =>
    callback(event.payload),
  );
}

/**
 * A `bridge-request-close` event payload (the Rust `SudoPromptCleanup` drop
 * guard emits it when a `sudo_exec` sub-prompt is DROPPED — a turn cancel
 * skips the flow's exit-path cleanup, so the modal must be closed here). The
 * `requestId` is the sub-prompt's derived id (`"{id}:confirm"` /
 * `"{id}:password"`).
 */
export interface BridgeRequestClosePayload {
  sessionId: string;
  requestId: string;
}

export function listenBridgeRequestClose(
  callback: (payload: BridgeRequestClosePayload) => void,
): Promise<UnlistenFn> {
  return listen<BridgeRequestClosePayload>("bridge-request-close", (event) =>
    callback(event.payload),
  );
}

export function listenBridgeEvent(
  callback: (payload: BridgeEventPayload) => void,
): Promise<UnlistenFn> {
  return listen<BridgeEventPayload>("bridge-event", (event) =>
    callback(event.payload),
  );
}

export function listenSubagentSessionStarted(
  callback: (payload: SubagentSessionStartedPayload) => void,
): Promise<UnlistenFn> {
  return listen<SubagentSessionStartedPayload>(
    "subagent-session-started",
    (event) => callback(event.payload),
  );
}

export function listenSubagentClosed(
  callback: (payload: SubagentClosedPayload) => void,
): Promise<UnlistenFn> {
  return listen<SubagentClosedPayload>("subagent-closed", (event) =>
    callback(event.payload),
  );
}

/**
 * Answer a bridge request. The invoke key is `result` (the Rust command's
 * `result: serde_json::Value` param — Tauri's camelCase↔snake_case handles
 * `sessionId`/`requestId` but will not alias `response`↔`result`). The
 * `result` is written into the response frame verbatim (no wrapper).
 */
export async function respondBridgeRequest(
  sessionId: string,
  requestId: string,
  result: BridgeResponseDto,
): Promise<void> {
  return invoke("respond_bridge_request", { sessionId, requestId, result });
}
