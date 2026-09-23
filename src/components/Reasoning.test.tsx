import { render, screen, fireEvent, act } from "@testing-library/react";
import { describe, it, expect, vi, beforeAll, afterEach } from "vitest";
import { Reasoning, ReasoningTrigger, ReasoningContent, useReasoning } from "./Reasoning";

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
        <ReasoningTrigger streamingText="line one\n\nline two" />
      </Reasoning>
    );
    expect(screen.getByText(/line two/)).toBeDefined();
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

  it("duration updates", () => {
    vi.useFakeTimers();
    render(
      <Reasoning isStreaming={true}>
        <ReasoningTrigger />
      </Reasoning>
    );
    fireEvent.click(screen.getByTestId("reasoning-trigger"));
    expect(screen.getByText("1 seconds")).toBeDefined();
    act(() => vi.advanceTimersByTime(2000));
    expect(screen.getByText("2 seconds")).toBeDefined();
    // Re-render as not streaming
  });
});
