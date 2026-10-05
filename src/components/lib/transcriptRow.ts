/**
 * The transcript's ROW shape — the one set of classes every collapsible row
 * of the conversation uses: `Thought`, the tool rows (`Ran`, `Edited`, …) and
 * `Changes`. It is the `Reasoning` trigger's own shape, verbatim.
 *
 * A row is a PLAIN row: its icon + label ride the transcript's content edge
 * (the `MessageList`'s `p-4`), the row hugs its content (`inline-flex` +
 * `max-w-full`, so the chevron follows the text instead of being pushed to
 * the far edge), and there is NO hover band.
 *
 * The tool rows used to be `h-8 w-full px-2 rounded-lg hover:bg-surface-hover`
 * — a BAND. Two visible consequences: their icons sat 8px right of the prose
 * and of `Thought` (measured 24px vs 16px — the stream read as two
 * mis-indented columns), and they were 32px tall next to `Thought`'s 21px, so
 * the rows marched to different rhythms. Making the band's padding a bleed
 * fixed the indent but left the grey rectangle and the 32px stride; the row
 * type is simply not a band, so the band is gone.
 *
 * The one affordance a row keeps is its chevron, revealed on hover of the row
 * (each row's own `group/…` + `group-hover/…:opacity-100`).
 */
export const TRANSCRIPT_ROW =
  "inline-flex max-w-full min-w-0 items-center gap-2 self-start text-ui-base transition-colors";
