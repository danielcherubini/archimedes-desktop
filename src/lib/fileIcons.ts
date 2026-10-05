import type { LucideIcon } from "lucide-react";
import {
  FileIcon,
  FileCodeIcon,
  FileJsonIcon,
  FileCogIcon,
  FileLockIcon,
  FileArchiveIcon,
  FileImageIcon,
  FileTextIcon,
  TerminalIcon,
} from "lucide-react";
import { basenameOfPath } from "./paths";

export interface FileIconSpec {
  icon: LucideIcon;
  className: string;
}

const DEFAULT_SPEC: FileIconSpec = {
  icon: FileIcon,
  className: "text-foreground-subtlest",
};

const BY_EXTENSION: Record<string, FileIconSpec> = {
  js: { icon: FileCodeIcon, className: "text-file-js" },
  mjs: { icon: FileCodeIcon, className: "text-file-js" },
  cjs: { icon: FileCodeIcon, className: "text-file-js" },
  jsx: { icon: FileCodeIcon, className: "text-file-react" },
  ts: { icon: FileCodeIcon, className: "text-file-ts" },
  // ZCode maps `.tsx` to its `react_ts` glyph, whose fill is the SAME
  // typescript blue (only `.jsx`'s `react` glyph is the cyan).
  tsx: { icon: FileCodeIcon, className: "text-file-ts" },
  mts: { icon: FileCodeIcon, className: "text-file-ts" },
  cts: { icon: FileCodeIcon, className: "text-file-ts" },
  json: { icon: FileJsonIcon, className: "text-file-json" },
  jsonc: { icon: FileJsonIcon, className: "text-file-json" },
  jsonl: { icon: FileJsonIcon, className: "text-file-json" },
  html: { icon: FileCodeIcon, className: "text-file-html" },
  htm: { icon: FileCodeIcon, className: "text-file-html" },
  css: { icon: FileCodeIcon, className: "text-file-css" },
  scss: { icon: FileCodeIcon, className: "text-file-sass" },
  sass: { icon: FileCodeIcon, className: "text-file-sass" },
  md: { icon: FileTextIcon, className: "text-file-md" },
  markdown: { icon: FileTextIcon, className: "text-file-md" },
  rs: { icon: FileCodeIcon, className: "text-file-rs" },
  py: { icon: FileCodeIcon, className: "text-file-py" },
  sh: { icon: TerminalIcon, className: "text-file-sh" },
  bash: { icon: TerminalIcon, className: "text-file-sh" },
  zsh: { icon: TerminalIcon, className: "text-file-sh" },
  yaml: { icon: FileCogIcon, className: "text-file-yaml" },
  yml: { icon: FileCogIcon, className: "text-file-yaml" },
  toml: { icon: FileCogIcon, className: "text-file-toml" },
  ini: { icon: FileCogIcon, className: "text-file-toml" },
  lock: { icon: FileLockIcon, className: "text-file-lock" },
  png: { icon: FileImageIcon, className: "text-file-image" },
  jpg: { icon: FileImageIcon, className: "text-file-image" },
  jpeg: { icon: FileImageIcon, className: "text-file-image" },
  gif: { icon: FileImageIcon, className: "text-file-image" },
  webp: { icon: FileImageIcon, className: "text-file-image" },
  svg: { icon: FileImageIcon, className: "text-file-svg" },
  go: { icon: FileCodeIcon, className: "text-file-go" },
  vue: { icon: FileCodeIcon, className: "text-file-vue" },
  svelte: { icon: FileCodeIcon, className: "text-file-svelte" },
  java: { icon: FileCodeIcon, className: "text-file-java" },
  php: { icon: FileCodeIcon, className: "text-file-php" },
  xml: { icon: FileCodeIcon, className: "text-file-xml" },
  graphql: { icon: FileCodeIcon, className: "text-file-graphql" },
  sql: { icon: FileCodeIcon, className: "text-file-db" },
  zip: { icon: FileArchiveIcon, className: "text-file-archive" },
};

/**
 * The icon spec for a file path: the LAST extension (lowercased) →
 * `{ icon, className }`; unmapped / no extension / dotfiles → the
 * neutral default. Pure — unit-testable.
 *
 * The `className` is a FILE-TYPE DESCRIPTOR token (`text-file-*`), never a
 * semantic one — see the token block in `index.css` for why. The extension →
 * glyph/hue mapping follows ZCode's `resolveIconName` so the two apps colour
 * the same file the same way.
 */
export function fileIconFor(path: string): FileIconSpec {
  const name = basenameOfPath(path);
  const dot = name.lastIndexOf(".");
  const ext = dot >= 0 ? name.slice(dot + 1).toLowerCase() : "";
  return BY_EXTENSION[ext] ?? DEFAULT_SPEC;
}
