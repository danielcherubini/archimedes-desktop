import { describe, expect, it } from "vitest";

/**
 * The file-type DESCRIPTOR hues must clear 3:1 against the surface they sit
 * on, in EVERY theme. They are icon glyphs (16px, non-text UI), so 3:1 is the
 * WCAG non-text minimum — but they are also the only thing distinguishing a
 * `.py` chip from a `.json` one, so a pale hue is not just a contrast failure,
 * it is the whole signal disappearing.
 *
 * This is the trap ZCode walks into: its chips are Material Icon Theme SVGs
 * with FIXED fills (`#ffca28` javascript amber), which read fine on its dark
 * surface and vanish on a white one. Our tokens are theme-overridable, so the
 * base values carry light-safe shades and each palette block carries the vivid
 * originals for ITS surface.
 *
 * Hence a TABLE, not a constant. Every `.theme-*` palette that ships a dark
 * surface of its own is a row here, because "dark" is not one background:
 * `.theme-zai-dark` renders on neutral `#161616`, `.theme-dracula` on the
 * purple-black `#282a36`, and a hue that clears one does not clear the other —
 * Zai's `--color-file-css` `#7e57c2` is 3.47 on the first and 2.73 on the
 * second, which is why Dracula re-reads it. A new palette adds a row; it must
 * never be judged against someone else's surface.
 *
 * Which is also why the parser is LOUD. It used to match only bare `#rrggbb`,
 * so a descriptor written as `color-mix(…)`, `var(…)`, `rgb(…)` or an 8-digit
 * hex was silently dropped, the merged lookup fell back to the `@theme` base,
 * and the token was graded on a surface it never touches — the exact failure
 * this file exists to prevent, arriving by parser instead of by palette. Every
 * value is now parsed and RESOLVED to the opaque colour that actually paints
 * (alpha composited over the row's own surface); anything unresolvable throws
 * with the token name.
 */

// This app deliberately ships no `@types/node` (its globals stay DOM-only) and
// Vitest stubs CSS imports (`?raw` resolves to `""`), so the stylesheet is
// read through Node's `fs` via a COMPUTED specifier — an untyped `any` import
// that `tsc` accepts without the Node type packages.
const NODE_FS = "node:" + "fs";
const { readFileSync } = (await import(NODE_FS)) as {
  readFileSync: (path: string, encoding: string) => string;
};
/**
 * The stylesheet, with every CSS comment removed BEFORE anything is parsed.
 *
 * Parsing the raw text fails OPEN in two ways, both of which this file used to
 * have:
 *  - `block()` can land on a selector named in PROSE (`.dark`, `@theme` and
 *    `.theme-dracula` are all discussed in comments long before any block
 *    opens), and a comment parsed as a block yields ZERO tokens — so an absence
 *    assertion passes for the wrong reason;
 *  - worse for the positive assertions, a declaration that is COMMENTED OUT
 *    still matches the token regex. Comment out `--color-file-ts: #0288d1;`
 *    inside the block's own doc-comment and the token reads as declared with
 *    the value it no longer has, so the contrast suite grades a hue that is not
 *    on screen. That is exactly the "judged against someone else's surface"
 *    failure this file exists to prevent.
 * `paletteCompleteness.test.ts` already stripped; the siblings kept the bug.
 */
const css = readFileSync("src/index.css", "utf8").replace(
  /\/\*[\s\S]*?\*\//g,
  "",
);

/** The strip is load-bearing, so pin that it actually removed something — a
 * no-op regex would silently restore the fail-open behaviour above. */
if (!/\/\*[\s\S]*?\*\//.test(readFileSync("src/index.css", "utf8"))) {
  throw new Error("index.css has no comments: the comment-strip guard is vacuous");
}

/**
 * Extract a rule's body by COUNTING BRACE DEPTH, selector ANCHORED AT LINE
 * START. The `\n}` heuristic this file used to use truncates at the first
 * nested block, and a bare `/\.theme-x/` can land on a prose mention of the
 * selector in the stylesheet's comments — both silently shrink the set of
 * tokens that were actually looked at. (Comments are already gone from `css`,
 * so the second trap cannot fire here.)
 */
