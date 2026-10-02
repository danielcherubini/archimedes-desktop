import { useRef, useState } from "react";
import { startSession } from "../lib/tauri";
import { useSessions, type SpaceView } from "../store/sessions";

/**
 * Start a NEW conversation in a space (the `ChatStream` "New conversation"
 * logic, extracted): `spacePath = view.path` (the backend canonicalizes) —
 * a native session (there is one harness — no agent to choose).
 *
 * With the one-live cap lifted, a new conversation does NOT
 * displace a live one — they coexist.
 *
 * `view === undefined` → no-op.
 */
export function useStartNewConversation(view: SpaceView | undefined): {
  startNewConversation: () => Promise<void>;
  error: string | null;
  clearError: () => void;
} {
  const addSession = useSessions((s) => s.addSession);
  const addSpace = useSessions((s) => s.addSpace);
  const [error, setError] = useState<string | null>(null);
  // Double-invoke guard: ⌘N twice fast (or double-clicking a group's `+`)
  // must fire ONE `startSession`, not two. (Suppresses CONCURRENT
  // double-invokes only.)
  const inFlight = useRef(false);

  const startNewConversation = async () => {
    if (view === undefined) return;
    if (inFlight.current) return;
    inFlight.current = true;
    setError(null);
    try {
      const info = await startSession(view.path);
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
