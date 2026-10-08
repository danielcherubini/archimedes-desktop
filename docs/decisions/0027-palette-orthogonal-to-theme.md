---
status: accepted
date: 2026-10-06
superseded-by:
---

# `Palette` is a color-scheme axis orthogonal to `Theme` (and Dracula pins dark)

> **Amended 2026-10-06** (same day, spec audit; see *Amendments* at the end). Two things
> in the original text were wrong or have since changed: the premise that
> Dracula "has no light variant" is **factually false** — the specification
> publishes Alucard Classic, an official light companion — and the decision to
> leave the `--color-terminal-*` tokens inherited has been **reversed**. The
> axis decision below stands; the premise it was argued from did not.

The Client ships its color tokens as CSS custom properties (`.dark` /
`.theme-zai-light` / `.theme-zai-dark` in `src/index.css`) and wants a second
color scheme — the user's Dracula palette, already the house palette for the
agent TUI (`pi-dracula`). Dracula was taken to have **no light variant**
(`dracula` and `dracula-soft` are both dark), while the existing persisted
`theme` setting is a light/dark **mode** (`system | dark | light`). We decided:
add a second persisted field `palette: "zai" | "dracula"` beside `theme` rather
than widening `theme` into a flat `zai-light | zai-dark | dracula` list, and
**`palette: "dracula"` pins the app dark** — the mode is ignored, `system`
included, so an OS-light user gets the dark app (the Settings row states
"Dracula is dark-only"). The syntax-highlight theme is *derived* from the
palette (Shiki `dracula` / `github-dark`), not a third setting.

**Why:** mode and scheme are genuinely independent axes — a future second dark
scheme, or an authored Dracula Light, must drop in without re-touching the mode
logic or the `system` → OS-scheme resolution. Collapsing them into one list
would conflate the two and make `system` incoherent (OS-light + a dark-only
scheme has no answer). Storage is free: `Settings.theme` is already a free-form
`String` with a serde default, so `palette` follows the `spinner_style:
Option<String>` precedent — a new field with `#[serde(default)]`, **no
migration**, and the corrupt-file → defaults path untouched (the same shape as
ADR 0026's fourth `api` value).

**Considered Options**

- **A third `AppTheme`** (`zai-light | zai-dark | dracula` in the one Theme
  picker): rejected — simplest UI, but it conflates mode with scheme and leaves
  `system` undefined against a palette with no light reading *of its own*.
- **Palette applies in dark only; light mode always renders Zai Light**:
  rejected — a `system` user on an OS-light machine who explicitly picked
  Dracula would silently get no Dracula.
- **Author a Dracula Light variant**: rejected — invented design rather than the
  palette supplied, and it doubles the token surface to maintain for a scheme
  the user did not ask for. (Amended note: this was argued against a
  hand-authored variant. There IS an official light variant, Alucard Classic,
  so "author one" is not the only alternative to "ship none" — but adopting it
  is still a separate decision, see *Amendments*.)

**Consequences**

- **`index.css` gains a fourth block, `.theme-dracula`, placed after
  `.theme-zai-dark`.** The layering already supports it: `.dark` is a COMPLETE
  dark palette (140 `--color-*` + 2 `--animated-gradient-text-*`),
  `.theme-zai-dark` redeclares **all** of them and adds 25 `file-*` descriptors
  of its own (167 total — it is a complete standalone dark palette, and `.dark`
  minus `.theme-zai-dark` is empty), and `.theme-zai-light` declares exactly
  `.dark`'s set. The mechanism that matters is the LAYERING — a `.theme-*` class
  over `.dark` — and because `.dark` itself declares every identity token, a new
  palette may be a PARTIAL block: `.theme-dracula` declares 83 structural
  overrides + 25 `file-*` restatements + 6 ramp tokens (114) and inherits 59 of
  `.theme-zai-dark`'s 167. (Originally 61 + 25 + 6 = 92 with 81 inherited; the
  amendment below moves the 22 terminal tokens from inherited to owned.)
- **`applySettingsTheme` must REMOVE the sibling palette class.** Both palette
  blocks are single-class selectors of equal specificity, so if two ever
  co-exist the winner is decided by **source order in `index.css`** — silently,
  and it would flip on any reorder. This is a standing constraint on every future
  palette.
