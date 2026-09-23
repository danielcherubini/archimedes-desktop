import { render, screen, fireEvent, act } from "@testing-library/react";
import { describe, it, expect, vi, beforeAll, afterEach } from "vitest";
import { Reasoning, ReasoningTrigger, ReasoningContent, useReasoning, shouldAutoCollapseReasoning, resolveReasoningStreamingSummary } from "./Reasoning";

// Stub matchMedia for Radix
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

afterEach(() => {
  vi.useRealTimers();
});

function Probe() {
  const { shouldRenderContent } = useReasoning();
  return <span data-testid="probe">{String(shouldRenderContent)}</span>;
}

describe("Reasoning component", () => {
  it("collapsed by default", () => {
    render(
      <Reasoning isStreaming={false}>
        <ReasoningTrigger />
        <ReasoningContent>body</ReasoningContent>
      </Reasoning>
    );
    expect(screen.getByText("Thought")).toBeDefined();
    expect(screen.getByText("a few seconds")).toBeDefined();
    expect(screen.queryByText("body")).toBeNull();
  });

  it("streaming label shows Thinking", () => {
    render(
      <Reasoning isStreaming={true}>
        <ReasoningTrigger />
      </Reasoning>
    );
    const trigger = screen.getByTestId("reasoning-trigger");
    expect(trigger.querySelector(".animated-gradient-text")).toBeDefined();
    expect(screen.getByText("Thinking")).toBeDefined();
  });

  it("live summary shows last non-empty line", () => {
    render(
      <Reasoning isStreaming={true}>
        {/* A JS expression, NOT a JSX string attribute: in a string
            attribute `\n` is a literal backslash-n (the split would be a
            no-op), so the escapes only process inside an expression.
            The trailing blank lines cover the trailing-blank-lines case. */}
        <ReasoningTrigger streamingText={"line one\n\nline two\n\n"} />
      </Reasoning>
    );
    const trigger = screen.getByTestId("reasoning-trigger");
    expect(trigger.textContent).toContain("line two");
    // Only the LAST non-empty line shows — the earlier one does not.
    expect(trigger.textContent).not.toContain("line one");
  });

  it("expand shows content", () => {
    render(
      <Reasoning isStreaming={false}>
        <ReasoningTrigger />
        <ReasoningContent>body</ReasoningContent>
      </Reasoning>
    );
    fireEvent.click(screen.getByTestId("reasoning-trigger"));
    expect(screen.getByText("body")).toBeDefined();
  });

  it("delayed content unmount", () => {
    vi.useFakeTimers();
    render(
      <Reasoning isStreaming={false}>
        <ReasoningTrigger />
        <ReasoningContent>body</ReasoningContent>
        <Probe />
      </Reasoning>
    );
    fireEvent.click(screen.getByTestId("reasoning-trigger"));
    expect(screen.getByTestId("probe").textContent).toBe("true");
    fireEvent.click(screen.getByTestId("reasoning-trigger"));
    expect(screen.getByTestId("probe").textContent).toBe("true");
    act(() => vi.advanceTimersByTime(300));
    expect(screen.getByTestId("probe").textContent).toBe("false");
  });

  it("shouldAutoCollapseReasoning logic", () => {
    expect(shouldAutoCollapseReasoning({ autoCollapseKey: "b", previousAutoCollapseKey: "a", userInteracted: false })).toBe(true);
    expect(shouldAutoCollapseReasoning({ autoCollapseKey: "b", previousAutoCollapseKey: "a", userInteracted: true })).toBe(false);
    expect(shouldAutoCollapseReasoning({ autoCollapseKey: null, previousAutoCollapseKey: "a", userInteracted: false })).toBe(false);
  });

  it("auto-collapse integration", () => {
    vi.useFakeTimers();
    const { rerender } = render(
      <Reasoning defaultOpen isStreaming={true} autoCollapseKey={null}>
        <ReasoningTrigger />
        <ReasoningContent>body</ReasoningContent>
        <Probe />
      </Reasoning>
    );

    expect(screen.getByTestId("probe").textContent).toBe("true");

    rerender(
      <Reasoning defaultOpen isStreaming={false} autoCollapseKey="complete">
        <ReasoningTrigger />
        <ReasoningContent>body</ReasoningContent>
        <Probe />
      </Reasoning>
    );

    act(() => vi.advanceTimersByTime(300));
    expect(screen.getByTestId("probe").textContent).toBe("false");
  });

  it("duration updates and freezes", () => {
    vi.useFakeTimers();
    const { rerender } = render(
      <Reasoning isStreaming={true} defaultOpen>
        <ReasoningTrigger />
      </Reasoning>
    );
    // Clicked trigger isn't strictly needed if defaultOpen is true
    expect(screen.getByText("1 seconds")).toBeDefined();
    act(() => vi.advanceTimersByTime(2000));
    expect(screen.getByText("2 seconds")).toBeDefined();

    rerender(
      <Reasoning isStreaming={false} defaultOpen>
        <ReasoningTrigger />
      </Reasoning>
    );

    act(() => vi.advanceTimersByTime(5000));
    expect(screen.getByText("2 seconds")).toBeDefined();
    expect(screen.queryByText("3 seconds")).toBeNull();
  });
});

