import { create } from "zustand";
import {
  deleteSession as deleteSessionCommand,
  loadHistory,
  resumeSession as resumeSessionCommand,
  type AcpSessionUpdate,
  type AcpToolCallStatus,
  type MessageRow,
  type SessionInfo,
  type StopReason,
  type ToolCallContent,
} from "../lib/tauri";
import { unifiedPatch } from "../lib/diff";
import { usePermissions } from "./permissions";

export type { AcpSessionUpdate } from "../lib/tauri";

export type ToolCallUiStatus = "pending" | "completed" | "failed";

export interface DiffRef {
  path: string;
  patch: string;
}

export type Message =
  | { kind: "user"; text: string; at: number }
  | { kind: "agent-text"; messageId: string; text: string; at: number }
  | {
      kind: "tool-call";
      id: string;
      title: string;
      status: ToolCallUiStatus;
      diff?: DiffRef;
      at: number;
    }
  | { kind: "diff"; path: string; patch: string; at: number };

function mapStatus(status: AcpToolCallStatus | undefined): ToolCallUiStatus {
  switch (status) {
    case "completed":
      return "completed";
    case "failed":
      return "failed";
    default:
      // pending | in_progress | unknown → still open
      return "pending";
  }
}

/** Pull every `ToolCallContent::Diff` out of a tool call's content list. */
function extractDiffs(content: ToolCallContent[] | undefined): DiffRef[] {
  if (!content) return [];
  const diffs: DiffRef[] = [];
  for (const item of content) {
    if (
      item.type === "diff" &&
      typeof item.path === "string" &&
      typeof item.newText === "string"
    ) {
      diffs.push({
        path: item.path,
        patch: unifiedPatch(item.path, item.oldText ?? null, item.newText),
      });
    }
  }
  return diffs;
}

function diffMessages(diffs: DiffRef[], at: number): Message[] {
  return diffs.map((d) => ({ kind: "diff" as const, path: d.path, patch: d.patch, at }));
}

/**
 * Pure reducer for `session-update` events: returns a NEW message array.
 *
 * - `agent_message_chunk`: appended to the trailing agent-text message with
 *   the same `messageId`; a new `messageId` (or an intervening non-text
 *   message) starts a new message — real agents emit several messages per
 *   turn, so we never just "append to the last agent-text".
 * - `tool_call` / `tool_call_update`: create/update the tool-call message;
 *   diffs inside the content are extracted into standalone diff messages
 *   (there is no dedicated file-edit update type).
 * - unknown update types: ignored (forward-compat).
 */
export function applySessionUpdate(
  messages: Message[],
  update: AcpSessionUpdate,
  at: number,
): Message[] {
  switch (update.sessionUpdate) {
    case "agent_message_chunk": {
      const content = update.content;
      if (!content || content.type !== "text" || typeof content.text !== "string") {
        return messages;
      }
      const text = content.text;
      if (text === "") return messages;
      const messageId = update.messageId ?? "default";
      const last = messages[messages.length - 1];
      if (last && last.kind === "agent-text" && last.messageId === messageId) {
        return [...messages.slice(0, -1), { ...last, text: last.text + text }];
      }
      return [...messages, { kind: "agent-text", messageId, text, at }];
    }

    case "tool_call": {
      const diffs = extractDiffs(update.content);
      const msg: Message = {
        kind: "tool-call",
        id: update.toolCallId,
        title: update.title ?? update.toolCallId,
        status: mapStatus(update.status),
        diff: diffs[0],
        at,
      };
      return [...messages, msg, ...diffMessages(diffs, at)];
    }

    case "tool_call_update": {
      const diffs = extractDiffs(update.content);
      const index = messages.findIndex(
        (m) => m.kind === "tool-call" && m.id === update.toolCallId,
      );
      if (index === -1) {
        // Update for a tool call we never saw: treat it as a new one.
        const msg: Message = {
          kind: "tool-call",
          id: update.toolCallId,
          title: update.title ?? update.toolCallId,
          status: mapStatus(update.status),
          diff: diffs[0],
          at,
        };
        return [...messages, msg, ...diffMessages(diffs, at)];
      }
      const prev = messages[index];
      if (prev.kind !== "tool-call") return messages;
      const updated: Message = {
        ...prev,
        title: update.title ?? prev.title,
        status: update.status ? mapStatus(update.status) : prev.status,
        diff: diffs[0] ?? prev.diff,
      };
      const next = [...messages];
      next[index] = updated;
      return [...next, ...diffMessages(diffs, at)];
    }

    default:
      return messages;
  }
}

