import { useEffect, useRef, useState } from "react";
import {
  respondBridgeRequest,
  type AskQuestionDto,
  type AskResponsePayload,
} from "../lib/tauri";
import { useBridge } from "../store/bridge";
import { useSessions } from "../store/sessions";

const OTHER_OPTION = "Other (type your own)";
const RECOMMENDED_TAG = " (Recommended)";

/**
 * Per-question draft: which option indexes are selected, whether `Other` is
 * chosen, the `Other` free text, and a note per option index (mirrors the
 * TUI picker's `noteByOptionIndex`).
 */
interface QuestionDraft {
  selected: number[];
  other: boolean;
  otherText: string;
  notes: Record<number, string>;
}

const EMPTY_DRAFT: QuestionDraft = {
  selected: [],
  other: false,
  otherText: "",
  notes: {},
};

/**
 * Build one question's result entry (mirrors the TUI's
 * `buildSingleSelectionResult` / `buildMultiSelectionResult`): a selected
 * option with a note becomes `"<label> - <note>"`; the `Other` option's
 * text becomes `customInput`.
 */
function buildResult(
  question: AskQuestionDto,
  draft: QuestionDraft,
): { selectedOptions: string[]; customInput?: string } {
  const selectedOptions: string[] = [];
  for (const index of draft.selected) {
    const label = question.options[index]?.label ?? "";
    const note = draft.notes[index]?.trim();
    selectedOptions.push(note ? `${label} - ${note}` : label);
  }
  const customInput = draft.other ? draft.otherText.trim() : "";
  if (customInput) return { selectedOptions, customInput };
  return { selectedOptions };
}

/** Whether the draft has an answer to submit (a selection or `Other` text). */
function draftHasAnswer(draft: QuestionDraft): boolean {
  return (
    draft.selected.length > 0 || (draft.other && draft.otherText.trim() !== "")
  );
}

/**
 * Inline card for a bridge `ask` request — the ask UI ported from the TUI
 * (accent separator, circular radio list — filled amber dot = selected;
 * `Other (type your own)` free-text; checkboxes for `multi`;
 * `(Recommended)` suffix; a per-option note field; footer hints; a final
 * batch review for multi-question asks).
 *
 * **Correlation** is keyed by `(source, toolCallId)` or the frame UUID
 * `requestId` — never by bare `toolCallId` (the store keys by `requestId`;
 * the `toolCallId` is carried for anchoring to the ACP `tool_call` frame
 * when present). Subagent asks (`source` starts with `subagent:`) render a
 * labeled card anchored by `source` (the desktop has no ACP `tool_call`
 * frame for child tool calls). If the request arrives before its ACP
 * `tool_call` frame, the card renders queued (bounded ~1.5 s) then
 * unanchored/labeled.
 *
 * Submit → `respondBridgeRequest` with the `AskResponsePayload` +
 * `removeRequest` (the card collapses on settle). The card takes focus on
 * mount when UNANCHORED (a standalone card — no focus steal from the
 * message stream) so the advertised Enter/Esc shortcuts work immediately.
 * Cancel → `{ cancelled: true, results: [] }` + `removeRequest`.
 */
