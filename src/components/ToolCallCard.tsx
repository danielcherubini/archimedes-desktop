import { useEffect, useRef, useState } from "react";
import { CheckIcon, ChevronRightIcon, CopyIcon } from "lucide-react";
import type { DiffRef, ToolCallUiStatus } from "../store/sessions";
import {
  editChangeStat,
  failureText,
  fileSummaries,
  normalizeToolOutput,
  summarizeToolCall,
  toolIcon,
  toolVerb,
} from "../lib/toolOutput";
import FileChip from "./FileChip";
import DiffCount from "./DiffCount";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "./ui/tooltip";
import DiffBlock from "./DiffBlock";

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
 * else a `rounded-xl border bg-panel` panel — a `$` prompt + mono
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
  const verb = toolVerb(title, status);
  const Icon = toolIcon(title);
  const files = fileSummaries(title, rawInput);
  const stat = editChangeStat(title, rawInput);
  const summary = summarizeToolCall(title, rawInput);
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
      ? normalizeToolOutput(rawOutput, status === "failed")
      : undefined;
  const failure = status === "failed" ? failureText(rawOutput) : undefined;

  // The failure tooltip's copy button (ZCode's pattern: copy → check
  // for 1.5s; a no-op when there is no failure text).
  const [copied, setCopied] = useState(false);
  const resetRef = useRef<number | null>(null);
  const handleCopy = () => {
    if (!failure) return;
    void navigator.clipboard?.writeText(failure)?.then(() => {
      setCopied(true);
      if (resetRef.current !== null) window.clearTimeout(resetRef.current);
      resetRef.current = window.setTimeout(() => setCopied(false), 1500);
    });
  };
  useEffect(
    () => () => {
      if (resetRef.current !== null) window.clearTimeout(resetRef.current);
    },
    [],
  );

  return (
    <div className="w-full">
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        className="group/tool-summary flex h-8 w-full items-center gap-2 rounded-lg px-2 text-left hover:bg-surface-hover"
      >
        <Icon className="size-4 shrink-0 text-foreground-subtle" />
        <span
          className={`shrink-0 whitespace-nowrap font-medium ${
            status === "pending"
              ? "animated-gradient-text"
              : "text-foreground-subtlest"
          }`}
        >
          {verb ?? title}
        </span>
        {files.length > 0 ? (
          <FileChip path={files[0].path} />
        ) : isShell && typeof command === "string" && command !== "" ? (
          <span className="min-w-0 truncate font-sans text-foreground-subtle">
            {command}
          </span>
        ) : summary ? (
          <span className="min-w-0 truncate text-ui-sm text-foreground-subtlest">
            {summary}
          </span>
        ) : null}
        {stat && <DiffCount stat={stat} />}
        {status === "failed" && (
          <TooltipProvider>
            <Tooltip>
              <TooltipTrigger asChild>
                <span className="shrink-0 cursor-help whitespace-nowrap text-destructive underline decoration-dotted underline-offset-2">
                  Failed
                </span>
              </TooltipTrigger>
              {failure && (
                <TooltipContent side="top" align="start" className="max-w-96">
                  <div className="flex max-w-96 items-center gap-2">
                    <span className="line-clamp-3 min-w-0 flex-1 whitespace-pre-wrap break-words">
                      {failure}
                    </span>
                    <button
                      type="button"
                      onClick={(event) => {
                        event.preventDefault();
                        event.stopPropagation();
                        handleCopy();
                      }}
                      aria-label={copied ? "Error copied" : "Copy error"}
                      title={copied ? "Error copied" : "Copy error"}
                      className="shrink-0 text-foreground-subtle hover:text-foreground"
                    >
                      {copied ? (
                        <CheckIcon className="size-3" />
                      ) : (
                        <CopyIcon className="size-3" />
                      )}
                    </button>
                  </div>
                </TooltipContent>
              )}
            </Tooltip>
          </TooltipProvider>
        )}
        <ChevronRightIcon
          aria-hidden
          className={`size-4 shrink-0 text-foreground-subtlest opacity-0 transition-transform transition-opacity duration-200 ease-out group-hover/tool-summary:opacity-100 ${
            open ? "rotate-90 opacity-100" : "rotate-0"
          }`}
        />
      </button>
      {open && (
        <div className="mt-1 rounded-xl border border-border bg-panel px-4 py-3">
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
