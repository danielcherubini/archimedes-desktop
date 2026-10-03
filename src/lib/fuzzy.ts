/**
 * A case-insensitive SUBSEQUENCE match (the fzf-style "fuzzy" the model
 * picker's search uses): every query character appears in the target, in
 * order — so `qwen` matches `Qwen3.8`, but `nw` does not (the `n` comes
 * after the `w`). A substring match is a special case (consecutive
 * characters are still "in order"), so this is a strict generalization.
 * An empty query matches everything (the unfiltered list).
 */
export function fuzzyMatch(needle: string, target: string): boolean {
  const n = needle.toLowerCase();
  if (n === "") return true;
  const t = target.toLowerCase();
  let i = 0;
  for (const c of n) {
    i = t.indexOf(c, i);
    if (i === -1) return false;
    i++;
  }
  return true;
}