export default function AskQuestionCard({
  sessionId,
  requestId,
}: {
  sessionId: string;
  requestId: string;
}) {
  const request = useBridge((state) =>
    (state.requests[sessionId] ?? []).find((r) => r.requestId === requestId),
  );
  const removeRequest = useBridge((state) => state.removeRequest);
  // The card takes focus on mount when unanchored (see the component doc
  // above) so the Enter/Esc `onKeyDown` — which only fires with focus
  // INSIDE the card — works immediately for a standalone card.
  const cardRef = useRef<HTMLDivElement>(null);
  // Anchoring: the ACP `tool_call` frame for this request is in the stream
  // (only a `main` ask with a `toolCallId` can anchor — the desktop has no
  // `tool_call` frame for child tool calls).
  const anchored = useSessions((state) => {
    if (!request || request.source !== "main" || !request.toolCallId) {
      return false;
    }
    const messages = state.messages[sessionId];
    if (!messages) return false;
    return messages.some(
      (m) => m.kind === "tool-call" && m.id === request.toolCallId,
    );
  });
  const isSubagent = request?.source.startsWith("subagent:") ?? false;
  // Queued: the request arrived before its ACP `tool_call` frame — render a
  // bounded (~1.5 s) placeholder, then the full card (unanchored/labeled).
  // Subagent asks never get a `tool_call` frame, so they are never queued.
  const [queued, setQueued] = useState(
    () => !isSubagent && request?.toolCallId !== undefined,
  );
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [drafts, setDrafts] = useState<Record<string, QuestionDraft>>({});

  // Queued: the request arrived before its ACP `tool_call` frame — render a
  // bounded (~1.5 s) placeholder, then the full card (unanchored/labeled).
  useEffect(() => {
    if (!queued) return;
    if (anchored) {
      // The `tool_call` frame arrived: unqueue immediately.
      setQueued(false);
      return;
    }
    const timer = setTimeout(() => setQueued(false), 1500);
    return () => clearTimeout(timer);
  }, [queued, anchored]);

  useEffect(() => {
    if (!queued) {
      // Unanchored (a standalone card): take focus so the Enter/Esc
      // shortcuts work immediately. Anchored (inline in the message
      // stream): don't steal focus — the shortcuts work once the user
      // clicks into the card.
      if (!anchored) cardRef.current?.focus();
    }
  }, [queued, anchored]);

  if (!request) return null;
  const questions =
    (request.params as { questions?: AskQuestionDto[] }).questions ?? [];
  if (questions.length === 0) return null;

  const draft = (id: string): QuestionDraft => drafts[id] ?? EMPTY_DRAFT;
  const updateDraft = (id: string, fn: (d: QuestionDraft) => QuestionDraft) =>
    setDrafts((prev) => ({ ...prev, [id]: fn(prev[id] ?? EMPTY_DRAFT) }));

  const toggleOption = (question: AskQuestionDto, index: number) =>
    updateDraft(question.id, (d) => {
      const selected = d.selected.includes(index)
        ? d.selected.filter((i) => i !== index)
        : question.multi
          ? [...d.selected, index]
          : [index];
      // Single: choosing an option clears `Other` (they are exclusive).
      // Multi: `Other` stays additive.
      const other = question.multi ? d.other : false;
      return { ...d, selected, other };
    });

  const toggleOther = (question: AskQuestionDto) =>
    updateDraft(question.id, (d) => {
      const other = !d.other;
      const selected = question.multi ? d.selected : other ? [] : d.selected;
      return { ...d, other, selected };
    });

  const respond = async (cancelled: boolean) => {
    setBusy(true);
    setError(null);
    const payload: AskResponsePayload = cancelled
      ? { cancelled: true, results: [] }
      : {
          cancelled: false,
          results: questions.map((q) => {
            const result = buildResult(q, draft(q.id));
            return result.customInput !== undefined
              ? {
                  id: q.id,
                  selectedOptions: result.selectedOptions,
                  customInput: result.customInput,
                }
              : { id: q.id, selectedOptions: result.selectedOptions };
          }),
        };
    try {
      await respondBridgeRequest(sessionId, requestId, payload);
      removeRequest(sessionId, requestId);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  // Queued: a bounded placeholder while the ACP `tool_call` frame is
  // expected (then the full card, unanchored/labeled).
  if (queued) {
    return (
      <div className="rounded-md border border-amber-600/50 bg-amber-950/40 px-3 py-2">
        <p className="text-sm text-amber-200">
          {isSubagent ? `${request.source} is asking…` : "Agent is asking…"}
        </p>
      </div>
    );
  }

  const canSubmit = questions.every((q) => draftHasAnswer(draft(q.id)));

  return (
    <div
      ref={cardRef}
      tabIndex={-1}
      className="rounded-md border border-amber-600/50 bg-amber-950/40 px-3 py-2 focus-visible:outline focus-visible:outline-amber-500/60"
      onKeyDown={(e) => {
        if (e.key === "Enter" && canSubmit && !busy) {
          e.preventDefault();
          void respond(false);
        } else if (e.key === "Escape" && !busy) {
          e.preventDefault();
          void respond(true);
        }
      }}
    >
      {/* Accent separator (the TUI's accent line). */}
      <div className="mb-2 flex items-center gap-2">
        <span className="h-px flex-1 bg-amber-500/60" aria-hidden />
        <span className="text-xs text-amber-400">
          {isSubagent ? request.source : "Agent question"}
        </span>
        <span className="h-px flex-1 bg-amber-500/60" aria-hidden />
      </div>

      <div className="space-y-4">
        {questions.map((question) => {
          const d = draft(question.id);
          return (
            <div key={question.id}>
              <p className="text-sm text-amber-200">{question.question}</p>
              {question.description && (
                <p className="mt-1 text-xs text-neutral-400">
                  {question.description}
                </p>
              )}
              <div className="mt-2 space-y-1">
                {question.options.map((option, index) => {
                  const selected = d.selected.includes(index);
                  const recommended = question.recommended === index;
                  return (
                    <div key={option.label}>
                      <button
                        type="button"
                        disabled={busy}
                        onClick={() => toggleOption(question, index)}
                        className="flex w-full items-center gap-2 rounded px-1 py-0.5 text-left text-sm hover:bg-neutral-800/60 disabled:opacity-50"
                      >
                        {question.multi ? (
                          <span aria-hidden className="text-amber-400">
                            {selected ? "☑" : "☐"}
                          </span>
                        ) : (
                          <span
                            aria-hidden
                            className={
                              selected ? "text-amber-400" : "text-neutral-600"
                            }
                          >
                            {selected ? "●" : "○"}
                          </span>
                        )}
                        <span
                          className={
                            selected ? "text-amber-200" : "text-neutral-300"
                          }
                        >
                          {option.label}
                          {recommended && RECOMMENDED_TAG}
                        </span>
                      </button>
                      {selected && (
                        <input
                          value={d.notes[index] ?? ""}
                          onChange={(e) =>
                            updateDraft(question.id, (dd) => ({
                              ...dd,
                              notes: {
                                ...dd.notes,
                                [index]: e.target.value,
                              },
                            }))
                          }
                          placeholder="note (optional)"
                          className="ml-6 mt-1 w-3/4 rounded border border-neutral-700 bg-neutral-900 px-2 py-0.5 text-xs outline-none focus:border-amber-500"
                        />
                      )}
                    </div>
                  );
                })}
                <div>
                  <button
                    type="button"
                    disabled={busy}
                    onClick={() => toggleOther(question)}
                    className="flex w-full items-center gap-2 rounded px-1 py-0.5 text-left text-sm hover:bg-neutral-800/60 disabled:opacity-50"
                  >
                    {question.multi ? (
                      <span aria-hidden className="text-amber-400">
                        {d.other ? "☑" : "☐"}
                      </span>
                    ) : (
                      <span
                        aria-hidden
                        className={
                          d.other ? "text-amber-400" : "text-neutral-600"
                        }
                      >
                        {d.other ? "●" : "○"}
                      </span>
                    )}
                    <span
                      className={
                        d.other ? "text-amber-200" : "text-neutral-300"
                      }
                    >
                      {OTHER_OPTION}
                    </span>
                  </button>
                  {d.other && (
                    <input
                      value={d.otherText}
                      onChange={(e) =>
                        updateDraft(question.id, (dd) => ({
                          ...dd,
                          otherText: e.target.value,
                        }))
                      }
                      placeholder="Type your own answer"
                      className="ml-6 mt-1 w-3/4 rounded border border-neutral-700 bg-neutral-900 px-2 py-0.5 text-xs outline-none focus:border-amber-500"
                    />
                  )}
                </div>
              </div>
            </div>
          );
        })}
      </div>

      {/* Final batch review for multi-question asks. */}
      {questions.length > 1 && (
        <div className="mt-3 rounded border border-neutral-800 bg-neutral-900/60 p-2">
          <p className="text-xs font-medium text-neutral-400">Review</p>
          <ul className="mt-1 space-y-0.5">
            {questions.map((question) => {
              const d = draft(question.id);
              const parts: string[] = [];
              for (const index of d.selected) {
                const label = question.options[index]?.label ?? "";
                const note = d.notes[index]?.trim();
                parts.push(note ? `${label} - ${note}` : label);
              }
              if (d.other) {
                parts.push(
                  d.otherText.trim() ? `Other: ${d.otherText.trim()}` : "Other",
                );
              }
              return (
                <li key={question.id} className="text-xs text-neutral-300">
                  {question.question}:{" "}
                  {parts.length > 0 ? parts.join(", ") : "—"}
                </li>
              );
            })}
          </ul>
        </div>
      )}

      {/* Footer hints + actions. */}
      <div className="mt-3 flex items-center justify-between gap-2">
        <p className="text-xs text-neutral-500">Enter submit · Esc cancel</p>
        <div className="flex gap-2">
          <button
            type="button"
            disabled={busy}
            onClick={() => void respond(true)}
            className="rounded bg-neutral-700 px-3 py-1 text-sm text-neutral-200 hover:bg-neutral-600 disabled:opacity-50"
          >
            Cancel
          </button>
          <button
            type="button"
            disabled={busy || !canSubmit}
            onClick={() => void respond(false)}
            className="rounded bg-amber-600 px-3 py-1 text-sm text-black hover:bg-amber-500 disabled:opacity-50"
          >
            Submit
          </button>
        </div>
      </div>
      {error && <p className="mt-1 text-xs text-red-400">{error}</p>}
    </div>
  );
}
