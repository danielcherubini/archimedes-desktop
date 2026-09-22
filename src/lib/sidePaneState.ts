/**
 * The side pane's collapsed flag — a tiny shared module (no store, no
 * React) so the `SidePane` frame (Task 4) and the header toggle (Task 6)
 * agree without event guessing: the toggle consumes this via
 * `useSyncExternalStore(subscribeSidePane, getSidePaneCollapsed)`.
 *
 * The module is the SINGLE OWNER of the persistence: `setSidePaneCollapsed`
 * writes `localStorage["side-pane-collapse"]` itself, so the toggle's
 * `setSidePaneCollapsed(!collapsed)` reaches localStorage with no extra
 * step. The module SEEDS the flag from the persisted value at import
 * (no mount-time hydration elsewhere — a re-read would be a no-op echo
 * unless storage and memory disagree, in which case it would clobber the
 * live flag). The `setItem` is guarded: a quota/lockdown throw must not
 * take down the toggle's click handler (the state flip + notify still
 * happen).
 */

const STORAGE_KEY = "side-pane-collapse";

let collapsed =
  typeof localStorage !== "undefined" &&
  localStorage.getItem(STORAGE_KEY) === "true";

const listeners = new Set<() => void>();

export function getSidePaneCollapsed(): boolean {
  return collapsed;
}

export function setSidePaneCollapsed(v: boolean): void {
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

export function subscribeSidePane(fn: () => void): () => void {
  listeners.add(fn);
  return () => {
    listeners.delete(fn);
  };
}
