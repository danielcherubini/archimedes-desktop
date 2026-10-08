import { describe, expect, it } from "vitest";

/**
 * The two RAMP token families — `--color-caution` (the context bar's 50–69%
 * band) and `--color-thinking-*` (the thinking-LEVEL indicator hue) — must be
 * real theme-overridable tokens, never raw Tailwind hues in a component.
 *
 * Two rules are pinned here.
 *
 * (1) A thinking LEVEL's hue is an IDENTITY color (`CONTEXT.md`), not a
 *     status: it answers "which level is this", exactly like `--color-file-*`
 *     answers "which file type is this". Riding it on the status tokens
 *     (`--color-warning` / `--color-destructive`) would make a max-effort
 *     model look like a failure, and would let a Palette re-hue the level.
 *     So the family is its own (`--color-thinking-low` … `-max`), the sibling
 *     of the `THINKING_GLYPHS` table in `src/lib/toolOutput.ts` that supplies
 *     the glyph for the same levels.
 *
 * (2) WHERE each reading lives, given the class model `src/lib/theme.ts`
 *     installs: zai dark is `.dark` + `.theme-zai-dark`, zai LIGHT is
 *     `.theme-zai-light` ALONE (no `.dark`), and no class at all reads the
 *     `@theme` block. So the LIGHT-SAFE value must live in `@theme` — that is
 *     what a light mode reads — and the dark reading in `.dark`, which zai
 *     dark also gets. `.theme-zai-light` must therefore NOT declare them: a
 *     second light value there would silently shadow the `@theme` one and
 *     leave the classless reading unlegible. This is the same structure the
 *     `--color-file-*` descriptors already use.
 *
 * The floors: a thinking glyph is a non-text UI indicator (WCAG 3:1), the
 * `caution` token is ALSO a text label (the context percentage sits in it),
 * so it is held to the 4.5:1 text floor.
 */

import { readResolvedIndexCss, stripCssComments } from "./cssSource";
/**
 * The resolved stylesheet (`src/index.css` with its `src/styles/*.css` imports
 * inlined — see `cssSource.ts`), with every CSS comment removed BEFORE anything
 * is parsed. `paletteCompleteness.test.ts` already did this and its
 * doc-comment explains why: a rule's selector can be matched in PROSE (`.dark`,
 * `@theme` and the ramp tokens are all discussed in comments long before any
 * block opens), and a comment parsed as a block yields ZERO tokens, which makes
 * an absence assertion pass for the wrong reason. The mirror hazard is the
 * positive one: a declaration that is COMMENTED OUT still matches the
 * declaration regex, so `--color-thinking-max` can read as declared with a
 * value the browser never sees. This file used to have both.
 */
const css = stripCssComments(readResolvedIndexCss());

/** Pin that the strip really is doing work — a no-op regex would silently put
 * the fail-open behaviour back. */
if (!/\/\*[\s\S]*?\*\//.test(readResolvedIndexCss())) {
  throw new Error("index.css has no comments: the comment-strip guard is vacuous");
}

/**
 * Extract a rule's body by COUNTING BRACE DEPTH. The `\n}` heuristic the
 * sibling `fileIconContrast.test.ts` uses truncates at the FIRST nested block
 * (any `@media` / `@keyframes` inside a selector would silently cut the rest
 * of it away), which for an absence assertion fails OPEN — the tokens would
 * look undeclared because they were never looked at.
 */
const block = (selector: RegExp): string => {
  // Anchored at line start (`^…{`) so the match cannot land on a prose
  // mention of the selector — `.dark` and `@theme` are both discussed in the
  // stylesheet's comments before either block opens.
  const head = css.match(selector);
  if (!head || head.index === undefined) throw new Error(`no ${selector} block`);
  const open = css.indexOf("{", head.index);
  if (open < 0) throw new Error(`no opening brace for ${selector}`);
  let depth = 0;
  for (let i = open; i < css.length; i++) {
    const ch = css[i];
    if (ch === "{") depth++;
    else if (ch === "}") {
      depth--;
      if (depth === 0) return css.slice(open + 1, i);
    }
  }
  throw new Error(`unbalanced braces after ${selector}`);
};

