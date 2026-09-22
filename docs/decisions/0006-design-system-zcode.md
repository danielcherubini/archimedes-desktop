---
status: accepted
date: 2026-09-22
superseded-by:
---

# Design system: adopt ZCode's token layer and primitives as the Client's UI source of truth

The Client's UI was styled with ad-hoc Tailwind values (`bg-neutral-950`, `border-neutral-800`, `bg-sky-600`) with no shared design language. We decided to make the sibling `ZCode` codebase's design system the Client's source of truth: port its token layer (`packages/ui/src/styles.css` — the `--ui-font-size` scale, the `--color-*` semantic tokens, the `--font-mono` stack, and the `theme-zai-light`/`theme-zai-dark` theme blocks) into `src/index.css`, port its ~15 shadcn-style primitives into `src/components/ui/`, and restyle the Client's existing components in place against it. The app applies `theme-zai-dark` on `document.documentElement` at boot (dark by default, no user-facing theme switcher — the Client has no settings surface in this scope).

Considered and rejected:

- **Port ZCode's view components and rewire them to the Client's stores:** faster to pixel-parity, but drags in ZCode-internal machinery (its own zustand stores, i18n provider, telemetry TID constants, dnd-kit, settings/config code the Client doesn't want) and 1000+ line orchestration files; the code stops reading like the Client.
- **Restyle by hand from the screenshot with no token layer:** the result drifts from ZCode's system and every future UI decision is a fresh guess.

Consequences: the Client's visual language is now coupled to ZCode's design system — a ZCode token rename or `DESIGN.md` rule change is a Client concern. All UI text uses the `text-ui-*` scale (never bare `text-sm`/`text-xs`/arbitrary px); all color comes from semantic `--color-*` tokens (the old `neutral-*`/`sky-*` values are gone). The `:root` defaults stay light; only `theme-zai-dark` is applied, so light is available later at zero cost if a theme switcher is ever added. The Rust/ACP layer and the Client's stores are untouched — this is a presentation-layer decision only.
