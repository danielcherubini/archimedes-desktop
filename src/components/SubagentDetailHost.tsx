import type { KeyboardEvent } from "react";
import { X } from "lucide-react";
import { useSubagents } from "../store/subagents";
import { useSubagentSelection } from "../store/subagentSelection";
import { SubagentTranscript } from "./SubagentTranscript";
import { StatusIcon, STATUS_CHIP_STYLES } from "./SubagentStatusIcon";

/**
 * The dedicated subagent transcript view — the single ALWAYS-MOUNTED
 * component rendered at the `App` root. It replaces the "always mounted"
 * role `SubagentDirectory` played (Task 6 removes that role): every
 * subagent's `SubagentTranscript` is rendered EXACTLY ONCE —
 *
 * - the SELECTED subagent (from `useSubagentSelection.selectedSessionId`):
 *   in the visible modal (the `fixed` right-side sheet);
 * - every NON-selected subagent: in a HIDDEN host (the `hidden` attribute
 *   = CSS `display: none` — the component stays MOUNTED, so the prompt
 *   cards are always rendered and never hang until the bridge timeout).
 *
 * The modal's OUTER `div` is ALWAYS mounted (the `hidden` class toggles
 * `display: none`); the INNER content is conditionally mounted. When the
 * user closes the modal (`select(null)`), the selected entry's
 * `SubagentTranscript` moves from the modal (unmounted) to the hidden host
 * (mounted) in the SAME render (it is no longer `selected`, so the hidden
 * host now includes it) — the prompt cards stay rendered (no gap).
 *
 * KNOWN BEHAVIOR (accepted, not a bug): the modal's and the hidden host's
 * `SubagentTranscript` are DIFFERENT component instances, so any LOCAL
 * state in the transcript's children (e.g. text typed into an
 * `AskQuestionCard` reply) RESETS when the modal opens or closes (the
 * instance is remounted on the swap). The PROMPT CARDS themselves stay
 * rendered (the invariant holds) — only their local UI state resets. Do
 * NOT "fix" this by lifting the instance (that would break the
 * exactly-once rendering).
 *
 * Esc closes the modal via a React `onKeyDown` on the SHEET (NOT a
 * window-level handler). `ChatStream` registers a WINDOW-level `keydown`
 * listener (active while `inTurn`) that cancels the turn on Escape. A
 * window-level CAPTURE handler would run first and (a) close the modal on
 * EVERY Escape (even ones consumed by a nested consumer) and (b)
 * `stopPropagation()` the event so a nested consumer's own Esc handler
 * never runs — a REGRESSION for `AskQuestionCard`'s documented
 * Esc-dismiss and the stacked sudo modals. The sheet's `onKeyDown`
 * avoids both: its `stopPropagation()` (which React forwards to
 * `nativeEvent.stopPropagation()`) stops the native event at the React
 * root BEFORE it reaches `window` (so `ChatStream`'s listener does NOT
 * fire — the turn is NOT cancelled), AND because the sheet handler is a
 * PARENT React handler, a nested consumer's own `stopPropagation()` (e.g.
 * `AskQuestionCard`'s Esc-dismiss) stops the event before it reaches the
 * sheet (so the modal does NOT close when a nested consumer consumes the
 * Esc). The sudo modals are `fixed` overlays at the `SidePane` root —
 * NOT in the sheet's subtree — so their Esc handlers run independently.
 *
 * KNOWN LIMITATION (accepted): Esc only closes the modal when focus is
 * INSIDE the sheet (the user clicked a row to open it, so focus is in it);
 * an Esc with focus OUTSIDE the sheet does not close it (the user uses
 * the `X` or re-clicks a row).
 */
export default function SubagentDetailHost() {
  const entries = useSubagents((s) => s.entries);
  const entryList = Object.values(entries);
  const selectedId = useSubagentSelection((s) => s.selectedSessionId);
  const select = useSubagentSelection((s) => s.select);
  // The selected entry (or `undefined` — the modal is hidden).
  const selected = entryList.find((e) => e.sessionId === selectedId);
  // Esc closes the modal (see the component doc for WHY this is a React
  // `onKeyDown` on the sheet and NOT a window-level handler).
  const handleEsc = (e: KeyboardEvent) => {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      select(null);
    }
  };

  return (
    <>
      {/* The hidden host: a HIDDEN `SubagentTranscript` for every
          NON-selected entry (the `hidden` attribute = `display: none` —
          the component stays MOUNTED, so the prompt cards are always
          rendered — the "always mounted" invariant). */}
      {entryList
        .filter((e) => e.sessionId !== selectedId)
        .map((e) => (
          <div key={e.sessionId} hidden>
            <SubagentTranscript sessionId={e.sessionId} />
          </div>
        ))}
      {/* The modal: a `fixed` right-side sheet, ALWAYS mounted (the
          `hidden` class toggles `display: none`), hidden when `selected`
          is `undefined`. */}
      <div
        role="dialog"
        aria-label={`${selected?.agentName ?? ""} — transcript`}
        onKeyDown={handleEsc}
        className={
          "fixed top-0 right-0 z-50 flex h-full w-[480px] max-w-[90vw] flex-col border-l border-border bg-background shadow-xl " +
          (selected ? "" : "hidden")
        }
      >
        {selected && (
          <>
            <div className="flex items-center justify-between gap-2 border-b border-border/50 p-3">
              <div className="flex min-w-0 items-center gap-2">
                <StatusIcon status={selected.status} />
                <span className="min-w-0 truncate text-ui-base font-medium">
                  {selected.agentName}
                </span>
                <span
                  className={`shrink-0 text-ui-xs ${STATUS_CHIP_STYLES[selected.status]}`}
                >
                  {selected.status}
                </span>
              </div>
              <button
                type="button"
                onClick={() => select(null)}
                aria-label="Close subagent transcript"
                className="flex size-6 shrink-0 items-center justify-center text-foreground-subtle hover:text-foreground"
              >
                <X className="size-4" />
              </button>
            </div>
            <div className="flex-1 overflow-y-auto p-3">
              <SubagentTranscript sessionId={selected.sessionId} />
            </div>
          </>
        )}
      </div>
    </>
  );
}
