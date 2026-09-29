import { describe, expect, it } from "vitest";
import { expandSkillMentions, splitSkillBlocks } from "./skills";
import type { SkillInfo } from "./tauri";

/** A full `SkillInfo` with `debug` defaults; override any field. */
function makeSkill(overrides: Partial<SkillInfo> = {}): SkillInfo {
  return {
    name: "debug",
    description: "A debug skill",
    path: "/s/.agents/skills/debug/SKILL.md",
    dir: "/s/.agents/skills/debug",
    scope: "space",
    body: "Step 1. Step 2.",
    ...overrides,
  };
}

/** The pi-format block for a skill (mirrors the function's exact literal). */
function block(skill: SkillInfo): string {
  return `<skill name="${skill.name}" location="${skill.path}">\nReferences are relative to ${skill.dir}.\n\n${skill.body}\n</skill>`;
}

/** Count the `<skill ` opening tags in a string. */
function countBlocks(text: string): number {
  return text.match(/<skill /g)?.length ?? 0;
}

describe("expandSkillMentions", () => {
  it("no_tokens_returns_the_text_unchanged", () => {
    expect(expandSkillMentions("hello world", [makeSkill()])).toBe(
      "hello world",
    );
  });

  it("expands_a_single_mention_in_pi_format", () => {
    const skill = makeSkill();
    expect(expandSkillMentions("fix $debug", [skill])).toBe(
      "fix $debug\n\n" +
        '<skill name="debug" location="/s/.agents/skills/debug/SKILL.md">\n' +
        "References are relative to /s/.agents/skills/debug.\n\n" +
        "Step 1. Step 2.\n" +
        "</skill>",
    );
  });

  it("skill_names_match_case_insensitively", () => {
    // A SYNTHETIC catalog: a real discovered catalog can never hold both
    // (discover_in_roots dedupes by lowercased name), but the matching rule
    // is case-insensitive on the skill-name side.
    const debugA = makeSkill({
      name: "DeBuG",
      path: "/s/.agents/skills/DeBuG/SKILL.md",
      dir: "/s/.agents/skills/DeBuG",
      body: "A",
    });
    const debugB = makeSkill({
      name: "DEBUG",
      path: "/s/.agents/skills/DEBUG/SKILL.md",
      dir: "/s/.agents/skills/DEBUG",
      body: "B",
    });
    const result = expandSkillMentions("x $debug", [debugA, debugB]);
    // Exactly two blocks (one per matched skill) ...
    expect(countBlocks(result)).toBe(2);
    // ... and each keeps the frontmatter name VERBATIM while the MATCHING was
    // case-insensitive.
    expect(result).toContain('<skill name="DeBuG"');
    expect(result).toContain('<skill name="DEBUG"');
  });

  it("uppercase_tokens_are_not_mentions", () => {
    const debug = makeSkill(); // name "debug"
    // Uppercase in the TOKEN is not a skill mention (regex is lowercase-only
    // — ZCode parity): the text passes through verbatim.
    expect(expandSkillMentions("$DEBUG", [debug])).toBe("$DEBUG");
    expect(expandSkillMentions("$DeBuG", [debug])).toBe("$DeBuG");
  });

  it("unmatched_token_passes_through", () => {
    const debug = makeSkill(); // name "debug"
    expect(expandSkillMentions("use $nope", [debug])).toBe("use $nope");
  });

  it("one_block_per_skill_even_when_mentioned_twice", () => {
    const debug = makeSkill();
    const result = expandSkillMentions("use $debug and $debug again", [debug]);
    expect(countBlocks(result)).toBe(1);
  });

  it("multiple_skills_in_first_mention_order", () => {
    const a = makeSkill({
      name: "a",
      path: "/s/.agents/skills/a/SKILL.md",
      dir: "/s/.agents/skills/a",
      body: "A",
    });
    const b = makeSkill({
      name: "b",
      path: "/s/.agents/skills/b/SKILL.md",
      dir: "/s/.agents/skills/b",
      body: "B",
    });
    // $a then $b then $a → blocks in order a, b (two total, deduped).
    const result = expandSkillMentions("x $a y $b z $a", [a, b]);
    expect(countBlocks(result)).toBe(2);
    const order = [...result.matchAll(/<skill name="([^"]*)"/g)].map(
      (m) => m[1],
    );
    expect(order).toEqual(["a", "b"]);
  });

  it("hyphenated_names_match", () => {
    const skill = makeSkill({
      name: "my-skill-x",
      path: "/s/.agents/skills/my-skill-x/SKILL.md",
      dir: "/s/.agents/skills/my-skill-x",
      body: "B",
    });
    expect(expandSkillMentions("run $my-skill-x", [skill])).toContain(
      '<skill name="my-skill-x"',
    );
  });

  it("tokens_embedded_in_words_still_expand_zcode_parity", () => {
    // ZCode parity: the regex is boundary-free on the LEFT, so a `$` mid-word
    // is still a mention — `a$debug` expands.
    const debug = makeSkill();
    expect(expandSkillMentions("a$debug", [debug])).toBe(
      "a$debug\n\n" + block(debug),
    );
  });

  it("empty_skills_list_returns_text_unchanged", () => {
    expect(expandSkillMentions("use $debug", [])).toBe("use $debug");
  });
});

