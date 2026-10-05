import { useEffect, useRef, useState } from "react";
import { CheckIcon, ChevronRightIcon, CopyIcon } from "lucide-react";
import {
  failureText,
  summarizeToolCall,
  toolIcon,
  toolVerb,
  type ToolFileSummary,
} from "../lib/toolOutput";
import type { ToolCallUiStatus } from "../store/sessions";
import FileChip from "./FileChip";
import DiffCount from "./DiffCount";
import { TRANSCRIPT_ROW_BLEED } from "./lib/transcriptRail";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "./ui/tooltip";

/**
 * The `ToolCallCard`'s header row (the `<button>`: icon + verb + summary +
 * stat + failure tooltip + chevron) — extracted so the `SubagentDelegatingCard`
 * can reuse it. `files`/`stat`/`range`/`isShell`/`command` are computed ONCE
 * by the caller (the `ToolCallCard`'s body uses them too) and passed in —
 * the header does NOT recompute them (no `SHELL_TOOLS` circular import).
 * The `verb`/`Icon`/`summary`/`failure` derivations + the failure-tooltip
 * `copied` state live HERE (moved from `ToolCallCard`). The toggle is
 * delegated to the caller via `onToggle` (the `open` state stays in the
 * `ToolCallCard` — its auto-open logic owns it).
 */
export function ToolCallCardHeader({
  title,
  status,
  rawInput,
  rawOutput,
  open,
  onToggle,
  files,
  stat,
  range,
  isShell,
  command,
}: {
  title: string;
  status: ToolCallUiStatus;
  rawInput?: unknown;
  rawOutput?: unknown;
  open: boolean;
  onToggle: () => void;
  files: ToolFileSummary[];
  stat: { added: number; removed: number } | undefined;
  range: string | undefined;
  isShell: boolean;
  command: string | undefined;
}) {
  const verb = toolVerb(title, status);
  const Icon = toolIcon(title);
  const summary = summarizeToolCall(title, rawInput);
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
    <button
      type="button"
      onClick={onToggle}
      className={`group/tool-summary flex h-8 items-center gap-2 rounded-lg text-left hover:bg-surface-hover ${TRANSCRIPT_ROW_BLEED}`}
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
        <>
          <FileChip path={files[0].path} />
          {range && (
            <span className="shrink-0 text-ui-sm text-foreground-subtlest">
              {range}
            </span>
          )}
        </>
      ) : isShell && typeof command === "string" && command !== "" ? (
        <span className="min-w-0 truncate font-mono text-ui-sm text-foreground-subtle">
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
  );
}
