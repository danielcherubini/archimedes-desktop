import { create } from "zustand";
import type { SubagentMetrics } from "../lib/tauri";

export type { SubagentMetrics } from "../lib/tauri";

/**
 * Retention cap for CLOSED (completed/failed) entries. Subagent sessions are
 * EPHEMERAL (never persisted — the desktop's Rust side records them with
 * `db: None`), so the panel only needs the most recent K; without the cap,
 * entries would accumulate for the app's process lifetime (N dispatches per
 * app session). RUNNING entries are never evicted.
 */
export const MAX_CLOSED_ENTRIES = 20;

/**
 * If the number of closed entries exceeds the cap, delete the OLDEST closed
 * entries (insertion order — `Object.keys` order; deletion preserves the
 * survivors' order). Running entries are never evicted.
 */
function evictOldestClosed(
  entries: Record<string, SubagentEntry>,
): Record<string, SubagentEntry> {
  const closedIds = Object.keys(entries).filter(
    (id) => entries[id].status !== "running",
  );
  if (closedIds.length <= MAX_CLOSED_ENTRIES) return entries;
  const evict = new Set(
    closedIds.slice(0, closedIds.length - MAX_CLOSED_ENTRIES),
  );
  const next: Record<string, SubagentEntry> = {};
  for (const [id, entry] of Object.entries(entries)) {
    if (!evict.has(id)) next[id] = entry;
  }
  return next;
}

export interface SubagentEntry {
  sessionId: string;
  parentSessionId: string;
  agentName: string;
  task: string;
  status: "running" | "completed" | "failed";
  /**
   * Wall-clock arrival (`Date.now()` at `addSession`). The directory's
   * relative time for RUNNING entries (elapsed since arrival). A
   * re-delivered frame keeps the ORIGINAL value (a re-sent frame must not
   * reset the clock).
   */
  startedAt: number;
  /** Wall-clock close (`Date.now()` at `markClosed`) — the directory's relative time for ENDED entries (when it ended — distinct from `metrics.durationMs`, the DURATION). */
  endedAt?: number;
  error?: string;
  /**
   * Snapshot from the `subagent-closed` payload. The metrics line MUST read
   * THIS (never `useBridge.cost[sessionId]`): the `session-closed` handler
   * calls `useBridge.dismissSession`, which DELETES `cost`/`agentState` for
   * the id — and `session-closed` (driver teardown) and `subagent-closed`
   * (worker task) are CONCURRENT, so the ordering is racy. This store is
   * the only one `dismissSession` never touches.
   */
  metrics?: SubagentMetrics;
}

interface SubagentState {
  /** Keyed by the subagent session id (insertion order = arrival order). */
  entries: Record<string, SubagentEntry>;
  /** "subagent-session-started" (the store stamps `startedAt`; a re-delivery keeps the original). */
  addSession: (e: Omit<SubagentEntry, "startedAt" | "endedAt">) => void;
  /** "subagent-closed" */
  markClosed: (
    sessionId: string,
    status: "completed" | "failed",
    error?: string,
    metrics?: SubagentMetrics,
  ) => void;
  /** User dismiss (panel header). */
  dismiss: (sessionId: string) => void;
}

export const useSubagents = create<SubagentState>((set) => ({
  entries: {},

  addSession: (e) =>
    set((state) => {
      // A re-delivered frame replaces the entry (latest wins).
      // Re-delivery semantics: a re-delivered `subagent-session-started`
      // for an EVICTED id re-inserts it as `running` (unevictable until
      // a `markClosed` that may never come) — accepted as-is: it requires
      // double-delivered events, which the listener registration guards
      // against (App.tsx's StrictMode double-registration guard).
      const existing = state.entries[e.sessionId];
      const entries = {
        ...state.entries,
        [e.sessionId]: {
          ...e,
          // A re-sent frame must not reset the directory's elapsed-time
          // clock: keep the ORIGINAL arrival.
          startedAt: existing?.startedAt ?? Date.now(),
        },
      };
      return { entries: evictOldestClosed(entries) };
    }),

  markClosed: (sessionId, status, error, metrics) =>
    set((state) => {
      const existing = state.entries[sessionId];
      if (!existing) return state; // unknown / dismissed id: no-op
      const entries = {
        ...state.entries,
        [sessionId]: {
          ...existing,
          status,
          error,
          metrics,
          // The directory's relative time for ENDED entries (when it
          // ended — distinct from `metrics.durationMs`, the DURATION).
          endedAt: Date.now(),
        },
      };
      return { entries: evictOldestClosed(entries) };
    }),

  dismiss: (sessionId) =>
    set((state) => {
      if (!state.entries[sessionId]) return state;
      const entries = { ...state.entries };
      delete entries[sessionId];
      return { entries };
    }),
}));
