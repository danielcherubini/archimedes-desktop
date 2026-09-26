import { useState } from "react";
import { CheckIcon, LoaderIcon, XIcon } from "lucide-react";
import type { DiffRef, ToolCallUiStatus } from "../store/sessions";
import { normalizeToolOutput, summarizeToolCall } from "../lib/toolOutput";
import DiffBlock from "./DiffBlock";

/**
 * One `h-8` tool row: a status-aware icon (pending → spinner, completed →
 * check, failed → ✕) + the title + a muted one-line summary derived from
 * `rawInput` (the command / file path / pattern, per the design table).
 * Expandable body: the extracted diff via `DiffBlock` when present, else
 * the normalized `rawOutput` text (scrollable, capped at 20k chars), else a
 * muted "(no output)" line.
 */
export default function ToolCallCard({
  title,
  status,
  diff,
  rawInput,
  rawOutput,
}: {
  title: string;
  status: ToolCallUiStatus;
  diff?: DiffRef;
  rawInput?: unknown;
  rawOutput?: unknown;
}) {
  const [open, setOpen] = useState(false);
  const summary = summarizeToolCall(title, rawInput);
  // Defer output normalization until the card is expanded: the body is the
  // only consumer, and joining/serializing large results on every streaming
  // render while collapsed is wasted work.
  const output = open && !diff ? normalizeToolOutput(rawOutput) : undefined;
  return (
    <div className="w-full">
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        className="flex h-8 w-full items-center gap-2 rounded-lg px-2 text-left hover:bg-surface-hover"
      >
        {status === "pending" ? (
          <LoaderIcon className="size-4 shrink-0 animate-spin text-foreground-subtle" />
        ) : status === "completed" ? (
          <CheckIcon className="size-4 shrink-0 text-foreground-subtle" />
        ) : (
          <XIcon className="size-4 shrink-0 text-destructive" />
        )}
        <span
          className={`min-w-0 truncate text-ui-base ${
            status === "failed"
              ? "text-destructive"
              : status === "completed"
                ? "text-foreground-subtle"
                : ""
          }`}
        >
          {title}
        </span>
        {summary && (
          <span className="min-w-0 truncate text-ui-sm text-foreground-subtlest">
            {summary}
          </span>
        )}
      </button>
      {open && (
        <div className="mt-1 rounded-md bg-surface p-2">
          {diff ? (
            <DiffBlock path={diff.path} patch={diff.patch} />
          ) : output !== undefined ? (
            <div>
              <pre className="font-mono max-h-80 overflow-auto whitespace-pre-wrap text-ui-sm">
                {output.length > 20000 ? output.slice(0, 20000) + "…" : output}
              </pre>
              {output.length > 20000 && (
                <p className="text-ui-sm text-foreground-subtlest">
                  … (truncated — {output.length} chars total)
                </p>
              )}
            </div>
          ) : (
            <p className="text-ui-sm text-foreground-subtlest">(no output)</p>
          )}
        </div>
      )}
    </div>
  );
}
