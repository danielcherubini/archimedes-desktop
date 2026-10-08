# Theme token rationale — the `@theme` base readings

Design record for the base token readings in the `@theme` block
(`src/styles/theme.css`). The block itself keeps a one-line pointer to each
section below; the full rationale — including the measured contrast figures —
lives here, because the block is a ~270-declaration list that
`paletteCompleteness.test.ts` parses from disk and the prose used to drown it.

The other blocks keep their records inline: the `.dark` block's comments
explain the dark readings (and the `--color-context-track` split), and the
`.theme-dracula` block (`src/styles/dracula.css`) carries the ADR 0027
spec-gate record, which is its own document and stays with the block.

`src/lib/themeTokens.test.ts` is the gate for the ramp section below (hue
pairing, cool→hot ordering, status-collision, contrast floors) and
`src/lib/fileIconContrast.test.ts` grades the descriptor hues against their
real surfaces.

## Font stack (`--font-sans` / `--font-mono`)

  /* Pin the sans stack explicitly so it does NOT drift with Tailwind's
   * version-dependent default. Tailwind 4.3 (this app) changed the default
   * to a longer stack (-apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto,
   * 'Helvetica Neue', 'Noto Sans', Arial, sans-serif, ...); ZCode is on
   * 4.2.2 (ui-sans-serif, system-ui, sans-serif, ...). Left unpinned, a box
   * with Roboto/Segoe installed would render the two apps in different body
   * fonts. Pinning to ZCode's 4.2.2 stack keeps them in lockstep everywhere
   * (on this machine both already resolve to Noto Sans, so no visible
   * change). The app default (Settings' `null` family) resolves to
   * "Noto Sans" FIRST (the `index.html` Google Fonts family) — the system
   * tail below is the offline fallback. */


## File-type descriptors (`--color-file-*`)

  /* FILE-TYPE DESCRIPTOR COLORS (ZCode's DESIGN.md: "Keep file-type icons on
   * their own descriptor colors"). Deliberately NOT the semantic tokens:
   * `--color-brand` is WHITE in zai-dark (so a `.tsx` chip painted with it
   * rendered colorless), and painting `.rs`/`.py` with `--color-destructive`/
   * `--color-success` claims a file failed or was added.
   *
   * These BASE values are the light-theme-safe shades (every one clears 3:1
   * on the white background — several of ZCode's material-icon fills are pale
   * yellows that vanish there); `.theme-zai-dark` overrides them with
   * ZCode's exact glyph fills, which are tuned for a dark surface. A file
   * type is not a light/dark concern, so the pair is the hue's two readings
   * of the same identity — like `--color-trajectory-*`.
   *
   * The extension → hue mapping follows ZCode's `resolveIconName`. */


## Ramp tokens (`--color-caution`, `--color-thinking-*`)

  /* RAMP TOKENS — the thinking-LEVEL indicator and the context-usage bar.
   * Both were raw Tailwind hues in components (`text-blue-500`,
   * `bg-yellow-500`), which no Palette can move: they would sit unchanged
   * beside Dracula's own orange/purple/yellow and clash.
   *
   * The thinking levels are IDENTITY colors (CONTEXT.md), not statuses: the
   * hue answers "which level is this" — like `--color-file-*` answers "which
   * file type" — so they get their OWN family rather than riding
   * `--color-warning`/`--color-destructive`, which would make a max-effort
   * model read as a failure and would let a Palette re-hue the level. The
   * family pairs with `THINKING_GLYPHS` (src/lib/toolOutput.ts): the glyph is
   * the level's magnitude, the hue its identity. `--color-caution` IS a
   * status — context pressure genuinely is a warning ramp — and slots between
   * `success` and `warning`.
   *
   * These BASE values are the LIGHT reading, and that placement is load-
   * bearing: `applyThemeToDocument` gives zai light `.theme-zai-light` with NO
   * `.dark` class, and a classless document reads `@theme`, so `@theme` is
   * what a light mode resolves — and the hues these replace did not clear it
   * there. Measured on what ACTUALLY rendered (this repo is Tailwind v4, whose
   * palette is `oklch()`, so `text-blue-500` was `oklch(62.3% 0.214 259.815)` =
   * `#2b7fff` at 3.76:1 on white and `bg-yellow-500` was `#f0b100` at a
   * near-invisible 1.91:1 — not the v3 hexes' 3.68/1.92 that an earlier version
   * of this comment quoted, though the conclusion is identical). The glyph is a
   * level's only signal, so a pale hue is not a contrast
   * warning, it is the signal gone. Each value below is the darkest shade of
   * its own hue that still reads as that hue: the thinking ramp is non-text UI
   * (3:1), `caution` also paints the percentage LABEL so it holds the 4.5:1
   * text floor. The dark readings live in `.dark`, which zai dark also gets, so
   * `.theme-zai-dark` needs no entry — the same two-reading structure as
   * `--color-file-*`, and the reason `.theme-zai-light` must stay silent here:
   * a second light value there would shadow this one and leave a classless
   * document unlegible. */

