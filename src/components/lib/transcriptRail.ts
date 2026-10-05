/**
 * The transcript's ROW RAIL — the one left edge every row of the
 * conversation shares (the `MessageList`'s `p-4` content box): the user's
 * text, the agent's prose, `Thought`, the tool rows, `Changes`.
 *
 * The prose rows sit ON that edge (they have no inset of their own), but a
 * TOOL row is a hover BAND — it needs horizontal padding for the highlight
 * to breathe past its icon. Without help that `px-2` pushes the row's icon
 * 8px right of the prose (measured: prose/`Thought` at 16px, tool rows at
 * 24px — the transcript read as two mis-indented columns).
 *
 * The fix is the classic bleed: keep the `px-2` inset for the band's own
 * breathing room and pull the row back out by the same amount with
 * `-mx-2`, so the ICON lands on the shared rail while the band still bleeds
 * 8px past it. The `w-[calc(100%+1rem)]` re-adds what the negative margins
 * take from the row's own width (a `w-full` + `-mx-2` band would bleed the
 * LEFT side only, ending 8px short of every other row's right edge). The
 * bleed stays inside the container's 16px padding box, so it never opens a
 * horizontal scrollbar.
 */
export const TRANSCRIPT_ROW_BLEED = "-mx-2 w-[calc(100%+1rem)] px-2";