describe("resolveReasoningStreamingSummary", () => {
  it("returns null for empty / whitespace-only input", () => {
    expect(resolveReasoningStreamingSummary("")).toBeNull();
    expect(resolveReasoningStreamingSummary("   ")).toBeNull();
    expect(resolveReasoningStreamingSummary(" \n  \n ")).toBeNull();
  });

  it("extracts the last non-empty line (trimmed)", () => {
    expect(resolveReasoningStreamingSummary("only line")).toEqual({
      key: expect.any(String),
      text: "only line",
    });
    const cases: Array<[string, string]> = [
      ["a\nb", "b"],
      ["a\n\nb\n\n", "b"],
      ["  spaced  ", "spaced"],
    ];
    for (const [input, expected] of cases) {
      const result = resolveReasoningStreamingSummary(input);
      expect(result).not.toBeNull();
      expect(result?.text).toBe(expected);
    }
  });

  it("treats CRLF line endings the same as LF", () => {
    const cases: Array<[string, string]> = [
      ["a\r\nb", "b"],
      ["a\r\n\r\nb\r\n", "b"],
    ];
    for (const [input, expected] of cases) {
      const result = resolveReasoningStreamingSummary(input);
      expect(result).not.toBeNull();
      expect(result?.text).toBe(expected);
    }
  });

  it("key is stable while the last line grows and changes when a new line starts", () => {
    const growing = resolveReasoningStreamingSummary("line one\nsecond");
    const grown = resolveReasoningStreamingSummary(
      "line one\nsecond (growing)",
    );
    expect(growing).not.toBeNull();
    expect(grown).not.toBeNull();
    if (!growing || !grown) return;
    expect(grown.key).toBe(growing.key);
    const next = resolveReasoningStreamingSummary(
      "line one\nsecond (growing)\nthird",
    );
    expect(next).not.toBeNull();
    if (!next) return;
    expect(next.key).not.toBe(grown.key);
    expect(next.text).toBe("third");
  });

  it("is O(last line), not O(whole text) — the summary runs on every render of the streaming trigger while the thinking text grows unboundedly", () => {
    const line = "x".repeat(1000);
    const small = `a\nb\n${line}`; // ~1KB
    const big = `y\n`.repeat(500_000) + line; // ~1MB, SAME last line
    const timeIt = (s: string) => {
      const t0 = performance.now();
      for (let i = 0; i < 10; i++) resolveReasoningStreamingSummary(s);
      return performance.now() - t0;
    };
    // A `replace`+`split` over the whole text makes the 1MB input orders of
    // magnitude slower than the 1KB one; a backwards line scan is ~equal.
    expect(timeIt(big) / Math.max(0.001, timeIt(small))).toBeLessThan(5);
  });
});
