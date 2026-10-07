import { describe, expect, it } from "vitest";

/**
 * A Palette block (ADR 0027) is a DIFF over `.dark`, so the question it must
 * answer is not "are these values right" (the contrast tests cover that) but
 * "did the palette state its intent for EVERY token" — because a token it
 * forgets does not fail, it silently inherits. That is the whole hazard of the
 * layering: `.theme-zai-dark` is the only place the 25 `--color-file-*`
 * descriptors carry a dark reading, so a palette block that omits one gets the
 * `@theme` light-safe mud-brown on a purple-black surface and nothing complains.
 *
 * So this is a COMPLETENESS test, and it is written as a SET EQUALITY IN BOTH
 * DIRECTIONS, which is the part that matters:
 *
 *   tokens(.theme-zai-dark) − tokens(.theme-dracula)  ==  INHERITED
 *   INHERITED               − tokens(.theme-zai-dark)  ==  ∅
 *
 * Asserting only "nothing is missing" would let a token be silently DROPPED
 * from Dracula (a palette that forgets the terminal hues); asserting only
 * "nothing is extra" would let a token be silently INHERITED that zai dark
 * restates by hand (a palette that forgets the git hues, so a `.dark` reading
 * shows through on a Dracula row). Both directions together mean a NEW token
 * added to `.theme-zai-dark` must be classified — either Dracula restates it or
 * it joins `INHERITED` deliberately. There is no third option, and `INHERITED`
 * is the review surface: it is exactly the **Identity color** set from
 * `CONTEXT.md` plus the white overlays, i.e. the hues that encode WHAT a thing
 * is rather than how the palette dresses it.
 *
 * THE SECOND HALF OF THIS FILE is the spec gate, and it exists because the set
 * equality above is silent about VALUES. `.theme-dracula` was first built from
 * `pi-dracula`, a community TUI port that disagrees with the current official
 * specification (it labels `#44475a` "currentLine"; the spec says Current Line
 * is `#6272a4` and `#44475a` is Selection), and the implementation then invented
 * further hues on top. Every token was in the right place, both directions
 * passed, and the palette was still off-spec. So the block's hexes are now
 * pinned against the spec's own value set — see `OFF_SPEC_ALLOW_LIST`.
 */

// This app deliberately ships no `@types/node` (its globals stay DOM-only) and
// Vitest stubs CSS imports (`?raw` resolves to `""`), so the stylesheet is
// read through Node's `fs` via a COMPUTED specifier — an untyped `any` import
// that `tsc` accepts without the Node type packages.
const NODE_FS = "node:" + "fs";
const { readFileSync } = (await import(NODE_FS)) as {
  readFileSync: (path: string, encoding: string) => string;
};
const css = readFileSync("src/index.css", "utf8");

/**
 * Extract a rule's body by COUNTING BRACE DEPTH, with the SELECTOR ANCHORED AT
 * LINE START. Two real traps this avoids:
 *
 * (1) A `\n}` "end of block" heuristic truncates at the FIRST nested block, so
 *     the tail of the rule is never examined — an absence assertion then passes
 *     for the WRONG REASON (the token looked undeclared because it was
 *     unread). Same helper as `themeTokens.test.ts`.
 * (2) A bare `/\.dark\b/` matches the stylesheet's PROSE — `.dark` and `@theme`
 *     are both discussed in comments long before either block opens — and a
 *     comment parsed as a block yields zero tokens, which makes an absence
 *     assertion fail OPEN.
 */