- **A palette block must be COMPLETE, and every palette-owned token must be
  restated.** The 25 `--color-file-*` descriptor tokens ~~exist ONLY in
  `.theme-zai-dark`~~ — **Amended note: false as written.** `@theme` declares all
  25 (light-safe) and `.theme-zai-dark` declares all 25 (vivid dark); `.dark` and
  `.theme-zai-light` declare only the three `file-node*` role chips. The hazard
  this bullet is about is real, only the mechanism differs: a block that omits a
  descriptor does not fail, it resolves through `.dark` to the `@theme` light-safe
  value (muddy on a `#282a36` surface — 5 of 25 fall below the 3:1 floor:
  `file-ts`/`file-py` 2.97, `file-sass`/`file-graphql` 2.88, `file-css` 2.73).
  This is why the palette test is a completeness test, not a spot check.
- **A palette may NOT reuse a hue across semantic roles.** Dracula's `comment`
  `#6272a4` is a fine border (and is used as `border-hover`) but only **3.03:1**
  as text, so `foreground-subtle` gets a lightened reading (`#9aa4c4`, 5.75)
  while `foreground-subtlest` keeps the true comment. Similarly `destructive`
  `#ff5555` passes on the page surface (4.53) but fails on a raised row (2.91),
  so diffs use `pi-dracula`'s `diffAdd`/`diffDelete` instead. Hue faithfulness
  never outranks contrast.
- **Data-identity palettes stay inherited** (usage charts, context-breakdown,
  git status, trajectory, role chips) — **except the terminal, which the
  amendment below moves into palette ownership**. Those encode *what kind of
  thing this is*, not decoration; deriving them from Dracula's 11 hues would
  make distinct file types and git states visually indistinguishable.
- **The Shiki theme loads both themes in one `createHighlighter`** and picks per
  `codeToHtml` call, so switching palette re-highlights without rebuilding the
  module-level singleton. Note Shiki emits an **inline** `background-color` on
  its `<pre>`, which beats any Tailwind class — so a `bg-card` on the code body
  is already dead CSS under both palettes.

**Amendments (2026-10-06, same day, spec audit)**

A user looked at the shipped palette, said it looked bad, and pointed at
<https://draculatheme.com/spec>. Auditing against it found that the block had
been built from `pi-dracula` — a legacy community TUI port that disagrees with
the current specification — and had further invented hues on top. The axis
design in this ADR was never the problem; the VALUES were. Three things are
recorded here because they change what this ADR asserts.

1. **The "no light variant" premise was false, and the decision is re-grounded
   rather than reversed.** The specification defines two variants: Dracula
   Classic (dark, `#282A36`) and **Alucard Classic**, its complementary light
   theme (`#FFFBEB` background, its own core, ANSI and UI tables). So the
   orthogonality argument never needed Dracula to be dark-only, and the shipped
   dark-pinning is retained as an **explicit scope decision**, not as a
   consequence of the palette's nature: we ship Dracula **Classic** only, so
   `palette: "dracula"` resolves dark and ignores the mode, and the Settings row
   saying "Dracula is dark-only" describes what this app offers, which is true.
   Adding Alucard Classic later is precisely the case the two-axis design was
   bought for — it drops in as a light reading of the palette axis without
   touching the mode logic or the `system` resolution. It is NOT implemented:
   it doubles the palette's token surface and its own elevation ladder and
   accent set for a variant nobody asked for. Note this also means "Dracula" is
   now an ambiguous label in the UI for a *variant* of a *family*; if Alucard
   ever ships, the picker names the variant.
