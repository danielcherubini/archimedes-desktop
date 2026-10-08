import { useState } from "react";
import { PlusIcon } from "lucide-react";
import { basenameOfPath } from "../lib/paths";
import { useSessions } from "../store/sessions";
import NewSpaceDialog from "./NewSpaceDialog";

/**
 * The Spaces as browser-style tabs (the top of the chat column — the old
 * session header row's position): one tab per Space (its base name), the
 * active one selected; clicking a tab selects the Space (the store's
 * `selectSpace` — its most recent session opens: live first, then the
 * newest stored). The `+` opens the New-Space dialog (picking a folder
 * IS creating a space — the same flow as the sidebar's `Open Space`).
 *
 * The `...` menu (`New Session in this Space`) moved to the sidebar's
 * `Sessions` header (right-aligned, next to the word `Sessions`), and the
 * side-pane toggle moved to the `SidePane`'s footer — the tab bar is
 * tabs + the `+` only.
 */
export default function SpaceTabs() {
  const spaces = useSessions((s) => s.spaces);
  const activeSpacePath = useSessions((s) => s.activeSpacePath);
  const selectSpace = useSessions((s) => s.selectSpace);
  const [newSpaceOpen, setNewSpaceOpen] = useState(false);

  return (
    // The tabs live in the top CHROME BAR (the window titlebar row —
    // browser-style, like the reference screenshot): `h-full` (the bar
    // is 40px) + `flex-1` (fill the space between the logo segment and
    // the window controls). `pl-px`: the first tab sits 1px RIGHT of
    // the logo segment's right edge (the sidebar's 260px width) — the
    // user's "1px too far left" nudge. The tabs + `+` are left-aligned;
    // the empty strip after the `+` is just a spacer (the CHROME BAR
    // container itself is the drag region — the browser's empty tab
    // strip drags the window; double-click maximizes).
    <div
      className="flex h-full min-w-0 flex-1 items-center gap-1 pl-px pr-2"
      data-testid="space-tabs"
    >
      {spaces.map((s) => {
        const name = basenameOfPath(s.path) || s.path;
        const active = s.path === activeSpacePath;
        // ALL TABS ARE INSET PILLS — active and inactive share the pill
        // geometry (`h-8` inside the `h-10` bar, `rounded-md`). The active one
        // is the same pill lifted: a stronger fill (`bg-surface-hover` over
        // the `bg-surface` the inactive ones wear), full-contrast text and
        // `font-medium`. No bridge: this tab used to BE the sheet's top edge
        // (a full-height `rounded-t-md` tab with a hard 5px shadow merging it
        // into the chat slab) — the user found that a tab that is not the
        // same kind of object as its siblings read as inconsistent, so the
        // active tab is now the same pill, one step lifted.
        return (
          <button
            key={s.path}
            type="button"
            role="tab"
            aria-selected={active}
            onClick={() => selectSpace(s.path)}
            title={s.path}
            className={`max-w-48 truncate px-3 text-ui-base ${
              active
                ? "h-8 rounded-md bg-surface-hover font-medium text-foreground"
                : "h-8 rounded-md bg-surface text-foreground-subtle hover:bg-surface-hover"
            }`}
          >
            {name}
          </button>
        );
      })}
      <button
        type="button"
        aria-label="New space"
        onClick={() => setNewSpaceOpen(true)}
        className="size-8 shrink-0 rounded-md text-foreground-subtle hover:bg-surface-hover"
      >
        <PlusIcon className="size-4" />
      </button>
      {/* The empty strip after the `+` (the browser's empty tab strip —
          a plain spacer; the chrome bar container is the drag region).
          */}
      <div className="flex-1" />
      {newSpaceOpen && (
        <NewSpaceDialog onClose={() => setNewSpaceOpen(false)} />
      )}
    </div>
  );
}