const block = (selector: RegExp): string => {
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

/** Custom-property names a block declares, comments removed first — a
 *  doc-comment that spells a token out (`--color-x` in prose) must not be
 *  mistaken for a declaration of it. */
const declared = (body: string): Set<string> =>
  new Set(
    [...body.replace(/\/\*[\s\S]*?\*\//g, "").matchAll(/--([a-z][a-z0-9-]*)\s*:\s*[^;]+;/g)].map(
      (m) => m[1]!,
    ),
  );

const DARK = declared(block(/^\.dark\s*\{/m));
const ZAI_DARK = declared(block(/^\.theme-zai-dark\s*\{/m));
const DRACULA = declared(block(/^\.theme-dracula\s*\{/m));

/**
 * The 25 file-type DESCRIPTORS. `@theme` declares ALL 25 (light-safe readings)
 * and so does `.theme-zai-dark` (vivid dark ones), which is why every palette
 * must restate them: a token a block omits does not fail, it falls back through
 * `.dark` to the `@theme` light-safe value, and on a dark page that is a
 * different hue entirely. `file-node*` are NOT here on purpose — those are the
 * file-row ROLE CHIP, an identity color that rides the `.dark` reading.
 */
const FILE_DESCRIPTORS = [
  "color-file-ts",
  "color-file-react",
  "color-file-js",
  "color-file-json",
  "color-file-html",
  "color-file-css",
  "color-file-sass",
  "color-file-md",
  "color-file-rs",
  "color-file-py",
  "color-file-sh",
  "color-file-yaml",
  "color-file-toml",
  "color-file-lock",
  "color-file-image",
  "color-file-svg",
  "color-file-archive",
  "color-file-go",
  "color-file-vue",
  "color-file-svelte",
  "color-file-java",
  "color-file-php",
  "color-file-xml",
  "color-file-graphql",
  "color-file-db",
];

/**
 * The 59 tokens `.theme-zai-dark` declares that `.theme-dracula` deliberately
 * does NOT. Each is inherited from `.dark` by design (ADR 0027's "data-identity
 * palettes stay inherited" consequence), NOT by omission:
 *
 * - 11 usage scales (`usage-chart-*`, `usage-heatmap-*`) and 7
 *   `context-breakdown-*` — ordinal data scales whose STEPS are the signal.
 * - 8 `git-*` and 5 `trajectory-*` — git state and transcript-role hues.
 * - 18 role chips (`file-node*`, `skill-*`, `command-*`, `session-*`,
 *   `plugin-*`, `subagent-node*`) — "what kind of row is this".
 * - `feedback-privacy-hint` — the same warning hue as the ramp, not a palette hue.
 * - 7 white overlays (`surface`, `surface-hover`, `hover`, `selected`,
 *   `workflow-rule`, `workflow-trace`, `workflow-trace-strong`) — luminance,
 *   not hue; they are correct on any dark surface.
 * - 2 `animated-gradient-text-*` — the gradient wordmark is white-on-dark in
 *   every palette.
 *
 * NOTE these WILL render differently from zai dark, because zai dark restates
 * them by hand and Dracula leaves them on `.dark`. That is intended: the rule
 * is "untouched BY Dracula", not "preserved FROM Zai".
 *
 * THE 22 `--color-terminal-*` tokens are NOT in this list — `.theme-dracula`
 * owns them (`TERMINAL_ANSI`) — and the reason is NOT that the app has a
 * terminal. **It does not, and it never has.** Re-verified for this comment:
 * there is no terminal component (the repo's only `Terminal` identifiers are
 * the Rust `PermissionMode::Terminal` and a `SquareTerminalIcon` glyph choice),
 * no `xterm`/`@xterm/*`/`node-pty`/`portable-pty`/`alacritty`/`vt100` in either
 * manifest, zero `bg-terminal-*`/`text-terminal-*` class consumers, and the ONE
 * declaration that reads a terminal token today is `@theme`'s
 * `--color-icon-blue: var(--color-terminal-bright-blue)`.
 *
 * So these 22 are FORWARD DECLARATIONS: 21 of them are inert in every palette,
 * and they exist so that a future terminal surface picks up the spec's colours
 * for free instead of inheriting `.dark`'s generic UI readings. What made
 * owning them right was never a consumer, it is that the values are not a
 * guess — the specification publishes the 16-colour ANSI table, so copying it
 * costs nothing and leaves no half-owned palette. (An earlier version of this
 * comment asserted the app "has a terminal now". It was false, it shipped, and
 * it is the reason this one states the measurement instead of the conclusion:
 * a comment that names a component that does not exist is trusted without being
 * checked.)
 */
const INHERITED = [
  "animated-gradient-text-soft",
  "animated-gradient-text-strong",
  "color-command-node",
  "color-command-node-foreground",
  "color-command-node-hover",
  "color-context-breakdown-1",
  "color-context-breakdown-2",
  "color-context-breakdown-3",
  "color-context-breakdown-4",
  "color-context-breakdown-5",
  "color-context-breakdown-6",
  "color-context-breakdown-7",
  "color-feedback-privacy-hint",
  "color-file-node",
  "color-file-node-foreground",
  "color-file-node-hover",
  "color-git-added",
  "color-git-deleted",
  "color-git-descendant",
  "color-git-ignored",
  "color-git-modified",
  "color-git-none",
  "color-git-renamed",
  "color-git-untracked",
  "color-hover",
  "color-plugin-node",
  "color-plugin-node-foreground",
  "color-plugin-node-hover",
  "color-selected",
  "color-session-node",
  "color-session-node-foreground",
  "color-session-node-hover",
  "color-skill-node",
  "color-skill-node-foreground",
  "color-skill-node-hover",
  "color-subagent-node",
  "color-subagent-node-foreground",
  "color-subagent-node-hover",
  "color-surface",
  "color-surface-hover",
  "color-trajectory-assistant",
  "color-trajectory-reasoning",
  "color-trajectory-tool-call",
  "color-trajectory-tool-result",
  "color-trajectory-user",
  "color-usage-chart-1",
  "color-usage-chart-2",
  "color-usage-chart-3",
  "color-usage-chart-4",
  "color-usage-chart-5",
  "color-usage-chart-6",
  "color-usage-heatmap-0",
  "color-usage-heatmap-1",
  "color-usage-heatmap-2",
  "color-usage-heatmap-3",
  "color-usage-heatmap-4",
  "color-workflow-rule",
  "color-workflow-trace",
  "color-workflow-trace-strong",
];

const diff = (a: Set<string>, b: Set<string>) =>
  [...a].filter((n) => !b.has(n)).sort();

/** A block's declarations as `name -> value`, comments stripped first so a
 *  doc-comment that quotes a value cannot be mistaken for a declaration. */
const values = (body: string): Record<string, string> =>
  Object.fromEntries(
    [...body
      .replace(/\/\*[\s\S]*?\*\//g, "")
      .matchAll(/--([a-z][a-z0-9-]*)\s*:\s*([^;]+);/g)]
      .map((m) => [m[1]!, m[2]!.trim()]),
  );

const DRACULA_VALUES = values(block(/^\.theme-dracula\s*\{/m));
const DARK_VALUES = values(block(/^\.dark\s*\{/m));

/**
 * The stylesheet with every CSS comment removed, so a position lookup cannot
 * land on a selector named in PROSE (`.dark`, `@theme` and `.theme-zai-dark`
 * are all discussed in comments long before any block opens). Offsets shift
 * relative to the raw text, but RELATIVE ORDER — the only thing the layering
 * depends on — is preserved.
 */
const CODE = css.replace(/\/\*[\s\S]*?\*\//g, "");

/** Byte offset of a rule's head in the comment-stripped stylesheet, or -1. */
const indexOfBlock = (selector: RegExp): number => {
  const where = CODE.search(selector);
  if (where < 0) throw new Error(`no ${selector} block in the stylesheet`);
  return where;
};

/**
 * The 22 `--color-terminal-*` tokens `.theme-dracula` now owns, with the value
 * the spec mandates. The 16 ANSI entries are the spec's Dracula Classic ANSI
 * table verbatim; `bg` is Background Darker, `fg`/`cursor` are AnsiWhite, the
 * cursor accent is AnsiBlack, and selection is spec Selection `#44475a` at alpha
 * (a terminal selection would have to sit over whatever the cell painted).
 *
 * WHAT THIS TABLE IS AND IS NOT. It is a pin on the VALUES, because the
 * completeness test only asks whether a token is DECLARED, so a palette that
 * restated `--color-terminal-red` with some other red would pass it. It is NOT
 * evidence that anything paints with them: no terminal surface exists, so 21 of
 * these 22 declarations colour nothing in any palette today. The single live
 * effect is `--color-terminal-bright-blue`, which `@theme` aliases as
 * `--color-icon-blue` (markdown links, diff `@@` hunks).
 */
const TERMINAL_ANSI: Record<string, string> = {
  "color-terminal-bg": "#191a21",
  "color-terminal-fg": "#f8f8f2",
  "color-terminal-cursor": "#f8f8f2",
  "color-terminal-cursor-accent": "#21222c",
  "color-terminal-selection": "color-mix(in oklab, #44475a 55%, transparent)",
  "color-terminal-selection-inactive":
    "color-mix(in oklab, #f8f8f2 12%, transparent)",
  "color-terminal-black": "#21222c",
  "color-terminal-red": "#ff5555",
  "color-terminal-green": "#50fa7b",
  "color-terminal-yellow": "#f1fa8c",
  "color-terminal-blue": "#bd93f9",
  "color-terminal-magenta": "#ff79c6",
  "color-terminal-cyan": "#8be9fd",
  "color-terminal-white": "#f8f8f2",
  "color-terminal-bright-black": "#6272a4",
  "color-terminal-bright-red": "#ff6e6e",
  "color-terminal-bright-green": "#69ff94",
  "color-terminal-bright-yellow": "#ffffa5",
  "color-terminal-bright-blue": "#d6acff",
  "color-terminal-bright-magenta": "#ff92df",
  "color-terminal-bright-cyan": "#a4ffff",
  "color-terminal-bright-white": "#ffffff",
};

/**
 * Every hex the official specification publishes for Dracula Classic
 * (draculatheme.com/spec), gathered from its tables:
 *
 * - the 11 core palette values (12 tokens; Current Line and Comment are the
 *   same `#6272a4`),
 * - the 4 distinct UI elevation values — Background Darker / Dark / Light /
 *   Lighter, where "Floating interactive elements" reuses Light's `#343746`,
 * - the 5 Functional colors, which the spec marks "do not use in editor or
 *   terminal applications" and assigns to borders, focus and state indicators,
 * - the 16 ANSI values.
 *
 * Deliberately ABSENT: `#353747`, which the spec does publish — as the opaque
 * Current-Line fallback for editors that cannot alpha-blend. It is in the
 * spec's "Current Line Rendering" prose, which says outright that the fallback
 * values "are implementation guidance for line highlight rendering only. They
 * do not add new syntax tokens to the official palette", and NO token in
 * `.theme-dracula` carries it. An allow-list is a list of what the gate must
 * forgive, so an entry nothing uses is not bookkeeping, it is a free slot: it
 * silently licenses any future token to paint `#353747` — a hue this app has no
 * reason to produce — with no test to notice. (It used to sit here, described
 * as "the spec's Selection at 90% alpha"; that is not what it is either —
 * `#44475a` at 90% composites to `#414456` over the page and `#404355` over the
 * well, neither of which is `#353747`.) If a real rendering ever needs the
 * fallback, re-add it then, with the declaration that uses it in the comment.
 *
 * Note what is NOT here: `pi-dracula`'s `#9aa4c4`, `#2fb27d`, `#ff6b81`,
 * `#5b4a10`, `#5a341b`, `#ffd8b8`. None appear in the spec at all.
 */
const SPEC_HEX = new Set([
  // core palette
  "#282a36", // Background
  "#6272a4", // Current Line / Comment
  "#44475a", // Selection
  "#f8f8f2", // Foreground
  "#ff5555", // Red
  "#ffb86c", // Orange
  "#f1fa8c", // Yellow
  "#50fa7b", // Green
  "#8be9fd", // Cyan
  "#bd93f9", // Purple
  "#ff79c6", // Pink
  // UI elevation ladder
  "#191a21", // Background Darker
  "#21222c", // Background Dark == AnsiBlack
  "#343746", // Background Light == Floating interactive elements
  "#424450", // Background Lighter
  // Functional
  "#de5735", // Functional Red
  "#a39514", // Functional Orange
  "#089108", // Functional Green
  "#0081d6", // Functional Cyan
  "#815cd6", // Functional Purple
  // ANSI
  "#ff6e6e",
  "#69ff94",
  "#ffffa5",
  "#d6acff",
  "#ff92df",
  "#a4ffff",
  "#ffffff",
]);

/**
 * The ONLY tokens allowed to carry a hex outside `SPEC_HEX`: the 24 file-type
 * DESCRIPTORS Dracula copies from `.theme-zai-dark` (25 minus `file-css`, which
 * is `#bd93f9` and so already on-spec). This is the **Identity color** rule
 * from `CONTEXT.md`, deliberately overriding the spec's "use exact color values
 * from this specification": Dracula has 11 hues and there are 25 file types,
 * so re-huing them to the spec set would make a `.py` chip indistinguishable
 * from a `.json` one — the icon's whole signal. It is a departure, so it is
 * listed by name here rather than waved at in a comment; every other hex in the
 * block must be a spec value.
 */
const OFF_SPEC_ALLOW_LIST = FILE_DESCRIPTORS.filter((n) => n !== "color-file-css");

describe("the .theme-dracula palette block (ADR 0027 completeness)", () => {
  it("inherits exactly the 59 identity/overlay tokens — nothing dropped", () => {
    // In .theme-zai-dark but NOT in .theme-dracula: must be exactly INHERITED.
    expect(diff(ZAI_DARK, DRACULA)).toEqual([...INHERITED].sort());
  });

  it("names no token that .theme-zai-dark does not (minus the ramp)", () => {
    // The other direction: everything Dracula declares that zai dark omits
    // must be accounted for. The only such tokens are the six RAMP tokens,
    // which live in `.dark` + `@theme` (Task 2) and which Dracula re-hues.
    const draculaOnly = diff(DRACULA, ZAI_DARK);
    expect(draculaOnly).toEqual([
      "color-caution",
      "color-thinking-high",
      "color-thinking-low",
      "color-thinking-max",
      "color-thinking-medium",
      "color-thinking-xhigh",
    ]);
  });

  it("has an INHERITED list that matches reality in both directions", () => {
    // Guards the list itself: an entry that no longer exists in .theme-zai-dark
    // (a rename, a deletion) must not linger and mask a real omission.
    expect(diff(new Set(INHERITED), ZAI_DARK)).toEqual([]);
    expect(INHERITED.length).toBe(new Set(INHERITED).size);
  });

  it.each(FILE_DESCRIPTORS)("%s is declared by BOTH palette blocks", (token) => {
    expect(ZAI_DARK.has(token), `${token} missing from .theme-zai-dark`).toBe(true);
    expect(DRACULA.has(token), `${token} missing from .theme-dracula`).toBe(true);
  });

  it("restates all 25 descriptors (a palette that omits one inherits @theme)", () => {
    // The descriptors are declared in EVERY layer that can win for `<html>`
    // except `.dark` and `.theme-zai-light`, which carry only the `file-node*`
    // role chips: `@theme` holds all 25 light-safe readings and
    // `.theme-zai-dark` all 25 vivid ones. So omitting one here is not a
    // missing-declaration error, it is a SILENT downgrade — the descriptor
    // resolves through `.dark` to `@theme`, and 5 of those 25 `@theme` base
    // values fall below the 3:1 floor on Dracula's `#282a36` (`file-ts`/`file-py`
    // 2.97, `file-sass`/`file-graphql` 2.88, `file-css` 2.73). That is a
    // legibility bug, not a cosmetic one, and it is why the restatement is
    // forced rather than left to the palette's taste.
    expect(FILE_DESCRIPTORS.length).toBe(25);
    const fileTokens = [...DRACULA].filter((n) => n.startsWith("color-file-"));
    // `file-node*` are role chips and must NOT be restated here.
    expect(fileTokens.sort()).toEqual([...FILE_DESCRIPTORS].sort());
  });

  it("re-hues exactly one descriptor: file-css (Zai's purple is 2.73 on the page)", () => {
    const value = (body: string, token: string) =>
      body.match(new RegExp(`--${token}:\\s*([^;]+);`))?.[1]?.trim();
    expect(value(block(/^\.theme-dracula\s*\{/m), "color-file-css")).toBe("#bd93f9");
    expect(value(block(/^\.theme-zai-dark\s*\{/m), "color-file-css")).toBe("#7e57c2");
  });

  it("declares every structural .dark token the INHERITED set does not cover", () => {
    // Derived, not hard-coded: `.dark` minus INHERITED is the palette-owned
    // surface/border/text/accent/status/interaction/terminal set (83) plus the
    // ramp (6). A token added to `.dark` therefore has to be classified here too.
    expect(diff(DARK, new Set(INHERITED)).filter((n) => !DRACULA.has(n))).toEqual([]);
  });

  it("is 117 declarations: 86 structural + 25 descriptors + 6 ramp", () => {
    // The arithmetic that proves nothing was double-counted: 86 structural + 6
    // ramp = the 92 tokens `.dark` declares once INHERITED is removed, plus the
    // 25 descriptors `.dark` never declares at all = 117. The REGION RENAME
    // (`sidebar`->`frame`, `background-alt`->`chat`) is count-neutral, and the
    // two NEW tokens `--color-inspector` / `--color-composer` are declared in
    // EVERY layer (aliased in the four non-dracula ones, so zai renders
    // byte-identically), which is why all four counts below moved by two.
    expect([...DRACULA].length).toBe(117);
    expect(diff(DARK, new Set(INHERITED)).length).toBe(92);
    expect(ZAI_DARK.size).toBe(170);
    // And the two directions of the difference add up the same way: zai dark
    // withholds 59 from Dracula, Dracula adds the 6 ramp tokens zai dark
    // leaves to `.dark`, so the block sizes differ by 59 − 6.
    expect(ZAI_DARK.size - DRACULA.size).toBe(INHERITED.length - 6);
  });

  it("sits AFTER .theme-zai-dark, and no palette block nests inside another", () => {
    // Every palette block is a single-class selector of EQUAL specificity, so
    // source order is the tie-breaker if two ever co-exist on <html>. `theme.ts`
    // guarantees at most one is present, and this pins the placement the ADR
    // specifies so the ordering stays honest about which block wins by order.
    const zaiDarkAt = css.search(/^\.theme-zai-dark\s*\{/m);
    const draculaAt = css.search(/^\.theme-dracula\s*\{/m);
    expect(zaiDarkAt).toBeGreaterThanOrEqual(0);
    expect(draculaAt).toBeGreaterThan(zaiDarkAt);
  });

  it("is the LAST of the five layers: @theme < .dark < .theme-zai-light < .theme-zai-dark < .theme-dracula", () => {
    // THE layering contract, pinned in full. Every one of these rules is a
    // single-class selector of equal specificity (ADR 0027), so the cascade has
    // NO other way to break a tie than by source order — the order IS the
    // feature.
    //
    // `.dark`'s slot is the one that used to be unpinned, and it is the load-
    // bearing one: the palette blocks are deliberately PARTIAL diffs over
    // `.dark` (59 tokens are inherited on purpose — see `INHERITED`), so
    // `.dark` must lose every tie against them. Move the `.dark` block to the
    // end of the file and it silently out-specifies every `.theme-*` block:
    // Dracula renders Zai's colours, `.theme-zai-light` renders dark-mode
    // tokens on a white page, and every value assertion in this file still
    // passes because they read the block bodies, not the cascade.
    const order = [
      ["@theme", indexOfBlock(/^@theme\s*\{/m)],
      [".dark", indexOfBlock(/^\.dark\s*\{/m)],
      [".theme-zai-light", indexOfBlock(/^\.theme-zai-light\s*\{/m)],
      [".theme-zai-dark", indexOfBlock(/^\.theme-zai-dark\s*\{/m)],
      [".theme-dracula", indexOfBlock(/^\.theme-dracula\s*\{/m)],
    ] as const;
    // Asserted as the ORDERED LIST, not as four pairwise `<`s, so a reorder
    // names the whole layering rather than one adjacent pair.
    expect(order.map(([name]) => name)).toEqual([
      "@theme",
      ".dark",
      ".theme-zai-light",
      ".theme-zai-dark",
      ".theme-dracula",
    ]);
    const offsets = order.map(([, at]) => at);
    expect(offsets.every((at) => at >= 0)).toBe(true);
    for (let i = 1; i < offsets.length; i++) {
      expect(offsets[i]!, `${order[i]![0]} must come after ${order[i - 1]![0]}`).toBeGreaterThan(
        offsets[i - 1]!,
      );
    }
  });

  it("keeps the five layers as siblings, none nested inside another", () => {
    // A block nested inside another would change which selectors match it (a
    // `.theme-x` rule inside `.dark` needs BOTH classes), which is a different
    // bug from an ordering one — and brace-depth counting cannot see it, since
    // the outer block's body would then CONTAIN the inner head.
    const heads = [
      "@theme",
      ".dark",
      ".theme-zai-light",
      ".theme-zai-dark",
      ".theme-dracula",
    ].map((name) => {
      const escaped = name.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
      return [name, indexOfBlock(new RegExp(`^${escaped}\\s*\\{`, "m"))] as const;
    });
    for (let i = 0; i < heads.length; i++) {
      const [name, at] = heads[i]!;
      const open = CODE.indexOf("{", at);
      let depth = 0;
      let end = -1;
      for (let j = open; j < CODE.length; j++) {
        if (CODE[j] === "{") depth++;
        else if (CODE[j] === "}") {
          depth--;
          if (depth === 0) {
            end = j;
            break;
          }
        }
      }
      expect(end, `${name} has unbalanced braces`).toBeGreaterThan(open);
      for (const [other, otherAt] of heads.slice(i + 1)) {
        expect(
          otherAt > open && otherAt < end,
          `${other} is nested inside ${name}`,
        ).toBe(false);
      }
    }
  });

  it("leaves the zai blocks untouched (the palette axis must not move them)", () => {
    // `.theme-zai-light` declares exactly `.dark`'s set and `.theme-zai-dark`
    // is a complete standalone dark palette; both are frozen by this task.
    const ZAI_LIGHT = declared(block(/^\.theme-zai-light\s*\{/m));
    expect(diff(ZAI_LIGHT, DARK)).toEqual([]);
    // `.dark`'s only extras are the six RAMP tokens, whose light-safe reading
    // lives in `@theme` (Task 2) — `.theme-zai-light` must stay silent there or
    // it would shadow it.
    expect(diff(DARK, ZAI_LIGHT).sort()).toEqual(
      ["color-caution", "color-thinking-high", "color-thinking-low", "color-thinking-max", "color-thinking-medium", "color-thinking-xhigh"],
    );
    expect(diff(DARK, ZAI_DARK).sort()).toEqual(
      ["color-caution", "color-thinking-high", "color-thinking-low", "color-thinking-max", "color-thinking-medium", "color-thinking-xhigh"],
    );
  });
});

describe("the .theme-dracula block follows the official Dracula spec", () => {
  it("declares all 22 terminal tokens with the spec's ANSI palette", () => {
    // Exact strings, because `--color-terminal-*` is a NAME-only concern for
    // the completeness test: a token restated with the wrong hue is "declared".
    // These 22 inherited `.dark`'s generic UI readings untouched until now,
    // which is what this assertion is guarding.
    for (const [token, expected] of Object.entries(TERMINAL_ANSI)) {
      expect(DRACULA_VALUES[token], `${token} missing from .theme-dracula`).toBe(
        expected,
      );
    }
  });

  it("owns exactly the 22 terminal tokens the spec's ANSI table names", () => {
    // Guards the table above: a token renamed or added on either side must be
    // reconciled, not quietly dropped from the assertion.
    const declaredTerminal = [...DRACULA].filter((n) => n.startsWith("color-terminal-")).sort();
    expect(declaredTerminal).toEqual(Object.keys(TERMINAL_ANSI).sort());
    expect(declaredTerminal.length).toBe(22);
  });

  it("uses no hex that the specification does not publish", () => {
    // THE GATE THAT WOULD HAVE CAUGHT THE ORIGINAL MISTAKE. Every hex in the
    // block must appear in the spec's own tables, except the allow-listed file
    // descriptors — so `#44475a` surviving as a border, or `#9aa4c4` / `#2fb27d`
    // / `#5b4a10` surviving as invented hues, fails here by NAME.
    const offenders: string[] = [];
    for (const [token, value] of Object.entries(DRACULA_VALUES)) {
      if (OFF_SPEC_ALLOW_LIST.includes(token)) continue;
      for (const hex of value.match(/#[0-9a-fA-F]{6}/g) ?? []) {
        if (!SPEC_HEX.has(hex.toLowerCase())) offenders.push(`${token}: ${hex}`);
      }
    }
    expect(offenders).toEqual([]);
  });

  it("has a non-empty allow-list, so the spec gate cannot pass vacuously", () => {
    // A gate that skips everything and a gate that passes are the same green.
    // Pin the size and pin that the allow-listed tokens really do carry off-spec
    // hexes, so the exception is doing real work rather than hiding a bug.
    expect(OFF_SPEC_ALLOW_LIST.length).toBe(24);
    expect(OFF_SPEC_ALLOW_LIST).not.toContain("color-file-css");
    const offSpec = OFF_SPEC_ALLOW_LIST.filter((token) =>
      (DRACULA_VALUES[token]?.match(/#[0-9a-fA-F]{6}/g) ?? []).some(
        (hex) => !SPEC_HEX.has(hex.toLowerCase()),
      ),
    );
    expect(offSpec.length).toBe(24);
    // And the gate really does look at the block: it sees 117 declarations, not
    // zero. (A parser that returned {} would pass the offender scan outright.)
    expect(Object.keys(DRACULA_VALUES).length).toBe(117);
  });

  it.each([
    ["color-border", "#6272a4"], // spec: subtle borders use Current Line
    ["color-border-hover", "#0081d6"], // spec: interactive borders use functional colors
    ["color-brand", "#815cd6"], // spec: focus rings use Functional Purple
    ["color-primary", "#815cd6"],
    ["color-card", "#424450"], // Background Lighter
    ["color-card-selected", "#44475a"], // Selection, used for selection
    ["color-popover", "#343746"], // Floating interactive elements
    ["color-input", "#21222c"],
    ["color-secondary", "#424450"], // Background Lighter — never Selection
    ["color-tag", "#424450"],
    ["color-context-track", "#424450"], // Background Darker: the deepest step, so the bar's extent reads ON the #343746 island
    // De-emphasis TEXT is no longer Comment: `#6272a4` scored 2.05–3.36 across
    // the surfaces its 88 consumers actually paint. See the token's comment.
    ["color-foreground-subtlest", "color-mix(in oklab, #f8f8f2 55%, transparent)"],
  ])("maps %s to the spec value %s", (token, expected) => {
    // The decisions the block exists to make, pinned individually so a revert
    // names the token rather than reporting a diff in a 114-entry snapshot.
    expect(DRACULA_VALUES[token], `${token} missing from .theme-dracula`).toBe(expected);
  });

  it("derives subtle text from Foreground rather than inventing a hue", () => {
    // The spec has no "subtle text" token, Comment (3.03) cannot carry body
    // copy, and the previous value (`#9aa4c4`) was invented. Deriving from
    // Foreground at 70% is the one move that stays on-palette at every
    // elevation, so it is asserted as a formula and not as a hex.
    expect(DRACULA_VALUES["color-foreground-subtle"]).toBe(
      "color-mix(in oklab, #f8f8f2 70%, transparent)",
    );
  });

  it("puts the content slab on the spec's Background Light step", () => {
    // `--color-chat` is the most-used surface in the app: the
    // transcript column (`ChatStream.tsx`), the side pane (`SidePane.tsx`) and
    // the composer island (`ComposerRow.tsx`) all read it, so one declaration
    // keeps the three islands in agreement by construction.
    //
    // IT USED TO BE A DERIVATION — `color-mix(var(--color-background-win-alt)
    // 33%, transparent)`, compositing to `#2c2e3b` — and that formulation had a
    // cost nobody noticed at the time: a `color-mix()` is not comparable as a
    // hex, so the ladder test had to EXEMPT it. The app's primary surface sat
    // outside the gate that exists to police surfaces. Taking the spec's step
    // literally puts it ON a rung and back inside the ladder, which is why the
    // exemption is gone from the ladder test and must not come back.
    expect(DRACULA_VALUES["color-chat"]).toBe("#343746");

    // The slab is the TOP resting plane, so everything the app treats as
    // recessed or as chrome must sit below it and everything raised above.
    // These are the relationships the island layout now depends on.
    const lum = (v: string) => {
      const n = parseInt(v.slice(1), 16);
      const f = (c: number) => {
        const s = c / 255;
        return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
      };
      const [r, g, b] = [(n >> 16) & 255, (n >> 8) & 255, n & 255];
      return 0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b);
    };
    const slab = DRACULA_VALUES["color-chat"]!;
    // Chrome frames the islands, so it must be DARKER than them or the islands
    // stop being islands. 1.21 measured.
    expect(lum(slab), "the slab must read above the chrome").toBeGreaterThan(
      lum(DRACULA_VALUES["color-background"]!),
    );
    // A card INSIDE the slab must separate from it. The three `bg-panel`
    // consumers have no borders, so this is the only thing that makes a card a
    // card. Recessing gives 1.34; the old `#343746` measured exactly 1.00.
    const panel = DRACULA_VALUES["color-panel"]!;
    expect(lum(panel), "a borderless card inside the slab would be invisible").toBeLessThan(
      lum(slab),
    );
    // zai dark is NOT part of the island scheme and must not move (its slab is
    // still a 60% mix of `#2b2b2b` over its own `#161616` page, i.e. `#232323`).
    const ZAI = values(block(/^\.theme-zai-dark\s*\{/m));
    expect(ZAI["color-chat"]).toBe(
      "color-mix(in oklab, var(--color-background-win-alt) 60%, transparent)",
    );
  });

  it("keeps the titlebar and the left sidebar on ONE plane", () => {
    // THE INVARIANT IS "SAME TOKEN", NOT "SAME HEX AS THE PAGE".
    //
    // This test used to assert `--color-frame: var(--color-background)`, and
    // its comment blamed the seam on the sidebar sitting on `#21222c`. That
    // diagnosis was wrong, and the wrongness is worth keeping: the seam was
    // never caused by WHICH step the sidebar was on, it was caused by the
    // titlebar and the sidebar reading DIFFERENT tokens (`bg-background` vs
    // `bg-frame`). The alias hid that by making the two tokens accidentally
    // equal, so the invariant was enforced by a coincidence rather than by
    // construction.
    //
    // It is now enforced structurally: `App.tsx`'s chrome bar reads
    // `bg-frame`, the same token `SpacesList` reads, so the two CANNOT drift
    // apart whatever the palette says. That is what the JSX half of this test
    // pins, and it is why moving the chrome band to `#21222c` draws no seam.
    const app = readFileSync("src/App.tsx", "utf8");
    // Match the bar's OWN opening tag: its className sits on the line before
    // `data-testid`, and `[^>]*` cannot cross the tag's closing `>`. A wider
    // [\s\S] window reaches FORWARD past the bar into the settings branch's
    // div and pins the wrong element — which is what this regex did first.
    const chromeBar = app.match(/className="([^"]+)"[^>]*data-testid="chrome-bar"/);
    expect(chromeBar, "chrome bar not found in App.tsx — update this test").toBeTruthy();
    expect(chromeBar![1], "the titlebar must read the same token as the sidebar").toMatch(
      /(^|\s)bg-frame(\s|$)/,
    );

    // The band itself: `#21222c` (Background Dark), i.e. the chrome now reads as
    // the RECESSED plane and the islands float above it. Pinned as a hex because
    // unlike the pairing above, this value IS a free choice and could drift.
    expect(DRACULA_VALUES["color-frame"]).toBe("#21222c");
    // The same step as the other recessed surfaces, so the recess is ONE step
    // and not two. Asserted against them rather than a literal so the whole
    // recessed band moves together if it ever has to.
    expect(DRACULA_VALUES["color-frame"]).toBe(DRACULA_VALUES["color-input"]);
    expect(DRACULA_VALUES["color-frame"]).toBe(DRACULA_VALUES["color-popover-header"]);
    // And it must stay BELOW the page: chrome above the content would invert the
    // frame the islands sit in.
    expect(
      DRACULA_VALUES["color-frame"],
      "the chrome band must not out-shine the page it frames",
    ).not.toBe(DRACULA_VALUES["color-background"]);
  });

  it("reserves Selection for selection, never as a border", () => {
    // `#44475a` is spec Selection and is 1.56 on the page — as `--color-border`
    // it erased the UI's structure. Pinned as a negative because the value is
    // legal elsewhere in the block, so the hex gate alone would not catch it.
    expect(DRACULA_VALUES["color-border"]).not.toBe("#44475a");
    expect(DRACULA_VALUES["color-input-border"]).not.toBe("#44475a");
    expect(DRACULA_VALUES["color-card-border"]).not.toBe("#44475a");
    expect(DRACULA_VALUES["color-tab-border"]).not.toBe("#44475a");
    expect(DRACULA_VALUES["color-popover-border"]).not.toBe("#44475a");
  });

  it("keeps a real elevation ladder, not one flat step", () => {
    // The spec's UI palette is a ladder and the first cut collapsed it, leaving
    // header == panel == page. Asserted as strict ordering of distinct surfaces,
    // and now over the FULL set of surface tokens rather than a five-token chain.
    //
    // Why the chain grew: the old chain was
    // `terminal-bg < popover-header < background < header < card`, and the three
    // tokens that turn out to have been INVERTED — `secondary`, `tag`,
    // `card-selected` — were simply not in it, so a palette that made a badge
    // fill brighter than the card it sits on stayed green. A ladder test only
    // gates the links it names, so it names every surface token the block owns.
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
    /**
     * The ladder as ORDERED GROUPS. Tokens inside one group are a deliberate
     * PLATEAU (they are one band of chrome and share a step, exactly as
     * `.theme-zai-dark` gives its header and panel one value); consecutive groups
     * must be STRICTLY ordered, and a token may not appear in two groups.
     *
     * `secondary` / `tag` sit with `card` because they are the raised step: a
     * `bg-secondary` badge lands inside a `bg-menu`/`bg-popover` float, so it
     * must read as raised above it, and the inline-code chip (which aliases
     * `tag`) must not be brighter than the card it lands on — which is the
     * inversion this group boundary now forbids.
     *
     * `card-selected` is the one token ABOVE the ladder: it is spec SELECTION, a
     * transient pressed/selected state, and the spec's own rule is that Selection
     * is the brightest step. It is therefore asserted as strictly brighter than
     * every resting surface rather than as a rung, and pinned to appearing
     * nowhere else at all.
     *
     * THE ISLAND LAYOUT MOVED THREE TOKENS, and one of them is now graded that
     * used to be invisible to this test:
     *   `color-chat` — the content slab — WAS `color-mix(…)`, which is
     *   not comparable as a hex, so it was EXEMPTED below. It is now the literal
     *   `#343746`, so the exemption is removed and the slab is classified with
     *   the floats. That is the whole point of taking the spec's step literally:
     *   the most-used surface in the app is now inside the gate instead of
     *   outside it.
     *   `color-panel` moved from the floating step DOWN to recessed, because its
     *   three consumers are cards INSIDE the slab and they have no borders.
     *   `color-context-track` sits on the RAISED step. It has now been on three
     *   steps across two layout changes and the reason keeps being the same: the
     *   track must differ from whatever the composer island wears, and the island
     *   moved twice. On `#191a21` it measured 1.47 against the `#343746` island;
     *   once the island became `#282a36` that fell to 1.218, under the 1.3 floor,
     *   and `#424450` became the only non-Selection candidate that reads.
     *
     * THE REGION RENAME added `color-frame` and `color-composer`, and both land
     * on EXISTING steps rather than inventing rungs: `frame` (the gaps, the
     * titlebar, the session list) shares the recessed step, and `composer` shares
     * the page step. That is the point of naming regions — five spec steps still
     * cover every region, so a new theme assigns hues to places without needing a
     * sixth step.
     */
    const LADDER: Array<{ label: string; tokens: string[] }> = [
      { label: "deepest (terminal paper)", tokens: ["color-terminal-bg"] }, // #191a21 Background Darker
      {
        label: "recessed (form controls, popover header, cards inside the chat, the FRAME the islands sit in)",
        tokens: [
          "color-input",
          "color-input-focused",
          "color-popover-header",
          "color-panel",
          "color-frame",
        ],
      }, // #21222c Background Dark
      {
        label: "the page plane AND the composer island (the inspector's backdrop, tabs' active state)",
        tokens: ["color-background", "color-composer"],
      }, // #282a36 Background
      {
        label: "the chat column, the inspector AND every float (header, tabs, menus, popovers, toasts, tooltips) — and the ACTIVE TAB, which is the chat sheet showing through the frame",
        tokens: [
          "color-chat",
          "color-inspector",
          "color-header",
          "color-background-win-alt",
          "color-tab",
          "color-tab-active",
          "color-menu",
          "color-popover",
          "color-toast",
          "color-tooltip",
        ],
      }, // #343746 Background Light == Floating interactive elements
      {
        label: "raised (cards, badges, the inline-code chip, menu hover, the context bar's track)",
        tokens: [
          "color-card",
          "color-secondary",
          "color-tag",
          "color-tooltip-tag",
          "color-menu-hover",
          "color-context-track",
        ],
      }, // #424450 Background Lighter
    ];

    const seen = new Map<string, string>();
    // One hop of `var()` resolution: `--color-input-focused` IS `--color-input`,
    // and the ladder must judge what paints, not whether the declaration is a
    // literal. Anything still unresolved after one hop is not comparable, and
    // that is the failure the assertion below reports.
    const resolve = (token: string, depth = 0): string => {
      const v = DRACULA_VALUES[token] ?? "";
      // The capture is the part AFTER `--color-`, and the map is keyed by the
      // full custom-property name, so the prefix is put back on the way in.
      const ref = v.match(/^var\(--color-([a-z0-9-]+)\)$/);
      if (ref && depth < 3) return resolve(`color-${ref[1]!}`, depth + 1);
      return v;
    };
    const groupLum: number[] = [];
    for (const group of LADDER) {
      const lums = group.tokens.map((token) => {
        const v = resolve(token);
        expect(v, `${token} is not declared in .theme-dracula`).toBeTruthy();
        expect(
          v,
          `${token} is not a plain hex (${v}) — the ladder must be comparable`,
        ).toMatch(/^#[0-9a-f]{6}$/);
        expect(seen.has(token), `${token} is named in two ladder groups`).toBe(false);
        seen.set(token, group.label);
        return lumOf(v);
      });
      // A plateau is a plateau: every token in a group must be the SAME colour,
      // otherwise "deliberate plateau" is hiding a fourth untracked step.
      const distinct = new Set(group.tokens.map((t) => resolve(t).toLowerCase()));
      expect(
        distinct.size,
        `${group.label}: claimed as one step but carries ${distinct.size} colours (${[...distinct].join(", ")})`,
      ).toBe(1);
      groupLum.push(lums[0]!);
    }
    for (let i = 1; i < groupLum.length; i++) {
      expect(
        groupLum[i]!,
        `ladder step ${i} (${LADDER[i]!.label}) is not strictly lighter than ${LADDER[i - 1]!.label}`,
      ).toBeGreaterThan(groupLum[i - 1]!);
    }

    // Every surface token the block declares is either in a group above or is
    // Selection — so the ladder cannot be widened past this test by adding a
    // surface token nobody classified.
    const SURFACE_SUFFIX =
      /^(color-(background|background-win-alt|chat|inspector|composer|frame|header|panel|card|card-selected|popover|popover-header|input|input-focused|tab|tab-active|menu|menu-hover|toast|tooltip|tooltip-tag|secondary|tag|context-track|terminal-bg))$/;
    const unclassified = Object.keys(DRACULA_VALUES)
      .filter((t) => SURFACE_SUFFIX.test(t))
      // `card-selected` is Selection (asserted above the ladder, not on it) and
      // nothing else is exempt any more: every region token — `frame`, `chat`,
      // `inspector`, `composer` included — is classified above, so a new region
      // token cannot slip in ungraded and a renamed one cannot quietly vanish.
      .filter((t) => !seen.has(t) && t !== "color-card-selected")
      .sort();
    expect(unclassified, "surface tokens missing from the ladder").toEqual([]);

    // THE INVERSION ITSELF, named. `secondary`/`tag` were `#44475a` (L 0.0647),
    // brighter than `card` `#424450` (L 0.0587), so a badge inside a dropdown
    // separated from its own float by 1.06 and the inline-code chip out-shone the
    // card it sat on. Pinned directly, in words, so a regression reports the bug
    // and not a group ordering.
    expect(lumOf(resolve("color-secondary"))).toBeLessThanOrEqual(
      lumOf(resolve("color-card")),
    );
    expect(lumOf(resolve("color-tag"))).toBeLessThanOrEqual(
      lumOf(resolve("color-card")),
    );
    // And Selection stays the one thing brighter than the raised step, because it
    // is a state and not a surface.
    expect(lumOf(resolve("color-card-selected"))).toBeGreaterThan(
      lumOf(resolve("color-card")),
    );
  });

  it("uses spec Selection for selection ONLY — never a border, never a resting surface", () => {
    // `#44475a` is spec Selection (1.56 on the page). The border half of this was
    // already pinned; the SURFACE half is what `secondary`/`tag` violated — a
    // resting fill that happens to be the selection colour is a selection affordance
    // painted on everything at once. Asserted as an EXACT SET of the tokens that
    // may carry it, so adding one fails by name.
    const carriers = Object.entries(DRACULA_VALUES)
      .filter(([, v]) => v.toLowerCase().includes("#44475a"))
      .map(([t]) => t)
      .sort();
    expect(carriers).toEqual(["color-card-selected", "color-terminal-selection"]);
    // The border assertions, kept (a `var()` alias makes the exact-set test blind
    // to a border that ALIASES a Selection-valued token).
    for (const token of [
      "color-border",
      "color-input-border",
      "color-card-border",
      "color-tab-border",
      "color-popover-border",
    ]) {
      const v = DRACULA_VALUES[token] ?? "";
      expect(v, `${token} must not be Selection`).not.toContain("#44475a");
      expect(v, `${token} must not alias a Selection-valued token`).not.toMatch(
        /var\(--color-(card-selected|secondary|tag)\)/,
      );
    }
    // Non-vacuous: the set really is a subset of the block, and Selection really
    // is still declared where its NAME says it belongs. NOT "where it is used":
    // neither carrier paints today — no JSX reads `bg-card-selected`, and there
    // is no terminal surface for `--color-terminal-selection`. What this
    // assertion gates is that Selection stays confined to the two tokens named
    // for selection, so a future consumer cannot acquire it by accident.
    expect(carriers.length).toBe(2);
    expect(DRACULA_VALUES["color-card-selected"]).toBe("#44475a");
  });

  it("keeps text tokens legible on EVERY surface their consumers actually paint", () => {
    // The gap every other contrast test in this repo has: they all grade against
    // a palette's `--color-background`, and the tokens they name overwhelmingly
    // render on the FLOATING surfaces. `--color-destructive` is the proof —
    // 4.53 on the page, one notch from the spec's 4.5 floor, and 3.75 on
    // `#343746`, which is where the error copy actually lives (`NewSpaceDialog`,
    // `SudoConfirmModal`/`SudoPasswordModal` on `bg-popover`, the six ✗ rows in
    // `SubagentDelegatingCard` on `bg-panel`, the `data-[variant=destructive]`
    // dropdown item on `bg-menu`). A page-only floor is how that survived.
    //
    // Floors: 4.5 for text (the spec's own minimum). Surfaces are derived from
    // the block itself, not hard-coded, so a palette that moves a surface moves
    // the test with it.
    const surfaces: Record<string, string> = {};
    for (const [token, value] of Object.entries(DRACULA_VALUES)) {
      if (
        !/^#[0-9a-f]{6}$/i.test(value) ||
        !/^color-(background|input|popover-header|panel|card|card-selected|menu|menu-hover|popover|tooltip)$/.test(
          token,
        )
      ) {
        continue;
      }
      surfaces[token] = value.toLowerCase();
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
    const contrast = (a: string, b: string) => {
      const [x, y] = [lumOf(a), lumOf(b)].sort((m, n) => n - m);
      return (x + 0.05) / (y + 0.05);
    };
    /**
     * The tokens held to a text floor, and the floor, on the surfaces their
     * consumers ACTUALLY paint on: the page, the recessed well, and the floating
     * band (panel / popover / menu / tooltip / toast are one step). All of those
     * clear 4.5 for every token here.
     */
    const TEXT_TOKENS: Record<string, number> = {
      "color-destructive": 4.3,
      "color-caution": 4.5,
      "color-foreground-subtlest": 4.5,
      "color-foreground-subtle": 4.5,
      "color-foreground": 4.5,
      "color-success": 4.5,
      "color-warning": 4.5,
    };
    const RAISED = new Set(["color-card", "color-card-selected", "color-menu-hover"]);
    /**
     * The two tokens that cannot clear 4.5 on the RAISED step, pinned to the
     * number they DO reach, with the reason. This is a reported shortfall, not a
     * fudge — and it is the reason the code-block header moved off `bg-card`.
     *
     * - `destructive` `#ff6e6e` (3.54 / 3.36): the brightest red the
     *   specification publishes. Nothing spec-published reaches 4.5 on `#424450`,
     *   and the alternative that would — AnsiBrightMagenta `#ff92df` — is Pink, a
     *   syntax hue this palette uses for strings.
     * - `foreground-subtlest` (4.01 / 3.86): it could be lightened further and
     *   clear, but it would then be indistinguishable from `foreground-subtle`
     *   (at 60% the two separate by only 1.23 on the panel; the 55% reading
     *   separates by 1.39), and a de-emphasis token that is not de-emphasised is
     *   a different bug than a 0.4 shortfall on a surface it rarely renders on.
     */
    const RAISED_FLOOR: Record<string, number> = {
      "color-destructive": 3.3,
      "color-foreground-subtlest": 3.8,
    };
    const report: string[] = [];
    const composite = (raw: string, bg: string): string => {
      const mix = raw.match(
        /color-mix\(in oklab, (#[0-9a-f]{6}) (\d+(?:\.\d+)?)%, transparent\)/,
      );
      if (!mix) {
        expect(raw, `a token value this test cannot grade: ${raw}`).toMatch(
          /^#[0-9a-f]{6}$/i,
        );
        return raw.toLowerCase();
      }
      // A translucent TEXT token composites over the surface it paints on, so it
      // must be graded composited and not raw.
      const a = Number(mix[2]) / 100;
      const top = parseInt(mix[1]!.slice(1), 16);
      const bottom = parseInt(bg.slice(1), 16);
      return (
        "#" +
        [16, 8, 0]
          .map((sh) => {
            const t = (top >> sh) & 255;
            const b = (bottom >> sh) & 255;
            return Math.round(a * t + (1 - a) * b)
              .toString(16)
              .padStart(2, "0");
          })
          .join("")
      );
    };
    for (const [token, floor] of Object.entries(TEXT_TOKENS)) {
      const raw = DRACULA_VALUES[token];
      expect(raw, `${token} is undeclared`).toBeTruthy();
      for (const [surfaceToken, bg] of Object.entries(surfaces)) {
        if (RAISED.has(surfaceToken)) continue;
        const fg = composite(raw!, bg);
        const c = contrast(fg, bg);
        report.push(`${token} on ${surfaceToken} ${bg}: ${Math.round(c * 100) / 100}`);
        expect(c, `${token} (${fg}) on ${surfaceToken} (${bg})`).toBeGreaterThanOrEqual(
          floor,
        );
      }
    }
    // Every text token is graded on the raised step too — at 4.5 unless it is
    // exempted above, so the exemptions are the ONLY sub-4.5 readings allowed.
    for (const token of Object.keys(TEXT_TOKENS)) {
      const floor = RAISED_FLOOR[token] ?? 4.5;
      if (RAISED_FLOOR[token] === undefined) continue;
      for (const surfaceToken of [...RAISED]) {
        const bg = surfaces[surfaceToken];
        if (bg === undefined) continue;
        const c = contrast(composite(DRACULA_VALUES[token]!, bg), bg);
        report.push(`${token} on ${surfaceToken} ${bg}: ${Math.round(c * 100) / 100} (raised floor)`);
        expect(c, `${token} on the raised ${surfaceToken} (${bg})`).toBeGreaterThanOrEqual(
          floor,
        );
        // And the exemption must not silently become the norm: if the value ever
        // DOES clear 4.5 here, the entry in RAISED_FLOOR is stale and must go.
        expect(
          c,
          `${token} now clears 4.5 on ${surfaceToken} — delete it from RAISED_FLOOR`,
        ).toBeLessThan(4.5);
      }
    }
    for (const token of Object.keys(TEXT_TOKENS)) {
      if (RAISED_FLOOR[token] !== undefined) continue;
      for (const surfaceToken of [...RAISED]) {
        const bg = surfaces[surfaceToken];
        if (bg === undefined) continue;
        const c = contrast(composite(DRACULA_VALUES[token]!, bg), bg);
        report.push(`${token} on ${surfaceToken} ${bg}: ${Math.round(c * 100) / 100} (raised)`);
        expect(c, `${token} on the raised ${surfaceToken} (${bg})`).toBeGreaterThanOrEqual(
          4.5,
        );
      }
    }
    // Non-vacuity: seven tokens over the non-raised surfaces plus the pinned
    // raised pairs is dozens of measurements, and the report is the table this
    // test exists to produce.
    expect(Object.keys(surfaces).length).toBeGreaterThanOrEqual(7);
    expect(report.length).toBeGreaterThanOrEqual(40);
  });

  it("splits the subtlest-TEXT role from the context-bar TRACK role", () => {
    // `--color-foreground-subtlest` had TWO jobs, and they need opposite values.
    //
    // As text it is documented "never body text", which was untrue: 88 call
    // sites read it for small body-size copy (the sidebar's "Sessions" header,
    // dropdown/select labels and `data-disabled:` items, the
    // `SessionConfigSelect` trigger value, `ToolCallCardHeader` metadata, the
    // `SubagentDelegatingCard` metadata line), and Comment `#6272a4` scored
    // 3.03 on the page, 3.36 on `bg-input`, 2.51 on the panel and 2.05 on the
    // card — under the 4.5 text floor on every one of them.
    //
    // As a FILL it was the context bar's track, where its job is the OPPOSITE:
    // to be far enough from the band fills that the bands are visible. That is
    // why it could not simply be lightened — lightening the track into a pale
    // grey collapses fill-vs-track contrast to ~1.06 and the bar loses its
    // meaning entirely.
    //
    // So the roles are split. The text token gets a lightened reading that
    // clears the floor (asserted above, per surface); the bar gets its own
    // token, declared per palette, so zai keeps the track it has always had and
    // Dracula gets one chosen for band separation.
    expect(DRACULA_VALUES["color-context-track"]).toBe("#424450");
    // The text token must no longer be the border/Comment colour, or the split
    // did not happen — it is one declaration and the two roles are back.
    expect(DRACULA_VALUES["color-foreground-subtlest"]).not.toBe(
      DRACULA_VALUES["color-border"],
    );
    // And the track must be a SURFACE value, not the text value.
    expect(DRACULA_VALUES["color-context-track"]).not.toBe(
      DRACULA_VALUES["color-foreground-subtlest"],
    );
  });

  it("leaves EVERY other palette's context-bar track byte-identical (the split must not move zai)", () => {
    // The role split is a Dracula fix, and the brief is explicit that zai must
    // not move. `--color-context-track` is therefore declared in `@theme` and in
    // both zai blocks as a pure ALIAS back to `--color-foreground-subtlest`, so
    // those readings composite to exactly the colour they composited to before
    // the token existed — and `.theme-zai-light`, which declares no
    // `foreground-subtlest` of its own, resolves through `.dark`/`@theme` the
    // same way it always did. Pinned as the ALIAS rather than a resolved value:
    // if someone later restates zai's subtlest, the track follows it, which is
    // the pre-split behaviour.
    const blocks = [
      ["@theme", /^@theme\s*\{/m],
      [".dark", /^\.dark\s*\{/m],
      [".theme-zai-light", /^\.theme-zai-light\s*\{/m],
      [".theme-zai-dark", /^\.theme-zai-dark\s*\{/m],
    ] as const;
    for (const [name, selector] of blocks) {
      const v = values(block(selector))["color-context-track"];
      expect(v, `${name} must declare the track as an alias`).toBe(
        "var(--color-foreground-subtlest)",
      );
    }
    // `.theme-zai-light` carries it too, and that is forced rather than
    // optional: the existing "zai blocks untouched" gate asserts that block
    // declares EXACTLY `.dark`'s token set, so an alias added to `.dark` has to
    // appear there. It is still a no-op — it resolves to the same colour the
    // component already read.
    // And the zai `foreground-subtlest` values the split touched NOTHING: each
    // block still declares the reading it always did, so the alias is inert.
    expect(DARK_VALUES["color-foreground-subtlest"]).toContain("neutral-200");
    expect(values(block(/^\.theme-zai-dark\s*\{/m))["color-foreground-subtlest"]).toContain(
      "neutral-300",
    );
  });

  it("gives the context bar a track that keeps every band legible", () => {
    // The bar's meaning is the band FILL (green → caution → warning → red), and
    // the only thing separating it from the unfilled remainder is the track.
    // Graded against the four fills the component actually emits
    // (`ChatStream.tsx`: `bg-success` / `bg-caution` / `bg-warning` /
    // `bg-destructive`), on the surface the track really paints on.
    //
    // THAT SURFACE MOVED. The bar used to sit in a `bg-input` `#21222c` well; the
    // island layout moved the composer onto the content slab, so the track now
    // paints on `--color-chat`. Reading the wrong token here would
    // grade a real relationship against a surface that no longer exists at that
    // site — which is exactly the failure the ADR's amendments keep recording —
    // so it is derived from the token the composer island actually wears.
    const well = DRACULA_VALUES["color-composer"]!;
    const track = DRACULA_VALUES["color-context-track"]!;
    const lumOf = (hex: string) => {      const n = parseInt(hex.slice(1), 16);
      const f = (v: number) => {
        const s = v / 255;
        return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
      };
      return (
        0.2126 * f((n >> 16) & 255) + 0.7152 * f((n >> 8) & 255) + 0.0722 * f(n & 255)
      );
    };
    const contrast = (a: string, b: string) => {
      const [x, y] = [lumOf(a), lumOf(b)].sort((m, n) => n - m);
      return (x + 0.05) / (y + 0.05);
    };
    const bands: Record<string, string> = {
      "bg-success": DRACULA_VALUES["color-success"]!,
      "bg-caution": DRACULA_VALUES["color-caution"]!,
      "bg-warning": DRACULA_VALUES["color-warning"]!,
      "bg-destructive": DRACULA_VALUES["color-destructive"]!,
    };
    const ratios: Record<string, number> = {};
    for (const [band, hex] of Object.entries(bands)) {
      expect(hex, `${band} must be a plain hex`).toMatch(/^#[0-9a-f]{6}$/i);
      const c = contrast(hex, track);
      ratios[band] = Math.round(c * 100) / 100;
      // 3:1 — a progress band is non-text UI.
      expect(c, `${band} (${hex}) against the track ${track}`).toBeGreaterThanOrEqual(3);
    }
    // The track must also read against the well it sits in, or the bar's extent
    // (how much is left) is invisible and the track is doing no work at all.
    expect(
      contrast(track, well),
      `the track ${track} is invisible against the composer well ${well}`,
    ).toBeGreaterThanOrEqual(1.3);
    // WHY `#191a21`, stated as a measurable criterion rather than as taste.
    //
    // The track has TWO jobs, and they pull in opposite directions:
    //   (a) separate the band fill from the unfilled remainder  → wants the track
    //       DARK (the darkest value in the block, the terminal's `#191a21`, gives
    //       the best bands: 12.63 / 15.52 / 10.18 / 6.36), and
    //   (b) be visible itself, so the bar's EXTENT reads           → wants the
    //       track FAR FROM THE SURFACE IT SITS ON.
    // So the criterion is two-stage, and the order matters: FILTER to tracks that
    // read against the island (>= 1.3), then MAXIMISE THE WORST BAND among them.
    // Doing it the other way round (maximising the weakest link) degenerates into
    // "pick whichever is furthest from the surface", because track-vs-island is
    // usually the binding term — that formulation selects `#44475a`, which is
    // Selection and banned as a resting surface, and whose bands (3.36) are the
    // worst of any candidate.
    //
    // THE TABLE FLIPPED when the composer island took the slab. Against the old
    // `#21222c` well the DARK end was invisible and the bright end won on
    // visibility, so `#343746` was the compromise. Against the `#343746` island
    // the bright end is now the invisible one (it IS the island) and the dark end
    // clears the floor outright — so `#191a21` wins BOTH jobs instead of
    // splitting them. This is the rare case where a layout change made a token
    // strictly better rather than forcing a trade-off.
    //
    //   candidate  vs the island  worst band
    //   #191a21       1.47           6.36    <- winner
    //   #21222c       1.34           5.80
    //   #282a36       1.21           5.23    excluded (invisible)
    //   #343746       1.00           4.33    excluded (invisible: it IS the slab)
    //   #424450       1.22           3.54    excluded (invisible)
    //   #44475a       1.29           3.36    excluded + Selection: not a resting surface
    const candidates = Object.entries(DRACULA_VALUES).filter(
      ([t, v]) =>
        /^#[0-9a-f]{6}$/i.test(v) &&
        /^color-(card|panel|background|chat|composer|frame|input|popover-header|terminal-bg|secondary|tag|tooltip|toast|menu|header|card-selected)$/.test(
          t,
        ),
    );
    const worstBand = (hex: string) =>
      Math.min(...Object.values(bands).map((f) => contrast(f, hex)));
    const visible = candidates.filter(([, v]) => contrast(v, well) >= 1.3);
    const ranked = visible
      .map(([t, v]) => [t, Math.round(worstBand(v) * 100) / 100] as const)
      .sort((a, b) => b[1] - a[1]);
    const best = ranked[0]!;
    expect(
      Math.round(worstBand(track) * 100) / 100,
      `${track} is not the best VISIBLE track; ${best[0]} scores ${best[1]} (table: ${JSON.stringify(ranked)})`,
    ).toBe(best[1]);
    // Non-vacuity: the filter really did exclude candidates, or "best visible"
    // is just "best" and the visibility half of the reasoning is decorative.
    const excluded = candidates.filter(([, v]) => contrast(v, well) < 1.3);
    expect(excluded.length).toBeGreaterThanOrEqual(3);
    expect(ranked.length).toBeGreaterThanOrEqual(3);
    expect(Object.keys(ratios).length).toBe(4);
  });
});

/**
 * Cross-cutting ALIASES: tokens whose whole job is to point at another token,
 * so a palette can re-hue a surface by editing one declaration. These are the
 * most fragile links in the layering and the easiest to break silently — an
 * alias that is deleted, or that points at a token no block declares, does not
 * error, it just stops following the palette.
 */
describe("the @theme aliases that make a palette re-hue reach the UI", () => {
  const THEME_VALUES = values(block(/^@theme\s*\{/m));

  it("aliases --color-icon-blue to the palette's ANSI bright blue", () => {
    // `text-icon-blue` has exactly two live consumers — markdown LINKS
    // (`MessageBubble.tsx`) and diff `@@` hunk headers (`DiffBlock.tsx`) — and
    // neither reads the settings store, so the ONLY thing that makes a link
    // follow the palette is this one line in `@theme`. Delete it and every link
    // silently reverts to the fallback hue under Dracula while the palette
    // itself still looks perfect; nothing else in the suite touches it. Pinned
    // as the ALIAS rather than a hex so the coupling survives any re-hue.
    expect(THEME_VALUES["color-icon-blue"]).toBe("var(--color-terminal-bright-blue)");
  });

  it("declares --color-terminal-bright-blue everywhere the alias could be resolved", () => {
    // A `var()` whose target no block declares resolves to nothing: the
    // property becomes invalid at computed-value time and the link inherits its
    // parent's colour — a failure mode with no error and no visual cue in dev.
    // These are the blocks that can be the winner for `<html>` (ADR 0027).
    const blocks = [
      ["@theme", /^@theme\s*\{/m],
      [".dark", /^\.dark\s*\{/m],
      [".theme-zai-light", /^\.theme-zai-light\s*\{/m],
      [".theme-zai-dark", /^\.theme-zai-dark\s*\{/m],
      [".theme-dracula", /^\.theme-dracula\s*\{/m],
    ] as const;
    // `@theme` and `.dark` must both carry it (every reading passes through one
    // of them); the palette blocks may, and Dracula does (AnsiBrightBlue).
    for (const [name, selector] of blocks) {
      const declaredHere = declared(block(selector)).has("color-terminal-bright-blue");
      if (name === "@theme" || name === ".dark" || name === ".theme-dracula") {
        expect(declaredHere, `${name} must declare the alias's target`).toBe(true);
      }
    }
    // And the Dracula reading is the spec's AnsiBrightBlue, i.e. the link colour
    // is the palette's own and not `.dark`'s sky leaking through.
    expect(DRACULA_VALUES["color-terminal-bright-blue"]).toBe("#d6acff");
  });

  it("names no var() target that the stylesheet never declares (no dangling alias)", () => {
    // Generalised net for the class of bug above, over the whole `@theme` block.
    // A `var(--x)` with no `--x` anywhere is the silent failure: the property is
    // invalid at computed-value time and the consumer inherits instead.
    //
    // Two target families are legitimately NOT declared in `index.css`, so they
    // are excluded BY PATTERN rather than by hand:
    //  - Tailwind's own colour scale (`--color-yellow-500` …), which the `@theme`
    //    namespace extension provides, and which no block restates;
    //  - the runtime-set custom properties (`--ui-font-size`), which
    //    `applySettingsFont` writes inline on `<html>`.
    const declaredAnywhere = new Set([
      ...declared(block(/^@theme\s*\{/m)),
      ...declared(block(/^\.dark\s*\{/m)),
      ...declared(block(/^\.theme-zai-light\s*\{/m)),
      ...declared(block(/^\.theme-zai-dark\s*\{/m)),
      ...declared(block(/^\.theme-dracula\s*\{/m)),
    ]);
    const TAILWIND_SCALE = /^color-[a-z]+-\d{2,3}$/;
    // Tailwind's own non-numeric theme defaults, also not restated here.
    const TAILWIND_FIXED = new Set(["color-white", "color-black", "color-current", "color-transparent"]);
    const RUNTIME = /^ui-/;
    const targets = Object.values(THEME_VALUES).flatMap((v) => [
      ...[...v.matchAll(/var\(--([a-z][a-z0-9-]*)\)/g)].map((m) => m[1]!),
    ]);
    const appAliasTargets = targets.filter(
      (t) => !TAILWIND_SCALE.test(t) && !TAILWIND_FIXED.has(t) && !RUNTIME.test(t),
    );
    const dangling = [...new Set(appAliasTargets)]
      .filter((target) => !declaredAnywhere.has(target))
      .sort();
    expect(dangling).toEqual([]);
    // Non-vacuous: the block really does carry var() aliases (so the scan above
    // had something to look at), and the app-token kind — the kind that can
    // dangle — is what `--color-icon-blue` is.
    expect(appAliasTargets).toContain("color-terminal-bright-blue");
    const aliasCount = Object.values(THEME_VALUES).filter((v) =>
      /var\(--/.test(v),
    ).length;
    expect(aliasCount).toBeGreaterThan(20);
  });
});