2. **`--color-terminal-*` is now palette-owned** (all 22 tokens, in
   `.theme-dracula`). **Correcting the record: an earlier draft of this
   amendment asserted "the app has a real terminal surface". It does not.**
   Verified: no xterm dependency, no PTY dependency, no terminal component, and
   zero `(bg|text|border)-terminal-*` class consumers. The original reasoning
   ("no terminal component") was *true*; what was wrong was the second half —
   that an ANSI set would have to be *invented*. It does not: the specification
   publishes a 16-colour ANSI table to copy verbatim. So the honest basis for
   owning these tokens is (a) the values are given, not invented, (b) the block
   is otherwise exhaustive, and a partially-owned palette is the failure mode
   `paletteCompleteness.test.ts` exists to prevent, and (c) there IS one live
   effect today: `@theme` aliases `--color-icon-blue` to
   `--color-terminal-bright-blue`, so markdown links and diff `@@` hunks take
   the ANSI bright-blue `#d6acff` under Dracula (7.60 on the page, 6.29 on a
   floating panel, 5.15 on a card — all above the text floor). The rest are
   forward-declared for a terminal that does not exist yet.
   `paletteCompleteness.test.ts`'s `INHERITED` set therefore drops the 22 names
   (81 → **59**) and pins all 22 against the spec's ANSI table by exact value —
   name-only completeness would not catch a restated-but-wrong hue.
   This narrows the identity rule, it does not remove it: the ANSI set is a
   *protocol* (what a shell means by "red"), whereas `git-*`, `usage-*`,
   `context-breakdown-*`, `trajectory-*` and the role chips remain identity
   hues that encode *what a thing is* and stay inherited.
3. **The block's hexes are now gated against the specification.** "Use exact
   color values from this specification" is the spec's own quality standard, and
   name-level completeness could not enforce it — every token was in the right
   slot while carrying off-spec hues. `paletteCompleteness.test.ts` now rejects
   any hex in `.theme-dracula` that the spec does not publish, with exactly one
   allow-list: the 24 file-type descriptors (25 minus `file-css`, which is
   `#bd93f9` and so already on-spec). That exception is the **Identity color**
   rule knowingly outranking the spec — 11 hues cannot name 25 file types — and
   it is enumerated by name rather than granted to a whole family.
   Two further departures are recorded in the block's own doc-comment, both
   contrast-forced: state-indicator TEXT keeps the bright syntax hues because
   the Functional set fails the spec's own 4.5:1 floor as text (Functional
   Green 3.43, Functional Red 3.74) while those tokens are consumed as text far
   more than as fills — the Functional palette is instead applied to brand,
   focus and interactive borders, exactly where the spec points at it — and
   `--color-foreground-subtle` is Foreground at 70% rather than a hue, because
   the spec has no subtle-text token.

**Amendment (2026-10-06, later the same day) — the Consequences figures were
stale, and one of them was never true**

The `Consequences` list above quotes token counts taken from the stylesheet as
it stood when the ADR was written. Four commits later (`cc475d5`, `82662b1`,
`5417224` and this pass) they are wrong, and one of them was wrong on the day it
was written. The DECISION — the two axes, Dracula pinning dark, blocks restating
palette-owned tokens — is untouched; only the measurements are corrected, and
they are restated here rather than edited in place, so the original text stays
as the record of what was believed.

Every figure below was recomputed from `src/index.css` with the comments
stripped and declarations counted by balanced-brace parse (the same method
`paletteCompleteness.test.ts` uses), not copied from a prior document.

