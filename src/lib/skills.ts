import type { AgentDefinitionDto, McpServerInfo, SkillInfo } from "./tauri";

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
 * The `$`-only ORACLE for `expandMentions` — retained deliberately, NOT dead
 * code. It has NO production caller (the composer expands via
 * `expandMentions`) and exists solely to pin the `$` path's output
 * byte-for-byte: the `dollar_expansion_matches_expandSkillMentions` test in
 * `skills.test.ts` asserts `expandMentions` is byte-identical to this
 * function for every `$`-only case, and that byte-identity is what the
 * no-mention `mergeDedupeKey` resume guarantee rests on. Retiring one means
 * retiring the OTHER in the same change — never strip this one alone.
 *
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
 * The `$`-only ORACLE for `splitMentionBlocks` — retained deliberately, NOT
 * dead code. It has NO production caller (`MessageBubble` splits via
 * `splitMentionBlocks`) and exists solely to pin the `$`-only split
 * behaviour: the `splitSkillBlocks` tests in `skills.test.ts` (fed with
 * `expandSkillMentions` output) pin the shapes — including the verbatim
 * NO-BLOCKS contract the content-based `mergeDedupeKey` resume-merge depends
 * on — that the generalized function must reproduce byte-for-byte. Retiring
 * one means retiring the OTHER in the same change — never strip this one
 * alone.
 *
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

/**
 * The `$`-only ORACLE for `activeMentionToken` — retained deliberately, NOT
 * dead code. It has NO production caller: the composer's `onChange`
 * re-computation, the keydown re-evaluation and `selectMention` all use
 * `activeMentionToken`. The `activeSkillToken` tests in `skills.test.ts` pin
 * the exact `{ remainder, start }` contract that `activeMentionToken` must
 * reproduce for a `$` token. Retiring one means retiring the OTHER in the
 * same change — never strip this one alone.
 *
 * The active skill token at the caret: the span between the nearest
 * preceding whitespace (or start-of-line) and the caret. Returns `{ remainder, start }` when the span
 * starts with `$` — `remainder` is the token's remainder AFTER the `$` (a
 * bare `$` is `""`), `start` is the `$`'s index in the string — else `null`
 * (no active token: the span doesn't start with `$`, or it contains a
 * character that can't be part of a skill name, e.g. uppercase — the regex
 * `[a-z0-9-]*$` simply won't reach the caret).
 */
export function activeSkillToken(
  value: string,
  caret: number,
): { remainder: string; start: number } | null {
  const before = value.slice(0, caret);
  const m = before.match(/(^|\s)(\$[a-z0-9-]*)$/);
  if (!m) return null;
  return { remainder: m[2]!.slice(1), start: (m.index ?? 0) + m[1]!.length };
}

// ---------------------------------------------------------------------------
// Generalized picker-token + mention mechanism (`$` skills / `@` agents /
// `#` MCP servers are **Mention**s; `?` File completion shares the picker and
// the token scan, but expands NOTHING — ADR 0031 vs ADR 0033)
// ---------------------------------------------------------------------------

export type MentionKind = "skill" | "agent" | "mcp";

/** The composer's picker prefixes. `$`/`@`/`#` are **Mention**s (they expand
 *  into a block on send); `?` is **File completion**, which expands NOTHING
 *  and only inserts a path — the three share a picker, not a semantics
 *  (ADR 0031 vs ADR 0033). */
export type ComposerPrefix = "$" | "@" | "#" | "?";

/**
 * The `@` mention regex (an agent definition): a leading whitespace or
 * line-start, then `@` + a lowercase [a-z0-9] run, hyphen-separated.
 * UNCHANGED asymmetry (invariant 3): the `#`/`@` regexes REQUIRE the
 * `(^|\s)` left boundary while the `$` one is boundary-free (ZCode parity).
 * A future reader must NOT "unify" them.
 */
export const AGENT_MENTION_RE = /(^|\s)@([a-z0-9]+(?:-[a-z0-9]+)*)/g;

/** The `#` mention regex (an MCP server) — same shape as `AGENT_MENTION_RE`. */
export const MCP_MENTION_RE = /(^|\s)#([a-z0-9]+(?:-[a-z0-9]+)*)/g;

