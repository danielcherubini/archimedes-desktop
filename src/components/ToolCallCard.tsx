import { useState } from "react";
import type { DiffRef, ToolCallUiStatus } from "../store/sessions";
import DiffBlock from "./DiffBlock";

const STATUS_STYLES: Record<ToolCallUiStatus, string> = {
  pending: "bg-amber-500/20 text-amber-300",
  completed: "bg-green-500/20 text-green-300",
  failed: "bg-red-500/20 text-red-300",
};

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
    <div className="rounded-md border border-neutral-700 bg-neutral-900">
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        className="flex w-full items-center gap-2 px-3 py-2 text-left text-sm"
      >
        <span aria-hidden>{open ? "▾" : "▸"}</span>
        <span
          className={`rounded px-1.5 py-0.5 text-xs ${STATUS_STYLES[status]}`}
        >
          {status}
        </span>
        <span className="truncate">{title}</span>
      </button>
      {open && diff && (
        <div className="border-t border-neutral-700 p-2">
          <DiffBlock path={diff.path} patch={diff.patch} />
        </div>
      )}
    </div>
  );
}