const declarations = (body: string) =>
  Object.fromEntries(
    [...body.matchAll(/--(color-[a-z0-9-]+)\s*:\s*([^;]+);/g)].map((m) => [
      m[1]!,
      m[2]!.trim(),
    ]),
  );

const THEME = declarations(block(/^@theme\s*\{/m));
const DARK = declarations(block(/^\.dark\s*\{/m));
const ZAI_LIGHT = declarations(block(/^\.theme-zai-light\s*\{/m));
const ZAI_DARK = declarations(block(/^\.theme-zai-dark\s*\{/m));
const DRACULA = declarations(block(/^\.theme-dracula\s*\{/m));

/** The six ramp tokens, and the Tailwind hue (as its v3 HEX) each `.dark` value
 *  REPLACES — asserting the pairing keeps this a token introduction rather than
 *  a re-design for zai dark.
 *
 *  NOT "the shipped rendering must not move by one byte", which is what this
 *  comment used to claim and which is false. This repo is Tailwind v4 and its
 *  palette is `oklch()` (`node_modules/tailwindcss/theme.css`:
 *  `--color-blue-500: oklch(62.3% 0.214 259.815)`), while the values below are
 *  the v3 hex equivalents. So a component that read `text-blue-500` was
 *  rendering `#2b7fff` and now renders `#3b82f6`. Decoding the real oklch and
 *  comparing, the worst case across these six is 0.0110 relative luminance
 *  (orange, the context band) and 0.19 contrast against the surface the glyph
 *  paints on — sub-perceptual, and every value still clears the floor this file
 *  enforces below, but this assertion pins the VALUE PAIRING, not pixel
 *  equality with what v4 rendered before. */
const RAMP: Record<string, string> = {
  // The context bar's 50–69% band, between `success` and `warning`.
  "color-caution": "#eab308", // = yellow-500
  // The thinking-level ramp (SessionConfigSelect's indicator glyph hue).
  "color-thinking-low": "#3b82f6", // = blue-500
  "color-thinking-medium": "#6366f1", // = indigo-500
  "color-thinking-high": "#a855f7", // = purple-500
  "color-thinking-xhigh": "#ec4899", // = pink-500
  "color-thinking-max": "#ef4444", // = red-500
};

const names = Object.keys(RAMP);

// The surfaces these are judged on. Light: the `@theme` card/white AND zai
// light's `#f8f8f8` page; dark: zai dark's `#161616` page.
const WHITE = "#ffffff";
const LIGHT_BG = "#f8f8f8"; // .theme-zai-light --color-background
const DARK_BG = "#161616"; // .theme-zai-dark --color-background

const parseHex = (h: string): [number, number, number] => {
  const n = parseInt(h.slice(1), 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
};
const relLum = ([r, g, b]: [number, number, number]) => {
  const f = (v: number) => {
    const s = v / 255;
    return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b);
};
const contrast = (a: string, b: string) => {
  const [x, y] = [relLum(parseHex(a)), relLum(parseHex(b))].sort((m, n) => n - m);
  return (x + 0.05) / (y + 0.05);
};

/** Hue angle (0–360) of a plain hex. */
const hue = (hex: string): number => {
  const [r, g, b] = parseHex(hex).map((v) => v / 255);
  const max = Math.max(r, g, b);
  const min = Math.min(r, g, b);
  const d = max - min;
  if (d === 0) return 0;
  const h =
    max === r ? ((g - b) / d) % 6 : max === g ? (b - r) / d + 2 : (r - g) / d + 4;
  return (h * 60 + 360) % 360;
};

/**
 * How far a hue sits from the COOL end. Cyan (hue 180) is the coolest colour
 * there is and red (hue 0) the hottest, so "hotter" is exactly "further from
 * 180", measured as an angle in 0–180.
 *
 * This is the definition the per-palette ramp assertion needs. The naive one —
 * "the hue angle increases" — is true of Zai (blue 217 → indigo 239 → purple
 * 271 → pink 330 → red 0) and FALSE of Dracula (cyan 190 → green 135 → yellow
 * 65 → orange 31 → red 0), yet both are unmistakably cool → hot. Asserting
 * increasing hue would make this file wrong about a palette that is entitled to
 * its own ramp ordering.
 */
const hotness = (hex: string): number => Math.abs(((hue(hex) + 360) % 360) - 180);

/** The five thinking LEVELS in magnitude order (the sibling of
 *  `THINKING_GLYPHS` in `src/lib/toolOutput.ts`). */
const THINKING_LEVELS = [
  "color-thinking-low",
  "color-thinking-medium",
  "color-thinking-high",
  "color-thinking-xhigh",
  "color-thinking-max",
];

/** The status family the ramp must never be mistaken for. */
const STATUS_TOKENS = [
  "color-success",
  "color-warning",
  "color-destructive",
  "color-caution",
];

/**
 * The tokens a thinking LEVEL must never print the same hex as.
 *
 * This used to be the four `STATUS_TOKENS`, and that was too narrow to express
 * the family's own rationale. The rule is "a level's hue is IDENTITY, not
 * status" — so any token that paints UI meaning is off-limits: the status set,
 * the interaction/diff hues a transcript row already uses to mean something,
 * the accent/brand fills, and the file descriptors. `@theme` and `.dark` carry
 * none of these as literal hexes (they are `var()` into Tailwind's scale), so
 * widening the set costs nothing there and catches everything in Dracula, where
 * every token is a literal.
 *
 * The 22 `--color-terminal-*` tokens are EXCLUDED, and deliberately so: the
 * spec's ANSI table reuses the palette's own syntax hues by construction (Ansi
 * Green IS `#50fa7b`), so a ramp built from spec hues shares hexes with the ANSI
 * set no matter which values it picks — the previous ramp already did (`#50fa7b`,
 * `#8be9fd`, `#f1fa8c`, `#ff5555` all appeared twice). No terminal token paints
 * UI today, so none can be mistaken for a status. That exclusion is the ONLY
 * one, and it is checked below so it cannot quietly widen.
 */
const MEANINGFUL_TOKENS = [
  "color-caution",
  "color-success",
  "color-warning",
  "color-destructive",
  "color-diff-added",
  "color-diff-removed",
  "color-interaction-ask-foreground",
  "color-interaction-confirmation-foreground",
  "color-brand",
  "color-primary",
  "color-accent",
  "color-idle-task",
  "color-foreground",
  "color-foreground-subtle",
  "color-foreground-subtlest",
  "color-border",
];

/**
 * The thinking-level ⇄ meaningful-token collisions that exist TODAY and are
 * accepted deliberately. Empty as of the ANSI-bright re-hue: every level now
 * prints a hex that no other UI-meaningful token in its block prints.
 *
 * The list is kept rather than deleted because it is the mechanism that makes
 * the gate specific — a NEW collision fails by name, and an entry that stops
 * being true fails as stale (asserted below), so the list cannot become a
 * blanket exemption and cannot rot.
 */
const KNOWN_STATUS_COLLISIONS: string[] = [];

/**
 * The levels of one block that print the SAME hex as another UI-meaningful
 * token of the same block, as `"<block>: <level> == <token> (<hex>)"`. Kept as
 * a function so the test can run it on a synthetic map and prove it detects a
 * collision at all — see the non-vacuity assertion in the test that reads the
 * allow-list.
 */
const statusCollisions = (
  blockName: string,
  tokens: Record<string, string>,
): string[] => {
  const out: string[] = [];
  for (const level of THINKING_LEVELS) {
    const v = tokens[level];
    if (v === undefined) continue;
    for (const status of MEANINGFUL_TOKENS) {
      if (status === level) continue;
      const s = tokens[status];
      if (s !== undefined && s.toLowerCase() === v.toLowerCase()) {
        out.push(`${blockName}: ${level} == ${status} (${v})`);
      }
    }
  }
  return out;
};

/**
 * Every block that ships its OWN reading of the ramp: `@theme` is the light (and
 * classless) reading, `.dark` is zai dark's, `.theme-dracula` is Dracula's.
 * `.theme-zai-light` and `.theme-zai-dark` are absent on purpose and are asserted
 * absent below — a value there would shadow the two readings above.
 */
const RAMP_BLOCKS: { name: string; tokens: Record<string, string> }[] = [
  { name: "@theme", tokens: THEME },
  { name: ".dark", tokens: DARK },
  { name: ".theme-dracula", tokens: DRACULA },
];

/**
 * The thinking-level ⇄ status-hue collisions that exist TODAY and are accepted
 * deliberately — see the test that reads this list for why clearing them is a
 * design decision rather than a fix. `@theme` and `.dark` contribute nothing:
 * their status tokens are `var()` into Tailwind's scale, so they share no
 * literal hex with the ramp. Entries are `"<block>: <level> == <status>"`.
 */

/** Text (4.5) for the token that carries a label, glyph (3) for the pure
 *  indicator hues. */
const FLOOR: Record<string, number> = {
  "color-caution": 4.5,
  ...Object.fromEntries(names.filter((n) => n !== "color-caution").map((n) => [n, 3])),
};

describe("the ramp tokens (--color-caution + --color-thinking-*)", () => {
  it.each(names)("%s is declared in @theme (the light-safe reading)", (name) => {
    expect(THEME[name], `${name} missing from @theme`).toMatch(/^#[0-9a-f]{6}$/);
  });

  it.each(names)("%s is declared in .dark (the dark reading)", (name) => {
    expect(DARK[name], `${name} missing from .dark`).toMatch(/^#[0-9a-f]{6}$/);
  });

  it("carries no raw Tailwind hue: every @theme value is a literal hex", () => {
    // A `var(--color-blue-500)` indirection would compile today and survive
    // every other assertion here, yet stay frozen across a palette swap.
    for (const name of names) {
      expect(THEME[name]).not.toContain("var(");
      expect(DARK[name]).not.toContain("var(");
    }
  });

  it.each(names)("%s keeps zai dark on the hue it replaces (value pairing)", (name) => {
    // The pairing, not pixel-identity: see the `RAMP` comment for why the v3 hex
    // and the v4 `oklch()` utility are not the same colour.
    expect(DARK[name]?.toLowerCase()).toBe(RAMP[name]);
  });

  it.each(names)(
    "%s is NOT declared in .theme-zai-light (light mode must inherit @theme)",
    (name) => {
      expect(ZAI_LIGHT[name], `${name} must not shadow @theme in .theme-zai-light`).toBeUndefined();
    },
  );

  it.each(names)("%s is NOT declared in .theme-zai-dark (.dark already covers it)", (name) => {
    expect(ZAI_DARK[name], `${name} belongs in .dark, not .theme-zai-dark`).toBeUndefined();
  });

  it.each(names)("%s clears its floor on white", (name) => {
    expect(contrast(THEME[name]!, WHITE)).toBeGreaterThanOrEqual(FLOOR[name]!);
  });

  it.each(names)("%s clears its floor on the zai-light background", (name) => {
    expect(contrast(THEME[name]!, LIGHT_BG)).toBeGreaterThanOrEqual(FLOOR[name]!);
  });

  it.each(names)("%s clears its floor on the zai-dark background", (name) => {
    expect(contrast(DARK[name]!, DARK_BG)).toBeGreaterThanOrEqual(FLOOR[name]!);
  });

  it("the thinking ramp is a real ramp, in EVERY palette reading", () => {
    // Formerly "in either reading" — and that comment was FALSE as written: the
    // test read only `@theme` and `.dark`, so `theme-dracula` appeared nowhere in
    // this file. Collapsing all five Dracula levels to one hex left 179 tests
    // green. There are three palette readings now (ADR 0027), so the ramp is
    // checked once per block that ships its own.
    for (const p of RAMP_BLOCKS) {
      const levels = THINKING_LEVELS.map((n) => p.tokens[n]);
      // (a) PRESENT: a level a palette forgets does not fail, it inherits the
      //     `@theme` light-safe hue onto a dark surface.
      for (const [i, name] of THINKING_LEVELS.entries()) {
        expect(
          levels[i],
          `${name} is undeclared in ${p.name} — it would silently inherit @theme`,
        ).toBeTruthy();
      }
      const hexes = levels.map((v) => v!.toLowerCase());
      // (b) MUTUALLY DISTINCT: two levels that render the same colour are
      //     indistinguishable, and the glyph already repeats for xhigh/max, so
      //     the hue is the only signal left.
      expect(new Set(hexes).size, `${p.name}: two thinking levels share a hue`).toBe(
        hexes.length,
      );
      // Every value must be a plain hex or this file cannot judge it.
      for (const [i, name] of THINKING_LEVELS.entries()) {
        expect(
          hexes[i],
          `${name} in ${p.name} is not a plain hex (${levels[i]}) — the ramp must be comparable by hue`,
        ).toMatch(/^#[0-9a-f]{6}$/);
      }
      // (c) ORDERED cool → hot, asserted as `hotness` strictly increasing — see
      //     `hotness` for why this is proximity-to-red and not "the hue angle
      //     increases". The zai-specific increasing-hue assertion is kept below.
      const heat = hexes.map(hotness);
      for (let i = 1; i < heat.length; i++) {
        expect(
          heat[i]!,
          `${p.name}: ${THINKING_LEVELS[i]} must read HOTTER than ${THINKING_LEVELS[i - 1]} (ramp hue order)`,
        ).toBeGreaterThan(heat[i - 1]!);
      }
      // The endpoints, named: the ramp must START cool and END on the hot end,
      // so a palette cannot satisfy "strictly ordered" by walking the wrong way.
      expect(heat[0]!, `${p.name}: -low must be a cool hue`).toBeLessThanOrEqual(60);
      expect(heat[4]!, `${p.name}: -max must sit on the hot end`).toBeGreaterThanOrEqual(150);
    }
  });

  it("orders the ramp cool → hot, matching the level's magnitude", () => {
    // The assertion this file shipped with, unchanged in substance and now
    // labelled for what it actually covered: the two ZAI readings (the shipped
    // dark one, and the light one). The per-palette version above is the one
    // that also looks at `.theme-dracula`.
    const hues = THINKING_LEVELS.map((n) => hue(DARK[n]!));
    expect(hues[0]).toBeLessThan(hues[1]!);
    expect(hues[1]!).toBeLessThan(hues[2]!);
    expect(hues[2]!).toBeLessThan(hues[3]!);
    expect(hues[4]!).toBeLessThan(30);
    // Same shape in the light reading.
    const lightHues = THINKING_LEVELS.map((n) => hue(THEME[n]!));
    expect(lightHues[0]).toBeLessThan(lightHues[1]!);
    expect(lightHues[1]!).toBeLessThan(lightHues[2]!);
    expect(lightHues[2]!).toBeLessThan(lightHues[3]!);
    expect(lightHues[4]!).toBeLessThan(30);
  });

  it("never RIDES a status token (--color-thinking-* is its own family)", () => {
    // The structural half of the family's rationale: `index.css` states the
    // levels are IDENTITY colors, so they get their own tokens rather than
    // riding `--color-warning`/`--color-destructive` — which would make a
    // max-effort model read as a failure AND let a Palette re-hue the level by
    // editing a status token. Checked as an ALIAS (what the CSS actually
    // promises), in every block that declares the ramp.
    for (const p of RAMP_BLOCKS) {
      for (const name of [...THINKING_LEVELS, "color-caution"]) {
        const v = p.tokens[name];
        if (v === undefined) continue;
        expect(
          /var\(--color-(success|warning|destructive|caution)\)/.test(v),
          `${name} in ${p.name} rides a status token: ${v}`,
        ).toBe(false);
      }
    }
    // Non-vacuous: the ramp really is declared in all three blocks.
    expect(RAMP_BLOCKS.length).toBe(3);
  });

  it("does not print the same hex as any other UI-meaningful token", () => {
    // The VALUE half of the family's rationale — "a max-effort model must not
    // look like a failure" — checked as a literal hex collision inside one block.
    //
    // This is the hole that let `.theme-dracula` paint FOUR of its five levels
    // with a status hex verbatim (`-medium` == success `#50fa7b`, `-high` ==
    // caution `#f1fa8c`, `-xhigh` == warning `#ffb86c`, `-max` == destructive
    // `#ff5555`), which is precisely the confusion the family's own doc-comment
    // says it exists to prevent. The ramp is now built from the spec's ANSI
    // BRIGHTS, which are hues no UI token in the block uses, so the allow-list is
    // EMPTY and every collision fails by name.
    //
    // Checked per block because `@theme` and `.dark` express their status tokens
    // as `var()` into Tailwind's scale and so share no literal hex with anything.
    for (const p of RAMP_BLOCKS) {
      const offenders = statusCollisions(p.name, p.tokens);
      const allowed = KNOWN_STATUS_COLLISIONS.filter((o) => o.startsWith(`${p.name}:`));
      // The offender strings carry the offending hex for the failure message, so
      // they are matched against the allow-list on the `block: level == token`
      // prefix — an entry cannot absorb a collision merely by naming the pair,
      // the pair has to be the one actually listed.
      const unexpected = offenders.filter(
        (o) => !allowed.some((a) => o.startsWith(`${a} (`)),
      );
      expect(unexpected, `${p.name}: new thinking/status hue collision`).toEqual([]);
    }

    // The gate is non-vacuous in two directions.
    // (1) A detector that always returned "no collisions" would be
    // indistinguishable
    //     from a clean palette, so it is run on a synthetic map where a collision
    //     is forced — with the allow-list EMPTY, so this proves the collision is
    //     reported as an OFFENDER and not swallowed by an exemption.
    const synthetic = {
      ...DRACULA,
      "color-thinking-low": DRACULA["color-success"]!,
    };
    expect(
      statusCollisions(".theme-dracula", synthetic),
      "the collision detector must report a forced collision",
    ).toContain(
      `.theme-dracula: color-thinking-low == color-success (${DRACULA["color-success"]})`,
    );
    // (2) The clean result is measured, not assumed: the block really does carry
    //     all five levels and all the meaningful tokens, so a zero-offender result
    //     came from comparing five things against many, not from an empty input.
    const draculaLevels = THINKING_LEVELS.filter((n) => DRACULA[n] !== undefined);
    expect(draculaLevels.length).toBe(THINKING_LEVELS.length);
    const compared = MEANINGFUL_TOKENS.filter((n) => DRACULA[n] !== undefined);
    expect(compared.length).toBeGreaterThan(10);
    // And every meaningful token named above must be a token the stylesheet can
    // actually resolve, or the set has silently shrunk.
    // The four status tokens are the CORE of the forbidden set — the whole
    // rationale of the family is about them — so pinning that they survived into
    // the wider list is what keeps "wider" from meaning "different".
    for (const status of STATUS_TOKENS) {
      expect(MEANINGFUL_TOKENS).toContain(status);
    }
    expect(MEANINGFUL_TOKENS).toContain("color-interaction-ask-foreground");

    // The allow-list may only name real blocks, levels and tokens — so it cannot
    // quietly accrete entries that match nothing.
    for (const entry of KNOWN_STATUS_COLLISIONS) {
      const [blockName, pair] = entry.split(": ");
      const [level, status] = pair.split(" == ");
      expect(RAMP_BLOCKS.some((b) => b.name === blockName), `stale block in list: ${entry}`).toBe(true);
      expect(THINKING_LEVELS).toContain(level);
      expect(MEANINGFUL_TOKENS).toContain(status);
    }
    // And an entry may not outlive the collision it excuses: each one must be
    // live in the block it names, or it is a permanent exemption wearing a
    // temporary one.
    for (const entry of KNOWN_STATUS_COLLISIONS) {
      const [blockName, pair] = entry.split(": ");
      const [level, status] = pair.split(" == ");
      const block = RAMP_BLOCKS.find((b) => b.name === blockName)!;
      expect(
        statusCollisions(blockName, block.tokens),
        `stale allow-list entry (the collision is gone): ${entry}`,
      ).toContain(`${blockName}: ${level} == ${status} (${block.tokens[level]})`);
    }
  });

  it("keeps the Dracula ramp legible as a glyph on both surfaces it renders on", () => {
    // The ramp's own floors, re-measured for the palette that owns them. The
    // thinking glyph rides the composer's config row (`bg-input`, `#191a21` under
    // Dracula) and the page; a glyph is non-text UI, so 3:1 is the floor. The
    // previous ramp's hottest level sat at 4.53 on the page and 3.75 on a panel,
    // so the numbers below are the ones the re-hue was chosen against.
    const surfaces: Array<[string, string]> = [
      ["the Dracula page", DRACULA["color-background"]!],
      ["the Dracula composer well (bg-input)", DRACULA["color-input"]!],
    ];
    for (const [label, bg] of surfaces) {
      expect(bg, "the surfaces must be plain hexes to be comparable").toMatch(
        /^#[0-9a-f]{6}$/,
      );
      for (const name of THINKING_LEVELS) {
        const v = DRACULA[name]!;
        expect(
          contrast(v, bg),
          `${name} (${v}) on ${label} ${bg}`,
        ).toBeGreaterThanOrEqual(3);
      }
    }
  });

  it("keeps the Dracula ramp ordered by PERCEIVED heat, not just by hue angle", () => {
    // `hotness` (proximity to red) is what orders the ramp, and it is an ANGLE:
    // it says `#ff92df` (318°) is nearer red than `#ffffa5` (60°) without saying
    // whether the two are tellable apart at 14px. So this pins the two things
    // `hotness` cannot express.
    //
    // (1) ADJACENT HUE SEPARATION. The naive version of this assertion is "the
    //     luminance contrast between adjacent levels must exceed 1.3", and that
    //     is the WRONG metric — it fails a ramp that is perfectly readable, because
    //     a cool→hot ramp necessarily passes through yellow, the BRIGHTEST hue
    //     there is, so luminance rises for the first half and falls for the second.
    //     (It also fails the shipped zai ramp, which is not a bug.) Hue distance
    //     is what a glyph is recognised by, so that is what is required: ≥ 25°
    //     between neighbours, measured the short way round the wheel.
    // (2) THE HOT END IS NOT THE PALE END. `hotness` is maximised by any pure red
    //     INCLUDING a near-white one, so without this a palette could satisfy the
    //     whole ramp by walking from saturated cyan to washed-out pink.
    const hexes = THINKING_LEVELS.map((n) => DRACULA[n]!.toLowerCase());
    const hueOf = (hex: string) => {
      const [r, g, b] = parseHex(hex).map((v) => v / 255);
      const max = Math.max(r, g, b);
      const min = Math.min(r, g, b);
      const d = max - min;
      if (d === 0) return 0;
      const h =
        max === r ? ((g - b) / d) % 6 : max === g ? (b - r) / d + 2 : (r - g) / d + 4;
      return (h * 60 + 360) % 360;
    };
    const hueDistance = (a: string, b: string) => {
      const d = Math.abs(hueOf(a) - hueOf(b));
      return Math.min(d, 360 - d);
    };
    for (let i = 1; i < hexes.length; i++) {
      expect(
        hueDistance(hexes[i - 1]!, hexes[i]!),
        `${THINKING_LEVELS[i - 1]} (${hexes[i - 1]}) and ${THINKING_LEVELS[i]} (${hexes[i]}) are less than 25° apart in hue — indistinguishable as a 14px glyph`,
      ).toBeGreaterThanOrEqual(25);
    }
    const lumOf = (hex: string) => {
      const n = parseInt(hex.slice(1), 16);
      const f = (v: number) => {
        const s = v / 255;
        return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
      };
      return (
        0.2126 * f((n >> 16) & 255) + 0.7152 * f((n >> 8) & 255) + 0.0722 * f(n & 255)
      );
    };
    expect(
      lumOf(hexes[4]!),
      `-max (${hexes[4]}) must not be brighter than -low (${hexes[0]}) — the hot end of the ramp is the saturated end, not the washed-out one`,
    ).toBeLessThan(lumOf(hexes[0]!));
    // And `max` specifically must be a RED, not a pink that merely scores 180:
    // red is the hue the family is allowed to end on because it is the palette's
    // own hot end.
    expect(hueOf(hexes[4]!)).toBeLessThan(15);
  });
});
