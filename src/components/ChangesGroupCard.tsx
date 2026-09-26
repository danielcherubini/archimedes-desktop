import { useLayoutEffect, useRef, useState } from "react";
import { ChevronRightIcon, PencilIcon } from "lucide-react";
import type { ToolCallMessage } from "../lib/toolGroups";
import {
  editChangeStat,
  fileSummaries,
  type ToolFileSummary,
} from "../lib/toolOutput";
import FileChip from "./FileChip";
import DiffCount from "./DiffCount";
import ToolCallCard from "./ToolCallCard";

const CHIP_GAP_PX = 8;
const TRAILING_PX = 24;

function dedupeFiles(files: ToolFileSummary[]): ToolFileSummary[] {
  const seen = new Set<string>();
  const out: ToolFileSummary[] = [];
  for (const f of files) {
    if (!seen.has(f.path)) {
      seen.add(f.path);
      out.push(f);
    }
  }
  return out;
}

function sumStats(
  stats: Array<{ added: number; removed: number } | undefined>,
): { added: number; removed: number } | undefined {
  let added = 0;
  let removed = 0;
  for (const s of stats) {
    if (s) {
      added += s.added;
      removed += s.removed;
    }
  }
  if (added === 0 && removed === 0) return undefined;
  return { added, removed };
}

function filesCountLabel(n: number): string {
  return `${n} file${n === 1 ? "" : "s"}`;
}

/**
 * A `Changes` card: consecutive `write`/`edit` tool calls folded into
 * one header (`Changes` + `N files` + a responsive file-chip list with
 * `+N` overflow + total `+N -M` stats; while any member is pending,
 * the latest member's chip shows instead of the full list) and,
 * expanded, the per-file cards indented under a left border.
 */
export default function ChangesGroupCard({
  messages,
}: {
  messages: ToolCallMessage[];
}) {
  const [open, setOpen] = useState(false);
  const files = dedupeFiles(
    messages.flatMap((m) => fileSummaries(m.title, m.rawInput)),
  );
  const totalStat = sumStats(
    messages.map((m) => editChangeStat(m.title, m.rawInput)),
  );
  const anyPending = messages.some((m) => m.status === "pending");
  const latest = anyPending
    ? messages.reduce((a, b) => (b.at >= a.at ? b : a))
    : undefined;
  const latestFile = latest
    ? fileSummaries(latest.title, latest.rawInput)[0]
    : undefined;

  // Responsive chip list (port of ZCode's `resolveResponsiveFileChipCount`):
  // measure the chips against the row's right boundary; overflow → `+N`.
  const listRef = useRef<HTMLSpanElement>(null);
  const chipRefs = useRef<Array<HTMLSpanElement | null>>([]);
  const overflowRef = useRef<HTMLSpanElement>(null);
  const [visibleCount, setVisibleCount] = useState(files.length);
  useLayoutEffect(() => {
    const update = () => {
      const list = listRef.current;
      if (!list) return;
      const boundary = list.closest("[data-changes-group-row]") ?? list;
      const rect = list.getBoundingClientRect();
      const boundaryRight = boundary.getBoundingClientRect().right;
      const available = Math.max(0, boundaryRight - rect.left - TRAILING_PX);
      const chipWidths = files.map(
        (_, i) => chipRefs.current[i]?.offsetWidth ?? 0,
      );
      const overflowWidth = overflowRef.current?.offsetWidth ?? 0;
      if (available <= 0) {
        setVisibleCount(0);
        return;
      }
      const allWidth =
        chipWidths.reduce((a, b) => a + b, 0) +
        CHIP_GAP_PX * Math.max(0, chipWidths.length - 1);
      if (allWidth <= available) {
        setVisibleCount(files.length);
        return;
      }
      let visible = 0;
      let width = 0;
      for (let i = 0; i < files.length; i++) {
        const next = i + 1;
        const required =
          width + chipWidths[i] + CHIP_GAP_PX * next + overflowWidth;
        if (required > available) break;
        width += chipWidths[i];
        visible = next;
      }
      setVisibleCount((c) => (c === visible ? c : visible));
    };
    update();
    const boundary =
      listRef.current?.closest("[data-changes-group-row]") ?? listRef.current;
    if (!boundary || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(update);
    observer.observe(boundary);
    window.addEventListener("resize", update);
    return () => {
      observer.disconnect();
      window.removeEventListener("resize", update);
    };
  }, [files.length, anyPending]);

  const hiddenCount = files.length - visibleCount;
  // While any member is pending, the chip list is NOT rendered — the
  // live latest-member chip (below) replaces it (ZCode's behavior).

  return (
    <div className="w-full" data-changes-group-row="">
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        className="group/tool-summary flex h-8 w-full items-center gap-2 rounded-lg px-2 text-left hover:bg-surface-hover"
      >
        <PencilIcon className="size-4 shrink-0 text-foreground-subtle" />
        <span className="shrink-0 whitespace-nowrap font-medium text-foreground-subtlest">
          Changes
        </span>
        {files.length > 1 && (
          <span className="shrink-0 text-foreground-subtlest">
            {filesCountLabel(files.length)}
          </span>
        )}
        {files.length > 1 && (
          <span className="shrink-0 text-foreground-subtlest">·</span>
        )}
        {files.length > 1 && !anyPending && (
          <span
            ref={listRef}
            className="relative inline-flex min-w-0 max-w-full flex-1 items-center gap-2 overflow-hidden"
          >
            {files.map((file, index) => {
              const isVisible = index < visibleCount;
              return (
                <span
                  key={file.path}
                  ref={(el) => {
                    chipRefs.current[index] = el;
                  }}
                  aria-hidden={isVisible ? undefined : true}
                  className={
                    isVisible
                      ? "inline-flex min-w-0 shrink-0"
                      : "pointer-events-none invisible absolute inline-flex shrink-0"
                  }
                >
                  <FileChip path={file.path} />
                </span>
              );
            })}
            {hiddenCount > 0 && (
              <span className="shrink-0 text-foreground-subtlest">
                +{hiddenCount}
              </span>
            )}
            <span
              ref={overflowRef}
              aria-hidden="true"
              className="pointer-events-none invisible absolute shrink-0"
            >
              +{files.length}
            </span>
          </span>
        )}
        {files.length === 1 && !anyPending && <FileChip path={files[0].path} />}
        {totalStat && <DiffCount stat={totalStat} />}
        {anyPending && latestFile && (
          <span className="inline-flex min-w-0 items-center gap-2">
            <FileChip path={latestFile.path} />
          </span>
        )}
        <ChevronRightIcon
          aria-hidden
          className={`size-4 shrink-0 text-foreground-subtlest opacity-0 transition-transform transition-opacity duration-200 ease-out group-hover/tool-summary:opacity-100 ${
            open ? "rotate-90 opacity-100" : "rotate-0"
          }`}
        />
      </button>
      {open && (
        <div className="ml-2 mt-1 space-y-2 border-l border-border pl-3.5">
          {messages.map((m, i) => (
            <ToolCallCard
              key={m.id ?? i}
              title={m.title}
              status={m.status}
              diff={m.diff}
              rawInput={m.rawInput}
              rawOutput={m.rawOutput}
            />
          ))}
        </div>
      )}
    </div>
  );
}
