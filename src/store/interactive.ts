import { create } from "zustand";

/**
 * The interactive store — the desktop's delegated interactive UI (Task 5).
 * Mirrors `permissions.ts` (pending prompts keyed by session id).
 *
 * **Password hygiene:** the `password` method's secret lives ONLY in the
 * modal's local `useState` — it is sent via `respondInteractiveRequest` and
 * is NEVER stored here (0010 / ADR 0022 invariant), so there is no
 * `markAnswered` step: a settled request is removed outright.
 *
 * **Session-id identity (B3):** the `sessionId` in every
 * `interactive-request` / `interactive-event` payload is the ACP session id
 * (the native `AgentLoop` is constructed with it, so the events always
 * carry it). The store keys by this ACP id, which is the same id the
 * frontend's `activeSessionId` holds (from `startSession`'s return) — so
 * the keys line up.
 */

/** A pending interactive request (an `interactive-request` event, keyed by `requestId`). */
export interface InteractiveRequestData {
  /** The frame UUID (the correlation key — never the bare `toolCallId`). */
  requestId: string;
  method: "ask" | "confirm" | "password";
  /** `"main"` or `"subagent:<name>"`. */
  source: string;
  /** The invoking process's tool-call id (absent for `confirm`/`password`). */
  toolCallId?: string;
  /** The method's params: the ask schema for `ask`; `{ command, reason }` for `confirm`/`password`. */
  params: Record<string, unknown>;
}

/** A todo list item (the `manage_todo_list` `TodoItem` shape). */
export interface TodoItem {
  content: string;
  description?: string;
  status: "pending" | "in_progress" | "completed";
}

/** The todo board columns for a session (main + one per subagent source). */
export interface TodoColumn {
  main: TodoItem[];
  subagents: Record<string, TodoItem[]>;
}

/** The bus `TODOS_UPDATE` payload (verbatim over the wire). */
export interface TodoUpdatePayload {
  source: string;
  todos: TodoItem[];
}

/** The bus `TODOS_CLEAR` payload (verbatim over the wire). */
export interface TodoClearPayload {
  source: string;
}

export interface CostUpdatePayload {
  inputTokens?: number;
  outputTokens?: number;
  cacheReadTokens?: number;
  cacheWriteTokens?: number;
  cost?: number;
  source?: string;
}

interface InteractiveState {
  /** Pending interactive requests, keyed by the ACP session id. */
  requests: Record<string, InteractiveRequestData[]>;
  /** Todo board columns, keyed by the ACP session id. */
  todos: Record<string, TodoColumn>;
  /** The agent state machine (`working`/`idle`/`blocked`) — wired, not rendered in v1. */
  agentState: Record<string, "working" | "idle" | "blocked">;
  /** The last `cost_update` payload — wired, not rendered in v1. */
  cost: Record<string, unknown>;
  /** The last `session` push payload — wired, not rendered in v1. */
  session: Record<string, unknown>;

  /** Add a request (dedup by `requestId` — a re-delivered frame replaces). */
  addRequest: (sessionId: string, data: InteractiveRequestData) => void;
  /** Remove a settled request. */
  removeRequest: (sessionId: string, requestId: string) => void;
  /** A `todos_update` push: set `main` for `source === "main"`, else `subagents[source]`. */
  applyTodoUpdate: (sessionId: string, payload: unknown) => void;
  /** A `todos_clear` push: delete the column. */
  applyTodoClear: (sessionId: string, payload: unknown) => void;
  /** A `state` push (`{ state }`) — store, don't render in v1. */
  applyState: (sessionId: string, payload: unknown) => void;
  /** A `cost_update` push — store, don't render in v1. */
  applyCost: (sessionId: string, payload: unknown) => void;
  /** A `session` push — store, don't render in v1. */
  applySession: (sessionId: string, payload: unknown) => void;
  /** Clear everything for a session (called on `session-closed`). */
  dismissSession: (sessionId: string) => void;
}

export const useInteractive = create<InteractiveState>((set) => ({
  requests: {},
  todos: {},
  agentState: {},
  cost: {},
  session: {},

  addRequest: (sessionId, data) =>
    set((state) => {
      const existing = (state.requests[sessionId] ?? []).filter(
        (r) => r.requestId !== data.requestId,
      );
      return {
        requests: { ...state.requests, [sessionId]: [...existing, data] },
      };
    }),

  removeRequest: (sessionId, requestId) =>
    set((state) => {
      const list = (state.requests[sessionId] ?? []).filter(
        (r) => r.requestId !== requestId,
      );
      return { requests: { ...state.requests, [sessionId]: list } };
    }),

  applyTodoUpdate: (sessionId, payload) =>
    set((state) => {
      const p = payload as { source?: string; todos?: TodoItem[] } | null;
      if (!p || typeof p.source !== "string" || !Array.isArray(p.todos)) {
        return state;
      }
      const column: TodoColumn = {
        main: state.todos[sessionId]?.main ?? [],
        subagents: { ...(state.todos[sessionId]?.subagents ?? {}) },
      };
      if (p.source === "main") {
        column.main = p.todos;
      } else {
        column.subagents[p.source] = p.todos;
      }
      return { todos: { ...state.todos, [sessionId]: column } };
    }),

  applyTodoClear: (sessionId, payload) =>
    set((state) => {
      const p = payload as { source?: string } | null;
      if (!p || typeof p.source !== "string") return state;
      const column = state.todos[sessionId];
      if (!column) return state;
      const cleared: TodoColumn = {
        main: [...column.main],
        subagents: { ...column.subagents },
      };
      if (p.source === "main") {
        cleared.main = [];
      } else {
        delete cleared.subagents[p.source];
      }
      return { todos: { ...state.todos, [sessionId]: cleared } };
    }),

  applyState: (sessionId, payload) =>
    set((state) => {
      const s = (payload as { state?: string } | null)?.state;
      if (s !== "working" && s !== "idle" && s !== "blocked") return state;
      return { agentState: { ...state.agentState, [sessionId]: s } };
    }),

  applyCost: (sessionId, payload) =>
    set((state) => {
      const p = payload as CostUpdatePayload | undefined;
      const prev = state.cost[sessionId] as CostUpdatePayload | undefined;
      if (!prev || typeof prev !== "object") {
        return { cost: { ...state.cost, [sessionId]: payload } };
      }
      const next: CostUpdatePayload = {
        inputTokens: (prev.inputTokens ?? 0) + (p?.inputTokens ?? 0),
        outputTokens: (prev.outputTokens ?? 0) + (p?.outputTokens ?? 0),
        cacheReadTokens: (prev.cacheReadTokens ?? 0) + (p?.cacheReadTokens ?? 0),
        cacheWriteTokens: (prev.cacheWriteTokens ?? 0) + (p?.cacheWriteTokens ?? 0),
        cost: (prev.cost ?? 0) + (p?.cost ?? 0),
        source: p?.source ?? prev.source,
      };
      return { cost: { ...state.cost, [sessionId]: next } };
    }),

  applySession: (sessionId, payload) =>
    set((state) => ({ session: { ...state.session, [sessionId]: payload } })),

  dismissSession: (sessionId) =>
    set((state) => {
      const requests = { ...state.requests };
      delete requests[sessionId];
      const todos = { ...state.todos };
      delete todos[sessionId];
      const agentState = { ...state.agentState };
      delete agentState[sessionId];
      const cost = { ...state.cost };
      delete cost[sessionId];
      const session = { ...state.session };
      delete session[sessionId];
      return { requests, todos, agentState, cost, session };
    }),
}));
