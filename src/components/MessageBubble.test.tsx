import { act, fireEvent, render, screen } from "@testing-library/react";
import { describe, it, expect, beforeAll, beforeEach, afterEach, vi } from "vitest";
import ReactMarkdown from "react-markdown";
import MessageBubble from "./MessageBubble";
import { Message } from "../store/sessions";
import { useSubagents } from "../store/subagents";

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

  // --- User-message images: read-only thumbnail grid (the transcript is
  // history — no remove buttons). ---

  it("renders a user message's images as a read-only thumbnail grid", () => {
    const message: Message = {
      kind: "user",
      text: "look",
      at: 1,
      images: [{ name: "a.png", mimeType: "image/png", sizeBytes: 3, data: "AQID" }],
    };
    render(<MessageBubble message={message} />);
    const img = screen.getByAltText("a.png");
    expect(img.getAttribute("src")).toBe("data:image/png;base64,AQID");
  });

  it("renders an image-only user message (empty text)", () => {
    const message: Message = {
      kind: "user",
      text: "",
      at: 1,
      images: [{ name: "a.png", mimeType: "image/png", sizeBytes: 3, data: "AQID" }],
    };
    render(<MessageBubble message={message} />);
    expect(screen.getByAltText("a.png")).toBeTruthy();
  });

  it("renders a plain user message (no images) without an <img>", () => {
    const message: Message = { kind: "user", text: "hello", at: 1 };
    render(<MessageBubble message={message} />);
    expect(screen.getByText("hello")).toBeTruthy();
    expect(screen.queryByRole("img")).toBeNull();
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

  // --- `subagent` tool call: render the `SubagentDelegatingCard` (not the
  // plain `ToolCallCard`); every other tool is unchanged. ---

  beforeEach(() => {
    for (const id of Object.keys(useSubagents.getState().entries)) {
      useSubagents.getState().dismiss(id);
    }
  });

  it("renders a subagent tool call as SubagentDelegatingCard (not the plain ToolCallCard)", () => {
    const message: Message = {
      kind: "tool-call",
      id: "t1",
      title: "subagent",
      status: "pending",
      at: 1,
    };
    render(<MessageBubble message={message} sessionId="main1" />);
    // The `SubagentDelegatingCard` is OPEN by default (no seeded entries →
    // the empty-state marker, unique to it, is visible).
    expect(screen.getByText("No subagents yet.")).toBeTruthy();
    // The plain `ToolCallCard` is collapsed by default and shows "No
    // output." when expanded — toggling here must NOT surface a plain-card
    // body (the nested card's empty state just hides).
    act(() => {
      fireEvent.click(screen.getByRole("button"));
    });
    expect(screen.queryByText("No subagents yet.")).toBeNull();
    expect(screen.queryByText("No output.")).toBeNull();
  });

  it("renders a non-subagent tool call as the plain ToolCallCard (unchanged)", () => {
    const message: Message = {
      kind: "tool-call",
      id: "t1",
      title: "bash",
      status: "completed",
      rawInput: { command: "ls -la" },
      at: 1,
    };
    render(<MessageBubble message={message} />);
    expect(screen.getByText("Ran")).toBeTruthy();
    expect(screen.getByText("ls -la")).toBeTruthy();
    // No `SubagentDelegatingCard` for a plain tool.
    expect(screen.queryByText("No subagents yet.")).toBeNull();
  });

  it("renders the session's subagent rows nested under the subagent tool call", () => {
    useSubagents.getState().addSession({
      sessionId: "sub1",
      parentSessionId: "main1",
      agentName: "reviewer",
      task: "review the diff",
      status: "running",
    });
    const message: Message = {
      kind: "tool-call",
      id: "t1",
      title: "subagent",
      status: "pending",
      at: 1,
    };
    render(<MessageBubble message={message} sessionId="main1" />);
    expect(screen.getByText("reviewer")).toBeTruthy();
    expect(screen.getByText("review the diff")).toBeTruthy();
    expect(screen.queryByText("No subagents yet.")).toBeNull();
  });
});