/**
 * Session-closed cleanup: mark every still-pending tool call failed so the
 * transcript doesn't show a spinner forever.
 */
export function finalizeSessionMessages(messages: Message[]): Message[] {
  return messages.map((m) =>
    m.kind === "tool-call" && m.status === "pending"
      ? { ...m, status: "failed" as const }
      : m,
  );
}

/**
 * Map a persisted `MessageRow` back to transcript messages. A tool-call row
 * also re-derives its standalone diff messages (diffs are stored inside the
 * tool call's content, not as separate rows).
 */
export function rowToMessages(row: MessageRow): Message[] {
  let payload: Record<string, unknown>;
  try {
    payload = JSON.parse(row.payloadJson) as Record<string, unknown>;
  } catch {
    return [];
  }
  switch (row.kind) {
    case "user":
      return typeof payload.text === "string"
        ? [{ kind: "user", text: payload.text, at: row.createdAt }]
        : [];
    case "agent-text":
      return typeof payload.text === "string"
        ? [
            {
              kind: "agent-text",
              messageId: row.messageKey ?? "default",
              text: payload.text,
              at: row.createdAt,
            },
          ]
        : [];
    case "tool-call": {
      const id =
        typeof payload.toolCallId === "string"
          ? payload.toolCallId
          : (row.messageKey ?? "unknown");
      const title = typeof payload.title === "string" ? payload.title : id;
      const diffs = extractDiffs(
        payload.content as ToolCallContent[] | undefined,
      );
      const msg: Message = {
        kind: "tool-call",
        id,
        title,
        status: mapStatus(payload.status as AcpToolCallStatus | undefined),
        diff: diffs[0],
        at: row.createdAt,
      };
      return [
        msg,
        ...diffs.map((d) => ({
          kind: "diff" as const,
          path: d.path,
          patch: d.patch,
          at: row.createdAt,
        })),
      ];
    }
    default:
      return [];
  }
}

interface SessionsState {
  /** Live (in-memory) sessions for this app run. */
  sessions: SessionInfo[];
  /** Stored sessions from the database that are not currently live. */
  historySessions: SessionInfo[];
  activeSessionId: string | null;
  /** Transcript per session id. Kept after close for history. */
  messages: Record<string, Message[]>;
  /** Whether a prompt turn is in flight for a session. */
  inTurn: Record<string, boolean>;
  /** Last stop reason reported for a session's turn. */
  stopReasons: Record<string, StopReason>;

  addSession: (info: SessionInfo) => void;
  setActiveSession: (sessionId: string | null) => void;
  /** Populate the history list from `list_sessions` (boot). */
  setHistorySessions: (rows: SessionInfo[]) => void;
  /**
   * Open a session (live or stored). For a stored session whose transcript
   * is not loaded yet, fetch its history first.
   */
  openSession: (sessionId: string) => void;
  /** Delete a stored session (command + local cleanup). */
  deleteSession: (sessionId: string) => Promise<void>;
  /**
   * Resume a stored session via `session/load`. On success the session
   * becomes live; the in-memory transcript is cleared so the agent's replay
   * is the source of truth (the database keeps the persistent record).
   */
  resumeSession: (sessionId: string) => Promise<SessionInfo>;
  addUserMessage: (sessionId: string, text: string) => void;
  beginTurn: (sessionId: string) => void;
  applySessionUpdate: (sessionId: string, update: AcpSessionUpdate) => void;
  handleSessionClosed: (sessionId: string, reason: string) => void;
  turnCompleted: (sessionId: string, stopReason: StopReason) => void;
}

