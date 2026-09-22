import { useEffect, useRef, useState } from "react";
import { listAgents, startSession, type AgentEntryDto } from "../lib/tauri";
import { useSessions, type SpaceView } from "../store/sessions";

/**
 * Start a NEW conversation in a space (the `ChatStream` "New conversation"
 * logic, extracted): `agentId = live ?? storedMostRecent ?? firstAgentId
 * (registry default)`, `spacePath = view.path` (the backend canonicalizes).
 *
 * `SpaceView` has no `liveSession` object and no `storedMostRecent` field —
 * both are DERIVED here from the store (`sessions` / `historySessions`),
 * exactly as `ChatStream` does. With the one-live cap lifted (ADR 0002) a
 * new conversation does NOT displace a live one — they coexist.
 *
 * `view === undefined` → no-op, and `agentId === ""` → no-op (the registry
 * hasn't loaded yet — the same guard `ChatStream` applied to the button).
 *
 * The registry fetch is a MODULE-LEVEL memoized promise: the hook is
 * consumed at `ChatStream` (the "…" menu item), `SpacesList` top-level
 * ("New Session"), and once per `SpaceGroup` (the per-space `+`) — ten
 * spaces ⇒ 12 identical `listAgents` IPC round-trips at boot without it.
 * All instances share ONE fetch (`.catch(() => [])` keeps the
 * no-op-on-error behavior).
 */
export function useStartNewConversation(view: SpaceView | undefined): {
  startNewConversation: () => Promise<void>;
  error: string | null;
  clearError: () => void;
} {
  const sessions = useSessions((s) => s.sessions);
  const historySessions = useSessions((s) => s.historySessions);
  const addSession = useSessions((s) => s.addSession);
  const addSpace = useSessions((s) => s.addSpace);
  // Registry for the `agentId` fallback (fetched `useEffect`-style;
  // `firstAgentId` is the default).
  const [agents, setAgents] = useState<AgentEntryDto[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Double-invoke guard: ⌘N twice fast (or double-clicking a group's `+`)
  // must fire ONE `startSession`, not two. (Suppresses CONCURRENT
  // double-invokes only — the `agentId === ""` no-op above still applies.)
  const inFlight = useRef(false);

  useEffect(() => {
    void loadAgents().then(setAgents);
  }, []);

  const live = sessions.find((s) => s.sessionId === view?.liveSessionId);
  const storedMostRecent = historySessions.find(
    (s) => s.sessionId === view?.storedSessionIds[0],
  );
  const firstAgentId = agents?.[0]?.id ?? "";
  const agentId = live?.agentId ?? storedMostRecent?.agentId ?? firstAgentId;

  const startNewConversation = async () => {
    if (view === undefined || agentId === "") return;
    if (inFlight.current) return;
    inFlight.current = true;
    setError(null);
    try {
      const info = await startSession(agentId, view.path);
      // `addSession` switches the view to the new session automatically.
      // No manual view-switch call.
      addSession(info);
      addSpace(info.cwd);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      inFlight.current = false;
    }
  };

  return {
    startNewConversation,
    error,
    clearError: () => setError(null),
  };
}

/**
 * Module-level memoized registry fetch — all hook instances share ONE
 * `listAgents` call. `.catch(() => [])` keeps the no-op-on-error behavior
 * (an empty registry ⇒ `firstAgentId === ""` ⇒ `startNewConversation` no-ops)
 * and poisons the memo with `[]` rather than a rejected promise.
 */
let agentsPromise: Promise<AgentEntryDto[]> | null = null;
const loadAgents = (): Promise<AgentEntryDto[]> =>
  (agentsPromise ??= listAgents().catch(() => []));
