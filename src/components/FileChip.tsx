import { basenameOfPath } from "../lib/paths";
import { fileIconFor } from "../lib/fileIcons";

/**
 * A file chip: a 16px language icon + the basename (`title` = the full
 * path). Non-clickable in v1 (the desktop has no code-viewer action
 * wired to the side pane yet).
 */
export default function FileChip({ path }: { path: string }) {
  const spec = fileIconFor(path);
  const name = basenameOfPath(path) || path;
  const Icon = spec.icon;
  return (
    <span
      className="inline-flex min-w-0 max-w-full items-center gap-1.5 text-foreground-subtle"
      title={path}
    >
      <Icon className={`size-4 shrink-0 ${spec.className}`} />
      <span className="min-w-0 truncate">{name}</span>
    </span>
  );
}
