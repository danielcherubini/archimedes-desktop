import { useSubagents } from "../store/subagents";
import { useInteractive } from "../store/interactive";
import { usePermissions } from "../store/permissions";

/**
 * Count the pending interactive requests across ALL `useSubagents` entry
 * session ids: `useInteractive` `ask`/`confirm`/`password` requests +
 * `usePermissions` prompts (the SAME derivation `SidePane`'s "Waiting"
 * pill used inline — moved here so the `SidePane` pill and the
 * `ChatStream` header toggle dot share ONE source of truth).
 *
 * The subagent entry count does NOT change when a request arrives, and a
 * subagent session id is never the ACTIVE session (so a per-active-session
 * count sees nothing) — without this cue a pending subagent request would
 * strand until the interactive timeout.
 */
export function usePendingSubagentRequests(): number {
  const subagentEntries = useSubagents((s) => s.entries);
  const interactiveRequests = useInteractive((s) => s.requests);
  const permissionPrompts = usePermissions((s) => s.prompts);
  let count = 0;
  for (const id of Object.keys(subagentEntries)) {
    count += (interactiveRequests[id] ?? []).filter(
      (r) =>
        r.method === "ask" || r.method === "confirm" || r.method === "password",
    ).length;
    count += (permissionPrompts[id] ?? []).length;
  }
  return count;
}
