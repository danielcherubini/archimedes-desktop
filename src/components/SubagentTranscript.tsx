import { useSubagents } from "../store/subagents";
import { useBridge } from "../store/bridge";
import { usePermissions } from "../store/permissions";
import { useSessions } from "../store/sessions";
import { groupConsecutiveFileWrites } from "../lib/toolGroups";
import { summarizeSubagentFor } from "../lib/toolOutput";
import { Reasoning, ReasoningTrigger, ReasoningContent } from "./Reasoning";
import MessageBubble from "./MessageBubble";
import ChangesGroupCard from "./ChangesGroupCard";
import ToolCallCard from "./ToolCallCard";
import DiffBlock from "./DiffBlock";
import PermissionPrompt from "./PermissionPrompt";
import AskQuestionCard from "./AskQuestionCard";

/**
 * Shared empty fallback (a STABLE reference — a `?? []` at the call site
 * would mint a fresh array per render; the per-key selectors below return
 * the store's slice reference or `undefined`, both stable).
 */
const EMPTY: never[] = [];

/**
 * One subagent session's READ-ONLY transcript (expanded INLINE in the
 * directory row — the ZCode `SubagentSessionSidePane` content, minus the
 * separate tab; the row itself is the header — name + status + time):
 *
 * - The entry is read FROM THE STORE by `sessionId` (NOT a prop): the
 *   transcript must stay fresh as the entry's status flips to terminal
 *   (`markClosed` — the `isStreaming` derivation and the metrics line all
 *   follow the store, not a stale snapshot). `null` when the entry is
 *   absent (dismissed/evicted — the row is gone anyway; belt-and-braces
 *   for the same render).
 * - The stream: the SAME `groupConsecutiveFileWrites` pipeline as the
 *   main chat (a maximal run of consecutive `write`/`edit` tool calls
 *   folds into a single `Changes` card; `MessageBubble` for
 *   `agent-text`, `ToolCallCard` for `tool-call` (collapsed by default),
 *   `DiffBlock` for `diff`, `Reasoning` for thinking). The stream is
 *   ALWAYS visible (the expansion IS the detail view — no auto-collapse
 *   on completion); the enclosing row caps it at `max-h-48` (the content
 *   scrolls). Reasoning keeps the main-chat treatment (streaming =
 *   expanded, completed = auto-collapsed, user-expandable).
 * - The prompt cards render HERE (the `SidePane` is the ONLY render site
 *   for subagent-session requests; the transcript is ALWAYS MOUNTED —
 *   the row's `hidden` attribute hides it visually, never unmounts it).
 * - The metrics line on close: the `subagent-closed` SNAPSHOT from
 *   `useSubagents` — NEVER `useBridge.cost[sessionId]` (the
 *   `session-closed` handler's `dismissSession` deletes it, and the two
 *   events are concurrent).
 */
export function SubagentTranscript({ sessionId }: { sessionId: string }) {
  // Read FROM THE STORE (not a prop — see the component doc): the entry
  // is a stable per-session reference (the store replaces it only when
  // THIS entry changes), so the transcript re-renders on its own churn
  // only.
  const entry = useSubagents((s) => s.entries[sessionId]);
  if (!entry) return null;
  // Per-KEY selectors (NOT the whole objects): the store replaces a
  // session's slice with a new reference only when THAT session's data
  // changes, so the transcript re-renders on its own churn only.
  // `undefined` is a stable reference too — the shared `EMPTY` fallback
  // is never minted per render (a `?? []` selector defeats slice
  // equality).
  const requests = useBridge((s) => s.requests[entry.sessionId]);
  const prompts = usePermissions((s) => s.prompts[entry.sessionId]);
  const messages = useSessions((s) => s.messages[entry.sessionId]);
  // The parent's `subagent` tool-call `details` (the progress envelope) —
  // the transcript's FALLBACK when the subagent's own stream is empty
  // (the `progressSummary` derivation below).
  const parentMessages = useSessions((s) => s.messages[entry.parentSessionId]);
  const requestList = requests ?? EMPTY;
  const promptList = prompts ?? EMPTY;
  const messageList = messages ?? EMPTY;
  // The stream is grouped ONCE per render (not inside the map).
  const units = groupConsecutiveFileWrites(messageList);
  const askRequests = requestList.filter((r) => r.method === "ask");
  // The progress fallback (the subagent's activity from the parent's
  // `subagent` tool-call `details` — rendered only while the subagent's
  // OWN stream is empty, so it never duplicates the real transcript):
  // the first `subagent` tool call's `rawOutput.details`, filtered to
  // THIS subagent (by `childSessionId` / `task`).
  const progressRawOutput =
    messageList.length === 0 && parentMessages
      ? (parentMessages.find(
          (m) => m.kind === "tool-call" && m.title === "subagent",
        ) as { rawOutput?: unknown } | undefined)?.rawOutput
      : undefined;
  const progressSummary = progressRawOutput !== undefined
    ? summarizeSubagentFor(
        (progressRawOutput as { details?: unknown }).details,
        entry.sessionId,
        entry.task,
      )
    : undefined;

  return (
    <div className="flex flex-col gap-2">
      {messageList.length > 0 ? (
        <div className="space-y-1">
          {units.map((unit, i) => {
            if (unit.kind === "changes-group") {
              return <ChangesGroupCard key={i} messages={unit.messages} />;
            }
            const m = unit.message;
            if (m.kind === "agent-text") {
              return <MessageBubble key={i} message={m} />;
            }
            if (m.kind === "agent-thought") {
              const isStreaming =
                entry.status === "running" && i === units.length - 1;
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
      ) : progressSummary !== undefined ? (
        // The progress fallback (the subagent's activity from the
        // parent's `subagent` tool-call `details` — shown while the
        // subagent's own stream is empty, so the expansion is never a
        // blank box).
        <pre className="whitespace-pre-wrap font-mono text-ui-sm text-foreground-subtle">
          {progressSummary}
        </pre>
      ) : null}
      {/* The prompt cards render in the transcript (the component is
          session-id-keyed and already generic; the transcript is ALWAYS
          MOUNTED, so a request is never unrendered — an unrendered
          request would hang until the bridge timeout). */}
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
      {/* The metrics line on close (see the component doc). */}
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
    </div>
  );
}
