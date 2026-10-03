import { useInteractive } from "../store/interactive";
import { usePermissions } from "../store/permissions";
import { useSessions } from "../store/sessions";
import { usePendingSubagentRequests } from "./usePendingSubagentRequests";

/**
 * Whether ANY pending attention exists for the active session: the active
 * session's permission prompts OR interactive `ask`/`confirm`/`password`
 * requests, plus pending subagent requests (a subagent session id is never
 * the ACTIVE session, so its count comes from the shared
 * `usePendingSubagentRequests` hook — the SAME derivation the `ChatStream`
 * header toggle dot used inline; moved here so the header dot, the
 * `SidePane` footer toggle dot, and the `SpacesList` collapse-button dot
 * share ONE source of truth).
 */
export function useHasPendingRequest(): boolean {
  const activeSessionId = useSessions((s) => s.activeSessionId);
  const prompts =
    usePermissions((s) =>
      activeSessionId ? s.prompts[activeSessionId] : undefined,
    ) ?? [];
  const interactiveRequests =
    useInteractive((s) =>
      activeSessionId ? s.requests[activeSessionId] : undefined,
    ) ?? [];
  const pendingSubagentRequests = usePendingSubagentRequests();
  return (
    prompts.length > 0 ||
    interactiveRequests.some(
      (r) =>
        r.method === "ask" || r.method === "confirm" || r.method === "password",
    ) ||
    pendingSubagentRequests > 0
  );
}