export interface MentionCatalogs {
  skills: SkillInfo[];
  /** The wire DTO (name/description/model/scope) — name + description are all the block needs. */
  agents: AgentDefinitionDto[];
  /** The wire DTO (name/kind/summary). */
  mcpServers: McpServerInfo[];
}

/**
 * The active picker token at the caret (generalizes `activeSkillToken`):
 * the span between the nearest preceding whitespace (or start) and the caret.
 * Returns `{ prefix, remainder, start }` when the span starts with one of the
 * four prefixes — `remainder` is the token's remainder AFTER the prefix (a
 * bare `$`/`@`/`#` glyph is `""`; a bare `?` is NOT a token at all), `start`
 * is the prefix's index in the string — else `null` (no active token: the span
 * doesn't start with a prefix, or it contains a character that can't be part
 * of a token, e.g. uppercase for a Mention — the regex `[a-z0-9-]*$` simply
 * won't reach the caret).
 * (`$` keeps the SAME live-caret boundary as `activeSkillToken` — the
 * boundary-free-left applies to the EXPANSION regex only.)
 */
export function activeMentionToken(
  value: string,
  caret: number,
): { prefix: ComposerPrefix; remainder: string; start: number } | null {
  const before = value.slice(0, caret);
  // One shared shape: the span between the nearest preceding whitespace
  // (or start) and the caret starts with a prefix + a token char class.
  const m = before.match(/(^|\s)([$#@][a-z0-9-]*)$/);
  if (m) {
    // The regex guarantees `m[2]` starts with one of the three Mention
    // glyphs — narrow explicitly (`charAt` returns `string`, which does NOT
    // satisfy the union return type under `strict` TS).
    const first = m[2]!.charAt(0) as ComposerPrefix;
    return {
      prefix: first,
      remainder: m[2]!.slice(1),
      start: (m.index ?? 0) + m[1]!.length,
    };
  }
  const f = before.match(FILE_TOKEN_RE);
  if (!f) return null;
  // VERBATIM — a path's case is PRESERVED (no lowercasing rule applies to a
  // path, so the Mention grammar's lowercase-only rule does not apply here).
  // The inserted text is then ordinary message text: it obeys the mention
  // grammar exactly like a hand-typed path — a `$name` inside it, or a
  // root-level `@name`/`#name`, expands if a resource of that exact name
  // exists (pre-existing grammar, not a surface of `?` — ADR 0033).
  return {
    prefix: "?",
    remainder: f[2]!,
    start: (f.index ?? 0) + f[1]!.length,
  };
}

// The `?` token (ADR 0033) — deliberately NOT the mention charset: a PATH
// query needs `.` `_` `/` and UPPERCASE (README.md is spelled that way, and the
// insertion is verbatim — no lowercasing rule applies to a path — so case
// is preserved). It ALSO requires ≥1 char (a bare `?` opens nothing): that
// is what keeps `??`
// (git-status porcelain, or a doubled question mark) from opening the
// picker and turning Enter into an INSERTION instead of a send. The
// `(^|\s)` boundary does the rest — `a?.b`, `x ? y` and `url?query` fail it.
const FILE_TOKEN_RE = /(^|\s)\?([a-zA-Z0-9._/-]+)$/;

/**
 * Expand `$` / `@` / `#` mentions in `text` into their block forms (the
 * generalization of `expandSkillMentions` — the `$` path produces the SAME
 * output the old function produced for a skills-only catalog).
 *
 * - Case policy (invariant 4): tokens are lowercase-only; catalog matching
 *   is case-insensitive on the resource-NAME side.
 * - A token matching NO catalog entry is left VERBATIM (no error, no
 *   stripping — invariant 5). One block per matched resource, deduped ACROSS
 *   THE WHOLE RUN. Blocks in first-mention (text-position) order.
 * - Tag-safety (invariant 6) — the SOUND argument, read it before changing
 *   the matching: the block literals interpolate the resource's CATALOG name
 *   UNESCAPED (into `name="…"`, and into the body's `"NAME"` / `agentName
 *   "NAME"` / `connect: "NAME"` spans), and nothing CHECKS that name for
 *   unsafe characters. It is safe because of an emergent property of EXACT
 *   MATCHING: a hit requires `entry.name.toLowerCase() === token`, and a
 *   token is `[a-z0-9]+(-[a-z0-9]+)*` by the regexes above. The LOAD-BEARING
 *   fact is therefore NARROWER than "the name is `[a-zA-Z0-9-]`": every
 *   character that is tag-UNSAFE or simply off the token charset (`"`, `<`,
 *   whitespace including `\n` / `\t`, and the other control characters) is
 *   LEFT ALONE by `toLowerCase()`, so a name containing one of them
 *   lowercases to a string still CONTAINING it, which no token equals. Such
 *   a name is thus UNMATCHABLE, hence never interpolated: none of them can
 *   reach a tag.
 *   (Do NOT restate this as "lowercasing is injective, so the name must be
 *   `[a-zA-Z0-9-]`" — that is FALSE: JS `"K".toLowerCase() === "k"` for
 *   U+212A KELVIN SIGN, so a name carrying U+212A CAN equal a token. It
 *   happens to be harmless because U+212A is itself TAG-SAFE — it is not a
 *   quote, a `<`, or a control character — so it cannot break the tag; the
 *   conclusion stands on the narrower fact above, not on injectivity.)
 *
 *   The name is size-UNCAPPED, but self-limiting: the picker row visibly
 *   renders the interpolated name, so an absurd one is on screen before the
 *   pick.
 *
 *   What tag-safety does NOT cover is the interpolated FREE text: an agent
 *   `description` is arbitrary prose, so it can carry
 *   `\n</agent>\n<agent name="evil">` and emit a degenerate two-tag text (the
 *   length cap cannot help — a newline is one code point). That is handled
 *   DOWNSTREAM by `splitMentionBlocks`' robustness policy: the per-kind tag
 *   count disagrees with the parsed-block count, so the display split returns
 *   the text VERBATIM with no blocks — no data loss, no corrupted bubble
 *   (pinned by the
 *   `a_hostile_agent_description_that_breaks_the_tag_degrades_to_verbatim`
 *   test).
 *
 *   Accepted divergence (do NOT "fix" it by case-folding in Rust): the Rust
 *   picker predicate `is_mentionable_name` requires `is_ascii_alphanumeric`,
 *   so it HIDES a name like `fe\u{212A}tch` — yet TS expansion WOULD match
 *   that name, since `"fe\u{212A}tch".toLowerCase() === "fetch"`. A
 *   divergence in the LOST-FEATURE direction (the picker won't offer it;
 *   a hand-typed `#fetch` still expands), never in the UNSAFE one — U+212A
 *   is tag-safe, so the block stays well-formed. Accepted as proportionate
 *   for a character nobody names resources with; case-folding before the
 *   grammar check in Rust is not worth the complexity.
 *
 *   ⚠ This holds ONLY while matching stays EXACT. If matching ever becomes
 *   prefix / fuzzy / substring (the PICKER already filters by substring, so
 *   the temptation is real), the argument COLLAPSES and the name MUST be
 *   validated or escaped at the injection point instead — otherwise
 *   `<agent name='x" onload="y'>` style breakout becomes reachable from
 *   repo-controlled files (a cloned repo's `.pi/mcp.json` /
 *   `.agents/agents/*.md`, whose server keys and frontmatter names are
 *   arbitrary JSON / YAML strings). The same applies if `toLowerCase()` ever
 *   gains a competitor that folds some tag-UNSAFE char ONTO a token char
 *   (U+212A shows such folds exist — it lands on `k`; today nothing unsafe
 *   folds onto `[a-z0-9]`, and that is part of what keeps the argument
 *   sound, so a change to the folding function is NOT a cosmetic change). The
 *   `expandMentions — tag safety of the interpolated catalog name` tests pin
 *   this consequence: an unsafe-named entry is UNMATCHABLE, so the text
 *   stays byte-identical. Treat a red pin there as "the invariant broke",
 *   not as a test to update.
 *
 *   Related: such a name is also UNNAMEABLE from the composer (no token can
 *   ever equal it), so the picker must NOT offer it — the Rust mention
 *   surfaces (`agent::mcp::config::server_infos`,
 *   `commands::agents::list_agent_definitions_for_space`) drop every name
 *   that cannot equal a token (`skills.rs`' `is_mentionable_name` — a name
 *   with a SPACE is tag-safe but still unnameable, so the picker predicate is
 *   strictly stronger than tag-safety) so a pick can never insert a dead
 *   token. Discovery itself must NOT filter (the harness's `agentName`
 *   dispatch and `list_agents` still honor unsafe-named definitions — see
 *   `agents.rs`).
 * - The agent block (the exact literal — the `—` is an EM DASH; the first
 *   line is `NAME — DESCRIPTION`, or `NAME` alone when the description is
 *   empty):
 *
 *     <agent name="NAME">
 *     NAME — DESCRIPTION
 *     The user has explicitly named the agent definition "NAME" for this
 *     request. Dispatch a subagent with agentName "NAME" to handle it
 *     (the definition's frontmatter defines its model, tools, and system
 *     prompt; your explicit tool params layer over the definition).
 *     </agent>
 *
 * - The MCP block (the exact literal; `SUMMARY` = the server's one-line
 *   summary — the `url` for HTTP, `command + args` for stdio). The summary IS
 *   capped, at the SAME budget as the agent `description`, BECAUSE it is
 *   equally repo-controlled: a cloned repo's `<cwd>/.pi/mcp.json` supplies
 *   `url` / `command` / `args` as arbitrary length-UNBOUNDED JSON strings
 *   under the very same threat model that made the picker filter names, so
 *   `{"mcpServers":{"github":{"command":"npx","args":["<10MB>"]}}}` has a
 *   mentionable name and one `#github` pick would otherwise flood the prompt
 *   AND the persisted message:
 *
 *     <mcp name="NAME">
 *     The user has explicitly named the MCP server "NAME" for this
 *     request. Connect to it via the mcp tool (mcp({ connect: "NAME" }))
 *     and use its tools. (Server summary: SUMMARY.)
 *     </mcp>
 *
 * - No matched token → return `text` UNCHANGED (byte-identical, invariant 1;
 *   the function is pure — invariant 2, no mutation of inputs).
 */
export function expandMentions(text: string, c: MentionCatalogs): string {
  // Collect hits from all three regexes in one stateless pass.
  // `matchAll` is used deliberately (NO shared-state `exec`/`test` loop —
  // the module's stale-`lastIndex` warning in `MENTION_RE`'s doc applies
  // to a manual loop; `matchAll` resets per call).
  const hits: { index: number; kind: MentionKind; token: string }[] = [];
  for (const m of text.matchAll(MENTION_RE))
    hits.push({ index: m.index, kind: "skill", token: m[1] });
  for (const m of text.matchAll(MCP_MENTION_RE))
    hits.push({ index: m.index + m[1].length, kind: "mcp", token: m[2] });
  for (const m of text.matchAll(AGENT_MENTION_RE))
    hits.push({ index: m.index + m[1].length, kind: "agent", token: m[2] });
  hits.sort((a, b) => a.index - b.index); // first-mention (text-position) order

  // Case-insensitive name lookup per kind (the skill-side `byName` map,
  // generalized — catalog order within a name group).
  const skillsByName = new Map<string, SkillInfo[]>();
  for (const s of c.skills) {
    const key = s.name.toLowerCase();
    const group = skillsByName.get(key);
    if (group) group.push(s);
    else skillsByName.set(key, [s]);
  }
  const agentsByName = new Map<string, AgentDefinitionDto[]>();
  for (const a of c.agents) {
    const key = a.name.toLowerCase();
    const group = agentsByName.get(key);
    if (group) group.push(a);
    else agentsByName.set(key, [a]);
  }
  const mcpByName = new Map<string, McpServerInfo[]>();
  for (const s of c.mcpServers) {
    const key = s.name.toLowerCase();
    const group = mcpByName.get(key);
    if (group) group.push(s);
    else mcpByName.set(key, [s]);
  }

  // Walk the hits in text order; a token matching NO catalog entry is
  // skipped (verbatim, invariant 5); one block per matched resource,
  // deduped ACROSS THE WHOLE RUN (a Set keyed by the catalog entry, like
  // the existing code).
  //
  // `emit` is GENERIC over the catalog-entry type, so each kind keeps its own
  // concrete `Map<string, T[]>` and its own block builder and NO cast is
  // needed anywhere (the single-kind `expandSkillMentions` this mirrors needs
  // none either; a union-typed `group` would have forced three). The `seen`
  // Set stays GLOBAL across the three kinds — kinds are disjoint by prefix, so
  // an entry can only ever be reached through one of them, and the identity
  // dedupe is exactly the old behaviour.
  const seen = new Set<unknown>();
  const blocks: string[] = [];
  const emit = <T,>(group: T[] | undefined, build: (entry: T) => string) => {
    if (group === undefined) return;
    for (const entry of group) {
      if (seen.has(entry)) continue;
      seen.add(entry);
      blocks.push(build(entry));
    }
  };
  for (const hit of hits) {
    if (hit.kind === "skill") emit(skillsByName.get(hit.token), buildBlock);
    else if (hit.kind === "agent")
      emit(agentsByName.get(hit.token), buildAgentBlock);
    else emit(mcpByName.get(hit.token), buildMcpBlock);
  }

  if (blocks.length === 0) return text; // NO hits matching any catalog → verbatim (invariant 1)
  return text + "\n\n" + blocks.join("\n\n");
}

/**
 * The budget for ANY repo-controlled free text interpolated into a mention
 * block (the agent `description` and the MCP `summary`) — the SKILL side's
 * number: `skills.rs` skips a skill whose frontmatter `description` exceeds
 * 1024 chars. Agents cannot be skipped at discovery (the harness's
 * `agentName` dispatch must keep honoring the definition) and MCP servers
 * cannot be skipped at the picker (the server stays live for the session and
 * connectable), so BOTH are TRUNCATED here instead — each is prompt-injected
 * AND comes from a repo-controlled file (`.agents/agents/*.md` frontmatter /
 * a cloned repo's `.pi/mcp.json`), so an uncapped value would flood the
 * prompt and the persisted message from a single `@name` / `#name`.
 *
 * Counts CODE POINTS (not UTF-16 units) so a cut never lands between the
 * halves of a surrogate pair, and uses the codebase's truncation marker
 * convention (`toolOutput.ts`'s private `truncate`: the ellipsis `…`).
 */
const MENTION_INTERPOLATED_BUDGET = 1024;

function capInterpolated(text: string): string {
  // `Array.from` iterates code points; `String.length` would count UTF-16
  // units (a CJK / emoji value would be cut ~2x too early, and the
  // `slice` could split a surrogate pair into a lone surrogate).
  const chars = Array.from(text);
  return chars.length > MENTION_INTERPOLATED_BUDGET
    ? chars.slice(0, MENTION_INTERPOLATED_BUDGET).join("") + "…"
    : text;
}

/** The agent block (the exact literal in `expandMentions`'s doc above). */
function buildAgentBlock(agent: AgentDefinitionDto): string {
  // The description is CAPPED before interpolation (see
  // `MENTION_INTERPOLATED_BUDGET`); the NAME needs no cap — no tag-UNSAFE
  // character can survive the exact match (invariant 6), and its size is
  // self-limiting (the picker row renders it).
  const description = capInterpolated(agent.description);
  const firstLine =
    description === "" ? agent.name : `${agent.name} — ${description}`;
  return (
    `<agent name="${agent.name}">` +
    `\n${firstLine}` +
    `\nThe user has explicitly named the agent definition "${agent.name}" for this` +
    `\nrequest. Dispatch a subagent with agentName "${agent.name}" to handle it` +
    `\n(the definition's frontmatter defines its model, tools, and system` +
    `\nprompt; your explicit tool params layer over the definition).` +
    `\n</agent>`
  );
}

/** The MCP block (the exact literal in `expandMentions`'s doc above). */
function buildMcpBlock(server: McpServerInfo): string {
  // The summary is CAPPED before interpolation, at the SAME budget as the
  // agent description: `url` / `command` + `args` are attacker-controlled,
  // length-unbounded JSON strings in a cloned repo's `.pi/mcp.json`. The NAME
  // needs no cap for the same reasons as the agent block's (invariant 6).
  const summary = capInterpolated(server.summary);
  return (
    `<mcp name="${server.name}">` +
    `\nThe user has explicitly named the MCP server "${server.name}" for this` +
    `\nrequest. Connect to it via the mcp tool (mcp({ connect: "${server.name}" }))` +
    `\nand use its tools. (Server summary: ${summary}.)` +
    `\n</mcp>`
  );
}

/** One mention block parsed out of an expanded user message. */
export interface MentionBlock {
  kind: MentionKind;
  /** The `name` attribute VERBATIM. */
  name: string;
  /** skill: the block's BODY (between the `References…` note and `</skill>`).
   *  agent / mcp: the block's hint content (between the tag lines). */
  body: string;
}

/**
 * The exact block shapes `expandMentions` produces (one match per block —
 * known-shape regexes, NOT a full HTML parser; the skill shape is the
 * EXISTING `SKILL_BLOCK_RE` unchanged).
 */
const AGENT_BLOCK_RE =
  /<agent name="([^"]*)">\n([\s\S]*?)\n<\/agent>/g;
const MCP_BLOCK_RE = /<mcp name="([^"]*)">\n([\s\S]*?)\n<\/mcp>/g;

