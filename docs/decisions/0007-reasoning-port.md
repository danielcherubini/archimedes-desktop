---
status: accepted
date: 2026-09-23
superseded-by:
---

# Reasoning display: port ZCode's `Reasoning` component faithfully, not by hand

The Client needed to display the agent's streamed internal reasoning (ACP `agent_thought_chunk` — previously dropped by both the Rust persistence layer and the frontend reducer) "like how ZCode does it". We decided to **port ZCode's `Reasoning` component faithfully** — `packages/ui/src/components/ai-elements/reasoning.tsx` (the collapsible `Reasoning` + `ReasoningTrigger` + `ReasoningContent`), its `ToolCallBlocks/QueuedSummaryContent.tsx` (the 254-line framer-motion "roll" for the one-line live summary), and its `mentions/components/scrollMask.ts` util — into `src/components/` / `src/lib/`, adapting only the imports (the already-ported `Collapsible` primitive, `cn`, `lucide-react`), the i18n lookups (→ literal English strings), and the TID constants (→ literal `data-testid` strings). This bends ADR 0006's "port the token layer and primitives, NOT the view components" rule for this one component.

Considered and rejected:

- **Simplified port (plain auto-scrolled summary, no `QueuedSummaryContent`):** YAGNI, no new dependencies, ~95% visually identical — but the summary roll is a visible part of ZCode's display, and re-implementing the component by hand is exactly the drift ADR 0006 warned about.
- **Minimal indicator ("Thinking…" line + static content):** least code, but loses the core live affordance — seeing what the model is thinking in real time while collapsed.

Consequences: the Client gains two dependencies (`motion` for `QueuedSummaryContent`; `@radix-ui/react-use-controllable-state` — a transitive dep of `radix-ui` that pnpm's strict node_modules requires listing to import directly) and a ZCode-internal component to keep in sync with ZCode's `reasoning.tsx` when it changes. The component's behavior is otherwise verbatim: collapsed by default, auto-collapse on the streaming→done transition (unless the user interacted), duration tracking, the one-line live summary (last non-empty line, auto-scroll to end, gradient edge mask), and the content box (max-h-60, plain text — no markdown, a deliberate performance choice; left border rail; auto-follow-bottom while streaming; scroll masks; 300ms delayed unmount so the collapse animation can read the content height).
