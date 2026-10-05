import { BrailleLoader } from "../ui/braille-loader";
import SudoConfirmModal from "../SudoConfirmModal";
import SudoPasswordModal from "../SudoPasswordModal";
import type { InteractiveRequestData } from "../../store/interactive";
import type { BrailleLoaderVariant } from "../../lib/braille-loader";

/**
 * The working indicator pinned above the composer (the braille spinner +
 * the rotating quip, or the waiting line while `blocked`).
 *
 * Its own component because it sits BETWEEN the error line and the composer
 * (a different tree position from the `fixed` modals), so the two pieces of
 * "composer chrome" are two exports of this one file. Props-only: `quip`
 * comes from `useSpinQuip`, which stays in `ChatStream` (it is called
 * unconditionally alongside the other hooks). Extracted verbatim (identical
 * DOM).
 */
export function WorkingIndicator({
  agentState,
  workingOrInTurn,
  spinnerStyle,
  quip,
}: {
  agentState: "working" | "idle" | "blocked" | undefined;
  workingOrInTurn: boolean;
  spinnerStyle: BrailleLoaderVariant;
  quip: string;
}) {
  // The working indicator is PINNED above the composer (the top of
  // the text input field — the reference TUI's editor bottom-border
  // row: the spinner + the rotating quip). It appears while the
  // agent works or waits for input; the message stream scrolls above
  // it. `blocked` takes precedence over `working` (a blocked agent
  // that is also mid-turn shows the waiting line, NOT the braille
  // loader — both would render otherwise). The `-mb-2` pulls the row
  // a few px down so it hugs the composer (the composer's `m-3` top
  // margin minus 8px).
  return (
    <>
      {agentState === "blocked" || workingOrInTurn ? (
        <div
          className="-mb-2 flex items-center gap-2 px-4 py-1.5"
          data-testid="working-indicator"
        >
          {agentState === "blocked" ? (
            <p className="text-ui-sm text-foreground-subtle">
              Waiting for your input…
            </p>
          ) : (
            <>
              <BrailleLoader
                variant={spinnerStyle}
                speed="normal"
                fontSize={14}
                label="Agent working"
              />
              <p className="text-ui-sm text-foreground-subtle">{quip}</p>
            </>
          )}
        </div>
      ) : null}
    </>
  );
}

/**
 * The interactive modals (the `confirm` / `password` overlays). Props-only;
 * the request lists are derived in `ChatStream` from the interactive store.
 * Extracted verbatim (identical DOM — `fixed` overlays rendered at the
 * `ChatStream` root, NOT inside the scroll region).
 */
export default function ComposerModals({
  activeSessionId,
  confirmRequests,
  passwordRequests,
}: {
  activeSessionId: string;
  confirmRequests: InteractiveRequestData[];
  passwordRequests: InteractiveRequestData[];
}) {
  return (
    <>
      {confirmRequests.map((r) => (
        <SudoConfirmModal
          key={r.requestId}
          sessionId={activeSessionId}
          requestId={r.requestId}
        />
      ))}
      {passwordRequests.map((r) => (
        <SudoPasswordModal
          key={r.requestId}
          sessionId={activeSessionId}
          requestId={r.requestId}
        />
      ))}
    </>
  );
}
