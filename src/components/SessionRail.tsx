import { Spinner } from "./ui/spinner";
import type { SessionStatus } from "../hooks/useSessionStatus";

/** One command, offered as an icon button at the head of the rail. */
export interface SessionRailAction {
  id: string;
  /** The accessible name AND the hover text (the rail has no room for a label). */
  label: string;
  icon: React.ReactNode;
  onClick: () => void;
}

export interface SessionRailRow {
  /** The session id — the list key, and the handle the statuses are keyed by. */
  key: string;
  status: SessionStatus;
  active: boolean;
}

/**
 * The sidebar's session list reduced to marks, for the collapsed left RAIL
 * (the 40px sliver): the shape of the list at a glance — how many sessions,
 * which one is active, and above all WHICH ONES ARE WORKING OR WAITING — with
 * no text and no scrolling.
 *
 * WHAT IT MIRRORS: the OPEN sidebar lists the ACTIVE space's sessions (live
 * first, then stored), so that is what the rail maps, in the same order — the
 * Nth mark is the Nth row, and that positional identity is the only reason the
 * column of dots means anything. (The Space itself needs no mark: it is the
 * active TAB, lit in the tab strip directly above the rail. Archived sessions
 * are a collapsed section in the list, and stay out of the rail.)
 *
 * THE STATES COME FROM `useSessionStatus` — the SAME hook the open rows use —
 * so the rail and the list cannot disagree about what "running" means.
 * Spinner = live + in a turn (the row's own spinner, the one glyph the user
 * asked for); warning dot = a pending permission / ask / password (the app's
 * attention colour, the one the toggle dots carry); filled dot = live and
 * idle; dim dot = stored. Waiting outranks running: a blocked session is not
 * working, it is waiting on YOU. The active session's mark takes the brighter
 * ink — the open row marks it with a `bg-selected` FILL, which a 10px dot has
 * no room for, so it is weight, not a rail-only colour.
 *
 * THE ACTIONS RIDE ALONG, and that is a deliberate reversal of the rail's
 * "readout only" rule. A collapsed sidebar used to cost you the pane's four
 * commands: the buttons are inside the content region (so they are `invisible`
 * here), `Skills` and `Settings` have NO keyboard shortcut, and `Settings` had
 * exactly one other door — the footer gear, which is `!collapsed`, i.e. not
 * rendered while collapsed. So collapsing the sidebar locked you out of
 * Settings until you re-expanded it. The icon strip is the same principle the
 * sliver already follows for the pane's OWN toggle: a collapsed pane keeps its
 * controls, it does not hand them to another surface and make them teleport.
 *
 * ORDER IS NOT COSMETIC, and the rail splits its commands around the marks by
 * where each command lives when the pane is OPEN: the list's own buttons (Open
 * Space, New Session, Skills) sit ABOVE the marks, at a fixed offset, so the
 * marks keep their positional contract (the Nth mark is the Nth row); the
 * footer's control (Settings) sits at the BOTTOM, in the 4px gap above the
 * pane's own toggle. Collapse therefore moves each command one SURFACE and not
 * one row up or down the screen — the gear stays where the eye already looks
 * for it, as it does in the inspector's bottom-right footer.
 *
 * The blocks also have OPPOSITE pointer rules in the same 40px column — the
 * marks are `pointer-events-none` (a readout must not steal the footer
 * toggle's click), the command strips must be clickable — so
 * `pointer-events-none` is set per block, never on the frame. And `flex-1`
 * belongs to the MARKS alone, which is what pushes the tail strip to the
 * bottom while still letting a long list clip.
 *
 * THE GEOMETRY IS THE `TodoRail` GRAMMAR, unchanged: rows are
 * `flex-1 min-h-5 max-h-10` in a top-anchored column (short lists cluster at
 * the top and read as a list, long lists floor at 20px and CLIP their tail —
 * never scroll), inside a marks area that is `flex-1 min-h-0` so it takes
 * exactly the height the action strip leaves.
 * See `TodoRail`'s doc for why clipping is directional and why no
 * `ResizeObserver` is involved.
 */

/** The mark box (a hair under the open row's 16px status slot). */
const MARK_PX = 10;

function SessionMark({
  status,
  active,
}: {
  status: SessionStatus;
  active: boolean;
}) {
  const ink = active ? "text-foreground" : "text-foreground-subtlest";
  if (status === "running") {
    // The row's own spinner at rail size. NO `title` anywhere in the rail:
    // the strip is `pointer-events-none`, so a tooltip could never appear, and
    // `role="img"` on the parent makes the marks decoration to AT — the
    // summary carries the count.
    return (
      <Spinner
        data-testid="session-mark"
        style={{ width: MARK_PX, height: MARK_PX }}
        className={`animate-spin ${ink}`}
      />
    );
  }
  const dot =
    status === "waiting"
      ? // The attention cue: the warning FILL (the toggle dots' idiom), which
        // outranks the live/stored RING so a blocked session reads hot even
        // while live.
        "bg-warning"
      : status === "live"
        ? `border-2 ${ink}`
        : `border ${ink}`;
  return (
    <span
      data-testid="session-mark"
      data-dot={status}
      style={{ width: MARK_PX, height: MARK_PX }}
      className={`shrink-0 rounded-full ${dot}`}
    />
  );
}