| claim as written above | measured now | status |
| --- | --- | --- |
| `.dark` is "140 `--color-*` + 2 `--animated-gradient-text-*`" | **147 `--color-*` + 2 = 149 declarations** | stale: `.dark` gained `--color-caution`, the five `--color-thinking-*`, and `--color-context-track` since |
| `.theme-zai-dark` is "167 total" | **168** | stale: it gained `--color-context-track` |
| "`.dark` minus `.theme-zai-dark` is empty" | **not empty — exactly the 6 ramp tokens** (`color-caution`, `color-thinking-low/medium/high/xhigh/max`) | false. It became non-empty the moment the ramp pair was introduced, and `paletteCompleteness.test.ts` now pins that difference as exactly those six |
| `.theme-zai-light` "declares exactly `.dark`'s set" | **143 declarations**; `.theme-zai-light` ⊆ `.dark`, and `.dark` minus it is the same 6 ramp tokens | still true in substance, stated exactly |
| `.theme-dracula` is "83 + 25 + 6 (114)" | **115 declarations = 84 structural + 25 descriptors + 6 ramp**, inheriting **59** of `.theme-zai-dark`'s 168 | stale by one (the `--color-context-track` role split); the block's own test pins 115 |
| the 25 `--color-file-*` descriptors "exist ONLY in `.theme-zai-dark`" | **`@theme` declares all 25**, and so does `.theme-zai-dark`; `.dark` and `.theme-zai-light` declare only the three `file-node*` role chips | **false, and it was false when written.** The hazard is real but the mechanism is different: a palette that omits a descriptor does not fail, it resolves through `.dark` to `@theme`'s light-safe reading — and 5 of those 25 fall below 3:1 on `#282a36` (`file-ts`/`file-py` 2.97, `file-sass`/`file-graphql` 2.88, `file-css` 2.73). That is why the restatement is forced. |
| `foreground-subtle` "gets a lightened reading (`#9aa4c4`, 5.75)" | `#9aa4c4` is gone; it is `color-mix(in oklab, #f8f8f2 70%, transparent)` — **7.34 on the page, 6.33 on `#343746`, 5.41 on `#424450`** | superseded by the spec audit (amendment 3 above), recorded here because the Consequences bullet still named the invented hex |
| `destructive` "`#ff5555` passes on the page (4.53) but fails on a raised row (2.91)" | `destructive` is `#ff6e6e`: **5.23 page / 4.33 on `#343746` / 3.54 on `#424450`**. `#ff5555` was 4.53 / 3.75 / 3.07 | both numbers stale, and the 2.91 quoted was `#ff5555` on Selection `#44475a`, not on a raised row |

Two further corrections that are about the app rather than the arithmetic, both
of the same kind as the retracted "no light variant" premise above — a claim
stated as fact that a grep would have falsified:

1. **There is no terminal.** An earlier draft of amendment 2 asserted the
   terminal tokens were owned because the app "has a real terminal surface". It
   does not: no terminal component, no `xterm`/`node-pty`/`portable-pty`/
   `alacritty`/`vt100` dependency, zero `(bg|text|border)-terminal-*` consumers.
   21 of the 22 tokens are forward declarations; the one live effect is
   `@theme`'s `--color-icon-blue` alias. See amendment 2, which already says
   this — the point recorded *here* is how it got written: a doc-comment
   asserting a component existed was trusted without being checked.
2. **`--color-background-win-alt` paints no titlebar.** It was documented as
   "the window's own darker surface … the titlebar band". It has zero JSX
   consumers, and the chrome bar (`App.tsx`, `data-testid="chrome-bar"`) is
   `bg-background`. It exists because `--color-background-alt` derives from it,
   and that does paint the content column.

**The generalisation, since this ADR has now retracted three separate
self-reported facts:** every quantitative claim in a design document here is
checkable by a script, and three that were written down confidently were not
checked. Count it, do not recall it. Where a claim is load-bearing it now lives
in a test that measures the stylesheet (`paletteCompleteness.test.ts`,
`themeTokens.test.ts`, `fileIconContrast.test.ts`) rather than in prose — which
is the only form of this correction that stops repeating itself.

**Amendment (the ladder, re-anchored one step down — everything a touch darker)**

A user reviewed the shipped Dracula palette and asked for the chrome/titlebar/
left pane and the gaps to read `#191A21`, the composer `#21222C`, and the chat
and right sidebar `#282A36` — "everything a touch darker." Those three regions
were moved, and because the user chose to shift the WHOLE ladder rather than
only the named regions, the spec's five steps re-anchored one rung down. The
DECISION is untouched (palette orthogonal to mode, the island layout, the
slab-float collision carried by shadow, the identity-hue rule); only the rung a
token sits on moved. Every value below is measured, not recalled.

| token | was | now | the spec's name of the new rung |
| --- | --- | --- | --- |
| `--color-frame` (titlebar, left sidebar, the gaps) | `#21222c` | `#191a21` | Background Darker |
| `--color-input` / `-popover-header` / `-panel` | `#21222c` | `#191a21` | Background Darker |
| `--color-background` (the page) / `--color-composer` | `#282a36` / `#282a36` | `#21222c` / `#21222c` | Background Dark |
| `--color-chat` / `--color-inspector` and every float | `#343746` | `#282a36` | Background |
| `--color-card` / `-secondary` / `-tag` / `-menu-hover` / `-context-track` | `#424450` | `#343746` | Background Light |
| `--color-card-selected` | `#44475a` | `#424450` | Background Lighter |

