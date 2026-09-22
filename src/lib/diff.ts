/**
 * Line-based unified diff, generated without any diff library.
 *
 * `oldText` is `null` for new files. The output is a standard unified patch
 * (single hunk, full context) that {@link DiffBlock} can render by reading
 * the `+`/`-`/space prefixes.
 */

function splitLines(text: string): string[] {
  const lines = text.split("\n");
  // A trailing newline produces one trailing empty element; drop it.
  if (lines.length > 0 && lines[lines.length - 1] === "") lines.pop();
  return lines;
}

export function unifiedPatch(
  path: string,
  oldText: string | null,
  newText: string,
): string {
  const oldLines = oldText === null ? [] : splitLines(oldText);
  const newLines = splitLines(newText);

  // Classic LCS dynamic program (bottom-up), then walk it top-down.
  const n = oldLines.length;
  const m = newLines.length;
  const dp: number[][] = Array.from({ length: n + 1 }, () =>
    new Array<number>(m + 1).fill(0),
  );
  for (let i = n - 1; i >= 0; i--) {
    for (let j = m - 1; j >= 0; j--) {
      dp[i][j] =
        oldLines[i] === newLines[j]
          ? dp[i + 1][j + 1] + 1
          : Math.max(dp[i + 1][j], dp[i][j + 1]);
    }
  }

  type Op = { kind: "ctx" | "del" | "add"; line: string };
  const ops: Op[] = [];
  let i = 0;
  let j = 0;
  while (i < n && j < m) {
    if (oldLines[i] === newLines[j]) {
      ops.push({ kind: "ctx", line: oldLines[i] });
      i++;
      j++;
    } else if (dp[i + 1][j] >= dp[i][j + 1]) {
      ops.push({ kind: "del", line: oldLines[i] });
      i++;
    } else {
      ops.push({ kind: "add", line: newLines[j] });
      j++;
    }
  }
  while (i < n) {
    ops.push({ kind: "del", line: oldLines[i] });
    i++;
  }
  while (j < m) {
    ops.push({ kind: "add", line: newLines[j] });
    j++;
  }

  const oldCount = ops.filter((o) => o.kind !== "add").length;
  const newCount = ops.filter((o) => o.kind !== "del").length;

  const header =
    oldText === null
      ? `--- /dev/null\n+++ b${path}\n`
      : `--- a${path}\n+++ b${path}\n`;

  return [
    header,
    `@@ -1,${oldCount} +1,${newCount} @@`,
    ...ops.map(
      (op) =>
        (op.kind === "ctx" ? " " : op.kind === "del" ? "-" : "+") + op.line,
    ),
  ].join("\n");
}

export interface DiffStats {
  additions: number;
  deletions: number;
}

/**
 * Count added/removed lines in a unified-diff patch. Lines starting with `+`
 * count as additions and lines starting with `-` as deletions, EXCEPT the
 * 3-char file headers: lines matching exactly `^(\+\+\+|---)\s` (i.e. `+++ ` /
 * `--- ` followed by whitespace and a path) are metadata, not content — so a
 * real added line whose content itself starts with `---` (e.g. `+---
 * separator`) still counts as an addition. Malformed input (no valid lines)
 * → { additions: 0, deletions: 0 }.
 */
export function parseDiffStats(patch: string): DiffStats {
  let additions = 0;
  let deletions = 0;
  for (const line of patch.split("\n")) {
    if (/^(\+\+\+|---)\s/.test(line)) continue; // file header — metadata
    if (line.startsWith("+")) additions++;
    else if (line.startsWith("-")) deletions++;
  }
  return { additions, deletions };
}
