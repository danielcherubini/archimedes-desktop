import type { RefObject } from "react";
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
 * The transcript scroll region: the empty-state hint, the render-unit loop
 * (the `kind`→component dispatch), the stacked permission/`ask` cards, the
 * file summary card, and the stop-reason line.
 *
 * Props-only by design — it MUST NOT read the stores (every value is derived
 * in `ChatStream` and passed down). Extracted verbatim from `ChatStream`
 * (identical DOM).
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
    <div ref={scrollRef} className="flex-1 space-y-3 overflow-y-auto p-4">
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
        if (unit.kind === "changes-group") {
          return (
            <ChangesGroupCard
              key={`${activeSessionId}:${i}`}
              messages={unit.messages}
            />
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
            return (
              <AskQuestionCard
                key={`${activeSessionId}:${i}`}
                sessionId={activeSessionId}
                requestId={anchored.requestId}
              />
            );
          }
        }
        return (
          <MessageBubble
            key={`${activeSessionId}:${i}`}
            message={message}
            isStreaming={
              message.kind === "agent-thought" && inTurn && i === units.length - 1
            }
            sessionId={activeSessionId}
          />
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