/**
 * One column of rail command buttons. Extracted because the rail uses it
 * TWICE — the head strip (Open Space / New Session / Skills) and the tail
 * strip (Settings, which lives in the open pane's footer and so keeps the
 * bottom when collapsed) — and the two must not drift in size or ink.
 */
function RailActions({
  testId,
  items,
  className = "",
}: {
  testId: string;
  items: SessionRailAction[];
  className?: string;
}) {
  if (items.length === 0) return null;
  return (
    <div
      data-testid={testId}
      className={`flex shrink-0 flex-col items-center gap-1 ${className}`}
    >
      {items.map((a) => (
        <button
          key={a.id}
          type="button"
          // `aria-label` AND `title`: the rail has no room for a label, so the
          // hover text is the only thing naming the icon.
          aria-label={a.label}
          title={a.label}
          onClick={a.onClick}
          // The same `size-6` box as the pane's footer toggle, so the rail is
          // one grid of controls rather than two sizes.
          className="flex size-6 shrink-0 items-center justify-center rounded-md text-foreground-subtlest hover:bg-surface-hover hover:text-foreground-subtle"
        >
          {a.icon}
        </button>
      ))}
    </div>
  );
}

export default function SessionRail({
  rows,
  actions,
  tailActions,
}: {
  rows: SessionRailRow[];
  /** Commands at the HEAD of the rail (the pane's top-of-list buttons). */
  actions?: SessionRailAction[];
  /** Commands at the TAIL (the pane's footer controls, e.g. Settings). */
  tailActions?: SessionRailAction[];
}) {
  if (
    rows.length === 0 &&
    (actions?.length ?? 0) === 0 &&
    (tailActions?.length ?? 0) === 0
  )
    return null;
  const running = rows.filter((r) => r.status === "running").length;
  const waiting = rows.filter((r) => r.status === "waiting").length;
  // The accessible name is the COUNTS — the marks carry no text, and the list
  // behind them is `invisible` while collapsed, so the summary is in the a11y
  // tree exactly once. Zero counts drop out: a plain list reads as a plain
  // list. (Titles are deliberately absent — the rail is positional, and the
  // names live in the tabs and the transcript.)
  const label = [
    `Sessions ${rows.length}`,
    running > 0 && `${running} running`,
    waiting > 0 && `${waiting} waiting`,
  ]
    .filter(Boolean)
    .join(", ");
  return (
    // The FRAME is positioned only — no `pointer-events-none` here, or the
    // action buttons below would be click-through. `bottom-9`: clear of the
    // footer's toggle (`p-2` + `size-6` = 32px of box).
    // NO folder/heading mark: the rail starts directly under the active TAB
    // (the tab strip's `pl-1` lines it up with this column), so the space it
    // belongs to is already lit one row above — herdr numbers its rail because
    // its rail IS the workspace list, while here the tabs are that list.
    <div
      data-testid="session-rail"
      className="absolute inset-x-0 top-2 bottom-9 flex flex-col gap-1 overflow-hidden"
    >
      <RailActions testId="session-rail-actions" items={actions ?? []} />
      {/* The marks, and ONLY when there are sessions: an empty `role="img"`
          reading "Sessions 0" is noise, and the command strip is exactly what
          a user with an empty space needs. `flex-1 min-h-0`: the area takes exactly the height the
          action strip leaves, and `min-h-0` is what LETS it shrink — a flex
          child's automatic minimum is its content's height, so without it a
          30-session list would push the frame taller instead of clipping.
          `role="img"` + the count lives HERE, not on the frame: the actions are
          real buttons in the a11y tree, and an `img` ancestor would hide them. */}
      {rows.length > 0 && (
        <div
          data-testid="session-rail-marks"
          role="img"
          aria-label={label}
          className="pointer-events-none flex min-h-0 flex-1 flex-col justify-start gap-1 overflow-hidden"
        >
          {rows.map((row) => (
            <div
              key={row.key}
              data-testid="session-row"
              className="flex max-h-10 min-h-5 w-full flex-1 items-center justify-center"
            >
              {/* Explicit props, NOT `{...row}`: React 19 warns when a spread
                  object contains `key`, and the rail re-renders on every
                  streamed chunk, so it would spam the dev console. */}
              <SessionMark status={row.status} active={row.active} />
            </div>
          ))}
        </div>
      )}
      {/* The TAIL strip, LAST in the column. `flex-1` is on the MARKS, not
          here; `mt-auto` (see the prop below) is what anchors this block to the
          bottom of the frame, landing it in the 4px gap above the pane's toggle
          (`bottom-9`) — which is where the gear lives when the pane is open, so
          collapsing moves the gear one surface and not one row up the screen. */}
      <RailActions
        testId="session-rail-tail-actions"
        items={tailActions ?? []}
        // `mt-auto`, not a `flex-1` sibling: the marks block is omitted when
        // the list is EMPTY, and an anchoring scheme that depends on that
        // block would leave the gear floating in the middle of the rail
        // exactly when the space has nothing else to show.
        className="mt-auto"
      />
    </div>
  );
}
