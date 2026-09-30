import { render } from "@testing-library/react";
import { describe, it, expect, beforeAll, vi } from "vitest";
import MessageBubble from "./MessageBubble";
import { Message } from "../store/sessions";

/**
 * The `AgentMarkdown` scale, asserted through the REAL `ReactMarkdown`
 * renderer (unlike `MessageBubble.test.tsx`, which mocks the module to spy
 * on parse frequency). These tests pin the ZCode markdown scale that
 * `AgentMarkdown` ports from `zcode/packages/ui/src/components/ai-elements/`:
 * root `leading-[1.75] tracking-wide`, `mt-6 mb-4` headings, margin-less
 * plain `p`s, `my-3`/`space-y-1.5` lists with subtle markers, the
 * `border-l-2` blockquote, the bordered scrollable table frame with
 * `px-3 py-2` cells, and the dotted hover-underline link.
 */
function renderAgentText(text: string) {
  const message: Message = { kind: "agent-text", messageId: "m1", text, at: 1 };
  return render(<MessageBubble message={message} />);
}

// The agent-text branch wraps `AgentMarkdown` in one `text-ui-base` div.
function markdownRoot(container: HTMLElement): HTMLElement {
  const wrapper = container.firstElementChild as HTMLElement;
  return wrapper.firstElementChild as HTMLElement;
}

beforeAll(() => {
  Object.defineProperty(window, "matchMedia", {
    writable: true,
    value: vi.fn().mockImplementation((query) => ({
      matches: false,
      media: query,
      onchange: null,
      addListener: vi.fn(),
      removeListener: vi.fn(),
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
      dispatchEvent: vi.fn(),
    })),
  });
});

describe("AgentMarkdown (the ZCode markdown scale)", () => {
  it("renders the markdown root on the ZCode reading scale", () => {
    const { container } = renderAgentText("hello");
    const root = markdownRoot(container);
    expect(root.className).toContain("text-ui-base");
    expect(root.className).toContain("leading-[1.75]");
    expect(root.className).toContain("tracking-wide");
    expect(root.className).toContain("[&>*:first-child]:mt-0");
    expect(root.className).toContain("[&>*:last-child]:mb-0");
  });

  it("renders h1 on the ZCode heading scale (mt-6 mb-4 text-ui-xl font-semibold)", () => {
    const { container } = renderAgentText("# Big");
    const h1 = container.querySelector("h1");
    expect(h1?.className).toContain("mt-6");
    expect(h1?.className).toContain("mb-4");
    expect(h1?.className).toContain("text-ui-xl");
    expect(h1?.className).toContain("font-semibold");
  });

  it("renders h6 at the bottom of the ZCode weight ramp (text-ui-base font-normal)", () => {
    const { container } = renderAgentText("###### Small");
    const h6 = container.querySelector("h6");
    expect(h6?.className).toContain("mt-6");
    expect(h6?.className).toContain("mb-4");
    expect(h6?.className).toContain("text-ui-base");
    expect(h6?.className).toContain("font-normal");
  });

  it("renders a plain p WITHOUT a margin class (the ZCode scale — paragraphs ride the 1.75 leading)", () => {
    const { container } = renderAgentText("para one\n\npara two");
    const ps = container.querySelectorAll("p");
    expect(ps.length).toBe(2);
    // Unmapped (or margin-less) p: no `my-*` class of its own.
    for (const p of ps) {
      expect(p.className).not.toMatch(/my-/);
    }
  });

  it("renders strong as font-medium (not the default bold)", () => {
    const { container } = renderAgentText("some **bold** text");
    const strong = container.querySelector("strong");
    expect(strong?.className).toContain("font-medium");
  });

  it("renders ul on the ZCode list scale (my-3, space-y-1.5, subtle marker)", () => {
    const { container } = renderAgentText("- a\n- b");
    const ul = container.querySelector("ul");
    expect(ul?.className).toContain("my-3");
    expect(ul?.className).toContain("list-disc");
    expect(ul?.className).toContain("space-y-1.5");
    expect(ul?.className).toContain("marker:text-foreground-subtlest");
    const li = container.querySelector("li");
    expect(li?.className).toContain("pl-1");
  });

  it("renders blockquote on the ZCode scale (border-l-2 pl-3, subtle text)", () => {
    const { container } = renderAgentText("> quoted words");
    const blockquote = container.querySelector("blockquote");
    expect(blockquote?.className).toContain("my-4");
    expect(blockquote?.className).toContain("border-l-2");
    expect(blockquote?.className).toContain("pl-3");
    expect(blockquote?.className).toContain("text-foreground-subtle");
  });

  it("renders links on the ZCode scale (icon-blue, dotted, underline on hover)", () => {
    const { container } = renderAgentText("[example](https://example.com)");
    const a = container.querySelector("a");
    expect(a?.className).toContain("text-ui-base");
    expect(a?.className).toContain("font-medium");
    expect(a?.className).toContain("text-icon-blue");
    expect(a?.className).toContain("no-underline");
    expect(a?.className).toContain("decoration-dotted");
    expect(a?.className).toContain("hover:underline");
  });

  describe("tables (the ZCode markdown-table scale)", () => {
    const tableMarkdown = "| a | b |\n| - | - |\n| 1 | 2 |\n| 3 | 4 |";

    it("wraps the table in a bordered, horizontally-scrollable frame", () => {
      const { container } = renderAgentText(tableMarkdown);
      const table = container.querySelector("table");
      const frame = table?.parentElement;
      expect(frame?.className).toContain("overflow-x-auto");
      expect(frame?.className).toContain("rounded-xl");
      expect(frame?.className).toContain("border-border");
      // The table itself is content-width but at least fills the frame.
      expect(table?.className).toContain("w-max");
      expect(table?.className).toContain("min-w-full");
    });

    it("styles header cells (px-3 py-2, border-b, unbolded, subtle)", () => {
      const { container } = renderAgentText(tableMarkdown);
      const th = container.querySelector("th");
      expect(th?.className).toContain("px-3");
      expect(th?.className).toContain("py-2");
      expect(th?.className).toContain("border-b");
      expect(th?.className).toContain("border-border");
      expect(th?.className).toContain("font-normal");
      expect(th?.className).toContain("text-foreground-subtlest");
      expect(th?.className).toContain("text-left");
    });

    it("styles body cells (px-3 py-2, border-b, align-top, wrapping)", () => {
      const { container } = renderAgentText(tableMarkdown);
      const td = container.querySelector("td");
      expect(td?.className).toContain("px-3");
      expect(td?.className).toContain("py-2");
      expect(td?.className).toContain("border-b");
      expect(td?.className).toContain("border-border");
      expect(td?.className).toContain("align-top");
      expect(td?.className).toContain("break-words");
    });

    it("highlights rows on hover and drops the last row's border", () => {
      const { container } = renderAgentText(tableMarkdown);
      const tr = container.querySelector("tbody tr");
      expect(tr?.className).toContain("hover:bg-hover/20");
      expect(tr?.className).toContain("last:[&>td]:border-b-0");
    });
  });
});