Consequences, all of them measured:

1. **The two deepest rungs merged.** Background Darker `#191a21` and Background
   Dark `#21222c` were adjacent rungs; a one-step-down shift of the whole
   content put the chrome (`frame`) and the recessed controls onto the SAME
   `#191a21`. The ladder test therefore grades four rungs, not five, and the
   merged plateau is asserted as one group. This is the app's choice of which
   token sits on which rung, not a change to the spec's ladder — the spec's
   `#191a21` and `#21222c` still exist; the app no longer uses `#21222c` for
   the chrome (it is now the page plane).
2. **The slab-float collision survived, one rung darker.** The slab and every
   float both read `#282a36`, so the fill-only separation is still exactly
   1.00 and the shadow is still the thing that carries a float
   (`slabFloatSeparation.test.ts` pins the slab at `#282a36`). Nothing about
   the island layout changed; only the colour the slab and its floats share.
3. **Selection is no longer spec Selection.** `card-selected` moved from
   `#44475a` (spec Selection) to `#424450` (spec Background Lighter). The
   selected state is still the brightest resting surface and still strictly
   above the raised card `#343746`. `#44475a` survives in exactly one place:
   the terminal's TRANSLUCENT selection overlay (`color-mix(… 55%,
   transparent)`), which sits over whatever the cell painted and is therefore
   not a resting surface. The selection-gate in
   `paletteCompleteness.test.ts` now names the terminal overlay as the sole
   raw-`#44475a` carrier.
4. **The context-bar track is `#343746`, not `#191a21`.** The composer island
   is the page plane `#21222c`, and against it the dark end (`#191a21`,
   1.10) falls below the 1.3 visibility floor while `#343746` (1.34) clears
   it — so the best-VISIBLE track is `#343746`, the same value the island
   layout picked before this shift. The track's gate recomputes the table and
   asserts the winner; it grades against `--color-composer`, not `--color-input`.
5. **Every text floor still holds; one exemption dropped off.** On the resting
   raised step `#343746`, `foreground-subtlest` (Foreground at 55%) now CLEARS
   4.5 (4.57), so it is no longer exempted there and the gate's "delete it
   from the floor" guard enforces that. It still cannot clear 4.5 on the
   selection STATE `#424450` (4.01), as can `destructive` (3.54); those two
   keep their exemptions on the selection state only. `destructive` also keeps
   its 4.3 exemption on the resting raised step (4.33) — the brightest red the
   spec publishes still cannot reach 4.5 there.
6. **The file-descriptor forbidden set shrank to one row.** The raised card
   `#343746` now clears 3:1 for ALL 25 descriptors (worst `file-py` 3.06), so
   the seven that failed on the old `#424450` (2.50) pass and `fileIconContrast.test.ts`
   lists no card failures. The selection state `#424450` remains the one
   forbidden surface with failures (7 of 25). The rendered surfaces (page
   `#21222c`, slab `#282a36`, panel `#191a21`) all clear 3:1, worst `file-py`
   3.69 on the slab.
7. **The terminal ANSI table did NOT move.** The 22 `--color-terminal-*` values
   are the spec's fixed 16-colour table plus `bg` = Background Darker; they
   are a protocol, not the UI ladder, and a one-step re-anchor of the UI must
   not re-hue a shell's red. `terminal-bg` stays `#191a21` and
   `terminal-black`/`cursor-accent` stay `#21222c` (spec AnsiBlack), so the
   terminal's paper and the app's chrome happen to share `#191a21` by the
   spec's own table rather than by design.

The `SPEC_HEX` gate is unchanged in strength: every resting surface still
resolves to a value the specification publishes (Background Darker / Dark /
Background / Light / Lighter), just mapped one rung down, and `#44475a` is
still spec-published (Selection) so its surviving use as the terminal's
translucent overlay is legal. The gate's job was never "use the ladder in the
order the spec prints it" — it was "use only the spec's published values," and
this shift does that.
