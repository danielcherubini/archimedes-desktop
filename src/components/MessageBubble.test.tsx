import { render, screen } from "@testing-library/react";
import { describe, it, expect, beforeAll, afterEach, vi } from "vitest";
import ReactMarkdown from "react-markdown";
import MessageBubble from "./MessageBubble";
import { Message } from "../store/sessions";

// Spy on the markdown renderer: the streaming-perf test below counts how
// often it runs when a bubble re-renders with an UNCHANGED message
// reference (what happens to every non-touched bubble when a new thought
// chunk arrives — the reducer only replaces the last message object).
vi.mock("react-markdown", () => ({ default: vi.fn() }));
const markdownSpy = ReactMarkdown as unknown as ReturnType<typeof vi.fn>;

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
  markdownSpy.mockClear();
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

  it("skips re-rendering when the message reference is unchanged (streaming perf)", () => {
    // A live thought chunk replaces ONLY the trailing message object in the
    // reducer; every other bubble gets the SAME reference. Re-rendering all
    // of them (incl. a full ReactMarkdown re-parse per agent-text bubble)
    // on every chunk is what made the app render real slow — the bubble
    // must be memoized so only the changed bubble re-renders.
    const message: Message = { kind: "agent-text", messageId: "m1", text: "hello world", at: 1 };
    const { rerender } = render(<MessageBubble message={message} />);
    const first = markdownSpy.mock.calls.length;
    expect(first).toBe(1);
    rerender(<MessageBubble message={message} />);
    expect(markdownSpy.mock.calls.length).toBe(first);
  });
});
