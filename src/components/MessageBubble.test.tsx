import { render, screen } from "@testing-library/react";
import { describe, it, expect, beforeAll, afterEach, vi } from "vitest";
import MessageBubble from "./MessageBubble";
import { Message } from "../store/sessions";

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

describe("MessageBubble", () => {
  it("renders agent-thought as collapsed Reasoning block", () => {
    const message: Message = { kind: "agent-thought", messageId: "m1", text: "pondering", at: 1 };
    render(<MessageBubble message={message} />);
    
    // Check for "Thought" label and "a few seconds"
    expect(screen.getByText("Thought")).toBeTruthy();
    expect(screen.getByText("a few seconds")).toBeTruthy();
    
    // Content should be absent (Reasoning component behavior)
    expect(screen.queryByText("pondering")).toBeNull();
  });

  it("renders agent-thought as streaming with isStreaming", () => {
    const message: Message = { kind: "agent-thought", messageId: "m1", text: "pondering", at: 1 };
    render(<MessageBubble message={message} isStreaming={true} />);
    
    // Trigger should show "Thinking"
    expect(screen.getByText("Thinking")).toBeTruthy();
    expect(screen.getByText("pondering")).toBeTruthy(); // streaming content is visible
  });

  it("renders existing kinds unaffected", () => {
    const userMsg: Message = { kind: "user", text: "hello", at: 1 };
    render(<MessageBubble message={userMsg} />);
    expect(screen.getByText("hello")).toBeTruthy();
  });
});
