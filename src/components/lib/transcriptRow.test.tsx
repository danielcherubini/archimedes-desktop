import { render } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { TRANSCRIPT_ROW } from "./transcriptRow";
import ToolCallCard from "../ToolCallCard";
import ChangesGroupCard from "../ChangesGroupCard";
import { Reasoning, ReasoningTrigger } from "../Reasoning";

/**
 * Every collapsible row of the transcript — `Thought`, the tool rows (`Ran`,
 * `Edited`, …), `Changes` — is ONE row type: the icon + label ride the
 * transcript's content edge, and nothing else. They previously differed in
 * two ways that made the stream read as two mis-indented columns: the tool
 * rows carried a hover BAND (`h-8` + `px-2` + `hover:bg-surface-hover`) that
 * pushed their icons 8px right of the prose, and made them 32px tall against
 * `Thought`'s 21px. The rows now share the `TRANSCRIPT_ROW` shape (the
 * `Thought` row's own classes) — asserted HERE by token, since jsdom has no
 * layout engine.
 */

function tokens(el: HTMLElement): string[] {
  return el.className.split(/\s+/).filter(Boolean);
}

function stubDom() {
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
}

/** The row's toggle element, whichever row type rendered. */
function rowOf(container: HTMLElement): HTMLElement {
  const row =
    container.querySelector<HTMLElement>('[data-testid="reasoning-trigger"]') ??
    container.querySelector<HTMLElement>("button");
  if (!row) throw new Error("no transcript row rendered");
  return row;
}

describe.each([
  [
    "the tool row",
    () =>
      render(
        <ToolCallCard
          title="bash"
          status="completed"
          rawInput={{ command: "ls" }}
        />,
      ),
  ],
  [
    "the Changes row",
    () =>
      render(
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
      ),
  ],
  [
    "the Thought row",
    () =>
      render(
        <Reasoning>
          <ReasoningTrigger />
        </Reasoning>,
      ),
  ],
])("%s is the shared transcript row", (_name, renderRow) => {
  it("carries the shared row shape", () => {
    stubDom();
    const { container } = renderRow();
    const own = tokens(rowOf(container));
    for (const cls of TRANSCRIPT_ROW.split(" ")) {
      expect(own, _name).toContain(cls);
    }
  });

  it("has NO hover band and NO inset (no background, no padding, no bleed, no fixed height)", () => {
    stubDom();
    const { container } = renderRow();
    const own = tokens(rowOf(container));
    expect(own.filter((c) => c.includes("bg-"))).toEqual([]);
    expect(own.filter((c) => /^-?(m|p)[xy]?-/.test(c))).toEqual([]);
    expect(own.filter((c) => /^h-/.test(c))).toEqual([]);
    expect(own.filter((c) => /^w-/.test(c))).toEqual([]);
  });

  it("reveals its chevron on hover of the ROW (the one hover affordance it keeps)", () => {
    stubDom();
    const { container } = renderRow();
    const row = rowOf(container);
    // The LAST svg is the chevron (the row's leading icon comes first). SVG
    // `className` is an `SVGAnimatedString`, so read the attribute.
    const svgs = row.querySelectorAll("svg");
    const chevron = svgs[svgs.length - 1] as SVGSVGElement;
    expect(row.className).toMatch(/group\/[a-z-]+/);
    const chevronClass = chevron.getAttribute("class") ?? "";
    expect(chevronClass, _name).toContain("opacity-0");
    expect(chevronClass, _name).toMatch(/group-hover\/[a-z-]+:opacity-100/);
  });
});
