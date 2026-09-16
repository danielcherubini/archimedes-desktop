/**
 * Folder display label: the base name of a (canonical, absolute) path.
 * Handles both `/` and `\` separators (Windows cwd strings).
 * Returns `""` for empty input or bare roots — the caller falls back to
 * the full path.
 */
export function basenameOfPath(p: string): string {
  if (p === "") return "";
  const trimmed = p.replace(/[\\/]+$/, "");
  if (trimmed === "") return "";
  const parts = trimmed.split(/[\\/]/);
  const last = parts[parts.length - 1];
  return last === "" ? "" : last;
}
