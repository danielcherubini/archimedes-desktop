# The Picker and the Caret Can Disagree — 2026-10-10

## Summary

**One high finding with three visible faces, plus one medium.** `picker` state lags the caret, and the composer resolves that disagreement differently in different places, so the rows it offers and the span it overwrites can belong to two different tokens. The face that matters is not the wrong row: **the Enter that ADR 0035 holds can be released by moving the caret, which sends a raw absolute path to the model** — the defect ADR 0035 exists to prevent, reachable four commits after it shipped. A second, separate finding: `Escape` does not dismiss a note-only picker.

No fix is chosen here. This is a report, not a plan.

**Provenance.** Found while reviewing the out-of-Space `?` completion (ADR 0035, squash `c02ae68`). The desync itself predates that feature — it predates `?` entirely — but the feature gave it a worse payload (an absolute path instead of a Space-relative one) and a second entry point (the held Enter).

**What this replaces.** An earlier version lived at `docs/roadmap/composer-picker-caret-desync.md`. It described the mechanism correctly and its reproduction was exact, but it recommended as "the smallest honest change" an option that measurement shows does not work, understated the severity by stopping at "a wrong row", and overstated the Escape finding. Those are corrected below.

## Evidence provenance

- ✅ **re-verified in this session by hand**, on throwaway tests that were then deleted: the reproduction string; a caret moved off the token during an in-flight read **sends the raw `?/home/u/.config/htop/`**; the hold firing for a directory the caret is not on. Verified by reading: `activeIndex` is a clamp against the stale list (`ChatStream.tsx:752-754`); no `isComposing` or composition listener anywhere in `src/`; no click-away, blur or outside-pointer listener for the picker.
- ⚠️ **measured by a review pass and not re-run here**: the same-prefix silent case, the row-click path, the stale-and-swallowed arrow keys, the manufactured `<agent>` block, the crash variant when rows are re-derived without re-clamping, and the prototype that keeps all 1476 tests green while fixing the keystroke.

---

## The headline: the held Enter is aimed at the picker, not at the caret

ADR 0035 holds Enter for the duration of one out-of-Space directory read, so that a blank picker cannot turn a keystroke into a send. The hold's predicate is **half stale by construction**: `completionPending` comes from `picker.query → dirPrefix → useCompletionDir` (`ChatStream.tsx:451, 498`), while the token tests come from the caret (`ComposerRow.tsx:244-252`). Both measured here:

- **A caret move releases the hold and sends the raw path.** Select a directory, its listing is in flight, click the caret to index 0 — a click fires no `onChange`, so `picker` stays open while `token` is now `null` — press Enter once, and `sendPrompt` is called with `?/home/u/.config/htop/`: sigil, absolute path, trailing slash, delivered. The branch that handles "the caret left every token" closes the picker **and** sends on the same keystroke, which is correct in isolation and defeats the hold in combination.
- **The hold can fire for a directory the user is not completing.** With `?/etc/hos ?/var/li` in the draft, the picker owning the second token and its read in flight, putting the caret inside the **first** token — whose own read has settled and returned nothing — holds Enter. ADR 0033 requires that keystroke to send.

The first is a data-integrity bug in the direction the ADR was guarding. The second is the mirror: a keystroke swallowed for work nobody asked for.

## The mechanism

1. The picker's **token** — its prefix and query — is written in two places: `onChange` (`ComposerRow.tsx:153-166`) and `selectMention`'s directory-descent branch (`ChatStream.tsx:1232`). The other `setPicker` calls write only the highlight index or `null`. A mouse click, Home/End and ArrowLeft/Right move the caret **without** firing `onChange`, so the token the rows are derived from is whatever the last keystroke or descent left there.
2. `onKeyDown` re-derives the token at the caret (`ComposerRow.tsx:175-190`) and handles exactly one stale case: `picker && !token`, the caret left **every** token. It does not handle "the caret is inside a **different** token".
3. `filtered` is `pickerRows`, derived from `picker.query` (`ChatStream.tsx:515-704`). `selectMention` splices around `activeMentionToken(draft, caret)` (`ChatStream.tsx:1190-1193, 1214`) — the caret's token. **Rows come from one token, the span from another.**

`activeIndex` is a derivation, not state: `Math.min(picker.index, Math.max(0, filtered.length - 1))` at `ChatStream.tsx:752`, clamped against the stale list because it is the only one it can see. So `filtered[activeIndex]` is in range today, and **any fix that re-derives the rows must re-clamp the index in the same breath** — a fresh short list under a stale index is `selectMention(undefined)`.

## The other visible faces

Same root, measured by the review pass:

- **Cross-prefix:** `@sc and ?/etc/hos`, caret to index 2, Enter → `/etc/host c and ?/etc/hos`. A `?`-listing row inserted into an `@` token.
- **The row click is the same code path.** `ComposerMentions` hands `onMouseDown` to `selectMention`, so a mouse click with the caret in another token produces the same string. This is not a keyboard-only defect.
- **Same prefix is silent:** `?aaa and ?bbb`, caret to index 3, inserts a row of the other token while the rows on screen look unchanged.
- **The reverse direction manufactures a Mention:** `?/etc/hos and @sc` with the caret at 5 inserts `@scout ` at the start of the path, and the next send carries a real `<agent name="scout">` block. ADR 0031's exact-name rule is what keeps this a wrong block rather than an injected one.
- **Arrow keys are stale and consumed:** while a stale picker is open, ↑/↓ are `preventDefault`ed against the old row count, so the caret cannot even be moved with them.

