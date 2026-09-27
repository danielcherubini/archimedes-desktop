import { useShallow } from "zustand/react/shallow";
import { useSubagents } from "../store/subagents";
import { useBridge, type BridgeRequestData } from "../store/bridge";
import SudoConfirmModal from "./SudoConfirmModal";
import SudoPasswordModal from "./SudoPasswordModal";

/**
 * The bridge sudo modals for the subagent entries (rendered at the
 * `SidePane` ROOT — a `fixed` overlay, NOT inside the scrollable content:
 * a collapsed pane never hides a pending modal; the `fixed` overlays
 * escape the frame's `width: 0` + `overflow: hidden` clipping).
 *
 * `ChatStream` renders the `SudoConfirmModal`/`SudoPasswordModal` ONLY for
 * the ACTIVE session's requests, and a subagent's session id is never
 * active — this component is the ONLY render site for subagent-session
 * `confirm`/`password` requests (an unrendered request would hang until
 * the bridge timeout). The answer path is unchanged
 * (`respondBridgeRequest` routes to the subagent manager).
 */
export function SubagentModals() {
  const entries = useSubagents((s) => s.entries);
  const entryList = Object.values(entries);
  // The entries' request slices, aggregated with SHALLOW compare: the
  // component re-renders when one of the ENTRIES' slices changes — not on
  // any session's request churn (a whole-object selector re-renders on
  // every store change, since `addRequest` replaces the `requests`
  // object).
  const entryRequests = useBridge(
    useShallow((s) => {
      // `| undefined` is honest: a session with no (yet) requests maps to
      // `undefined`, and the consumers guard with `?? []`.
      const out: Record<string, BridgeRequestData[] | undefined> = {};
      for (const e of entryList) {
        out[e.sessionId] = s.requests[e.sessionId];
      }
      return out;
    }),
  );
  const confirmRefs: Array<{ sessionId: string; requestId: string }> = [];
  const passwordRefs: Array<{ sessionId: string; requestId: string }> = [];
  for (const e of entryList) {
    for (const r of entryRequests[e.sessionId] ?? []) {
      if (r.method === "confirm") {
        confirmRefs.push({ sessionId: e.sessionId, requestId: r.requestId });
      } else if (r.method === "password") {
        passwordRefs.push({ sessionId: e.sessionId, requestId: r.requestId });
      }
    }
  }

  return (
    <>
      {confirmRefs.map((r) => (
        <SudoConfirmModal
          key={r.requestId}
          sessionId={r.sessionId}
          requestId={r.requestId}
        />
      ))}
      {passwordRefs.map((r) => (
        <SudoPasswordModal
          key={r.requestId}
          sessionId={r.sessionId}
          requestId={r.requestId}
        />
      ))}
    </>
  );
}
