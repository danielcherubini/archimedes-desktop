/**
 * The side pane's collapsed flag — a tiny shared module (no store, no
 * React) so the `SidePane` frame (Task 4) and the header toggle (Task 6)
 * agree without event guessing: the toggle consumes this via
 * `useSyncExternalStore(subscribeSidePane, getSidePaneCollapsed)`.
 *
 * The module is the SINGLE OWNER of the persistence: `setSidePaneCollapsed`
 * writes `localStorage["side-pane-collapse"]` itself, so the toggle's
 * `setSidePaneCollapsed(!collapsed)` reaches localStorage with no extra
 * step. `SidePane` hydrates the flag from localStorage on mount by calling
 * `setSidePaneCollapsed` (a harmless same-value echo — the module seeds
 * its state from the persisted value at load).
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
  localStorage.setItem(STORAGE_KEY, String(v));
  for (const fn of [...listeners]) fn();
}

export function subscribeSidePane(fn: () => void): () => void {
  listeners.add(fn);
  return () => {
    listeners.delete(fn);
  };
}
