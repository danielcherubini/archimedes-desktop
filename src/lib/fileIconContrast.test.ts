import { describe, expect, it } from "vitest";

/**
 * The file-type DESCRIPTOR hues must clear 3:1 against the surface they sit
 * on, in BOTH themes. They are icon glyphs (16px, non-text UI), so 3:1 is the
 * WCAG non-text minimum — but they are also the only thing distinguishing a
 * `.py` chip from a `.json` one, so a pale hue is not just a contrast failure,
 * it is the whole signal disappearing.
 *
 * This is the trap ZCode walks into: its chips are Material Icon Theme SVGs
 * with FIXED fills (`#ffca28` javascript amber), which read fine on its dark
 * surface and vanish on a white one. Our tokens are theme-overridable, so the
 * base values carry light-safe shades and `.theme-zai-dark` carries the vivid
 * originals.
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


const BLOCK = (selector: RegExp) => {
  const start = css.search(selector);
  if (start < 0) throw new Error(`no ${selector} block`);
  const end = css.indexOf("\n}", start);
  return css.slice(start, end);
};

const tokens = (block: string) =>
  Object.fromEntries(
    [...block.matchAll(/--color-(file-[a-z-]+):\s*(#[0-9a-f]{6})/g)].map((m) => [
      m[1]!,
      m[2]!,
    ]),
  );

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

const LIGHT_BG = "#f8f8f8"; // .theme-zai-light --color-background
const DARK_BG = "#161616"; // .theme-zai-dark --color-background

const BASE = tokens(BLOCK(/@theme\s*\{/));
const DARK = { ...BASE, ...tokens(BLOCK(/\.theme-zai-dark\s*\{/)) };

describe("the file-type descriptor palette", () => {
  const names = Object.keys(BASE).filter((n) => n.startsWith("file-"));

  it("is a real palette (the descriptors exist as tokens)", () => {
    expect(names.length).toBeGreaterThanOrEqual(20);
  });

  it.each(names)("%s clears 3:1 on the LIGHT background", (name) => {
    expect(contrast(BASE[name]!, LIGHT_BG)).toBeGreaterThanOrEqual(3);
  });

  it.each(names)("%s clears 3:1 on the DARK background", (name) => {
    expect(contrast(DARK[name]!, DARK_BG)).toBeGreaterThanOrEqual(3);
  });

  it("every descriptor has a dark override (the base values are the light reading)", () => {
    const missing = names.filter((n) => DARK[n] === undefined || DARK[n] === BASE[n]);
    // The three hues already legible on BOTH surfaces legitimately repeat
    // their base value; everything else must be re-read for the dark surface.
    expect(missing.sort()).toEqual(["file-css", "file-html", "file-php"]);
  });
});