const BLOCK = (selector: RegExp) => {
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

/**
 * Every `--color-file-*` declaration in a block, as `name -> raw value`.
 *
 * This parser used to be `/--color-(file-[a-z-]+):\s*(#[0-9a-f]{6})/g`, i.e. it
 * SILENTLY DROPPED any value that was not a bare 6-digit hex. A descriptor
 * written as `color-mix(…)`, `var(…)`, `rgb(…)` or an 8-digit hex therefore
 * vanished from the map, the merged lookup fell back to the `@theme` base, and
 * the descriptor was graded against a surface it does not sit on — precisely the
 * failure this file's doc-comment says it exists to prevent. Setting
 * `--color-file-ts` to a `color-mix(…)` passed all 128 tests.
 *
 * So: parse EVERY value, and RESOLVE it at the point of use against the surface
 * it is being judged on. Anything this file cannot resolve throws with the token
 * name — an unresolvable declaration must never read as "passed". Skipping is
 * only allowed for the ROLE CHIPS listed in `ROLE_CHIPS`, by name.
 */
const tokens = (body: string): Record<string, string> =>
  Object.fromEntries(
    [...body.matchAll(/--color-(file-[a-z0-9-]+)\s*:\s*([^;]+);/g)].map((m) => [
      m[1]!,
      m[2]!.trim(),
    ]),
  );

/** The file-ROW role chips (an identity colour, not a file-type descriptor).
 *  They are translucent by design and are NOT contrast-tested; naming them here
 *  is what keeps "not tested" a decision rather than a parse accident — a
 *  renamed descriptor would otherwise slip in via this exclusion or, before
 *  this change, via the hex-only regex.
 *
 *  HOW MANY DESCRIPTOR CONSUMERS THERE ACTUALLY ARE, since the exclusion list
 *  is only correct if the consumer census is. Grepped from the tree: exactly TWO
 *  components paint a raw `--color-file-*` descriptor — the code-block header
 *  (`MessageBubble.tsx`, glyph + language label, on `bg-panel`) and `FileChip`
 *  (glyph, on the transcript column). `FileChip` has five call sites
 *  (`ChangesGroupCard` ×3, `ToolCallCardHeader`, `ToolCallCard`), which is
 *  probably where a count like "7 consumers" comes from, but they are two
 *  renderers. The three chips excluded below are the other half of that census
 *  and they have **zero JSX consumers at all** — no `bg-file-node*`/
 *  `text-file-node*` appears anywhere in `src/`, and neither does any other
 *  `*-node` role chip — so they are excluded because they are role chips by
 *  contract, not because they were found on an elevated surface. There is also
 *  no raw descriptor on `bg-card`, `bg-input` or a `bg-panel` worktree chip: the
 *  header moved off `bg-card` for exactly the reason this file documents. If a
 *  new raw-descriptor site is added on a surface not in `DARK_SURFACES`, the
 *  `forbidden` rows below are where it fails. */
const ROLE_CHIPS = ["file-node", "file-node-hover", "file-node-foreground"];

const parseHex = (h: string): [number, number, number] => {
  const n = parseInt(h.slice(1), 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
};
const toHex = (rgb: [number, number, number]) =>
  "#" + rgb.map((v) => Math.round(Math.min(255, Math.max(0, v))).toString(16).padStart(2, "0")).join("");
/** Composite `color` (with alpha `a`, 0–1) over an OPAQUE `bg`. */
const over = (
  color: [number, number, number],
  a: number,
  bg: [number, number, number],
): [number, number, number] =>
  color.map((c, i) => a * c + (1 - a) * bg[i]!) as [number, number, number];

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

/**
 * Reduce a declaration's raw value to the OPAQUE hex that actually paints, on
 * `bg`. Resolves `#rgba`/`#rrggbbaa`, `rgb()`/`rgba()`, a single-colour
 * `color-mix(in oklab, <c> P%, transparent)` (composited over `bg`, which is
 * why the surface is an argument) and a `var()` indirection that lands on a hex
 * elsewhere in `map`. Anything else — `hsl()`, a two-colour `color-mix`, a
 * `var()` whose target is itself not a colour this file can see — THROWS with
 * the token name, because guessing here is what let a descriptor be judged
 * against someone else's surface.
 */
const resolveToHex = (
  name: string,
  raw: string,
  map: Record<string, string>,
  bg: string,
  seen: ReadonlySet<string> = new Set(),
): string => {
  const v = raw.trim();
  const fail = () => {
    throw new Error(
      `cannot resolve --color-${name} to an opaque colour: "${v}" — the descriptor ` +
        `would be judged on the wrong surface, which is what this file exists to catch`,
    );
  };
  if (/^#[0-9a-fA-F]{6}$/.test(v)) return v.toLowerCase();
  const bgRgb = parseHex(bg);
  // #rgba / #rrggbbaa
  const hex8 = v.match(/^#([0-9a-fA-F]{6})([0-9a-fA-F]{2})$/);
  if (hex8) {
    const a = parseInt(hex8[2]!, 16) / 255;
    return toHex(over(parseHex(hex8[1]!), a, bgRgb));
  }
  const hex4 = v.match(/^#([0-9a-fA-F]{3})([0-9a-fA-F])$/);
  if (hex4) {
    const expanded =
      "#" + [...hex4[1]!].map((c) => c + c).join("");
    const a = parseInt(hex4[2]! + hex4[2]!, 16) / 255;
    return toHex(over(parseHex(expanded), a, bgRgb));
  }
  const rgba = v.match(
    /^rgba?\(\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)\s*(?:,\s*([\d.]+)\s*)?\)$/,
  );
  if (rgba) {
    const a = rgba[4] === undefined ? 1 : Number(rgba[4]);
    return toHex(
      over([Number(rgba[1]), Number(rgba[2]), Number(rgba[3])], a, bgRgb),
    );
  }
  // var(--x): one hop through the merged map (the cascade the browser resolves).
  const ref = v.match(/^var\(--color-([a-z0-9-]+)\)$/);
  if (ref) {
    const target = `color-${ref[1]!}`;
    if (seen.has(target)) {
      throw new Error(`--color-${name} has a cyclic var() reference at --${target}`);
    }
    const next = map[target];
    if (next === undefined) fail();
    return resolveToHex(target, next!, map, bg, new Set([...seen, name]));
  }
  // color-mix(in <space>, <colour> P%, transparent) — a single colour at alpha.
  const mix = v.match(
    /^color-mix\(\s*in\s+\w+\s*,\s*(.+?)\s+([\d.]+)%\s*,\s*transparent\s*\)$/,
  );
  if (mix) {
    const inner = resolveToHex(name, mix[1]!, map, bg, seen);
    const a = Number(mix[2]) / 100;
    return toHex(over(parseHex(inner), a, bgRgb));
  }
  return fail();
};

const LIGHT_BG = "#f8f8f8"; // .theme-zai-light --color-background

/** Every palette block that ships a dark surface, with every surface in that
 *  palette a DESCRIPTOR could plausibly land on. `@theme` itself is not a row:
 *  with no class on `<html>` the descriptors read the light-safe bases below.
 *
 *  WHY A LIST OF SURFACES AND NOT ONE `bg` PER PALETTE. The original table had
 *  exactly one row per palette (`--color-background`), and that is precisely why
 *  it stayed green while the code-block header — a `bg-card` band — put 7 of the
 *  25 descriptors below the 3:1 floor under Dracula (`file-ts`/`file-py` 2.50,
 *  `file-html` 2.55, `file-sass`/`file-graphql` 2.57, `file-java` 2.62,
 *  `file-php` 2.63). A glyph is legible or it is not, and which surface it sits
 *  on is the other half of the question; grading only the page could not see a
 *  header that had moved up the elevation ladder. So a palette adds a ROW per
 *  SURFACE it paints a descriptor on (`kind: "rendered"`) and one per surface it
 *  must never paint a descriptor on (`kind: "forbidden"` — see below).
 *
 *  `rendered` = a descriptor really does ride this surface today, so EVERY
 *  descriptor must clear 3:1 there. Where those descriptors live: the
 *  TRANSCRIPT COLUMN (a `FileChip` inside a transcript row — `TRANSCRIPT_ROW`
 *  carries no background, so the chip shows the column behind it, which is
 *  `bg-background-alt`, i.e. the palette's `--color-background-win-alt` at 60%
 *  over the page: `#2f3240` under Dracula, `#222222` under zai dark) and the
 *  panel (the code-block header, and the `bg-panel` tool/detail panels). Both
 *  composite DARKER than the panel, so the panel row below is the binding one
 *  for the dark palettes; the page row is kept because `--color-background-alt`
 *  is a derivation, not a constant, and a palette could move it above the panel.
 *
 *  `forbidden` = a surface the descriptors CANNOT clear, pinned so that "the
 *  hues do not work there" is a fact in the file rather than an accident.
 *  Nothing may be allow-listed onto these surfaces: the hues are IDENTITY
 *  colours that a palette must not re-hue (the `--color-file-*` rule in
 *  `CONTEXT.md`), so when a surface is too bright for them the SURFACE moves.
 *  That is exactly the fix the code-block header took (`bg-card` → `bg-panel`),
 *  and this list is what makes the reverse move fail loudly. */
const DARK_SURFACES = [
  {
    block: /^\.theme-zai-dark\s*\{/m,
    name: ".theme-zai-dark",
    surfaces: [
      { kind: "rendered", label: "background", hex: "#161616" },
      { kind: "rendered", label: "panel", hex: "#202020" },
      { kind: "forbidden", label: "card", hex: "#2b2b2b" },
    ],
  },
  {
    block: /^\.theme-dracula\s*\{/m,
    name: ".theme-dracula",
    surfaces: [
      { kind: "rendered", label: "background", hex: "#282a36" },
      { kind: "rendered", label: "panel", hex: "#343746" },
      // The raised step, which is what the code-block header used to ride. NOTE
      // `--color-secondary` and `--color-tag` are the SAME `#424450` now, so
      // this row also covers a `bg-secondary` badge.
      { kind: "forbidden", label: "card", hex: "#424450" },
      { kind: "forbidden", label: "selection", hex: "#44475a" },
    ],
  },
];

const BASE = tokens(BLOCK(/^@theme\s*\{/m));
const PALETTES = DARK_SURFACES.map((s) => ({
  ...s,
  overrides: tokens(BLOCK(s.block)),
}));

/** The descriptor → ratio failures that are ACCEPTED on a `forbidden` surface,
 *  as `"<palette> <surface>: <descriptor> (<hex>) <ratio>"`. Pinned as an exact
 *  set: an entry that stops being true (the hue moved, or the surface moved) is
 *  a stale exemption and fails, and a NEW failure on a forbidden surface is the
 *  signal that a descriptor has been put somewhere it cannot be read.
 *
 *  These are not "skipped" — every one is measured, listed and explained. They
 *  are the reason the code-block header sits on `bg-panel` rather than `bg-card`. */
const FORBIDDEN_SURFACE_FAILURES = [
  // Zai dark's card. One descriptor only, and it is the hue zai dark itself
  // re-reads for the page (`#7e57c2`, 3.47 on `#161616`, 3.13 on the panel).
  ".theme-zai-dark card: color-file-css (#7e57c2)",
  // Dracula's raised step: 7 of 25. All seven clear 3:1 on the page (>= 3.69)
  // and on the panel (>= 3.06), which is what makes moving the surface the fix.
  ".theme-dracula card: color-file-ts (#0288d1)",
  ".theme-dracula card: color-file-py (#0288d1)",
  ".theme-dracula card: color-file-html (#e65100)",
  ".theme-dracula card: color-file-sass (#ec407a)",
  ".theme-dracula card: color-file-graphql (#ec407a)",
  ".theme-dracula card: color-file-java (#f44336)",
  ".theme-dracula card: color-file-php (#1e88e5)",
  // Dracula's Selection step — the brightest surface in the palette, and
  // therefore the worst one for a dark-ish identity hue. Listed rather than
  // assumed: `#44475a` is legal for `card-selected`/`terminal-selection`, so
  // nothing but this row stops a descriptor landing on it.
  ".theme-dracula selection: color-file-ts (#0288d1)",
  ".theme-dracula selection: color-file-py (#0288d1)",
  ".theme-dracula selection: color-file-html (#e65100)",
  ".theme-dracula selection: color-file-sass (#ec407a)",
  ".theme-dracula selection: color-file-graphql (#ec407a)",
  ".theme-dracula selection: color-file-java (#f44336)",
  ".theme-dracula selection: color-file-php (#1e88e5)",
  ".theme-dracula selection: color-file-yaml (#ff5252)",
  ".theme-dracula selection: color-file-svelte (#ff5722)",
];

describe("the file-type descriptor palette", () => {
  // Everything the parse saw, minus the role chips that are excluded BY NAME
  // above. A `file-*` token that is neither a descriptor nor a chip is a
  // classification failure, not something to quietly drop.
  const allNames = [...new Set([...Object.keys(BASE), ...PALETTES.flatMap((p) => Object.keys(p.overrides))])];
  const names = allNames.filter((n) => !ROLE_CHIPS.includes(n));

  it("is a real palette (the descriptors exist as tokens)", () => {
    expect(names.length).toBeGreaterThanOrEqual(20);
  });

  it("classifies every --color-file-* token it found", () => {
    // Dangling-parse net. The old hex-only regex ignored whatever it did not
    // recognise, so a new or renamed token could be tested by accident or
    // skipped by accident with no signal either way. Now every token the parse
    // finds is either a DESCRIPTOR under test or a ROLE CHIP named above, and
    // each block's declaration count is pinned so a parser that quietly found
    // fewer tokens than the file declares cannot pass.
    expect(ROLE_CHIPS.every((n) => allNames.includes(n))).toBe(true);
    const counts = {
      "@theme": Object.keys(BASE).length,
      ...Object.fromEntries(PALETTES.map((p) => [p.name, Object.keys(p.overrides).length])),
    };
    // 25 descriptors + the 3 role chips in the blocks that restate them; Dracula
    // declares the 25 descriptors only, since `file-node*` are INHERITED from
    // `.dark` by design (see `INHERITED` in `paletteCompleteness.test.ts`).
    expect(counts).toEqual({
      "@theme": 28,
      ".theme-zai-dark": 28,
      ".theme-dracula": 25,
    });
  });

  it("resolves EVERY descriptor to an opaque colour on all three surfaces", () => {
    // The loud half of the parser: a value this file cannot reduce to a hex
    // throws here, by name, instead of vanishing from the map and being graded
    // against the `@theme` base on a surface it never touches.
    for (const p of [
      { name: "@theme (light)", merged: BASE, surfaces: [{ hex: LIGHT_BG }] },
      ...PALETTES.map((p) => ({
        name: p.name,
        merged: { ...BASE, ...p.overrides },
        surfaces: p.surfaces,
      })),
    ]) {
      for (const name of names) {
        for (const surface of p.surfaces) {
          const raw = p.merged[name];
          expect(raw, `--color-${name} is undeclared in ${p.name}`).toBeTruthy();
          resolveToHex(name, raw!, p.merged, surface.hex);
        }
      }
    }
  });

  it.each(names)("%s clears 3:1 on the LIGHT background", (name) => {
    expect(contrast(resolveToHex(name, BASE[name]!, BASE, LIGHT_BG), LIGHT_BG)).toBeGreaterThanOrEqual(3);
  });

  for (const p of PALETTES) {
    const onSurface = { ...BASE, ...p.overrides };
    for (const surface of p.surfaces.filter((s) => s.kind === "rendered")) {
      it.each(names)(
        `%s clears 3:1 on the ${p.name} ${surface.label}`,
        (name) => {
          const hex = resolveToHex(name, onSurface[name]!, onSurface, surface.hex);
          expect(
            contrast(hex, surface.hex),
            `${name} (${hex} from "${onSurface[name]}") on ${p.name} ${surface.label} ${surface.hex}`,
          ).toBeGreaterThanOrEqual(3);
        },
      );
    }
  }

  it("holds the forbidden surfaces to their pinned failure set (no new hue may be dropped there)", () => {
    // A `forbidden` surface is not "a surface we did not look at" — every
    // descriptor is measured on it, and the set that fails 3:1 is compared to
    // `FORBIDDEN_SURFACE_FAILURES` exactly. So the list cannot rot (an entry
    // whose failure has been fixed fails here) and cannot be widened by
    // accident (a hue that drifts below 3:1 on one of these surfaces fails by
    // name, which is the situation the code-block header was in).
    const found: string[] = [];
    const detail: Record<string, number> = {};
    for (const p of PALETTES) {
      const onSurface = { ...BASE, ...p.overrides };
      for (const surface of p.surfaces.filter((s) => s.kind === "forbidden")) {
        for (const name of names) {
          const hex = resolveToHex(name, onSurface[name]!, onSurface, surface.hex);
          const ratio = contrast(hex, surface.hex);
          if (ratio < 3) {
            const key = `${p.name} ${surface.label}: color-${name} (${hex})`;
            found.push(key);
            detail[key] = Math.round(ratio * 100) / 100;
          }
        }
      }
    }
    expect(
      found.sort(),
      `sub-3:1 descriptors on forbidden surfaces (ratios: ${JSON.stringify(detail)})`,
    ).toEqual([...FORBIDDEN_SURFACE_FAILURES].sort());
    // Non-vacuity: the forbidden set really is being measured, and it really is
    // the WORST surface of each palette (otherwise "forbidden" is a label).
    expect(FORBIDDEN_SURFACE_FAILURES.length).toBeGreaterThan(0);
    for (const p of PALETTES) {
      const lumOf = (hex: string) => {
        const rgb = parseHex(hex);
        return relLum(rgb);
      };
      const rendered = p.surfaces.filter((s) => s.kind === "rendered");
      for (const forbidden of p.surfaces.filter((s) => s.kind === "forbidden")) {
        for (const r of rendered) {
          expect(
            lumOf(forbidden.hex),
            `${p.name} ${forbidden.label} ${forbidden.hex} is not brighter than ${r.label} ${r.hex} — a brighter surface is not a legibility problem for a dark hue`,
          ).toBeGreaterThanOrEqual(lumOf(r.hex));
        }
      }
    }
  });

  it("every descriptor is overridden somewhere in the dark reading", () => {
    // ONE assertion on the MERGED map, not one per block. Checked per block it
    // would be a lie: `.theme-zai-dark`'s `file-css` is `#7e57c2`, which EQUALS
    // the `@theme` base and so legitimately "repeats" it there — the hue is
    // already legible on `#161616` (3.47), so restating it would be noise. It
    // only stops repeating once the merge picks up Dracula's `#bd93f9`, the
    // reading `#282a36` needs. What matters is that no descriptor is left
    // WITHOUT a dark reading anywhere in the stylesheet.
    const merged = Object.assign({}, BASE, ...PALETTES.map((p) => p.overrides));
    const missing = names.filter((n) => merged[n] === undefined || merged[n] === BASE[n]);    // The two hues legible on EVERY surface (light page, both dark pages) at
    // their base value legitimately repeat it: `file-html`'s deep orange and
    // `file-php`'s blue clear 3:1 on `#f8f8f8`, `#161616` and `#282a36` alike.
    expect(missing.sort()).toEqual(["file-html", "file-php"]);
  });
});
