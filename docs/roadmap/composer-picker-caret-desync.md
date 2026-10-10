---
status: draft
done-when: with the picker open, clicking the caret into a DIFFERENT token and pressing Enter inserts a row belonging to the token under the caret and never a row from the token that was clicked away from; a test drives exactly that click-then-Enter sequence and reddens when the reconciliation is removed; and a keystroke is still never swallowed for a stray sigil in prose.
---

# The Picker and the Caret Can Disagree — Plan

**Goal:** make the composer insert what the caret is actually pointing at, and remove a class of bug rather than one instance of it.

**`status: draft` means the fix is NOT chosen.** The defect is reproduced and its mechanism is confirmed; the options below carry real trade-offs about what Enter is allowed to do, and that is a decision rather than a step.

**Provenance.** Found while reviewing the out-of-Space `?` completion (ADR 0035, squash `c02ae68`). **Pre-existing and untouched by that feature**, which only widened the payload from a Space-relative path to an absolute one — and a wrong absolute path is louder than a wrong relative one, which is why it surfaced now.

---

## The mechanism, in three verified steps

1. `picker` state is recomputed **only** in `onChange` (`src/components/chat/ComposerRow.tsx:153-166`). A mouse click, Home/End and ArrowLeft/Right move the caret **without** firing `onChange`.
2. `onKeyDown` therefore re-derives the token at the caret (`ComposerRow.tsx:175-190`) and handles exactly one stale case: `picker && !token` — the caret left **every** token, so the picker is closed. It does **not** handle `picker && token && token-differs-from-the-one-the-picker-was-opened-for`.
3. The insertion branch then runs with the two halves of that disagreement: `filtered[activeIndex]` comes from the **stale** `picker`'s query (`ChatStream.tsx` derives `filtered` from `picker.query`), while `selectMention` splices around `activeMentionToken(draft, caret)` — the **fresh** token at the caret (`ChatStream.tsx:1190-1193, 1214`).

So rows are chosen from one token and the splice span is taken from another. Measured during review: draft `@sc and ?/etc/hos`, caret clicked to index 2 (inside `@sc`), Enter → `/etc/host c and ?/etc/hos`. A `?`-listing row was inserted into an `@` token.

## Why it deserves a structural fix and not a guard clause

The same three-line disagreement now has to be resolved by **three** separate consumers: the insertion branch, the `completionPending` hold added by `560f610`, and `selectMention`'s own token re-derivation. Each of them re-derives the token and each of them has to remember to be consistent, and `ChatStream.tsx` already carries a comment warning that the ordering of `?` branches is behaviour-bearing. A fourth consumer will forget.

The structural version makes `picker` a **derivation** of `(draft, caret)` rather than state that lags it, which is what the `onChange` handler is already trying to be and cannot. Caret position is not currently React state, so that is the actual change: introduce it, or recompute at the moment of use everywhere. Note the `requestAnimationFrame` caret restore in `selectMention` — any design must say what happens when the caret is restored programmatically, since that fires neither `onChange` nor `onKeyDown`.

## The options, and what each one costs

1. **Recompute `picker` inside `onKeyDown` whenever the caret's token differs from the picker's.** The smallest honest change. It costs one keystroke: the Enter that discovered the mismatch cannot also insert, because the rows for the new token have not been derived — and for an out-of-Space token they may not have been **fetched**. So the first Enter re-aims the picker and the second one inserts. That is defensible and it is also exactly what ADR 0035 just fixed for the descent window, so read ADR 0035's consequence about holding Enter before choosing it.
2. **Track the caret as state** and derive the token — hence the picker — from `(draft, caret)` on every render. Removes the class rather than the instance, and makes `onChange`'s `setPicker` disappear. Costs: the caret is written by imperative code paths (`selectMention`'s rAF restore, attachment insertion) and every writer becomes load-bearing, which is the same coupling ADR 0030 warns about in miniature.
3. **Close the picker on any caret movement** by adding a `select`/`click` listener that clears it. Cheapest to reason about, safest, and it costs the mouse user their highlighted row every time they move the caret — which is why it is listed last and not first.
## Secondary, same handler, same file: `Escape` does not always dismiss

The `Escape` branch sits **inside** `if (picker && token && filtered.length > 0)` (`ComposerRow.tsx:204`, the branch; `Escape` at `:231`). When the picker is rendered because of a `note` with zero rows — the capped-listing case, and the in-flight window ADR 0035 describes — **Escape does nothing**, because the branch is unreachable. The box is on screen and the key that should dismiss it is not intercepted. Fix by hoisting dismissal out of the rows branch; keep `Escape` from stealing a native selection-clear in a textarea that has text selected, which is a reason to look at where it lives before moving it.

## Invariants whatever fix is chosen must keep

- **ADR 0033: a picker is never a gate.** A stray `?`, `$`, `@` or `#` in prose with zero rows still SENDS on Enter, never swallowed. `ChatStream.tsx`'s pending-Enter comment records why the `completionPending` flag alone cannot widen that rule to prose; do not widen it.
- **ADR 0035's held Enter** stays held for exactly one in-flight read and never longer, with a rejected fetch settling immediately.
- `Shift+Enter` is a newline and `Shift+Tab` moves focus backward. Intercepting either is a keyboard/a11y trap; both fall through today.
- Arrow-wrap uses `% filtered.length` against the ONE bounded rendered list; `activeIndex` arrives as a prop derived in `ChatStream` (`ComposerRow.tsx:43,102`), so it is upstream state too and part of the same staleness question.
- Insertion stays VERBATIM for `?` rows (case preserved, no Mention lowercase rule applied to a path) and keeps the `?` for a DIRECTORY row.
- Gates: `pnpm test`, `pnpm build`. Rust untouched, so `cargo test` / `cargo clippy --all-targets` / `cargo fmt --check` only if Rust is touched.
- Known pre-existing flake, not yours: `src/components/settings/SettingsPage.test.tsx`.

## Open questions

- Should a click into a different token preserve the highlight index, clamp it, or reset it to 0? Every option is visible to the user.
- Does the fix belong in `ComposerRow` (where the key handling is) or in `ChatStream` (where `picker` lives)? The split today is that `ComposerRow` owns the caret and `ChatStream` owns the catalogs, and the bug lives precisely on that seam.
