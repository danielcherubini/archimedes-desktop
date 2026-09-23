import { useState } from "react";
import { CheckIcon, LoaderIcon, XIcon } from "lucide-react";
import type { DiffRef, ToolCallUiStatus } from "../store/sessions";
import DiffBlock from "./DiffBlock";

/**
 * One `h-8` tool row: a status-aware icon (pending → spinner, completed →
 * check, failed → ✕) + the label. Expandable: the body (in a nested
 * `rounded-md bg-surface` block) shows the extracted diff via `DiffBlock`
 * when present, else a muted "(no output)" line — the `tool-call` message
 * has a `rawInput` field but NO raw output field, so there is no output
 * rendering to invent.
 */
export default function ToolCallCard({
  title,
  status,
  diff,
}: {
  title: string;
  status: ToolCallUiStatus;
  diff?: DiffRef;
}) {
  const [open, setOpen] = useState(false);

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
          className={`truncate text-ui-base ${
            status === "failed"
              ? "text-destructive"
              : status === "completed"
                ? "text-foreground-subtle"
                : ""
          }`}
        >
          {title}
        </span>
      </button>
      {open && (
        <div className="mt-1 rounded-md bg-surface p-2">
          {diff ? (
            <DiffBlock path={diff.path} patch={diff.patch} />
          ) : (
            <p className="text-ui-sm text-foreground-subtlest">(no output)</p>
          )}
        </div>
      )}
    </div>
  );
}
