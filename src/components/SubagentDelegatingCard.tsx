import { useState } from "react";
import { useSubagents } from "../store/subagents";
import { useSubagentSelection } from "../store/subagentSelection";
import { subagentActivityFor } from "../lib/toolOutput";
import { StatusIcon, STATUS_CHIP_STYLES } from "./SubagentStatusIcon";
import { ToolCallCardHeader } from "./ToolCallCardHeader";
import type { ToolCallUiStatus } from "../store/sessions";

/**
 * The `subagent` tool's card (the "Delegating" card): the standard
 * `ToolCallCardHeader` + the session's subagents nested under it (agent
 * name + task + status + a one-line live activity preview). Clicking a
 * row opens the subagent's dedicated transcript modal (the
 * `useSubagentSelection` store — read by the `SubagentDetailHost`).
 *
 * Session-level grouping (documented behavior, not a bug): the rows are
 * filtered by `parentSessionId === sessionId` (the parent's ACP id). In
 * a session with MULTIPLE `subagent` tool calls, EVERY card lists ALL
 * of that session's subagent rows (the `subagent-session-started`
 * payload carries no `toolCallId`, so per-card correlation is
 * impossible with today's payloads). The activity `preview` resolves
 * ONLY from each card's OWN `rawOutput.details` (a sibling card's row
 * gets `preview === undefined` and renders no activity line).
 */
export default function SubagentDelegatingCard({
  title,
  status,
  rawInput,
  rawOutput,
  sessionId,
}: {
  title: string;
  status: ToolCallUiStatus;
  rawInput?: unknown;
  rawOutput?: unknown;
  sessionId: string;
}) {
  const entries = useSubagents((s) => s.entries);
  const childEntries = Object.values(entries).filter(
    (e) => e.parentSessionId === sessionId,
  );
  const select = useSubagentSelection((s) => s.select);

  // The card is OPEN by default (the subagents ARE the "delegating"
  // detail the user wants to see — a deliberate deviation from the
  // `ToolCallCard`'s collapsed-by-default). A plain toggle: the user
  // closes it via the chevron and the close sticks (no auto-open).
  const [open, setOpen] = useState(true);

  return (
    <div className="w-full">
      <ToolCallCardHeader
        title={title}
        status={status}
        rawInput={rawInput}
        rawOutput={rawOutput}
        open={open}
        onToggle={() => setOpen((o) => !o)}
        files={[]}
        stat={undefined}
        range={undefined}
        isShell={false}
        command={undefined}
      />
      {open && childEntries.length > 0 && (
        <div className="mt-1 rounded-xl border border-border bg-panel px-4 py-3">
          {childEntries.map((entry) => {
            const preview = subagentActivityFor(
              (rawOutput as { details?: unknown } | undefined)?.details,
              entry.sessionId,
              entry.task,
            );
            return (
              <button
                key={entry.sessionId}
                type="button"
                onClick={() => select(entry.sessionId)}
                aria-label={`Open ${entry.agentName} transcript`}
                className="flex w-full items-start gap-2 rounded-lg px-2 py-1.5 text-left hover:bg-surface-hover focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
              >
                <span className="mt-0.5">
                  <StatusIcon status={entry.status} />
                </span>
                <div className="min-w-0 flex-1">
                  <div className="flex items-center gap-2">
                    <span className="min-w-0 truncate text-ui-base font-medium">
                      {entry.agentName}
                    </span>
                    <span
                      className={`shrink-0 text-ui-xs ${STATUS_CHIP_STYLES[entry.status]}`}
                    >
                      {entry.status}
                    </span>
                  </div>
                  {entry.task && (
                    <p className="truncate text-ui-sm text-foreground-subtle">
                      {entry.task}
                    </p>
                  )}
                  {preview && (
                    <p className="truncate text-ui-xs text-foreground-subtlest">
                      {preview}
                    </p>
                  )}
                </div>
              </button>
            );
          })}
        </div>
      )}
      {open && childEntries.length === 0 && (
        <p className="font-mono text-ui-base text-foreground-subtle">
          No subagents yet.
        </p>
      )}
    </div>
  );
}
