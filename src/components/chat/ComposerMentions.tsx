import { useEffect, useRef } from "react";
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
  /** What `selectMention` INSERTS (plus the ` ` / `/` its policy adds). */
  name: string;
  /**
   * What the PRIMARY line SHOWS, defaulting to `name` when absent. It exists
   * for ONE case (ADR 0035): an out-of-Space row must INSERT the ABSOLUTE path
   * — nothing in this app expands a tilde, so `~/notes/x` would resolve to a
   * directory literally named `~` — while the row DISPLAYS the abbreviated
   * `~/…`. Display and insertion are therefore deliberately two different
   * strings, and `label` is the display half. Nothing else sets it, so every
   * Mention row and every Space Listing row keeps showing `name` verbatim.
   */
  label?: string;
  /**
   * A DIRECTORY row (out-of-Space `?` only, ADR 0035): the primary line gains a
   * trailing `/`, and `selectMention` DESCENDS (keeps the `?`, appends `/`) or
   * it would consume the trigger and strand the user after one level. Never set
   * on a file row or on any Mention row.
   */
  isDir?: boolean;
  /** One-line (the `title` tooltip — the existing pattern). */
  description: string;
}

/** The PRIMARY line's full text: what the row shows, INCLUDING the trailing `/`
 *  a directory row appends. ONE definition because the `title` tooltip and the
 *  rendered text must be the same string — a tooltip that disagreed with the
 *  line it describes (by dropping the descent `/`) would be worse than none. */
function primaryLabel(row: MentionRow): string {
  return `${row.label ?? row.name}${row.isDir ? "/" : ""}`;
}

