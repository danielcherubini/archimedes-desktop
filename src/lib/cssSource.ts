/**
 * The single reader for `src/index.css` that every CSS-parsing test in this
 * repo must use.
 *
 * `src/index.css` is an ASSEMBLY MANIFEST, not the stylesheet: the real CSS
 * lives in `src/styles/*.css` and is inlined into it by Vite's CSS handling
 * (local `@import`s are resolved in place; `@tailwindcss/vite` then compiles
 * the Tailwind utilities and consumes `@theme`/`@custom-variant`). This module
 * replicates the inlining step for the tests so the two views cannot drift:
 *
 *  - LOCAL imports (`./styles/*.css`) are inlined IN PLACE, in order, so the
 *    resolved source keeps the file's cascade: `@theme` < `.dark` <
 *    `.theme-zai-light` < `.theme-zai-dark` < `.theme-dracula` (pinned in
 *    `paletteCompleteness.test.ts`), including the hand-maintained
 *    `@media (forced-colors: active)` block in `src/styles/base.css`.
 *  - Package imports (`tailwindcss`, `tw-animate-css`, `shadcn/tailwind.css`)
 *    are DROPPED here — the tests only read the app's own tokens and rules,
 *    and the utilities they compile to are graded through components, not
 *    through this file.
 *
 * nothing else is transformed: no minification, no comment stripping — each
 * test strips comments itself, the same way it did when `index.css` was
 * monolithic. (`paletteCompleteness.test.ts` reads the RAW resolved text for
 * its block-order offsets; the others strip first.)
 *
 * This app deliberately ships no `@types/node` (its globals stay DOM-only),
 * so Node's `fs` is reached through a COMPUTED specifier — an untyped `any`
 * import that `tsc` accepts without the Node type packages. The same trick
 * the CSS tests always used.
 */
const NODE_FS = "node:" + "fs";
const { readFileSync } = (await import(NODE_FS)) as {
  readFileSync: (path: string, encoding: string) => string;
};

/** Inline one local import's file, normalising `.` / `..` path segments. */
const joinPath = (dir: string, spec: string): string => {
  const parts = (dir + spec).split("/").filter((p) => p.length > 0 && p !== ".");
  const out: string[] = [];
  for (const p of parts) {
    if (p === "..") out.pop();
    else out.push(p);
  }
  return out.join("/");
};

/** Recursively inline a file's LOCAL `@import`s; drop package imports. */
const inlineImports = (relPath: string): string => {
  const css = readFileSync(relPath, "utf8");
  return css.replace(
    /@import\s+["']([^"']+)["'];/g,
    (_match, spec: string) => {
      if (!spec.startsWith(".")) return ""; // package import: Vite's job
      const dir = relPath.slice(0, relPath.lastIndexOf("/") + 1);
      return inlineImports(joinPath(dir, spec));
    },
  );
};

/**
 * `src/index.css` with its local imports inlined — i.e. the stylesheet Vite
 * sees before it compiles the Tailwind utilities. Relative to the process
 * working directory, the repo root, which is how every test here runs.
 */
export const readResolvedIndexCss = (): string => inlineImports("src/index.css");

/** Remove every CSS comment. Tests that match SELECTORS or VALUES do this
 *  before parsing, because a rule's selector can be named in PROSE (`.dark`
 *  and `@theme` are both discussed in comments long before any block opens)
 *  and a commented-out declaration would still match a declaration regex. */
export const stripCssComments = (css: string): string =>
  css.replace(/\/\*[\s\S]*?\*\//g, "");
