import { useEffect, useRef, useState } from "react";
import type { DiffRef, ToolCallUiStatus } from "../store/sessions";
import {
  editChangeStat,
  fileSummaries,
  normalizeToolOutput,
  readLineRange,
} from "../lib/toolOutput";
import FileChip from "./FileChip";
import DiffCount from "./DiffCount";
import DiffBlock from "./DiffBlock";
import { ToolCallCardHeader } from "./ToolCallCardHeader";

const SHELL_TOOLS = new Set(["bash", "powershell", "sudo_exec"]);

/**
 * One `h-8` tool row in the ZCode style: a static per-tool icon + a
 * past-tense verb (shimmer — `animated-gradient-text` — while running;
 * the past tense is the "done" signal, no check icon) + the primary
 * text (a file chip for read/write/edit, the command in `font-sans`
 * for shell tools, the `summarizeToolCall` summary otherwise) + a
 * `+N -M` change stat (edit only) + a red `Failed` word with a
 * dotted-underline tooltip (error text + copy) on failure + a hover
 * chevron. Expandable body: the diff via `DiffBlock` when present,
 * else a `rounded-xl bg-panel` panel — a `$` prompt + mono
 * command for shell tools, the file chip + stat for file tools —
 * then the normalized `rawOutput` (scrollable, capped at 20k chars)
 * or a muted "No output."
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
  // Auto-open a single file edit once when it finishes (ZCode's
  // one-shot `autoOpen`): the user sees the result without clicking;
  // they can close it, and it never re-opens on later updates.
  const prevStatusRef = useRef(status);
  const hasAutoOpenedRef = useRef(false);
  useEffect(() => {
    const was = prevStatusRef.current;
    prevStatusRef.current = status;
    const isFileTool = title === "write" || title === "edit";
    if (
      isFileTool &&
      was === "pending" &&
      status !== "pending" &&
      !hasAutoOpenedRef.current
    ) {
      hasAutoOpenedRef.current = true;
      setOpen(true);
    }
  }, [status, title]);
  const files = fileSummaries(title, rawInput);
  const stat = editChangeStat(title, rawInput);
  const range = readLineRange(rawInput);
  const isShell = SHELL_TOOLS.has(title);
  const command =
    isShell && typeof rawInput === "object" && rawInput !== null
      ? ((rawInput as Record<string, unknown>).command as string | undefined)
      : undefined;
  // Defer output normalization until the card is expanded: the body is
  // the only consumer, and joining/serializing large results on every
  // streaming render while collapsed is wasted work.
  const output =
    open && !diff
      ? normalizeToolOutput(rawOutput, status === "failed", title)
      : undefined;

  return (
    <div className="flex w-full flex-col">
      <ToolCallCardHeader
        title={title}
        status={status}
        rawInput={rawInput}
        rawOutput={rawOutput}
        open={open}
        onToggle={() => setOpen((o) => !o)}
        files={files}
        stat={stat}
        range={range}
        isShell={isShell}
        command={command}
      />
      {open && (
        <div className="mt-1 rounded-xl bg-panel px-4 py-3">
          {diff ? (
            <DiffBlock path={diff.path} patch={diff.patch} />
          ) : (
            <>
              {(isShell && typeof command === "string" || files.length > 0) && (
                <div className="mb-2 space-y-1">
                  {isShell && typeof command === "string" && command !== "" && (
                    <div className="flex items-start gap-2 font-sans text-ui-base text-foreground">
                      <span className="shrink-0 text-foreground-subtle">$</span>
                      <pre className="block min-w-0 max-h-15 flex-1 overflow-auto whitespace-pre-wrap break-words font-mono">
                        {command}
                      </pre>
                    </div>
                  )}
                  {files.length > 0 && (
                    <div className="flex items-center gap-2">
                      <FileChip path={files[0].path} />
                      {range && (
                        <span className="shrink-0 text-ui-sm text-foreground-subtlest">
                          {range}
                        </span>
                      )}
                      {stat && <DiffCount stat={stat} />}
                    </div>
                  )}
                </div>
              )}
              {output !== undefined ? (
                <div>
                  <pre className="font-mono max-h-80 overflow-auto whitespace-pre-wrap text-ui-sm">
                    {output.length > 20000
                      ? output.slice(0, 20000) + "…"
                      : output}
                  </pre>
                  {output.length > 20000 && (
                    <p className="text-ui-sm text-foreground-subtlest">
                      … (truncated — {output.length} chars total)
                    </p>
                  )}
                </div>
              ) : (
                <p className="font-mono text-ui-base text-foreground-subtle">
                  No output.
                </p>
              )}
            </>
          )}
        </div>
      )}
    </div>
  );
}
