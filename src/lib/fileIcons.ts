import type { LucideIcon } from "lucide-react";
import {
  FileIcon,
  FileCodeIcon,
  FileJsonIcon,
  FileCogIcon,
  FileLockIcon,
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
  js: { icon: FileCodeIcon, className: "text-warning" },
  jsx: { icon: FileCodeIcon, className: "text-warning" },
  ts: { icon: FileCodeIcon, className: "text-brand" },
  tsx: { icon: FileCodeIcon, className: "text-brand" },
  json: { icon: FileJsonIcon, className: "text-warning" },
  html: { icon: FileCodeIcon, className: "text-destructive" },
  htm: { icon: FileCodeIcon, className: "text-destructive" },
  css: { icon: FileCodeIcon, className: "text-foreground-subtle" },
  md: { icon: FileTextIcon, className: "text-foreground-subtle" },
  markdown: { icon: FileTextIcon, className: "text-foreground-subtle" },
  rs: { icon: FileCodeIcon, className: "text-destructive" },
  py: { icon: FileCodeIcon, className: "text-success" },
  toml: { icon: FileCogIcon, className: "text-foreground-subtlest" },
  ini: { icon: FileCogIcon, className: "text-foreground-subtlest" },
  yaml: { icon: FileCogIcon, className: "text-foreground-subtlest" },
  yml: { icon: FileCogIcon, className: "text-foreground-subtlest" },
  lock: { icon: FileLockIcon, className: "text-foreground-subtlest" },
  png: { icon: FileImageIcon, className: "text-foreground-subtle" },
  jpg: { icon: FileImageIcon, className: "text-foreground-subtle" },
  jpeg: { icon: FileImageIcon, className: "text-foreground-subtle" },
  gif: { icon: FileImageIcon, className: "text-foreground-subtle" },
  svg: { icon: FileImageIcon, className: "text-foreground-subtle" },
  webp: { icon: FileImageIcon, className: "text-foreground-subtle" },
  sh: { icon: TerminalIcon, className: "text-foreground-subtle" },
  bash: { icon: TerminalIcon, className: "text-foreground-subtle" },
};

/**
 * The icon spec for a file path: the LAST extension (lowercased) →
 * `{ icon, className }`; unmapped / no extension / dotfiles → the
 * neutral default. Pure — unit-testable.
 */
export function fileIconFor(path: string): FileIconSpec {
  const name = basenameOfPath(path);
  const dot = name.lastIndexOf(".");
  const ext = dot >= 0 ? name.slice(dot + 1).toLowerCase() : "";
  return BY_EXTENSION[ext] ?? DEFAULT_SPEC;
}
