/**
 * The left pane's (the `SpacesList` sidebar's) collapsed flag — a tiny
 * shared module (no store, no React) mirroring `sidePaneState` (the right
 * pane): the collapse toggle at the sidebar's footer consumes this via
 * `useSyncExternalStore(subscribeLeftPane, getLeftPaneCollapsed)`.
 *
 * The module is the SINGLE OWNER of the persistence: `setLeftPaneCollapsed`
 * writes `localStorage["left-pane-collapse"]` itself, so the toggle's
 * `setLeftPaneCollapsed(!collapsed)` reaches localStorage with no extra
 * step. The module SEEDS the flag from the persisted value at import
 * (no mount-time hydration elsewhere — a re-read would be a no-op echo
 * unless storage and memory disagree, in which case it would clobber the
 * live flag). The `setItem` is guarded: a quota/lockdown throw must not
 * take down the toggle's click handler (the state flip + notify still
 * happen).
 */

const STORAGE_KEY = "left-pane-collapse";

/**
 * The sidebar's expanded width — shared with the `App` chrome bar: the
 * chrome bar's logo segment (the drag region) is the sidebar's width,
 * so the tabs start where the center column begins; the segment follows the
 * sidebar's collapse state (260px expanded, `LEFT_PANE_RAIL` collapsed).
 */
export const LEFT_PANE_WIDTH = 260;

/**
 * The width the sidebar keeps while COLLAPSED — a sliver, NOT 0.
 *
 * The collapse toggle lives in the sidebar's own inner-bottom corner and
 * stays there in both states; the sliver is the room that keeps it on screen
 * (24px `size-6` button + the footer's 8px `p-2` + 8px of breathing room).
 * Collapsing to 0 would clip the toggle away with the pane, and there is no
 * keyboard shortcut for this pane — which is why the control used to live in
 * the chrome bar. It does not go back to the top: the pane keeps its control
 * at the bottom, in the place the user just used it.
 */
export const LEFT_PANE_RAIL = 40;

let collapsed =
  typeof localStorage !== "undefined" &&
  localStorage.getItem(STORAGE_KEY) === "true";

const listeners = new Set<() => void>();

export function getLeftPaneCollapsed(): boolean {
  return collapsed;
}

export function setLeftPaneCollapsed(v: boolean): void {
  if (v === collapsed) return; // same-value echo: no write, no notify
  collapsed = v;
  try {
    localStorage.setItem(STORAGE_KEY, String(v));
  } catch {
    // quota/lockdown: persistence is best-effort — the state flip and
    // the notify below still happen (the toggle's handler must NOT throw).
  }
  for (const fn of [...listeners]) fn();
}

export function subscribeLeftPane(fn: () => void): () => void {
  listeners.add(fn);
  return () => {
    listeners.delete(fn);
  };
}