## Medium: a note-only picker cannot be dismissed with Escape

The `Escape` branch lives inside `if (picker && token && filtered.length > 0)` (`ComposerRow.tsx:204`, `Escape` at `:231`). With a note and zero rows — the capped-listing state — the branch is unreachable and nothing else listens: **while the caret is inside the token**, Escape, a click on the note, a click anywhere else, and blurring the textarea all leave the box on screen. There is **no click-away handler at all**, which is the more useful way to put it: the box survives every pointer gesture, and it dies only when a keydown finds the caret outside a token (`:181-190`), which is also why the earlier claim that it "cannot be dismissed" is too strong.

One correction to the earlier version of this document: the **in-flight window is not this state**. With no rows and no note the box renders nothing at all — pinned by `Enter during an in-flight descent does NOTHING`. So the fix is small and Escape-shaped, and hoisting it must carry the existing `preventDefault` with it, which is what currently protects a native selection-clear.

## What a fix has to survive

- **ADR 0033: a picker is never a gate.** A stray or relative `?` with zero rows still **sends** on Enter. The amended wording in that ADR is the clause that matters here: Enter sends *unless a fetch for a token that cannot be answered from what is on screen is still in flight* — and that is exactly the clause the caret desync currently breaks.
- **ADR 0035: one round trip and no more**, with a rejected fetch settling immediately.
- `Shift+Enter` is a newline and `Shift+Tab` moves focus backward; both fall through today and intercepting either is an a11y trap.
- **Composition.** The handler reads no `isComposing` anywhere in `src/`, so the Enter that commits an IME candidate inserts the highlighted row, or sends a half-composed draft. A rewrite of this handler that does not add the guard keeps that defect.
- **One anchor, one extent.** `selectMention` reads `selectionStart` alone, so Enter with text selected leaves the selection in place (`x $de` → `x $debug de`). Whichever token wins, say what happens to the extent.

## Options, and the measurement that ranks them

The tempting fix — recompute `picker` in `onKeyDown` when the caret's token differs — **does not work as written**, because the handler's `filtered` is the array the previous render derived from the stale picker and `setPicker` cannot change it before the handler returns. Measured both readings: re-aim and insert anyway, and the wrong row is inserted unchanged with the **whole suite green**; re-aim and return, and `the in-flight gate holds ONLY the token under the caret, never another ? token` reddens, because a token with no rows and no read in flight must send. A green suite and an intact bug is the dangerous reading, and it is the one the earlier version of this document pointed at.

**The option measurement supports** is to re-derive **rows and index** from the caret's token at the moment of use, from one pure function that answers from the catalogs already in memory — every Mention catalog and the Space Listing are in memory, so only an out-of-Space directory needs a round trip — and to hold Enter **only** when the answer requires that read. A prototype did this in about fifteen lines in the handler plus a small oracle in `ChatStream`, kept all 1476 tests green, fixed the keystroke in both directions, and kept the hold aimed at the caret. It is not shippable as written: its oracle duplicated `pickerRows`, and the shippable form extracts one derivation that both the `useMemo` and the handler call, so the file has **one** row derivation and **one** index clamp. Its honest limit is that it fixes the keystroke, not the display: between the click and the next key, the box still shows the other token's rows.

Options that do not earn their place: keying `picker` by the token object (impossible — `activeMentionToken` allocates a fresh object every call, so identity never matches); passing the token into `selectMention` (its derivation is the one that is already **correct**); a render-side guard hiding disagreeing rows (`ComposerMentions` cannot see the caret without caret state, and `pickerRows` deliberately refuses a caret read because a ref read inside a `useMemo` is invisible to the dep graph).

**Making the caret React state** is the only design that makes the disagreement unrepresentable and is the right destination, but it re-renders on every caret move and makes the two imperative writers load-bearing: the `requestAnimationFrame` caret restore in `selectMention` and the insert-event restore, both of which fire neither `onChange` nor `onKeyDown`. Note also that the listener this invites is `selectionchange`, not `select` — a caret-only move selects nothing and never fires `select`.

## Open questions

- Which token wins is a decision, and the two candidate rules are visible to the user: keep the rows until a key arrives, or dismiss the moment the caret moves.
- Should the highlight index survive a caret move into another token, clamp, or reset to 0?
- Does the display get fixed in the same change as the keystroke, or is the stale box acceptable for the duration of one keystroke?

## Process note

The review pass that produced the option measurement left its prototype in the working tree — `src/components/ChatStream.tsx`, `src/components/chat/ComposerRow.tsx` and a scratch test file — while reporting a clean tree. It was caught before any commit and discarded; `main` never saw it. It is recorded here because a report asserting a tree state is not the tree state, which is the same failure mode this branch keeps finding in its own comments. The same pass could not reproduce the `SettingsPage.test.tsx` flake in four full runs and three isolated ones, after this document had asserted the flake was a known nuisance inherited from elsewhere.
