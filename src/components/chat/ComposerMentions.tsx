import type { ComposerPrefix } from "../../lib/skills";

/**
 * One row in the composer's picker (the floating row list above the
 * textarea). The picker is GENERALIZED across all four prefixes: the
 * `filtered` rows are ALWAYS from the active prefix's catalog (a `$`/`#`/`@`
 * mention, or a `?` file query, never mixes in one open list), so the
 * `prefix` here is uniform per render (it is the active token's prefix —
 * `ChatStream` derives it).
 */
export interface MentionRow {
  /** The catalog entry's identity (name + kind is unique per kind). */
  key: string;
  prefix: ComposerPrefix;
  name: string;
  /** One-line (the `title` tooltip — the existing pattern). */
  description: string;
}

/**
 * The composer's picker (the floating row list above the textarea):
 * the `$`/`#`/`@`-trigger catalog rows (and, from ADR 0033, the `?` file
 * rows), each with a prefix badge (the only
 * visual delta vs the old `$`-only skill picker), name (primary) and
 * description (secondary, truncated, `title` tooltip). Plus an optional
 * non-selectable `note` line (the `?` truncation explanation).
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
  note,
  onSelect,
}: {
  open: boolean;
  filtered: MentionRow[];
  activeIndex: number;
  /** The one explanation line (`?`'s cap notes) — NOT a row. */
  note?: string;
  onSelect: (row: MentionRow) => void;
}) {
  return (
    <>
      {/* The gate is widened to `rows || note`: the note is most useful
          exactly when the query matched NOTHING ("the listing is capped" is
          what explains an empty picker). This does NOT make Enter an
          insertion — the keydown intercept in `ComposerRow` is gated on
          `filtered.length > 0`, which the note never touches. */}
      {open && (filtered.length > 0 || note) && (
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
          {/* The note (the `?` truncation explanation). A plain `<div>`:
              never a `<button>`, never focusable, never part of `filtered` —
              so `activeIndex`, the arrow-wrap and Enter operate over the
              rows ONLY. `role="status"` because in the note-ONLY state (a `?`
              query that matched nothing while the listing is truncated) it is
              the picker's whole payload, and a screen-reader user would
              otherwise get no signal at all — it is exactly a transient status
              message. The role does not make it a row: still a non-focusable
              div, and the keyboard model stays gated on `filtered.length > 0`. */}
          {note && (
            <div
              role="status"
              data-testid="mention-picker-note"
              className="px-2 py-1 text-ui-sm text-foreground-subtle"
            >
              {note}
            </div>
          )}
        </div>
      )}
    </>
  );
}
