/**
 * One row in the composer's mention picker (the floating row list above the
 * textarea). The picker is GENERALIZED across all three prefixes: the
 * `filtered` rows are ALWAYS from the active prefix's catalog (a `$`/`#`/`@`
 * never mixes in one open list), so the `prefix` here is uniform per render
 * (it is the active token's prefix — `ChatStream` derives it).
 */
export interface MentionRow {
  /** The catalog entry's identity (name + kind is unique per kind). */
  key: string;
  prefix: "$" | "#" | "@";
  name: string;
  /** One-line (the `title` tooltip — the existing pattern). */
  description: string;
}

/**
 * The composer's mention picker (the floating row list above the textarea):
 * the `$`/`#`/`@`-trigger catalog rows, each with a prefix badge (the only
 * visual delta vs the old `$`-only skill picker), name (primary) and
 * description (secondary, truncated, `title` tooltip).
 *
 * Props-only: the `useMentionCatalogs` hook, the `picker` state (incl. the
 * active `prefix`), the `filtered`/`activeIndex` derivations and
 * `selectMention` all stay in `ChatStream` — the `picker` state is
 * coordinated with the textarea's `onChange` / `onKeyDown` (the token
 * re-evaluation, the arrow/Enter/Tab/Escape branches) and the send path, so
 * it cannot move here. Renders nothing while closed.
 */
export default function ComposerMentions({
  open,
  filtered,
  activeIndex,
  onSelect,
}: {
  open: boolean;
  filtered: MentionRow[];
  activeIndex: number;
  onSelect: (row: MentionRow) => void;
}) {
  return (
    <>
      {open && filtered.length > 0 && (
        <div
          data-testid="mention-picker"
          className="absolute left-3 right-3 -top-2 z-10 -translate-y-full rounded-lg border border-input-border bg-input p-1 shadow-lg"
        >
          {filtered.map((row, i) => (
            <button
              key={row.key}
              type="button"
              onMouseDown={(e) => {
                e.preventDefault();
                onSelect(row);
              }}
              className={`flex w-full flex-col gap-0.5 rounded-md px-2 py-1 text-left ${i === activeIndex ? "bg-surface-hover" : ""}`}
            >
              <span className="flex items-baseline gap-1.5">
                <span
                  className="shrink-0 text-ui-xs text-foreground-subtlest"
                  aria-hidden
                >
                  {row.prefix}
                </span>
                <span className="text-ui-base">{row.name}</span>
              </span>
              {row.description !== "" && (
                <span
                  className="truncate text-ui-sm text-foreground-subtle"
                  title={row.description}
                >
                  {row.description}
                </span>
              )}
            </button>
          ))}
        </div>
      )}
    </>
  );
}
