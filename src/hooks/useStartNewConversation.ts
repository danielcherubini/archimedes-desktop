import { useEffect, useState } from "react";
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

  useEffect(() => {
    listAgents().then(setAgents).catch(() => setAgents([]));
  }, []);

  const live = sessions.find((s) => s.sessionId === view?.liveSessionId);
  const storedMostRecent = historySessions.find(
    (s) => s.sessionId === view?.storedSessionIds[0],
  );
  const firstAgentId = agents?.[0]?.id ?? "";
  const agentId = live?.agentId ?? storedMostRecent?.agentId ?? firstAgentId;

  const startNewConversation = async () => {
    if (view === undefined || agentId === "") return;
    setError(null);
    try {
      const info = await startSession(agentId, view.path);
      // `addSession` switches the view to the new session automatically.
      // No manual view-switch call.
      addSession(info);
      addSpace(info.cwd);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  return {
    startNewConversation,
    error,
    clearError: () => setError(null),
  };
}
