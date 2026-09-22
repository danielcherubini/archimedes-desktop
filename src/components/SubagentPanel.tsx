import { useEffect, useRef, useState } from "react";
import { useShallow } from "zustand/react/shallow";
import { useSubagents, type SubagentEntry } from "../store/subagents";
import { useBridge, type BridgeRequestData } from "../store/bridge";
import { usePermissions } from "../store/permissions";
import { useSessions } from "../store/sessions";
import MessageBubble from "./MessageBubble";
import ToolCallCard from "./ToolCallCard";
import DiffBlock from "./DiffBlock";
import PermissionPrompt from "./PermissionPrompt";
import AskQuestionCard from "./AskQuestionCard";
import SudoConfirmModal from "./SudoConfirmModal";
import SudoPasswordModal from "./SudoPasswordModal";

const STATE_CHIP_STYLES: Record<"working" | "idle" | "blocked", string> = {
  working: "bg-emerald-500/20 text-emerald-300",
  idle: "bg-neutral-500/20 text-neutral-300",
  blocked: "bg-amber-500/20 text-amber-300",
};

/**
 * Shared empty fallback (a STABLE reference — a `?? []` at the call site
 * would mint a fresh array per render; the per-key selectors below return
 * the store's slice reference or `undefined`, both stable).
 */
const EMPTY: never[] = [];

const STATUS_CHIP_STYLES: Record<SubagentEntry["status"], string> = {
  running: "bg-sky-500/20 text-sky-300",
  completed: "bg-green-500/20 text-green-300",
  failed: "bg-red-500/20 text-red-300",
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
    <section className="rounded-md border border-neutral-800 bg-neutral-900/40 p-2">
      {/* Header: the agent's name (the label — the subagent's own `ask`
          cards render in its stream with `source: "main"` from its own
          session id), the `state` push chip (finally rendered), the
          subagent status, a pending-permission badge, and a dismiss. */}
      <div className="flex items-center gap-1.5">
        <p className="min-w-0 flex-1 truncate text-xs font-medium">
          {entry.agentName}
        </p>
        {agentState && (
          <span
            className={`rounded px-1 py-0.5 text-[10px] ${STATE_CHIP_STYLES[agentState]}`}
          >
            {agentState}
          </span>
        )}
        <span
          className={`rounded px-1 py-0.5 text-[10px] ${STATUS_CHIP_STYLES[entry.status]}`}
        >
          {entry.status}
        </span>
        {promptList.length > 0 && (
          <span className="rounded bg-amber-600/40 px-1 py-0.5 text-[10px] text-amber-200">
            {promptList.length} awaiting
          </span>
        )}
        <button
          type="button"
          onClick={() => dismiss(entry.sessionId)}
          aria-label={`Dismiss ${entry.agentName}`}
          className="text-xs text-neutral-500 hover:text-neutral-300"
        >
          ✕
        </button>
      </div>
      {/* The compact stream: the same pipeline, condensed (smaller
          font/spacing — NOT a new message renderer). `MessageBubble` for
          `agent-text`, `ToolCallCard` for `tool-call` (collapsed by
          default), `DiffBlock` for `diff`. */}
      {messageList.length > 0 && (
        <div className="mt-2 max-h-48 space-y-1 overflow-y-auto rounded bg-neutral-950 p-2 text-xs">
          {messageList.map((m, i) => {
            if (m.kind === "agent-text") {
              return <MessageBubble key={i} message={m} />;
            }
            if (m.kind === "tool-call") {
              return (
                <ToolCallCard
                  key={i}
                  title={m.title}
                  status={m.status}
                  diff={m.diff}
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
        <p className="mt-1 text-[10px] text-neutral-500">
          {entry.status === "failed" && entry.error && (
            <span className="text-red-400">{entry.error}</span>
          )}
          {entry.metrics && (
            <span>
              {entry.metrics.inputTokens} in / {entry.metrics.outputTokens} out / $
              {entry.metrics.cost.toFixed(2)} / {entry.metrics.durationMs} ms
            </span>
          )}
        </p>
      )}
    </section>
  );
}

/**
 * Collapsible right rail: one section per subagent session (arrival order).
 *
 * - No entries → render nothing (the rail collapses).
 * - Collapsed = a slim bar with the count badge; expanded = the sections.
 * - Auto-expands on dispatch (0 → >0 entries) and on a pending permission
 *   prompt (badge 0 → >0).
 * - The `confirm`/`password` `SudoConfirmModal`/`SudoPasswordModal` are
 *   rendered HERE (outside the collapsed content — a collapsed rail never
 *   hides a pending modal): `ChatStream` renders them ONLY for the ACTIVE
 *   session's requests, and the subagent's session id is never active.
 *   They are `fixed` overlays, so rendering them from the panel is fine;
 *   the answer path is unchanged (`respondBridgeRequest` routes to the
 *   subagent manager).
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
      const out: Record<string, BridgeRequestData[]> = {};
      for (const e of entryList) {
        out[e.sessionId] = s.requests[e.sessionId];
      }
      return out;
    }),
  );
  // The entries' pending-prompt count (a PRIMITIVE — no aggregate object
  // to shallow-compare; `undefined` slices count as 0).
  const promptCount = usePermissions((s) => {
    let n = 0;
    for (const e of entryList) {
      n += s.prompts[e.sessionId]?.length ?? 0;
    }
    return n;
  });
  const [expanded, setExpanded] = useState(false);

  // Auto-expand on dispatch (0 → >0 entries).
  const prevEntryCount = useRef(entryList.length);
  useEffect(() => {
    if (prevEntryCount.current === 0 && entryList.length > 0) {
      setExpanded(true);
    }
    prevEntryCount.current = entryList.length;
  }, [entryList.length]);

  // Auto-expand on a pending permission prompt (badge 0 → >0).
  const prevPromptCount = useRef(promptCount);
  useEffect(() => {
    if (prevPromptCount.current === 0 && promptCount > 0) {
      setExpanded(true);
    }
    prevPromptCount.current = promptCount;
  }, [promptCount]);

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

  if (entryList.length === 0) return null;

  return (
    <>
      <aside
        aria-label="Subagents"
        className="w-64 shrink-0 overflow-y-auto border-l border-neutral-800 bg-neutral-950 p-3"
      >
        <div className="flex items-center justify-between">
          <p className="text-xs font-medium text-neutral-400">
            Subagents
            <span className="ml-1 rounded bg-neutral-800 px-1.5 py-0.5 text-neutral-300">
              {entryList.length}
            </span>
          </p>
          <button
            type="button"
            onClick={() => setExpanded((v) => !v)}
            aria-label={expanded ? "Collapse subagents" : "Expand subagents"}
            className="text-xs text-neutral-400 hover:text-neutral-200"
          >
            {expanded ? "▾" : "▸"}
          </button>
        </div>
        {expanded && (
          <div className="mt-2 space-y-2">
            {entryList.map((e) => (
              <SubagentSection key={e.sessionId} entry={e} />
            ))}
          </div>
        )}
      </aside>
      {/* Bridge modals for the entries (rendered at the panel root —
          `fixed` overlays, NOT inside the collapsed content). */}
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
