import type { SkillInfo } from "./tauri";

/**
 * The mention regex (ZCode's exact form): a `$` followed by a lowercase
 * [a-z0-9] run, hyphen-separated. Uppercase or other chars after `$` are
 * NOT a skill token (the match must reach the end of the scanned span).
 * Implementation note: use `text.match(MENTION_RE)` (with the `/g` flag it
 * returns ALL matches and resets `lastIndex`); do NOT loop `exec`/`test`
 * against the shared module-level regex (a stale `lastIndex` across calls
 * can drop mentions on repeat sends).
 */
const MENTION_RE = /\$([a-z0-9]+(?:-[a-z0-9]+)*)/g;

/**
 * Expand `$name` mentions in `text` into pi-format `<skill>` blocks.
 *
 * - Each captured name (lowercase, by the regex) is matched against
 *   `skill.name.toLowerCase()` — i.e. matching is CASE-INSENSITIVE on the
 *   SKILL-NAME side: a skill named `DeBuG` is invoked by `$debug`. (Tokens
 *   are lowercase-only: `$DEBUG` is NOT a mention — the regex cannot match
 *   it, and it passes through verbatim.)
 * - A token matching no skill is left VERBATIM (no error, no stripping).
 * - One block per matched skill, even if its token appears multiple times
 *   (deduped).
 * - Blocks are appended AFTER the user's text, separated by a blank line,
 *   in the order the skills are FIRST mentioned.
 * - The block format is the pi skill-expansion format (the exact literal
 *   below — the shape pi's native `/skill:name` expansion produces, so the
 *   agent sees the same shape whether it loaded the skill itself or the
 *   desktop injected it):
 *
 *     <skill name="NAME" location="PATH">
 *     References are relative to DIR.
 *
 *     BODY
 *     </skill>
 *
 *   where `NAME` = the skill's frontmatter name VERBATIM (not lowercased),
 *   `PATH` = `skill.path`, `DIR` = `skill.dir`, `BODY` = `skill.body`.
 *   (pi's format also supports `args` after `/skill:name` — the mention
 *   model has no args: the token is a pure trigger, the rest of the
 *   message is the user's request.)
 * - No matched token → return `text` UNCHANGED (the function is pure — no
 *   mutation of inputs).
 */
export function expandSkillMentions(text: string, skills: SkillInfo[]): string {
  const matches = text.match(MENTION_RE);
  if (matches === null) return text;

  // Group the catalog by lowercased name (preserving catalog order within a
  // group) so a token's match is an O(1) lookup.
  const byName = new Map<string, SkillInfo[]>();
  for (const skill of skills) {
    const key = skill.name.toLowerCase();
    const group = byName.get(key);
    if (group) group.push(skill);
    else byName.set(key, [skill]);
  }

  // Walk the tokens in text order; for each, emit one block per matched
  // skill (catalog order), deduped ACROSS THE WHOLE RUN (a skill mentioned
  // twice yields a single block).
  const seen = new Set<SkillInfo>();
  const blocks: string[] = [];
  for (const match of matches) {
    const token = match.slice(1); // strip the leading `$`
    const group = byName.get(token);
    if (group === undefined) continue;
    for (const skill of group) {
      if (seen.has(skill)) continue;
      seen.add(skill);
      blocks.push(buildBlock(skill));
    }
  }

  if (blocks.length === 0) return text;
  return text + "\n\n" + blocks.join("\n\n");
}

/** The pi skill-expansion block for one skill (the exact literal in the doc above). */
function buildBlock(skill: SkillInfo): string {
  return (
    `<skill name="${skill.name}" location="${skill.path}">` +
    `\nReferences are relative to ${skill.dir}.` +
    `\n\n${skill.body}` +
    `\n</skill>`
  );
}

/** One `<skill>` block parsed out of an expanded user message. */
export interface SkillBlock {
  /** The `name` attribute VERBATIM (as in the block). */
  name: string;
  /** The block's body (between the `References…` note and `</skill>`). */
  body: string;
}

/** The exact block shape `expandSkillMentions` produces (one match per block).
 * `name` is captured verbatim; the body is the content between the blank line
 * after the `References…` note and the `</skill>` line. */
const SKILL_BLOCK_RE =
  /<skill name="([^"]*)" location="[^"]*">\nReferences are relative to[^\n]*\n\n([\s\S]*?)\n<\/skill>/g;

/**
 * Split an expanded user message back into the user's own text and the
 * appended `<skill>` blocks (the inverse of `expandSkillMentions`'s output
 * shape — display-only; the persisted/sent text is UNCHANGED).
 *
 * - `text` = the user's text with the blocks removed (trimmed of the
 *   surrounding blank line — the user's text verbatim).
 * - `blocks` = the parsed blocks in order (first-mention order).
 * - NO blocks → `{ text: <the input VERBATIM, UNCHANGED>, blocks: [] }`
 *   (byte-identical — critical for no-skill messages: a content-based
 *   `mergeDedupeKey` resume-merge stays byte-identical).
 * - Robust, not a full HTML parser: match the known shape. If a `<skill …>`
 *   opening tag is present but the shape doesn't parse cleanly (shouldn't
 *   happen — only our own expansion produces these), treat the whole text
 *   as no-blocks (verbatim).
 */
export function splitSkillBlocks(
  text: string,
): { text: string; blocks: SkillBlock[] } {
  // Count ANY `<skill` opening tag (a valid block's opening tag matches
  // exactly once; a stray `<skill` literal that is NOT a valid block makes
  // the counts disagree → verbatim).
  const opens = (text.match(/<skill[ >/]/g) ?? []).length;
  const matches: { index: number; end: number; name: string; body: string }[] =
    [];
  SKILL_BLOCK_RE.lastIndex = 0;
  let match: RegExpExecArray | null;
  while ((match = SKILL_BLOCK_RE.exec(text)) !== null) {
    matches.push({
      index: match.index,
      end: match.index + match[0].length,
      name: match[1],
      body: match[2].trim(),
    });
  }
  if (matches.length === 0 || matches.length !== opens) {
    return { text, blocks: [] };
  }
  // The blocks are APPENDED after the user's text, joined by a blank line
  // (the `expandSkillMentions` `\n\n` joiner), so the matches must be
  // CONTIGUOUS from the first match (each `match.index` === the previous
  // match's end + 2) AND nothing may follow the last match (the blocks are
  // the text's suffix). A gap or a suffix means the text is NOT the
  // expansion shape (e.g. a mis-terminated block whose embedded `</skill>`
  // truncated a real match, or a pasted block between the user's text and
  // an appended block) — verbatim (NO-LOSS: taking the prefix would
  // silently drop the gap/suffix text from the rendered bubble).
  for (let i = 1; i < matches.length; i++) {
    if (matches[i].index !== matches[i - 1].end + 2) {
      return { text, blocks: [] };
    }
  }
  if (text.slice(matches[matches.length - 1].end).trim() !== "") {
    return { text, blocks: [] };
  }
  // The user's text is everything before the first block (trimmed of the
  // surrounding blank line — the user's text verbatim).
  const prefix = text.slice(0, matches[0].index);
  return {
    text: prefix.replace(/\s+$/, ""),
    blocks: matches.map((m) => ({ name: m.name, body: m.body })),
  };
}
