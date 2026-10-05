import type { ReactNode, RefObject } from "react";
import MessageBubble from "../MessageBubble";
import ChangesGroupCard from "../ChangesGroupCard";
import PermissionPrompt from "../PermissionPrompt";
import AskQuestionCard from "../AskQuestionCard";
import FileSummaryCard from "../FileSummaryCard";
import type { RenderUnit } from "../../lib/toolGroups";
import type { PermissionPromptData } from "../../store/permissions";
import type { InteractiveRequestData } from "../../store/interactive";
import type { StopReason } from "../../lib/tauri";

/**
 * The transcript's VERTICAL RHYTHM (ZCode's model: rows carry no padding and
 * the CONTAINER owns the spacing — its work-item block is a `gap-4` flex
 * column and the turn's blocks ride a `gap-5`, never per-row `py-*`).
 *
 * Base gap (`gap-3`, 12px) on the scroll container: the prose ↔ `Thought`
 * rhythm, unchanged. `TRANSCRIPT_BREATHING_GAP` is then added to a row whose
 * IMMEDIATE NEIGHBOUR is a tool row — so a tool call gets 12+8 = 20px of air
 * on each side (ZCode's 20px text↔work rhythm) while thinking and chat stay
 * as close together as they were. It is a property of the PAIR (applied to
 * whichever side is second), which is what keeps a run of tool calls at a
 * steady 20px instead of alternating 20/28.
 */
const TRANSCRIPT_BREATHING_GAP = "mt-2";

/** A unit that reads as a TOOL ROW (a tool call, or a folded `Changes` group). */
function isToolUnit(unit: RenderUnit): boolean {
  return unit.kind === "changes-group" || unit.message.kind === "tool-call";
}

/**
 * The transcript scroll region: the empty-state hint, the render-unit loop
 * (the `kind`→component dispatch, each unit wrapped in the rhythm row above),
 * the stacked permission/`ask` cards, the file summary card, and the
 * stop-reason line.
 *
 * Props-only by design — it MUST NOT read the stores (every value is derived
 * in `ChatStream` and passed down).
 */
export default function MessageList({
  scrollRef,
  units,
  messageCount,
  activeSessionId,
  inTurn,
  askRequests,
  prompts,
  stackedAskRequests,
  hasPendingRequest,
  workingOrInTurn,
  agentState,
  turnDiffs,
  stopReason,
}: {
  scrollRef: RefObject<HTMLDivElement | null>;
  units: RenderUnit[];
  messageCount: number;
  activeSessionId: string;
  inTurn: boolean;
  askRequests: InteractiveRequestData[];
  prompts: PermissionPromptData[];
  stackedAskRequests: InteractiveRequestData[];
  hasPendingRequest: boolean;
  workingOrInTurn: boolean;
  agentState: "working" | "idle" | "blocked" | undefined;
  turnDiffs: { path: string; patch: string }[];
  stopReason: StopReason | undefined;
}) {
  return (
    <div
      ref={scrollRef}
      data-testid="transcript-scroll"
      className="flex-1 flex flex-col gap-3 overflow-y-auto p-4"
    >
      {/* The request cards and the working/blocked/stop-reason lines
          render UNCONDITIONALLY (regardless of the transcript's
          length — a native agent that asks at session start, or the
          window between `openSession` and `loadHistory` hydration,
          must not have its request swallowed by the hint). */}
      {messageCount === 0 &&
        !hasPendingRequest &&
        !workingOrInTurn &&
        agentState !== "blocked" && (
          <div className="flex h-full items-center justify-center">
            <p className="text-ui-base text-foreground-subtlest">
              Send a prompt to start
            </p>
          </div>
        )}
      {units.map((unit, i) => {
        // The PAIR's breathing room (see `TRANSCRIPT_BREATHING_GAP`): this row
        // gets it when the row ABOVE it is a tool row or this row IS one, and
        // it is not the first row (the container's `p-4` is its air). Applied
        // to the wrapper — never to the row itself, which stays padding-free.
        const breathes =
          i > 0 && (isToolUnit(units[i - 1]!) || isToolUnit(unit));
        const wrap = (child: ReactNode) => (
          <div
            key={`${activeSessionId}:${i}`}
            className={
              breathes
                ? `flex flex-col ${TRANSCRIPT_BREATHING_GAP}`
                : "flex flex-col"
            }
          >
            {child}
          </div>
        );
        if (unit.kind === "changes-group") {
          return wrap(
            <ChangesGroupCard messages={unit.messages} />,
          );
        }
        const message = unit.message;
        // The key is session-scoped: switching sessions must not reuse the
        // previous session's component at the same index (a `Reasoning`
        // would otherwise carry over expanded state, duration, and timers).
        // The `AskQuestionCard` replaces the pending `ask`
        // `ToolCallCard` (correlated via `(source, toolCallId)`/
        // `requestId` — a `main` ask whose `toolCallId` matches this
        // tool-call message).
        if (message.kind === "tool-call") {
          const anchored = askRequests.find(
            (r) => r.source === "main" && r.toolCallId === message.id,
          );
          if (anchored) {
            return wrap(
              <AskQuestionCard
                sessionId={activeSessionId}
                requestId={anchored.requestId}
              />,
            );
          }
        }
        return wrap(
          <MessageBubble
            message={message}
            isStreaming={
              message.kind === "agent-thought" && inTurn && i === units.length - 1
            }
            sessionId={activeSessionId}
          />,
        );
      })}
      {prompts.map((prompt) => (
        <PermissionPrompt
          key={prompt.requestId}
          sessionId={activeSessionId}
          requestId={prompt.requestId}
        />
      ))}
      {/* Stacked interactive `ask` cards (one per pending request, arrival
          order — concurrent asks stack vertically in the stream). */}
      {stackedAskRequests.map((r) => (
        <AskQuestionCard
          key={r.requestId}
          sessionId={activeSessionId}
          requestId={r.requestId}
        />
      ))}
      {turnDiffs.length > 0 && <FileSummaryCard diffs={turnDiffs} />}
      {!inTurn && stopReason && stopReason !== "end_turn" && (
        <p className="text-ui-sm text-foreground-subtlest">
          Turn ended: {stopReason}
        </p>
      )}
    </div>
  );
}
