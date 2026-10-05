import { render } from "@testing-library/react";
import { describe, expect, it, vi, beforeEach } from "vitest";
import { TRANSCRIPT_ROW_BLEED } from "./transcriptRail";
import ToolCallCard from "../ToolCallCard";
import ChangesGroupCard from "../ChangesGroupCard";
import MessageBubble from "../MessageBubble";
import { Reasoning, ReasoningTrigger } from "../Reasoning";

/**
 * The transcript's rows must all START at the same left rail (the
 * `MessageList`'s `p-4` content box): the user's text, the agent's prose,
 * `Thought`, the tool rows, `Changes`. jsdom has no layout engine, so this
 * asserts the CONTRACT (the class tokens) that produces that alignment — the
 * bleed — rather than measured pixels.
 */

/** The one hover-band row (`h-8 … hover:bg-surface-hover`). */
function bandRow(container: HTMLElement): HTMLElement {
  const row = container.querySelector("button.group\\/tool-summary");
  if (!row) throw new Error("no tool-summary row rendered");
  return row as HTMLElement;
}

function tokens(el: HTMLElement): string[] {
  return el.className.split(/\s+/);
}

describe("the transcript row rail (every row shares one left edge)", () => {  beforeEach(() => {
    vi.stubGlobal(
      "matchMedia",
      (q: string) => ({
        matches: false,
        media: q,
        addEventListener: vi.fn(),
        removeEventListener: vi.fn(),
        addListener: vi.fn(),
        removeListener: vi.fn(),
        dispatchEvent: vi.fn(),
      }),
    );
    Element.prototype.scrollIntoView = vi.fn();
    HTMLElement.prototype.scrollIntoView = vi.fn();
  });

  it("the tool row BLEEDS: the `px-2` band padding is cancelled by `-mx-2` so its icon lands on the prose rail", () => {
    const { container } = render(
      <ToolCallCard
        title="bash"
        status="completed"
        rawInput={{ command: "ls" }}
      />,
    );
    const row = bandRow(container);
    for (const cls of TRANSCRIPT_ROW_BLEED.split(" ")) {
      expect(tokens(row)).toContain(cls);
    }
  });

  it("the `Changes` group row bleeds the same way (it is the same hover band)", () => {
    const { container } = render(
      <ChangesGroupCard
        messages={[
          {
            kind: "tool-call",
            id: "e1",
            title: "edit",
            status: "completed",
            rawInput: { path: "/a/b.ts", oldText: "x", newText: "y" },
          },
        ] as never}
      />,
    );
    const row = bandRow(container);
    for (const cls of TRANSCRIPT_ROW_BLEED.split(" ")) {
      expect(tokens(row)).toContain(cls);
    }
  });

  it("`Thought` rides the rail with NO inset of its own (it is the rail's reference row)", () => {
    const { container } = render(
      <Reasoning>
        <ReasoningTrigger />
      </Reasoning>,
    );
    const trigger = container.querySelector(
      '[data-testid="reasoning-trigger"]',
    ) as HTMLElement;
    const own = tokens(trigger).filter((cls) =>
      /^-?(mx|px|ml|pl|mr|pr)-/.test(cls),
    );
    expect(own).toEqual([]);
  });

  it("a user row rides the rail too (no inset) — the two reference rows agree", () => {
    const { container } = render(
      <MessageBubble
        message={{ kind: "user", id: "u1", text: "hello" } as never}
      />,
    );
    const own = tokens(container.firstElementChild as HTMLElement).filter(
      (cls) => /^-?(m|p)[xy]?-/.test(cls),
    );
    expect(own).toEqual([]);
  });
});