export const useSessions = create<SessionsState>((set, get) => ({
  sessions: [],
  historySessions: [],
  activeSessionId: null,
  messages: {},
  inTurn: {},
  stopReasons: {},

  addSession: (info) =>
    set((state) => ({
      sessions: [
        ...state.sessions.filter((s) => s.sessionId !== info.sessionId),
        info,
      ],
      historySessions: state.historySessions.filter(
        (s) => s.sessionId !== info.sessionId,
      ),
      activeSessionId: state.activeSessionId ?? info.sessionId,
      messages: { ...state.messages, [info.sessionId]: [] },
    })),

  setActiveSession: (sessionId) => set({ activeSessionId: sessionId }),

  setHistorySessions: (rows) =>
    set((state) => ({
      historySessions: rows.filter(
        (row) => !state.sessions.some((s) => s.sessionId === row.sessionId),
      ),
    })),

  openSession: (sessionId) => {
    set({ activeSessionId: sessionId });
    if (get().messages[sessionId]) return; // already loaded
    void (async () => {
      try {
        const rows = await loadHistory(sessionId);
        // The user may have switched away while the fetch was in flight.
        if (get().activeSessionId !== sessionId) return;
        const messages = rows.flatMap(rowToMessages);
        set((state) => ({
          messages: { ...state.messages, [sessionId]: messages },
        }));
      } catch (err) {
        console.error(`failed to load history for ${sessionId}`, err);
      }
    })();
  },

  deleteSession: async (sessionId) => {
    await deleteSessionCommand(sessionId);
    set((state) => {
      const { [sessionId]: _gone, ...rest } = state.messages;
      return {
        sessions: state.sessions.filter((s) => s.sessionId !== sessionId),
        historySessions: state.historySessions.filter(
          (s) => s.sessionId !== sessionId,
        ),
        activeSessionId:
          state.activeSessionId === sessionId ? null : state.activeSessionId,
        messages: rest,
      };
    });
  },

  resumeSession: async (sessionId) => {
    const state = get();
    const session = [...state.sessions, ...state.historySessions].find(
      (s) => s.sessionId === sessionId,
    );
    if (!session) throw new Error(`unknown session: ${sessionId}`);
    const info = await resumeSessionCommand(session.agentId, sessionId, session.cwd);
    set((st) => ({
      sessions: [
        ...st.sessions.filter((s) => s.sessionId !== sessionId),
        info,
      ],
      historySessions: st.historySessions.filter(
        (s) => s.sessionId !== sessionId,
      ),
      // Clear the in-memory transcript: the agent's load-replay rebuilds it.
      messages: { ...st.messages, [sessionId]: [] },
      activeSessionId: st.activeSessionId ?? sessionId,
    }));
    return info;
  },

  addUserMessage: (sessionId, text) =>
    set((state) => ({
      messages: {
        ...state.messages,
        [sessionId]: [
          ...(state.messages[sessionId] ?? []),
          { kind: "user" as const, text, at: Date.now() },
        ],
      },
    })),

  beginTurn: (sessionId) =>
    set((state) => ({ inTurn: { ...state.inTurn, [sessionId]: true } })),

  applySessionUpdate: (sessionId, update) =>
    set((state) => ({
      messages: {
        ...state.messages,
        [sessionId]: applySessionUpdate(
          state.messages[sessionId] ?? [],
          update,
          Date.now(),
        ),
      },
    })),

  handleSessionClosed: (sessionId, _reason) => {
    // Dismiss the session's permission prompts (auto-cancelled server-side).
    usePermissions.getState().dismissSessionPrompts(sessionId);
    const closedInfo = useSessions
      .getState()
      .sessions.find((s) => s.sessionId === sessionId);
    set((state) => ({
      sessions: state.sessions.filter((s) => s.sessionId !== sessionId),
      // The session remains in the database: it moves to the history list.
      historySessions: closedInfo
        ? [
            ...state.historySessions.filter(
              (s) => s.sessionId !== sessionId,
            ),
            closedInfo,
          ]
        : state.historySessions,
      activeSessionId:
        state.activeSessionId === sessionId ? null : state.activeSessionId,
      messages: {
        ...state.messages,
        [sessionId]: finalizeSessionMessages(state.messages[sessionId] ?? []),
      },
      inTurn: { ...state.inTurn, [sessionId]: false },
    }));
  },

  turnCompleted: (sessionId, stopReason) =>
    set((state) => ({
      inTurn: { ...state.inTurn, [sessionId]: false },
      stopReasons: { ...state.stopReasons, [sessionId]: stopReason },
    })),
}));
