import { FileCodeIcon } from "lucide-react";
import { parseDiffStats } from "../lib/diff";

/**
 * The turn's file summary: one card listing every file the agent's most
 * recent turn touched (derived from the standalone `kind: "diff"` messages
 * — the `tool-call.diff` refs are deliberately NOT summed, that would
 * double-count), with per-file added/removed stats and the totals.
 */
export default function FileSummaryCard({
  diffs,
}: {
  diffs: Array<{ path: string; patch: string }>;
}) {
  // Dedupe by `path` (last wins) BEFORE computing stats: the store re-emits
  // a standalone `diff` message on EVERY `tool_call_update` carrying
  // content, so the same file can appear multiple times in a turn (an
  // agent re-sending its diff) — count each file once (its latest patch).
  const byPath = new Map<string, { path: string; patch: string }>();
  for (const d of diffs) byPath.set(d.path, d);
  const files = [...byPath.values()];

  const stats = files.map((f) => ({ ...f, ...parseDiffStats(f.patch) }));
  const totalAdditions = stats.reduce((sum, f) => sum + f.additions, 0);
  const totalDeletions = stats.reduce((sum, f) => sum + f.deletions, 0);

  return (
    <div className="overflow-hidden rounded-xl border border-border bg-card">
      <div className="flex h-10 items-center justify-between gap-2 px-2 hover:bg-hover">
        <div className="flex items-center gap-2">
          <FileCodeIcon className="size-4 text-foreground-subtle" />
          <span className="text-ui-base font-medium">
            {files.length} files changed
          </span>
          <span className="text-ui-base tabular-nums text-diff-added">
            +{totalAdditions}
          </span>
          <span className="text-ui-base tabular-nums text-diff-removed">
            −{totalDeletions}
          </span>
        </div>
      </div>
      <div className="border-t border-border">
        {stats.map((f) => (
          <div key={f.path} className="flex h-8 items-center gap-2 px-2">
            <FileCodeIcon className="size-4 shrink-0 text-foreground-subtle" />
            <span className="flex-1 truncate font-mono text-ui-base">
              {f.path}
            </span>
            <span className="text-ui-base tabular-nums text-diff-added">
              +{f.additions}
            </span>
            <span className="text-ui-base tabular-nums text-diff-removed">
              −{f.deletions}
            </span>
          </div>
        ))}
      </div>
    </div>
  );
}