/**
 * The composer's picker (the floating row list above the textarea):
 * the `$`/`#`/`@`-trigger catalog rows (and, from ADR 0033, the `?` file
 * rows), each with a prefix badge (the only
 * visual delta vs the old `$`-only skill picker), name (primary) and
 * description (secondary, truncated, `title` tooltip) — the last being EMPTY
 * for a `?` file row by design (a path already ends in its own basename), so a
 * `?` row renders on ONE line. The PRIMARY line shows `row.label ?? row.name`
 * with a trailing `/` appended when `row.isDir`: an out-of-Space row SHOWS the
 * abbreviated `~/…` while `name` carries the absolute path it will INSERT, and
 * the `/` is the visual half of "selecting this descends" (ADR 0035). That
 * `label ??` is load-bearing, not cosmetic — MUTATION-CHECKED: rendering
 * `row.name` here reddens SEVEN of `ChatStream.test.tsx`'s ADR 0035 tests, which
 * is the other half of why the row carries BOTH halves. The second line is
 * conditional on a NON-EMPTY description, so a `$`/`@`/`#` row has two
 * lines whenever its catalog entry
 * carries one and ONE when it does not (a skill with an empty `summary`, an
 * agent with no description): the guarantee is per description, never per
 * prefix. Plus an optional non-selectable `note` line (the `?` truncation
 * explanation).
 *
 * WHERE THE STATE LIVES: the `useMentionCatalogs` hook, the `picker` state
 * (incl. the active `prefix`), the `filtered`/`activeIndex` derivations and
 * `selectMention` all stay in `ChatStream` — the `picker` state is
 * coordinated with the textarea's `onChange` / `onKeyDown` (the token
 * re-evaluation, the arrow/Enter/Tab/Escape branches) and the send path, so
 * it cannot move here. Renders nothing while closed.
 *
 * It is therefore NOT props-only in the literal sense — it calls TWO hooks,
 * `useRef` and the ONE `useEffect` below, and that single effect is
 * deliberately the only one: keeping the highlighted row inside a box that
 * clamps its own height is PURELY LOCAL PRESENTATION (it reads `activeIndex`,
 * touches no state, and asks nothing of the composer, and the `useRef` exists
 * only to serve it), whereas everything above is state the composer owns. The
 * line to hold is that no DERIVATION may move here — if a future change needs
 * to know which row is selected in order to compute what the list IS, that
 * belongs in `ChatStream`, not in this component.
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
  // The highlighted row's DOM node, for the scroll-into-view effect below.
  // Attached to the ACTIVE row only (a conditional ref), which React handles
  // cleanly: every ref DETACH happens in the mutation phase and every ATTACH
  // in the layout phase, so by the time the passive effect runs the ref points
  // at the newly-highlighted row — including when the highlight moves BACK to
  // a row that was mounted the whole time (ArrowUp), where the naive
  // per-fiber ordering would have left it `null`.
  const activeRowRef = useRef<HTMLButtonElement | null>(null);
  // Focus-follows: the box clamps its own height (see the container class
  // below), so a list longer than the clamp scrolls, and an arrow key can
  // otherwise move the highlight to a row that is not on screen at all — the
  // selection would silently leave the visible window and Enter would insert
  // something the user never saw. `block: "nearest"` is the whole point: it
  // scrolls ONLY when the row is outside the box, so arrowing through rows
  // that are already visible never makes the list jump.
  //
  // DEPS (hand-checked — this repo has no lint script, so nothing guards
  // react-hooks rules automatically and a wrong array is invisible until it
  // loops): `activeIndex` is what this reacts to. `filtered` is a dep because
  // the ACTIVE ROW, not the index, is what the scroll is about: typing narrows
  // the list, so `filtered[activeIndex]` becomes a DIFFERENT row under an
  // unchanged index, and without `filtered` that new row would never be made
  // visible. It does mean the effect re-runs on each keystroke (the rows memo
  // re-derives on `draft`/`picker`, so `filtered` is a new identity then) — that
  // is accepted, not overlooked: `block: "nearest"` makes a visible row a
  // no-op, so the extra work is one call that moves nothing, and these two are
  // exactly the inputs the highlighted row's GEOMETRY depends on (the same pair
  // an exhaustive-deps lint would demand). Nothing here writes a dep, so the
  // effect can never re-trigger itself — no loop is reachable.
  useEffect(() => {
    activeRowRef.current?.scrollIntoView({ block: "nearest" });
  }, [activeIndex, filtered]);
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
          // The height clamp: `22` is ≈10 single-line rows. A one-line row is
          // `line-height(≈1.5 × var(--ui-font-size)) + 8px padding` ≈ `2.2 × F`,
          // so 10 rows ≈ `22 × F` ≈ 308px at the default 14px — a normal
          // dropdown. `--ui-font-size` is a USER setting applied as an inline
          // px value on `<html>` (`src/lib/settings.ts`), which is why the
          // clamp is a `calc()` over the variable and NOT a `rem` class like
          // `max-h-64`: a fixed clamp would show FEWER rows to a user who
          // raised the font size. `overflow-y-auto` is what makes the clamp a
          // scrollbar rather than a clip, and it is what protects the
          // deliberately UNCAPPED Mention lists (`$`/`@`/`#`): their bound is
          // geometry, not a row count.
          //
          // 22 is a deliberate ESTIMATE, not a measurement: jsdom has no
          // layout engine, so no test in this repo can measure a row's height
          // — the tests pin the CLASSES and the scroll behavior, never a
          // pixel.
          className="absolute left-3 right-3 -top-2 z-10 -translate-y-full max-h-[calc(var(--ui-font-size)*22)] overflow-y-auto rounded-lg border border-input-border bg-input p-1 shadow-lg"
        >
          {filtered.map((row, i) => (
            <button
              key={row.key}
              ref={i === activeIndex ? activeRowRef : undefined}
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
                {/* THE PRIMARY LINE TRUNCATES, and the two classes are a PAIR,
                    not decoration. A long absolute path is ONE unbreakable token
                    (a 200-char Windows path has no space to wrap at), the badge
                    beside it is `shrink-0`, and the container is
                    `overflow-y-auto` — which per CSS forces the visible x-axis to
                    compute to `auto` rather than `hidden`, so an unclipped label
                    made the row scroll or clip horizontally instead of eliding.
                    `min-w-0` is the half that lets a flex item shrink BELOW its
                    content's intrinsic width (the default `min-width: auto` is
                    what refuses, and it is the classic flexbox trap); `truncate`
                    is `overflow-hidden` + `text-overflow: ellipsis` +
                    `white-space: nowrap`. Either one alone still clips.

                    The `title` is the user-visible half of the deal: an elided
                    path is unreadable, and the FULL string is exactly what they
                    need, so the whole path lives on hover. It is also the only
                    part of any of this a test can assert — jsdom has NO layout
                    engine, so no test here measures a clip, an ellipsis or a
                    width; the elision itself is CSS-reasoned, and what
                    `ChatStream.test.tsx` pins is these two class names plus the
                    `title`. A row that wraps would ALSO break the container's
                    `max-h-[calc(var(--ui-font-size)*22)]` clamp ("~10 single-line
                    rows"), which is the other reason the label must not wrap.

                    A DIRECTORY row's `title` carries the trailing `/` too — the
                    same string the row SHOWS — because that slash is the visual
                    half of "selecting this descends" and a tooltip that dropped it
                    would disagree with the line it describes. */}
                <span className="min-w-0 truncate text-ui-base" title={primaryLabel(row)}>
                  {primaryLabel(row)}
                </span>
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
