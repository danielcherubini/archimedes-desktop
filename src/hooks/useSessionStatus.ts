import { usePermissions } from "../store/permissions";
import { useInteractive } from "../store/interactive";
import { useSessions } from "../store/sessions";
import type { PermissionPromptData } from "../store/permissions";
import type { InteractiveRequestData } from "../store/interactive";

/** The four states a session row can be in, most attention-grabbing first. */
export type SessionStatus = "waiting" | "running" | "live" | "stored";

/** The inputs the status is derived from (see `sessionStatusOf`). */
export interface SessionStatusInput {
  isLive: boolean;
  inTurn: boolean;
  prompts: PermissionPromptData[];
  requests: InteractiveRequestData[];
}

/**
 * The ONE status rule for a session, as a pure function so it can serve both
 * the single-row hook and the batch hook the collapsed rail uses (a hook cannot
 * be called once per row inside a list of changing length — the pure step is
 * what makes the two consumers share the rule rather than restate it).
 *
 * `waiting` WINS over `running`: a session holding a pending prompt is not
 * working, it is BLOCKED ON THE USER, and that is the fact both the row's
 * "Waiting" pill and the rail's mark exist to report. `running` requires a LIVE
 * session — an `inTurn` flag left behind by a session that has since closed
 * must not paint activity that is not happening (the open row's
 * `isLive && inTurn`, stated once, here).
 */
export function sessionStatusOf({
  isLive,
  inTurn,
  prompts,
  requests,
}: SessionStatusInput): SessionStatus {
  const waiting =
    prompts.length > 0 ||
    requests.some(
      (r) =>
        r.method === "ask" || r.method === "confirm" || r.method === "password",
    );
  if (waiting) return "waiting";
  if (isLive && inTurn) return "running";
  return isLive ? "live" : "stored";
}

/**
 * The status of ONE session — consumed by the open `SessionRow` (its leading
 * spinner + its "Waiting" pill). It is a hook rather than logic inside
 * `SessionRow` because the collapsed rail is the SAME fact in less space, and a
 * summary that disagrees with the thing it summarises is worse than no summary
 * — the reasoning that put the todo count in `useMainOpenTodoCount` instead of
 * in the panel that renders it.
 */
export function useSessionStatus(sessionId: string): SessionStatus {
  const isLive = useSessions((s) =>
    s.sessions.some((x) => x.sessionId === sessionId),
  );
  const inTurn = useSessions((s) => !!s.inTurn[sessionId]);
  const prompts = usePermissions((s) => s.prompts[sessionId]) ?? [];
  const requests = useInteractive((s) => s.requests[sessionId]) ?? [];
  return sessionStatusOf({ isLive, inTurn, prompts, requests });
}

/**
 * The status of MANY sessions at once, for the collapsed rail. Subscribes to
 * the three maps ONCE (a hook per row would violate the rules of hooks at a
 * changing list length) and applies `sessionStatusOf` per id, so the rail's
 * marks are derived by the same rule as the rows they summarise.
 *
 * Selectors return stable references — the maps themselves, never a fresh `{}`
 * fallback INSIDE the selector — or Zustand re-renders forever.
 */
export function useSessionStatuses(
  sessionIds: string[],
): Record<string, SessionStatus> {
  const live = useSessions((s) => s.sessions);
  const inTurn = useSessions((s) => s.inTurn);
  const prompts = usePermissions((s) => s.prompts);
  const requests = useInteractive((s) => s.requests);
  const liveIds = new Set(live.map((x) => x.sessionId));
  const out: Record<string, SessionStatus> = {};
  for (const id of sessionIds) {
    out[id] = sessionStatusOf({
      isLive: liveIds.has(id),
      inTurn: !!inTurn[id],
      prompts: prompts[id] ?? [],
      requests: requests[id] ?? [],
    });
  }
  return out;
}