/**
 * Split an expanded user message back into the user's own text and the
 * appended mention blocks (the inverse of `expandMentions`'s output shape —
 * the generalization of `splitSkillBlocks`;
 * display-only; the persisted/sent text is UNCHANGED).
 *
 * - `text` = the user's text with the blocks removed (trimmed of the
 *   surrounding blank line — the user's text verbatim, the existing policy).
 * - `blocks` = the parsed blocks in order (first-mention order).
 * - NO blocks → `{ text: <the input VERBATIM, UNCHANGED>, blocks: [] }`
 *   (byte-identical).
 * - Robust, not a full HTML parser (the EXISTING policy, generalized): the
 *   opening tags are counted PER KIND (`<skill[ >/]`, `<agent[ >/]`,
 *   `<mcp[ >/]`); the per-kind counts must equal the per-kind parsed-block
 *   counts (a stray tag that isn't a valid block makes them disagree → the
 *   whole text is verbatim). The matches (all kinds, index-sorted) must be
 *   CONTIGUOUS from the first (each `index` === the previous `end + 2` —
 *   the `\n\n` joiner) AND nothing may follow the last match (NO-LOSS). Any
 *   disagreement → `{ text, blocks: [] }` (verbatim, byte-identical).
 */
export function splitMentionBlocks(
  text: string,
): { text: string; blocks: MentionBlock[] } {
  type Parse = { index: number; end: number; kind: MentionKind; name: string; body: string };
  const parse = (kind: MentionKind, re: RegExp): Parse[] => {
    const out: Parse[] = [];
    for (const m of text.matchAll(re)) {
      out.push({
        index: m.index,
        end: m.index + m[0].length,
        kind,
        name: m[1],
        body: m[2].trim(),
      });
    }
    return out;
  };
  const parsed = [
    ...parse("skill", SKILL_BLOCK_RE),
    ...parse("agent", AGENT_BLOCK_RE),
    ...parse("mcp", MCP_BLOCK_RE),
  ];
  // Count the opening tags PER KIND (a valid block's opening tag matches
  // exactly once; a stray tag that isn't a valid block makes the counts
  // disagree → verbatim).
  const opensByKind: Record<MentionKind, number> = {
    skill: (text.match(/<skill[ >/]/g) ?? []).length,
    agent: (text.match(/<agent[ >/]/g) ?? []).length,
    mcp: (text.match(/<mcp[ >/]/g) ?? []).length,
  };
  if (parsed.length === 0) return { text, blocks: [] };
  for (const kind of Object.keys(opensByKind) as MentionKind[]) {
    if (parsed.filter((p) => p.kind === kind).length !== opensByKind[kind]) {
      return { text, blocks: [] };
    }
  }
  // The blocks are APPENDED after the user's text, joined by a blank line
  // (the `\n\n` joiner), so the matches (all kinds, index-sorted) must be
  // CONTIGUOUS from the first (each `index` === the previous `end + 2`) AND
  // nothing may follow the last match (NO-LOSS).
  parsed.sort((a, b) => a.index - b.index);
  for (let i = 1; i < parsed.length; i++) {
    if (parsed[i].index !== parsed[i - 1].end + 2) return { text, blocks: [] };
  }
  if (text.slice(parsed[parsed.length - 1].end).trim() !== "") {
    return { text, blocks: [] };
  }
  // The user's text is everything before the first block (trimmed of the
  // surrounding blank line — the user's text verbatim).
  const prefix = text.slice(0, parsed[0]!.index);
  return {
    text: prefix.replace(/\s+$/, ""),
    blocks: parsed.map(({ kind, name, body }) => ({ kind, name, body })),
  };
}
