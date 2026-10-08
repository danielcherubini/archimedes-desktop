import { Check } from "lucide-react";
import type { TodoItem } from "../store/interactive";

/**
 * The todo board reduced to ticks and dots, for the collapsed side-pane RAIL
 * (the 40px sliver): the board's shape at a glance — how far through the list
 * the session is, and which item is live — with no text and no scrolling.
 *
 * WHY IT EXISTS: the pane's collapse is `width: 40px` + `overflow: hidden`
 * with the content kept MOUNTED (the `fixed` sudo modals must survive a
 * collapse, so that is not negotiable), so the board used to paint INTO the
 * sliver — text wrapped to one character per line ("th / so / is / s …")
 * inside its own `overflow-y-auto`, which then grew a SCROLLBAR in a 40px
 * column. This is the only thing a 40px column can actually render.
 *
 * THE GEOMETRY IS CSS, NOT MEASURED. Each row is `flex-1 min-h-5 max-h-10` in
 * a top-anchored column: the rows share the rail's height, stop shrinking at
 * the 20px floor, and stop GROWING at 40px — so a 3-item board is a tight
 * cluster at the top (it reads as a list) and a 30-item board fills the rail
 * and clips its tail.
 *
 * CLIPPING IS DIRECTIONAL ON PURPOSE, and that pins `justify-start`: an
 * overflowing flex column CENTERS its overflow when justified to the middle,
 * which would cut the FIRST todos off the top of the rail. The head of a todo
 * list is the part that says "this is a list, and here is where it starts", so
 * the tail is what gets cut — which reads as "more items than fit" (true, and
 * stable), where a scrollbar would read as "scroll inside the sliver", the
 * behavior this replaces. Because the floor fixes the mark size, the check is
 * always legible and there is no conditional glyph to reason about. The obvious
 * alternative — a `ResizeObserver` computing a pitch — needs a measurement
 * round-trip before anything paints, and would keep shrinking marks past
 * legibility to force a 40-item list to fit.
 *
 * THREE STATES, THREE SHAPES: completed = the success ring + a check, in
 * progress = the warning FILL (the board's `◉`), pending = the hollow dot.
 * Shape carries the state as well as colour, because a rail is read in a
 * glance and a glance is not a contrast test.
 *
 * SCOPE: the MAIN list only, and the FULL list (completed included) — the open
 * count drives the pane, the whole list drives the strip, so a finished board
 * still reads as a row of ticks after the pane has auto-collapsed. Subagent
 * columns are deliberately NOT here: they are a per-child breakdown that needs
 * a name to mean anything, and the rail has no room for one — the
 * `SubagentDelegatingCard` in the transcript is that readout.
 */

/** A mark's box, and the check inside it. */
const MARK_PX = 12;
const CHECK_PX = 8;

function TodoMark({ status }: { status: TodoItem["status"] }) {
  const box = { width: MARK_PX, height: MARK_PX };
  if (status === "completed") {
    return (
      <span
        data-testid="todo-mark"
        style={box}
        className="flex shrink-0 items-center justify-center rounded-full border border-success text-success"
      >
        <Check style={{ width: CHECK_PX, height: CHECK_PX }} aria-hidden />
      </span>
    );
  }
  if (status === "in_progress") {
    return (
      <span
        data-testid="todo-mark"
        style={box}
        className="shrink-0 rounded-full bg-warning"
        aria-hidden
      />
    );
  }
  return (
    <span
      data-testid="todo-mark"
      style={box}
      className="shrink-0 rounded-full border border-foreground-subtlest"
      aria-hidden
    />
  );
}

export default function TodoRail({ items }: { items: TodoItem[] }) {
  if (items.length === 0) return null;
  const completed = items.filter((t) => t.status === "completed").length;
  // The accessible name is the SUMMARY, because the marks carry no text and the
  // real board behind them is `invisible` while collapsed — so the count is in
  // the tree exactly once, here.
  // `top-2` / `bottom-9`: the strip spans the rail but CLEAR of the frame's
  // bottom-left corner, where the pane's own toggle lives (`bottom-2` +
  // `size-6` = 32px of box, so 36px clears it with room to aim).
  // `pointer-events-none`: a readout must not steal the toggle's click.
  return (
    <div
      data-testid="todo-rail"
      role="img"
      aria-label={`Todos ${completed}/${items.length}`}
      className="pointer-events-none absolute inset-x-0 top-2 bottom-9 flex flex-col justify-start gap-1 overflow-hidden"
    >
      {items.map((item, i) => (
        <div
          key={i}
          data-todo-row
          className="flex max-h-10 min-h-5 w-full flex-1 items-center justify-center"
        >
          <TodoMark status={item.status} />
        </div>
      ))}
    </div>
  );
}
