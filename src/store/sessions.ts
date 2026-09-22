import { create } from "zustand";
import {
  deleteSession as deleteSessionCommand,
  deleteSpace as deleteSpaceCommand,
  loadHistory,
  resumeSession as resumeSessionCommand,
  type AcpSessionUpdate,
  type AcpToolCallStatus,
  type CloseReasonStr,
  type MessageRow,
  type SessionInfo,
  type SpaceRow,
  type StopReason,
  type ToolCallContent,
} from "../lib/tauri";
import { basenameOfPath } from "../lib/paths";
import { unifiedPatch } from "../lib/diff";
import { usePermissions } from "./permissions";
import { useSubagents } from "./subagents";

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
      /** The ACP `rawInput` (the tool's raw input) — kept for the todo board's `rawInput` fallback (the latest `manage_todo_list` input). */
      rawInput?: unknown;
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
        rawInput: update.rawInput,
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
          rawInput: update.rawInput,
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
        rawInput: update.rawInput ?? prev.rawInput,
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
 * Session-closed cleanup for EPHEMERAL (subagent) sessions: delete the
 * transcript entirely. Subagent sessions are never persisted (the
 * desktop's Rust side records them with `db: None`), so there is no
 * history to preserve. MAIN sessions are untouched — `handleSessionClosed`
 * keeps their transcript (finalized) for the history list, so this is a
 * no-op when the id has no subagent entry.
 */
export function discardSessionMessages(sessionId: string): void {
  // ASSUMES ACP session ids are globally unique across main + subagent
  // sessions: the subagent-entry guard below would delete a MAIN
  // transcript if a subagent id ever collided with a main id (a
  // collision would already merge the sessions' `session-update` event
  // streams into one `messages[id]` before the delete matters; the Rust
  // side is immune — separate `sessions` maps per manager).
  if (!useSubagents.getState().entries[sessionId]) return; // main: keep
  useSessions.setState((state) => {
    if (!(sessionId in state.messages)) return state;
    const { [sessionId]: _gone, ...rest } = state.messages;
    return { messages: rest };
  });
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
        rawInput: payload.rawInput,
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

/**
 * The session a fresh boot should land on: the most recently opened
 * space's newest session, live (in `sessions`) preferred over stored.
 * `null` when there's nothing.
 *
 * **Ordering note:** `SessionInfo` over IPC has NO `createdAt` field (it is
 * `{ sessionId, agentId, cwd, capabilities }` — the ACP `SessionInfo` struct
 * has no such field, and the Rust `list_sessions` command drops it when
 * mapping `SessionRow → SessionInfo`). So we CANNOT sort by `createdAt`.
 * We rely on SERVER order instead: `Db::list_sessions` returns
 * `ORDER BY created_at DESC, id DESC` and `Db::list_spaces` returns
 * `ORDER BY last_opened_at DESC, path ASC`, and the store slices only
 * `filter`/`map` those rows (preserving order). Therefore "newest-first"
 * **is** the input order — `spaces[0]` is the most recently opened space,
 * and within a space the head of its `historySessions` subsequence is its
 * newest stored session. Do NOT re-sort by a field that doesn't exist.
 */
export function autoSelectActive(
  spaces: SpaceRow[],
  sessions: SessionInfo[],
  historySessions: SessionInfo[],
): string | null {
  for (const space of spaces) {
    // At most one live session app-wide; prefer it in this space.
    const live = sessions.find((s) => s.cwd === space.path);
    if (live) return live.sessionId;
    // No re-sort: input order IS newest-first (see the ordering note).
    const stored = historySessions
      .filter((s) => s.cwd === space.path)
      .map((s) => s.sessionId);
    if (stored.length > 0) return stored[0];
  }
  return null;
}

/**
 * One per-space group for the UI (used by both this store and the Task 6
 * components). Built with the same input-order rule as `autoSelectActive`.
 *
 * Note: a session without a space is NOT a case here by design — a space is
 * born at the same time as a session (the `record_session → upsert_space`
 * hook), and it references a space row on boot after backfill. So there is
 * no "unknown sessions" group to render.
 */
export interface SpaceView {
  path: string;
  /** Display label (base name; `""` if the base name is empty — the UI falls back to `path`). */
  title: string;
  /** The live session in this space (there is at most one app-wide), or `null`. */
  liveSessionId: string | null;
  /** Stored sessions of this space, in `historySessions` order (newest-first as delivered by `list_sessions` — do NOT re-sort). */
  storedSessionIds: string[];
  /** Close reason of the space's newest session (live one preferred, else the head of the stored subsequence), if any. */
  lastReason: CloseReasonStr | undefined;
}

export function spaceViewFor(
  space: SpaceRow,
  sessions: SessionInfo[],
  historySessions: SessionInfo[],
  closeReasons: Record<string, CloseReasonStr>,
): SpaceView {
  const liveSessionId =
    sessions.find((s) => s.cwd === space.path)?.sessionId ?? null;
  // Input order = newest-first from `list_sessions`; NOT re-sorted.
  const storedSessionIds = historySessions
    .filter((s) => s.cwd === space.path)
    .map((s) => s.sessionId);
  // The newest session's close reason: the live one, else the head of the
  // stored subsequence; nothing when the space has no sessions.
  const newestId = liveSessionId ?? storedSessionIds[0];
  return {
    path: space.path,
    title: basenameOfPath(space.path),
    liveSessionId,
    storedSessionIds,
    lastReason:
      newestId === undefined ? undefined : closeReasons[newestId],
  };
}

interface SessionsState {
  /** Live (in-memory) sessions for this app run. */
  sessions: SessionInfo[];
  /** Stored sessions from the database that are not currently live. */
  historySessions: SessionInfo[];
  activeSessionId: string | null;
  /** Space bookkeeping rows (from `list_spaces` on boot; upserted by `addSpace`). */
  spaces: SpaceRow[];
  /**
   * Close reason recorded on `session-closed`, per session id (for the
   * "replaced" banner copy). Merge-only: a reason outlives its event, so
   * keys are never deleted.
   */
  closeReasons: Record<string, CloseReasonStr>;
  /** Transcript per session id. Kept after close for history. */
  messages: Record<string, Message[]>;
  /** Whether a prompt turn is in flight for a session. */
  inTurn: Record<string, boolean>;
  /** Last stop reason reported for a session's turn. */
  stopReasons: Record<string, StopReason>;

  addSession: (info: SessionInfo) => void;
  setActiveSession: (sessionId: string | null) => void;
  /**
   * Boot: replace the spaces list (from `list_spaces`, `lastOpenedAt`-desc).
   * If nothing is active yet, select via `autoSelectActive` (the most recent
   * space's newest session, live preferred) and load it: `autoSelectActive`
   * lands on a stored session (live ones never survive a restart), so the
   * `openSession` call loads its transcript and boot lands on CONTENT, not
   * an empty chat (no-op for live / already-loaded sessions; the fetch is
   * self-guarding against a mid-flight switch).
   */
  setSpaces: (rows: SpaceRow[]) => void;
  /**
   * Single-space upsert after a successful `start_session` (from the dialog):
   * existing row → refresh `lastOpenedAt`; missing row → append with
   * `createdAt = lastOpenedAt = Date.now()` (approximate; the next boot's
   * `list_spaces` corrects reality on the server side).
   */
  addSpace: (path: string) => void;
  /**
   * "Forget this space": `delete_space` command + local removal. Does NOT
   * touch `sessions`/`historySessions` (conversations stay stored — design
   * decision). `activeSessionId` is left alone.
   */
  removeSpace: (path: string) => Promise<void>;
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
  handleSessionClosed: (sessionId: string, reason: CloseReasonStr) => void;
  turnCompleted: (sessionId: string, stopReason: StopReason) => void;
}

export const useSessions = create<SessionsState>((set, get) => ({
  sessions: [],
  historySessions: [],
  activeSessionId: null,
  spaces: [],
  closeReasons: {},
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
      // A session that was just *started* (dialog or "new conversation")
      // is the one the user wants to look at: start flows ALWAYS switch
      // the view to it. (The live-session handling in `openSession` is
      // unchanged.)
      activeSessionId: info.sessionId,
      messages: { ...state.messages, [info.sessionId]: [] },
    })),

  setActiveSession: (sessionId) => set({ activeSessionId: sessionId }),

  setSpaces: (rows) => {
    const state = get();
    const selectedId =
      state.activeSessionId === null
        ? autoSelectActive(rows, state.sessions, state.historySessions)
        : state.activeSessionId;
    set({ spaces: rows, activeSessionId: selectedId });
    // `autoSelectActive` lands on a stored session (live ones never survive
    // a restart): load its transcript so boot lands on content, not an
    // empty chat. (No-op for live / already-loaded sessions.)
    if (selectedId) get().openSession(selectedId);
  },

  addSpace: (path) =>
    set((state) => {
      const existing = state.spaces.find((s) => s.path === path);
      const now = Date.now();
      const spaces = existing
        ? state.spaces.map((s) =>
            s.path === path ? { ...s, lastOpenedAt: now } : s,
          )
        : [...state.spaces, { path, createdAt: now, lastOpenedAt: now }];
      return { spaces };
    }),

  removeSpace: async (path) => {
    await deleteSpaceCommand(path);
    set((state) => ({ spaces: state.spaces.filter((s) => s.path !== path) }));
  },

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
      activeSessionId: st.activeSessionId ?? sessionId,
    }));
    // ACP `session/load` does not re-stream the transcript — reload it from
    // the persisted history so the pane shows the conversation on resume.
    void (async () => {
      try {
        const rows = await loadHistory(sessionId);
        // The user may have switched away while the fetch was in flight.
        if (get().activeSessionId !== sessionId) return;
        const messages = rows.flatMap(rowToMessages);
        set((st) => ({ messages: { ...st.messages, [sessionId]: messages } }));
      } catch (err) {
        console.error(`failed to load history for ${sessionId}`, err);
      }
    })();
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

  handleSessionClosed: (sessionId, reason) => {
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
      // A close is a PAUSE, not a discard: the conversation moves to the
      // history list but STAYS the displayed one, so `ChatStream` renders
      // its stored/paused banner (including the `replaced` copy) instead of
      // the `No active session` empty state. (Required for the Task 6
      // `Pause` behavior.)
      activeSessionId: state.activeSessionId,
      // Merge-only: a close reason outlives its event (never delete keys).
      closeReasons: { ...state.closeReasons, [sessionId]: reason },
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
