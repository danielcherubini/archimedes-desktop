import { X } from "lucide-react";
import { useShallow } from "zustand/react/shallow";
import { useSubagents, type SubagentEntry } from "../store/subagents";
import { useBridge, type BridgeRequestData } from "../store/bridge";
import { usePermissions } from "../store/permissions";
import { useSessions } from "../store/sessions";
import { Reasoning, ReasoningTrigger, ReasoningContent } from "./Reasoning";
import MessageBubble from "./MessageBubble";
import ToolCallCard from "./ToolCallCard";
import DiffBlock from "./DiffBlock";
import PermissionPrompt from "./PermissionPrompt";
import AskQuestionCard from "./AskQuestionCard";
import SudoConfirmModal from "./SudoConfirmModal";
import SudoPasswordModal from "./SudoPasswordModal";

const STATE_CHIP_STYLES: Record<"working" | "idle" | "blocked", string> = {
  working: "bg-warning/14 text-warning",
  idle: "text-foreground-subtlest",
  blocked: "text-foreground-subtle",
};

/**
 * Shared empty fallback (a STABLE reference — a `?? []` at the call site
 * would mint a fresh array per render; the per-key selectors below return
 * the store's slice reference or `undefined`, both stable).
 */
const EMPTY: never[] = [];

const STATUS_CHIP_STYLES: Record<SubagentEntry["status"], string> = {
  running: "text-warning",
  completed: "text-success",
  failed: "text-destructive",
};

/**
 * One section of the panel (a compact view of the subagent's SAME ACP
 * stream — `session-update` events for the subagent's session id flow
 * through the existing `useSessions.applySessionUpdate`, so the messages
 * accumulate in `useSessions.messages[sessionId]` with zero new plumbing).
 */
function SubagentSection({ entry }: { entry: SubagentEntry }) {
  // Per-KEY selectors (NOT the whole objects): the store replaces a
  // session's slice with a new reference only when THAT session's data
  // changes, so a section re-renders on its own churn only. `undefined`
  // is a stable reference too — the shared `EMPTY` fallback is never
  // minted per render (a `?? []` selector defeats slice equality).
  const agentState = useBridge((s) => s.agentState[entry.sessionId]);
  const requests = useBridge((s) => s.requests[entry.sessionId]);
  const prompts = usePermissions((s) => s.prompts[entry.sessionId]);
  const messages = useSessions((s) => s.messages[entry.sessionId]);
  const requestList = requests ?? EMPTY;
  const promptList = prompts ?? EMPTY;
  const messageList = messages ?? EMPTY;
  const dismiss = useSubagents((s) => s.dismiss);
  const askRequests = requestList.filter((r) => r.method === "ask");

  return (
    <section className="flex flex-col gap-2 rounded-xl border border-card-border bg-card p-3">
      {/* Header: the agent's name (the label — the subagent's own `ask`
          cards render in its stream with `source: "main"` from its own
          session id), the `state` push chip (finally rendered), the
          subagent status, a pending-permission badge, and a dismiss. */}
      <div className="flex items-center gap-1.5">
        <p className="min-w-0 flex-1 truncate text-ui-base font-medium">
          {entry.agentName}
        </p>
        {agentState && (
          <span
            className={`rounded px-1 py-0.5 text-ui-xs ${STATE_CHIP_STYLES[agentState]}`}
          >
            {agentState}
          </span>
        )}
        <span
          className={`rounded-full px-2 py-0.5 text-ui-xs font-medium ${STATUS_CHIP_STYLES[entry.status]}`}
        >
          {entry.status}
        </span>
        {promptList.length > 0 && (
          <span className="rounded-full bg-interaction-confirmation-surface px-2 py-0.5 text-ui-xs font-medium text-interaction-confirmation-foreground">
            {promptList.length} awaiting
          </span>
        )}
        <button
          type="button"
          onClick={() => dismiss(entry.sessionId)}
          aria-label={`Dismiss ${entry.agentName}`}
          className="flex size-6 items-center justify-center text-foreground-subtlest hover:text-foreground"
        >
          <X className="size-4" />
        </button>
      </div>
      {/* The compact stream: the same pipeline, condensed (smaller
          font/spacing — NOT a new message renderer). `MessageBubble` for
          `agent-text`, `ToolCallCard` for `tool-call` (collapsed by
          default), `DiffBlock` for `diff`, `Reasoning` for thinking. */}
      {messageList.length > 0 && (
        <div className="mt-1 max-h-48 space-y-1 overflow-y-auto rounded-md bg-surface p-2">
          {messageList.map((m, i) => {
            if (m.kind === "agent-text") {
              return <MessageBubble key={i} message={m} />;
            }
            if (m.kind === "agent-thought") {
              const isStreaming = entry.status === "running" && i === messageList.length - 1;
              return (
                <Reasoning
                  key={i}
                  isStreaming={isStreaming}
                  autoCollapseKey={isStreaming ? null : "complete"}
                >
                  <ReasoningTrigger streamingText={m.text} />
                  <ReasoningContent>{m.text}</ReasoningContent>
                </Reasoning>
              );
            }
            if (m.kind === "tool-call") {
              return (
                <ToolCallCard
                  key={i}
                  title={m.title}
                  status={m.status}
                  diff={m.diff}
                  rawInput={m.rawInput}
                  rawOutput={m.rawOutput}
                />
              );
            }
            if (m.kind === "diff") {
              return <DiffBlock key={i} path={m.path} patch={m.patch} />;
            }
            return null;
          })}
        </div>
      )}
      {/* The prompt itself renders in the section's stream (the component
          is session-id-keyed and already generic). */}
      {promptList.map((p) => (
        <PermissionPrompt
          key={p.requestId}
          sessionId={entry.sessionId}
          requestId={p.requestId}
        />
      ))}
      {askRequests.map((r) => (
        <AskQuestionCard
          key={r.requestId}
          sessionId={entry.sessionId}
          requestId={r.requestId}
        />
      ))}
      {/* The metrics line on close: the `subagent-closed` SNAPSHOT from
          `useSubagents` — NEVER `useBridge.cost[sessionId]` (the
          `session-closed` handler's `dismissSession` deletes it, and the
          two events are concurrent). */}
      {entry.status !== "running" && (entry.metrics || entry.error) && (
        <p className="text-ui-xs text-foreground-subtlest">
          {entry.status === "failed" && entry.error && (
            <span className="text-destructive">{entry.error}</span>
          )}
          {entry.metrics && (
            <span>
              {entry.metrics.inputTokens} in · {entry.metrics.outputTokens} out · $
              {entry.metrics.cost.toFixed(2)} · {entry.metrics.durationMs} ms
            </span>
          )}
        </p>
      )}
    </section>
  );
}

