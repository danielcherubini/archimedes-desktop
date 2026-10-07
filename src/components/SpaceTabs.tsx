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
        // THE BRIDGE TAB. The active tab is not a pill floating in the chrome
        // — it is the top edge of the chat sheet, so it has to be the SHEET'S
        // OWN FILL (`--color-tab-active` aliases `--color-chat`) AND
        // physically touch it, or the 4px frame gap reads as a slit cutting
        // one shape into two.
        //
        // The geometry (the bar is `h-10`; the chat `main` carries `m-1`, so
        // the sheet's top edge sits 44px below the top of the bar):
        //   `h-10`               the tab spans the bar — the Chrome idiom,
        //                        active fills the strip and inactive tabs are
        //                        inset pills.
        //   `shadow-[0_5px_0_0]` THE BRIDGE ITSELF, and deliberately not a
        //                        `translate-y`: a solid same-colour offset
        //                        shadow extends the FILL 5px below the box
        //                        (4px of frame gap + 1px of overlap, so no
        //                        subpixel rounding can re-open the seam)
        //                        while the box stays exactly where it was.
        //                        Translating instead would have dragged the
        //                        LABEL 5px out of line with the inactive
        //                        tabs, and moved what the pointer hits.
        //                        Blur 0 / spread 0 keeps it a hard rectangle;
        //                        it inherits `rounded-t-md`, so its own top
        //                        corners hide behind the box and only the
        //                        square bottom band shows — which is the
        //                        merge we want.
        //   `rounded-t-md`       square BOTTOM corners: a rounded bottom would
        //                        notch the seam instead of merging it.
        //   `relative z-20`      the fill now leaves the bar's box and must
        //                        paint over the slab, a LATER sibling with a
        //                        plain background. Without a transform there
        //                        is no paint-order accident to lean on, so the
        //                        stacking context is stated, not inherited.
        //
        // Inactive pills keep `rounded-md` and stay inset — they are objects
        // IN the chrome. Only the active one is the sheet.
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
                ? "relative z-20 h-10 rounded-t-md bg-tab-active font-medium text-foreground shadow-[0_5px_0_0_var(--color-tab-active)]"
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
