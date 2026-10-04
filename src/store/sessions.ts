import { create } from "zustand";
import type { ImageRef } from "../lib/chatAttachments";
import {
  deleteSession as deleteSessionCommand,
  deleteSpace as deleteSpaceCommand,
  loadHistory,
  resumeSession as resumeSessionCommand,
  setSessionArchived as setSessionArchivedCommand,
  setSpaceTrusted as setSpaceTrustedCommand,
  type AcpSessionUpdate,
  type AcpToolCallStatus,
  type CloseReasonStr,
  type MessageRow,
  type SessionConfigOption,
  type SessionInfo,
  type SpaceRow,
  type StopReason,
  type ToolCallContent,
} from "../lib/tauri";
import { basenameOfPath } from "../lib/paths";
import { unifiedPatch } from "../lib/diff";
import type { SessionUpdateBatchItem } from "../lib/batchSessionUpdates";
import { usePermissions } from "./permissions";
import { useSubagents } from "./subagents";

export type { AcpSessionUpdate } from "../lib/tauri";
export type { SessionConfigOption } from "../lib/tauri";

export type ToolCallUiStatus = "pending" | "completed" | "failed";

export interface DiffRef {
  path: string;
  patch: string;
}

export type Message =
  | { kind: "user"; text: string; at: number; images?: ImageRef[] }
  | { kind: "agent-text"; messageId: string; text: string; at: number }
  | { kind: "agent-thought"; messageId: string; text: string; at: number }
  | {
      kind: "tool-call";
      id: string;
      title: string;
      status: ToolCallUiStatus;
      diff?: DiffRef;
      /** The ACP `rawInput` (the tool's raw input) — kept for the todo board's `rawInput` fallback (the latest `manage_todo_list` input). */
      rawInput?: unknown;
      /** The ACP rawOutput (the tool's result — the RPC AgentToolResult, live-partial while running). */
      rawOutput?: unknown;
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
 * - `agent_thought_chunk`: appended to the trailing agent-thought message with
 *   the same `messageId`; a new `messageId` (or an intervening non-thought
 *   message) starts a new message — same segmentation rules as agent-text.
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

    case "agent_thought_chunk": {
      const content = update.content;
      if (!content || content.type !== "text" || typeof content.text !== "string") {
        return messages;
      }
      const text = content.text;
      if (text === "") return messages;
      const messageId = update.messageId ?? "default";
      const last = messages[messages.length - 1];
      if (last && last.kind === "agent-thought" && last.messageId === messageId) {
        return [...messages.slice(0, -1), { ...last, text: last.text + text }];
      }
      return [...messages, { kind: "agent-thought", messageId, text, at }];
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
        rawOutput: update.rawOutput,
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
          rawOutput: update.rawOutput,
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
        rawOutput: update.rawOutput ?? prev.rawOutput,
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
 * A stable key for deduping a merge of reloaded history rows with locally
 * added messages (the resume race above): an id when one exists on both
 * sides (`agent-text`/`agent-thought` `messageId`, `tool-call` `id`),
 * otherwise role + content equality. A `user` message has NO id on either
 * side (the local copy and the `record_message`d row), so it matches on
 * text + images; the image entries are compared field-by-field in a FIXED
 * order so key-order differences in the two constructions never break the
 * match. A `diff` matches on path + patch.
 *
 * NOTE: for a `user` message this key is content ONLY and is therefore
 * ambiguous — the merge must additionally require the matching reloaded
 * row's timestamp to be >= the added message's `at` (see the dedup in
 * `resumeSession`) so a re-sent identical prompt (an OLDER matching row)
 * is not mistaken for the just-committed message.
 */
function mergeDedupeKey(m: Message): string {
  switch (m.kind) {
    case "user": {
      const images = (m.images ?? [])
        .map((img) => `${img.name}|${img.mimeType}|${img.sizeBytes}|${img.data}`)
        .join("\u0000");
      return `user|${m.text}|${images}`;
    }
    case "agent-text":
    case "agent-thought":
      return `${m.kind}|${m.messageId}|${m.text}`;
    case "tool-call":
      return `tool-call|${m.id}|${m.title}|${m.status}`;
    case "diff":
      return `diff|${m.path}|${m.patch}`;
  }
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
    case "user": {
      if (typeof payload.text !== "string") return [];
      const rawImages = Array.isArray(payload.images) ? payload.images : [];
      const images = rawImages
        .filter(
          (img): img is Record<string, unknown> =>
            img !== null &&
            typeof img === "object" &&
            typeof (img as Record<string, unknown>).data === "string" &&
            typeof (img as Record<string, unknown>).mimeType === "string",
        )
        .map((img) => ({
          name: typeof img.name === "string" ? img.name : "",
          mimeType: img.mimeType as string,
          sizeBytes: typeof img.sizeBytes === "number" ? img.sizeBytes : 0,
          data: img.data as string,
        }));
      return [
        {
          kind: "user",
          text: payload.text,
          at: row.createdAt,
          ...(images.length > 0 ? { images } : {}),
        },
      ];
    }
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
    case "agent-thought":
      return typeof payload.text === "string"
        ? [
            {
              kind: "agent-thought",
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
        rawOutput: payload.rawOutput,
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
 * `{ sessionId, cwd, capabilities }` — the Rust `SessionInfo` struct has no
 * such field). So we CANNOT sort by `createdAt`.
 * We rely on SERVER order instead: `Db::list_sessions` returns
 * `ORDER BY created_at DESC, id DESC` and `Db::list_spaces` returns
 * `ORDER BY last_opened_at DESC, path ASC`, and the store slices only
 * `filter`/`map` those rows (preserving order). Therefore "newest-first"
 * **is** the input order — `spaces[0]` is the most recently opened space,
 * and within a space the head of its `historySessions` subsequence is its
 * newest stored session. Do NOT re-sort by a field that doesn't exist.
 */

/**
 * The live session a space "points at" (for the space row's live dot and
 * boot auto-select).
 *
 * **Multi-live (the one-live cap is lifted):** a space may hold
 * MORE than one live session (a second `start_session` / `resume_session`
 * no longer supersedes the first). `sessions` is in INSERTION order —
 * `addSession` / `resumeSession` APPEND — so this returns the
 * MOST-RECENTLY-STARTED live session in the space (the one `addSession`
 * made `activeSessionId`). That keeps the space row's live dot coherent
 * with the active chat. With a single live session (the common case) it is
 * just that session.
 */
function mostRecentLiveInSpace(
  sessions: SessionInfo[],
  path: string,
): SessionInfo | undefined {
  let found: SessionInfo | undefined;
  for (const s of sessions) {
    if (s.cwd === path) found = s; // keep the LAST (most recent) match
  }
  return found;
}

export function autoSelectActive(
  spaces: SpaceRow[],
  sessions: SessionInfo[],
  historySessions: SessionInfo[],
): string | null {
  for (const space of spaces) {
    // A space may hold MORE than one live session (the one-live cap is
    // lifted): `mostRecentLiveInSpace` picks the
    // most-recently-started one (coherent with `activeSessionId`).
    const live = mostRecentLiveInSpace(sessions, space.path);
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
  /**
   * The space's live session (the most-recently-started one when a space
   * holds more than one — the one-live cap is lifted), or `null`.
   */
  liveSessionId: string | null;
  /** Stored sessions of this space, in `historySessions` order (newest-first as delivered by `list_sessions` — do NOT re-sort). */
  storedSessionIds: string[];
  /**
   * Archived sessions of this space (NOT rendered in the Space group —
   * used only for `view` membership, so a space whose only sessions are
   * archived still resolves its `view`).
   */
  archivedSessionIds: string[];
  /** Whether the space is trusted (permission prompts for gated tools are auto-approved). */
  trusted: boolean;
  /** Close reason of the space's newest session (live one preferred, else the head of the stored subsequence, else the head of the archived subsequence), if any. */
  lastReason: CloseReasonStr | undefined;
}

export function spaceViewFor(
  space: SpaceRow,
  sessions: SessionInfo[],
  historySessions: SessionInfo[],
  closeReasons: Record<string, CloseReasonStr>,
  archivedSessions: SessionInfo[],
): SpaceView {
  // A space may hold MORE than one live session (the one-live cap is
  // lifted): show the most-recently-started one (coherent with
  // `activeSessionId`), not just the first.
  const liveSessionId =
    mostRecentLiveInSpace(sessions, space.path)?.sessionId ?? null;
  // Input order = newest-first from `list_sessions`; NOT re-sorted.
  const storedSessionIds = historySessions
    .filter((s) => s.cwd === space.path)
    .map((s) => s.sessionId);
  // Archived sessions are view-membership only (never rendered in the
  // Space group — see `SpaceView.archivedSessionIds`).
  const archivedSessionIds = archivedSessions
    .filter((s) => s.cwd === space.path)
    .map((s) => s.sessionId);
  // The newest session's close reason: the live one, else the head of the
  // stored subsequence, else the head of the archived subsequence (an
  // archived session is still the space's newest stored session — the
  // close-reason banner must not silently vanish for an archived-only
  // space); nothing when the space has no sessions.
  const newestId =
    liveSessionId ?? storedSessionIds[0] ?? archivedSessionIds[0];
  return {
    path: space.path,
    title: basenameOfPath(space.path),
    liveSessionId,
    storedSessionIds,
    archivedSessionIds,
    trusted: space.trusted,
    lastReason:
      newestId === undefined ? undefined : closeReasons[newestId],
  };
}

interface SessionsState {
  /** Live (in-memory) sessions for this app run. */
  sessions: SessionInfo[];
  /** Stored sessions from the database that are not currently live (archived flag OFF). */
  historySessions: SessionInfo[];
  /**
   * Stored sessions with the archived flag ON (ADR 0016). STICKY: a
   * resumed archived session stays here while it is live (the Archived
   * view filters out live ids), and a closing archived session does NOT
   * land in `historySessions`. The flag changes only via
   * `archiveSession` / `unarchiveSession` (which call the backend).
   */
  archivedSessions: SessionInfo[];
  /** Archive a stored session (`historySessions` → `archivedSessions` + `set_session_archived`). */
  archiveSession: (sessionId: string) => Promise<void>;
  /** Unarchive a session (`archivedSessions` → `historySessions` + `set_session_archived`). */
  unarchiveSession: (sessionId: string) => Promise<void>;
  activeSessionId: string | null;
  /**
   * The selected Space's path — the top tabs' state (a Space is a tab,
   * like a browser tab): the sidebar's Sessions list shows this space's
   * sessions, and `⌘N` / `New Session` start in it. `null` when no space
   * is selected (no spaces yet, or the last one was removed).
   */
  activeSpacePath: string | null;
  /** Space bookkeeping rows (from `list_spaces` on boot; upserted by `addSpace`). */
  spaces: SpaceRow[];
  /**
   * Close reason recorded on `session-closed`, per session id (for the
   * paused banner copy). Merge-only: a reason outlives its event, so
   * keys are never deleted.
   */
  closeReasons: Record<string, CloseReasonStr>;
  /** Transcript per session id. Kept after close for history. */
  messages: Record<string, Message[]>;
  /** Config options per session id. */
  configOptions: Record<string, SessionConfigOption[]>;
  /**
   * The session's context usage per session id (the composer's
   * context-percentage display — from `context_usage_update` frames the
   * harness emits: the current context size vs the model's window).
   * `undefined` until the first frame (a fresh session, or a stored
   * session that isn't live).
   */
  contextUsage: Record<string, { used: number; window: number } | undefined>;
  /** Whether a prompt turn is in flight for a session. */
  inTurn: Record<string, boolean>;
  /** Last stop reason reported for a session's turn. */
  stopReasons: Record<string, StopReason>;
  /**
   * The stalled state per session id (a Worker crash — the
   * `session-stalled` event, ADR 0025 Task 6): `null`/absent when the
   * session is healthy. Set by `markStalled`; cleared by a successful
   * `resumeSession` (the Supervisor re-spawned the Worker) and by
   * `handleSessionClosed` (a closed session cannot be stalled — the
   * banner would otherwise linger on the paused transcript).
   */
  stalled: Record<string, { at: number; crashLog: string | null } | undefined>;
  /** Mark a session stalled (the `session-stalled` event — the `StalledInfo` shape: `at` + `crashLog`). */
  markStalled: (sessionId: string, at: number, crashLog: string | null) => void;

  addSession: (info: SessionInfo) => void;
  setActiveSession: (sessionId: string | null) => void;
  /**
   * Select a Space (the tab click): it becomes `activeSpacePath` and its
   * most recent session opens — a LIVE session first (the most-recently
   * started one, the one-live-cap-lifted rule), then the newest stored
   * (input order — do NOT re-sort), then NOTHING (an empty space lands
   * on the chat's empty state). A no-op for an unknown path.
   */
  selectSpace: (path: string) => void;
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
  /**
   * Flip a space's `trusted` flag: optimistic local update first (the spaces
   * store has NO live refresh — the flag must flip immediately, not wait for
   * a round-trip), then the `set_space_trusted` command; on rejection the
   * flag rolls back to the previous value.
   */
  setSpaceTrusted: (path: string, trusted: boolean) => void;
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
  addUserMessage: (sessionId: string, text: string, images?: ImageRef[]) => void;
  beginTurn: (sessionId: string) => void;
  applyConfigOptions: (sessionId: string, options: SessionConfigOption[]) => void;
  applySessionUpdate: (sessionId: string, update: AcpSessionUpdate) => void;
  applySessionUpdates: (updates: SessionUpdateBatchItem[]) => void;
  handleSessionClosed: (sessionId: string, reason: CloseReasonStr) => void;
  turnCompleted: (sessionId: string, stopReason: StopReason) => void;
}

// Per-path queue of in-flight `set_space_trusted` commands: a toggle is
// chained after the previous one for the same path, so the DB commits in
// click order (two overlapping invokes could otherwise land in either
// order and the DB could end on the OPPOSITE value from the optimistic
// UI). The stored promise never rejects (its errors are handled below),
// so a failed toggle cannot break the chain for later ones.
const inFlightToggles = new Map<string, Promise<void>>();
// The last value known to be committed per path: a failed command rolls
// the flag back to THIS (not the value at click time — which may itself
// be an optimistic value from a still-pending toggle, and rolling two
// failures back to their per-click `previous` could settle on the wrong
// side). Seeded from the DB rows in `setSpaces` / `addSpace` (and
// pruned in `removeSpace`), so every known path has a committed
// baseline; the UI-value fallback below is a last resort only.
const committedTrusted = new Map<string, boolean>();

export const useSessions = create<SessionsState>((set, get) => ({
  sessions: [],
  historySessions: [],
  archivedSessions: [],
  activeSessionId: null,
  activeSpacePath: null,
  spaces: [],
  closeReasons: {},
  messages: {},
  configOptions: {},
  contextUsage: {},
  inTurn: {},
  stopReasons: {},
  stalled: {},

  markStalled: (sessionId, at, crashLog) =>
    set((state) => ({
      stalled: { ...state.stalled, [sessionId]: { at, crashLog } },
    })),

  addSession: (info) =>
    set((state) => ({
      sessions: [
        ...state.sessions.filter((s) => s.sessionId !== info.sessionId),
        info,
      ],
      historySessions: state.historySessions.filter(
        (s) => s.sessionId !== info.sessionId,
      ),
      // A fresh session id never collides with an archived one (defensive
      // no-op — keeps the "live ids are out of Archived" invariant cheaply
      // at the write site).
      archivedSessions: state.archivedSessions.filter(
        (s) => s.sessionId !== info.sessionId,
      ),
      // A session that was just *started* (dialog or "new conversation")
      // is the one the user wants to look at: start flows ALWAYS switch
      // the view to it. (The live-session handling in `openSession` is
      // unchanged.)
      activeSessionId: info.sessionId,
      messages: { ...state.messages, [info.sessionId]: [] },
      configOptions: info.configOptions
        ? { ...state.configOptions, [info.sessionId]: info.configOptions }
        : state.configOptions,
    })),

  applyConfigOptions: (sessionId, options) =>
    set((state) => ({
      configOptions: { ...state.configOptions, [sessionId]: options },
    })),

  setActiveSession: (sessionId) => set({ activeSessionId: sessionId }),

  setSpaces: (rows) => {
    const state = get();
    const selectedId =
      state.activeSessionId === null
        ? autoSelectActive(rows, state.sessions, state.historySessions)
        : state.activeSessionId;
    // The selected session's space (matched by `cwd` — the session's `cwd`
    // IS the Space's folder); when nothing is selected, the most recent
    // space (the FIRST row — `list_spaces` order is `lastOpenedAt` desc).
    const owner =
      selectedId === null
        ? null
        : rows.find((s) => {
            const live = state.sessions.find(
              (x) => x.sessionId === selectedId,
            );
            const stored =
              state.historySessions.find(
                (x) => x.sessionId === selectedId,
              ) ?? state.archivedSessions.find((x) => x.sessionId === selectedId);
            return (live?.cwd ?? stored?.cwd) === s.path;
          }) ?? null;
    set({
      spaces: rows,
      activeSessionId: selectedId,
      activeSpacePath: owner ? owner.path : rows[0]?.path ?? null,
    });
    // Seed the committed-trusted baseline from the DB rows: every path
    // now has a known-committed value, so a rollback target is never
    // inferred from a possibly-optimistic UI value.
    for (const s of rows) committedTrusted.set(s.path, s.trusted);
    // `autoSelectActive` lands on a stored session (live ones never survive
    // a restart): load its transcript so boot lands on content, not an
    // empty chat. (No-op for live / already-loaded sessions.)
    if (selectedId) get().openSession(selectedId);
  },

  addSpace: (path) => {
    // A fresh space is committed as `trusted: false` in the DB: seed the
    // baseline so the first toggle's rollback targets the DB value, not
    // a possibly-optimistic UI value. (A re-added space keeps its
    // existing baseline — `setSpaces` re-seeds it from the DB.)
    if (!committedTrusted.has(path)) committedTrusted.set(path, false);
    set((state) => {
      const existing = state.spaces.find((s) => s.path === path);
      const now = Date.now();
      const spaces = existing
        ? state.spaces.map((s) =>
            s.path === path ? { ...s, lastOpenedAt: now } : s,
          )
        : [...state.spaces, { path, createdAt: now, lastOpenedAt: now, trusted: false }];
      // The start flow (the dialog / `⌘N`) lands on the new space: it
      // becomes the selected one (the tab bar highlights it).
      return { spaces, activeSpacePath: path };
    });
  },

  selectSpace: (path) => {
    const state = get();
    if (!state.spaces.some((s) => s.path === path)) return;
    // Live first (the most-recently-started one — the one-live-cap-lifted
    // rule), then the newest stored (input order — do NOT re-sort), then
    // nothing (an empty space → the chat's empty state).
    const live = mostRecentLiveInSpace(state.sessions, path)?.sessionId ?? null;
    const stored =
      state.historySessions.find((s) => s.cwd === path)?.sessionId ?? null;
    const id = live ?? stored;
    set({ activeSpacePath: path, activeSessionId: id });
    // Load the opened session's transcript (a no-op when already loaded —
    // `openSession` self-guards a mid-flight switch too).
    if (id) get().openSession(id);
  },

  removeSpace: async (path) => {
    await deleteSpaceCommand(path);
    // Prune the per-path state: a re-added space must not inherit a
    // stale rollback target (or a stale in-flight queue entry) from a
    // removed one.
    inFlightToggles.delete(path);
    committedTrusted.delete(path);
    set((state) => ({
      spaces: state.spaces.filter((s) => s.path !== path),
      // The removed space was the selected one: fall back to the first
      // remaining space (input order — `lastOpenedAt` desc) or `null`.
      ...(state.activeSpacePath === path
        ? { activeSpacePath: state.spaces.filter((s) => s.path !== path)[0]?.path ?? null }
        : {}),
    }));
  },

  setSpaceTrusted: (path, trusted) => {
    // Optimistic: flip the flag now, roll back if the command fails (the
    // spaces store has no live refresh — the flag must not wait for a
    // round-trip). The rollback target is the last COMMITTED value, not
    // the UI value at click time (see `committedTrusted` above).
    const previous =
      committedTrusted.get(path) ??
      get().spaces.find((s) => s.path === path)?.trusted ??
      false;
    set((state) => ({
      spaces: state.spaces.map((s) =>
        s.path === path ? { ...s, trusted } : s,
      ),
    }));
    // Chain this command after the previous one for the path (the
    // per-path queue above) so the DB commits in click order. The stored
    // `done` promise never rejects (its errors are handled below), so a
    // failed toggle cannot break the chain for later ones.
    const run = () =>
      inFlightToggles
        .get(path)
        ?.then(() => setSpaceTrustedCommand(path, trusted))
        ?? setSpaceTrustedCommand(path, trusted);
    const done = run().then(
      () => {
        // Gate on the path still being present in the store: a success
        // that lands AFTER a `removeSpace` (its prune) must not
        // re-insert a baseline entry — a re-added space would then keep
        // the stale value over its fresh `trusted: false` DB row. (The
        // rejection handler below only maps `state.spaces` — a no-op for
        // a removed path — and never writes `committedTrusted`.)
        if (get().spaces.some((s) => s.path === path)) {
          committedTrusted.set(path, trusted);
        }
      },
      (err) => {
        console.error(`Failed to set trusted for ${path}:`, err);
        set((state) => ({
          spaces: state.spaces.map((s) =>
            s.path === path ? { ...s, trusted: previous } : s,
          ),
        }));
      },
    );
    inFlightToggles.set(path, done);
  },

  setHistorySessions: (rows) =>
    set((state) => {
      // Split by the archived flag; live ids go in NEITHER list (they are
      // already in `sessions`). Boot-only: do NOT re-run this mid-run
      // (re-splitting would drop sticky live+archived entries and silently
      // un-archive on the next close).
      const liveIds = new Set(state.sessions.map((s) => s.sessionId));
      const archived = rows.filter(
        (r) => r.archived && !liveIds.has(r.sessionId),
      );
      const history = rows.filter(
        (r) => !r.archived && !liveIds.has(r.sessionId),
      );
      return { historySessions: history, archivedSessions: archived };
    }),

  archiveSession: async (sessionId) => {
    // No-op when the entry is not a stored (flag-off) session — the UI
    // only offers archive there. (Re-checked inside `set` below as well —
    // the entry may have been deleted or moved mid-await, and a double
    // click may have landed the move already.)
    const entry = get().historySessions.find(
      (s) => s.sessionId === sessionId,
    );
    if (!entry) return;
    await setSessionArchivedCommand(sessionId, true);
    set((state) => {
      // Re-validate against the CURRENT state: a concurrent delete (or a
      // prior move of the same id) must leave no ghost row, and the
      // dedupe below keeps the id in `archivedSessions` exactly once.
      // Distinguish "gone" (in NEITHER list — deleted) from "live" (in
      // `sessions` — a resume landed mid-await and removed the id from
      // `historySessions`): the DB flag is now `true`, so a live id must
      // land in `archivedSessions` (the sticky semantics) — not no-op.
      const inHistory = state.historySessions.some(
        (s) => s.sessionId === sessionId,
      );
      const isLive = state.sessions.some(
        (s) => s.sessionId === sessionId,
      );
      if (!inHistory && !isLive) return state;
      return {
        // Only remove from `historySessions` when it is still there (a
        // resume already removed it — the "move" is then just "ensure it
        // is in `archivedSessions`").
        historySessions: inHistory
          ? state.historySessions.filter((s) => s.sessionId !== sessionId)
          : state.historySessions,
        // Append, preserving order (deduped — a concurrent move of the
        // same id already landed it here).
        archivedSessions: [
          ...state.archivedSessions.filter((s) => s.sessionId !== sessionId),
          entry,
        ],
      };
    });
  },

  unarchiveSession: async (sessionId) => {
    // The mirror of `archiveSession`: no-op when the entry is not in the
    // archived list. (Re-checked inside `set` below as well — the entry
    // may have been deleted or moved mid-await, and a double click may
    // have landed the move already.)
    const entry = get().archivedSessions.find(
      (s) => s.sessionId === sessionId,
    );
    if (!entry) return;
    await setSessionArchivedCommand(sessionId, false);
    set((state) => {
      // Re-validate against the CURRENT state: a concurrent delete (or a
      // prior move of the same id) must leave no ghost row, and the
      // dedupe below keeps the id in `historySessions` exactly once.
      // Distinguish "gone" (in NEITHER list — deleted) from "live" (in
      // `sessions` — a resume landed mid-await): a live id must NOT be
      // appended to `historySessions` (it would be in BOTH lists at once
      // — a duplicate sidebar row) — `handleSessionClosed` lands it in
      // `historySessions` on close (and only when not archived, which it
      // no longer is).
      const inArchived = state.archivedSessions.some(
        (s) => s.sessionId === sessionId,
      );
      const isLive = state.sessions.some(
        (s) => s.sessionId === sessionId,
      );
      if (!inArchived && !isLive) return state;
      return {
        archivedSessions: state.archivedSessions.filter(
          (s) => s.sessionId !== sessionId,
        ),
        // Append, preserving order (deduped — a concurrent move of the
        // same id already landed it here). Skipped while live — see
        // above.
        historySessions: isLive
          ? state.historySessions
          : [
              ...state.historySessions.filter(
                (s) => s.sessionId !== sessionId,
              ),
              entry,
            ],
      };
    });
  },

  openSession: (sessionId) => {
    // The opened session's space (matched by `cwd` — live, stored, OR
    // archived membership) becomes the selected one (the tab bar follows
    // the session); a session whose cwd is no Space (a legacy row) leaves
    // the selection alone.
    const state = get();
    const owner = state.spaces.find((s) => {
      const live = state.sessions.find((x) => x.sessionId === sessionId);
      const stored =
        state.historySessions.find((x) => x.sessionId === sessionId) ??
        state.archivedSessions.find((x) => x.sessionId === sessionId);
      return (live?.cwd ?? stored?.cwd) === s.path;
    });
    set({
      activeSessionId: sessionId,
      ...(owner ? { activeSpacePath: owner.path } : {}),
    });
    if (state.messages[sessionId]) return; // already loaded
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
        archivedSessions: state.archivedSessions.filter(
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
    // Search ALL THREE lists: an archived session must be resumable from
    // the Archived section (otherwise this throws `unknown session`).
    const session = [
      ...state.sessions,
      ...state.historySessions,
      ...state.archivedSessions,
    ].find((s) => s.sessionId === sessionId);
    if (!session) throw new Error(`unknown session: ${sessionId}`);
    const info = await resumeSessionCommand(sessionId, session.cwd);
    // STICKY: `archivedSessions` is deliberately NOT touched — a resumed
    // archived session stays in it while live (the Archived view filters
    // out live ids).
    set((st) => ({
      sessions: [
        ...st.sessions.filter((s) => s.sessionId !== sessionId),
        info,
      ],
      historySessions: st.historySessions.filter(
        (s) => s.sessionId !== sessionId,
      ),
      activeSessionId: st.activeSessionId ?? sessionId,
      // A successful resume clears the stalled state (the Supervisor
      // re-spawned the Worker — the banner hides; the `session-stalled`
      // bookkeeping is backend-side, this is the frontend's view).
      stalled: { ...st.stalled, [sessionId]: undefined },
      configOptions: info.configOptions
        ? { ...st.configOptions, [sessionId]: info.configOptions }
        : st.configOptions,
    }));
    // ACP `session/load` does not re-stream the transcript — reload it from
    // the persisted history so the pane shows the conversation on resume.
    void (async () => {
      try {
        // The reload is fire-and-forget (NOT awaited by the caller): a
        // `send()` on the resumed session adds the user message to
        // `messages[sessionId]` WHILE the reload is in flight. Applying the
        // reloaded rows verbatim would WIPE that message (the reload
        // predates its persistence — the user message is `record_message`d
        // by `send_prompt`, which runs after the reload starts). Merge
        // instead: the reloaded rows, then any messages added after the
        // reload started, appended after.
        const prior = get().messages[sessionId] ?? [];
        const rows = await loadHistory(sessionId);
        // The user may have switched away while the fetch was in flight.
        if (get().activeSessionId !== sessionId) return;
        const reloaded = rows.flatMap(rowToMessages);
        const added = (get().messages[sessionId] ?? []).slice(prior.length);
        // DEDUPE the merge: `send_prompt`'s `record_message` can commit
        // the new user message BEFORE the history query reads — then the
        // reloaded rows INCLUDE it while `added` still holds its local
        // copy, and appending verbatim would render the prompt TWICE
        // (attachments included). Drop an `added` message already present
        // in the reloaded rows: by id when one exists on both sides
        // (agent-text/agent-thought `messageId`, tool-call `id`), and for
        // a `user` message (NO id on either side) by text + images — but
        // ONLY when the matching reloaded row is NEWER-OR-EQUAL by
        // timestamp. Content alone is ambiguous because user messages have
        // no stable id: the matching reloaded row is either the SAME
        // message committed after the local copy was created (committed-
        // before-read: the DB commit's `createdAt` >= the local `at` =
        // `Date.now()` at `addUserMessage` time → dedup, no duplicate)
        // or an OLDER re-sent prompt (the reloaded row predates the new
        // message's creation time → the new turn is NOT a duplicate and
        // must be kept). Equal timestamps dedup (the safe direction).
        const reloadedKeys = new Set<string>();
        const reloadedUserAt = new Map<string, number>();
        for (const m of reloaded) {
          const key = mergeDedupeKey(m);
          if (m.kind === "user") {
            // Several reloaded rows can share a content key (the prompt was
            // re-sent earlier): the LATEST one is the only one that could
            // be the just-committed message.
            reloadedUserAt.set(key, Math.max(reloadedUserAt.get(key) ?? 0, m.at));
          } else {
            reloadedKeys.add(key);
          }
        }
        const fresh = added.filter((m) => {
          const key = mergeDedupeKey(m);
          if (m.kind === "user") {
            const reloadedAt = reloadedUserAt.get(key);
            return reloadedAt === undefined || reloadedAt < m.at;
          }
          return !reloadedKeys.has(key);
        });
        set((st) => ({
          messages: { ...st.messages, [sessionId]: [...reloaded, ...fresh] },
        }));
      } catch (err) {
        console.error(`failed to load history for ${sessionId}`, err);
      }
    })();
    return info;
  },

  addUserMessage: (sessionId, text, images) =>
    set((state) => ({
      messages: {
        ...state.messages,
        [sessionId]: [
          ...(state.messages[sessionId] ?? []),
          {
            kind: "user" as const,
            text,
            at: Date.now(),
            ...(images && images.length > 0 ? { images } : {}),
          },
        ],
      },
    })),

  beginTurn: (sessionId) =>
    set((state) => ({ inTurn: { ...state.inTurn, [sessionId]: true } })),

  applySessionUpdate: (sessionId, update) =>
    set((state) => {
      const messages = applySessionUpdate(
        state.messages[sessionId] ?? [],
        update,
        Date.now(),
      );

      let configOptions = state.configOptions;
      if (update.sessionUpdate === "config_option_update") {
        configOptions = { ...state.configOptions, [sessionId]: update.configOptions };
      }

      let contextUsage = state.contextUsage;
      if (update.sessionUpdate === "context_usage_update") {
        contextUsage = {
          ...state.contextUsage,
          [sessionId]: { used: update.usedTokens, window: update.windowTokens },
        };
      }

      return {
        messages: {
          ...state.messages,
          [sessionId]: messages,
        },
        configOptions,
        contextUsage,
      };
    }),

  // Batch form of `applySessionUpdate`: applies a frame's worth of updates in
  // ONE `setState` (one React render per frame, not one per event — see
  // `createBatchedSessionUpdate`). Grouped per session in arrival order; the
  // pure reducer returns the SAME reference for no-op updates, so a batch that
  // changes nothing returns the same state object and notifies nobody.
  applySessionUpdates: (updates) =>
    set((state) => {
      if (updates.length === 0) return state;
      const now = Date.now();

      const order: string[] = [];
      const bySession = new Map<string, AcpSessionUpdate[]>();
      for (const { sessionId, update } of updates) {
        const list = bySession.get(sessionId);
        if (list) {
          list.push(update);
        } else {
          bySession.set(sessionId, [update]);
          order.push(sessionId);
        }
      }

      let messages = state.messages;
      let messagesChanged = false;
      let configOptions = state.configOptions;
      let configChanged = false;
      let contextUsage = state.contextUsage;
      let contextChanged = false;
      for (const sessionId of order) {
        const sessionUpdates = bySession.get(sessionId)!;
        let sessionMessages = messages[sessionId] ?? [];
        let sessionChanged = false;
        for (const update of sessionUpdates) {
          const next = applySessionUpdate(sessionMessages, update, now);
          if (next !== sessionMessages) sessionChanged = true;
          sessionMessages = next;
          if (update.sessionUpdate === "config_option_update") {
            configOptions = { ...configOptions, [sessionId]: update.configOptions };
            configChanged = true;
          }
          // Last-wins within the batch: a later frame in the same batch
          // replaces the earlier one (the compaction re-estimate lands
          // after the pre-compaction `Usage` frames).
          if (update.sessionUpdate === "context_usage_update") {
            contextUsage = {
              ...contextUsage,
              [sessionId]: { used: update.usedTokens, window: update.windowTokens },
            };
            contextChanged = true;
          }
        }
        if (sessionChanged) {
          if (!messagesChanged) messages = { ...messages };
          messages[sessionId] = sessionMessages;
          messagesChanged = true;
        }
      }

      if (!messagesChanged && !configChanged && !contextChanged) return state;
      return {
        ...state,
        ...(messagesChanged ? { messages } : {}),
        ...(configChanged ? { configOptions } : {}),
        ...(contextChanged ? { contextUsage } : {}),
      };
    }),

  handleSessionClosed: (sessionId, reason) => {
    // Dismiss the session's permission prompts (auto-cancelled server-side).
    usePermissions.getState().dismissSessionPrompts(sessionId);
    const closedInfo = useSessions
      .getState()
      .sessions.find((s) => s.sessionId === sessionId);
    set((state) => {
      return {
        sessions: state.sessions.filter((s) => s.sessionId !== sessionId),
        // The session remains in the database: it moves to the history
        // list — UNLESS it is archived (sticky, ADR 0016): a closing
        // archived session stays ONLY in `archivedSessions`.
        historySessions:
          closedInfo &&
          !state.archivedSessions.some((s) => s.sessionId === sessionId)
            ? [
                ...state.historySessions.filter(
                  (s) => s.sessionId !== sessionId,
                ),
                closedInfo,
              ]
            : state.historySessions,
        // A close is a PAUSE, not a discard: the conversation moves to the
        // history list but STAYS the displayed one, so `ChatStream` renders
        // its stored/paused banner instead of the `No active session` empty
        // state. (Required for the Task 6 `Pause` behavior.)
        activeSessionId: state.activeSessionId,
        // Merge-only: a close reason outlives its event (never delete keys).
        closeReasons: { ...state.closeReasons, [sessionId]: reason },
        // The config options + context usage STAY (a pause, not a
        // discard): the last known values populate the stored session's
        // bottom bar — the selectors render disabled; a resume's
        // `config_option_update` / `context_usage_update` frames refresh
        // them. (The row's own `configOptions` / `contextUsage` — the Rust
        // `list_sessions` shape, persisted on every frame — is the
        // fallback when the store's map has no entry: a session never
        // live in this app session.)
        messages: {
          ...state.messages,
          [sessionId]: finalizeSessionMessages(state.messages[sessionId] ?? []),
        },
        inTurn: { ...state.inTurn, [sessionId]: false },
        // A closed session cannot be stalled (the banner would linger on
        // the paused transcript — a close supersedes the crash).
        stalled: { ...state.stalled, [sessionId]: undefined },
      };
    });
  },

  turnCompleted: (sessionId, stopReason) =>
    set((state) => ({
      inTurn: { ...state.inTurn, [sessionId]: false },
      stopReasons: { ...state.stopReasons, [sessionId]: stopReason },
    })),
}));