/**
 * One card stack (the `SidePane`'s Subagents tab): one card per subagent
 * session (arrival order).
 *
 * - No entries → an empty state ("No subagent sessions"). Collapse is the
 *   `SidePane` frame's job now (via `sidePaneState`) — the old rail
 *   collapse UI and auto-expand behavior are dropped (the tab trigger's
 *   count badge covers discoverability).
 * - The `confirm`/`password` `SudoConfirmModal`/`SudoPasswordModal` are
 *   rendered HERE (outside the scrollable content — a collapsed pane never
 *   hides a pending modal): `ChatStream` renders them ONLY for the ACTIVE
 *   session's requests, and the subagent's session id is never active —
 *   this panel is the ONLY render site for subagent-session requests
 *   (an unrendered request would hang until the bridge timeout). They are
 *   `fixed` overlays, so rendering them from the panel is fine; the answer
 *   path is unchanged (`respondBridgeRequest` routes to the subagent
 *   manager).
 */
export default function SubagentPanel() {
  // The entries (the whole object — one reference per subagents change).
  const entries = useSubagents((s) => s.entries);
  const entryList = Object.values(entries);
  // The entries' request slices, aggregated with SHALLOW compare: the
  // panel re-renders when one of the ENTRIES' slices changes — not on any
  // session's request churn (a whole-object selector re-renders on every
  // store change, since `addRequest` replaces the `requests` object).
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
  // The entries' pending `confirm`/`password` requests (the modals render
  // at the panel level — see the component doc above).
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

  if (entryList.length === 0) {
    return (
      <p className="text-center text-ui-sm text-foreground-subtlest">
        No subagent sessions
      </p>
    );
  }

  return (
    <>
      <div className="flex flex-col gap-2">
        {entryList.map((e) => (
          <SubagentSection key={e.sessionId} entry={e} />
        ))}
      </div>
      {/* Bridge modals for the entries (rendered at the panel root —
          `fixed` overlays, NOT inside the scrollable content). */}
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