describe("splitSkillBlocks", () => {
  it("no_blocks_returns_the_text_verbatim", () => {
    expect(splitSkillBlocks("hello world")).toEqual({
      text: "hello world",
      blocks: [],
    });
  });

  it("extracts_a_single_block", () => {
    const skill = makeSkill();
    const expanded = expandSkillMentions("fix $debug", [skill]);
    const { text, blocks } = splitSkillBlocks(expanded);
    expect(text).toBe("fix $debug");
    expect(blocks).toEqual([{ name: "debug", body: "Step 1. Step 2." }]);
  });

  it("multiple_blocks_in_order", () => {
    const a = makeSkill({
      name: "a",
      path: "/s/.agents/skills/a/SKILL.md",
      dir: "/s/.agents/skills/a",
      body: "A",
    });
    const b = makeSkill({
      name: "b",
      path: "/s/.agents/skills/b/SKILL.md",
      dir: "/s/.agents/skills/b",
      body: "B",
    });
    const expanded = expandSkillMentions("x $a y $b", [a, b]);
    const { text, blocks } = splitSkillBlocks(expanded);
    expect(text).toBe("x $a y $b");
    expect(blocks.map((block) => block.name)).toEqual(["a", "b"]);
  });

  it("a_block_is_not_confused_with_plain_text", () => {
    // A `<skill` literal WITHOUT the full valid shape (no `name`/`location`
    // attributes, no `References…` note) → verbatim, no blocks.
    expect(splitSkillBlocks("see <skill docs>")).toEqual({
      text: "see <skill docs>",
      blocks: [],
    });
  });

  it("splitSkillBlocks_text_after_a_mis_terminated_block_is_not_discarded", () => {
    // Repro A (realistic): a skill whose `body` CONTAINS a `</skill>` line
    // (e.g. a SKILL.md documenting the block format) followed by more
    // content. `expandSkillMentions` emits `…intro\n</skill>\nOUTRO\n</skill>`
    // — the regex matches ONE block (the counts agree: `</skill>` does not
    // increment `opens`), but the match is TRUNCATED at the embedded
    // `</skill>`. The text after the last match is NOT the expansion's
    // suffix → the whole text renders VERBATIM (no input text is lost).
    const skill = makeSkill({
      name: "x",
      path: "/x/SKILL.md",
      dir: "/x",
      body: "intro\n</skill>\nOUTRO",
    });
    const expanded = expandSkillMentions("use $x", [skill]);
    expect(splitSkillBlocks(expanded)).toEqual({
      text: expanded,
      blocks: [],
    });
  });

  it("splitSkillBlocks_a_pasted_block_followed_by_an_appended_block_is_verbatim", () => {
    // Repro B (exotic): the user PASTES a valid-shaped pi block mid-message
    // AND a real mention is expanded (appended). The counts agree (2==2),
    // but the matches are NOT contiguous (gap text between them) and the
    // appended block is not the text's suffix → verbatim (the gap text is
    // NOT silently dropped).
    const x = makeSkill({
      name: "x",
      path: "/x/SKILL.md",
      dir: "/x",
      body: "PASTED-BODY",
    });
    const pasted =
      '<skill name="x" location="/x/SKILL.md">' +
      "\nReferences are relative to /x." +
      "\n\nPASTED-BODY" +
      "\n</skill>";
    const userText = `part1 ${pasted} part2 $x`;
    const expanded = expandSkillMentions(userText, [x]);
    expect(splitSkillBlocks(expanded)).toEqual({
      text: expanded,
      blocks: [],
    });
  });
});
