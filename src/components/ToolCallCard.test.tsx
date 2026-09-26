import { fireEvent, render, screen } from "@testing-library/react";
import { describe, it, expect, vi } from "vitest";
import ToolCallCard from "./ToolCallCard";
import {
  summarizeToolCall,
  normalizeToolOutput,
} from "../lib/toolOutput";
import * as toolOutput from "../lib/toolOutput";

describe("summarizeToolCall", () => {
  it("summarizes bash by its command", () => {
    expect(summarizeToolCall("bash", { command: "ls -la" })).toBe("ls -la");
  });
  it("summarizes read by path + line range when offset/limit present", () => {
    expect(
      summarizeToolCall("read", { path: "src/a.ts", offset: 5, limit: 50 }),
    ).toBe("src/a.ts (L5–54)");
  });
  it("summarizes read by path alone when no offset/limit", () => {
    expect(summarizeToolCall("read", { path: "src/a.ts" })).toBe("src/a.ts");
  });
  it("falls back to compact JSON for an unknown tool", () => {
    expect(summarizeToolCall("mystery_tool", { a: 1 })).toBe('{"a":1}');
  });
  it("returns undefined for empty or missing input", () => {
    expect(summarizeToolCall("mystery_tool", {})).toBeUndefined();
    expect(summarizeToolCall("mystery_tool", undefined)).toBeUndefined();
  });
});

describe("normalizeToolOutput", () => {
  it("joins text items of an AgentToolResult content array", () => {
    expect(
      normalizeToolOutput({
        content: [
          { type: "text", text: "line1" },
          { type: "text", text: "line2" },
        ],
      }),
    ).toBe("line1\nline2");
  });
  it("counts image items", () => {
    expect(
      normalizeToolOutput({
        content: [
          { type: "text", text: "t" },
          { type: "image" },
          { type: "image" },
        ],
      }),
    ).toBe("t\n(+2 images)");
  });
  it("shows a bare string as-is", () => {
    expect(normalizeToolOutput("world")).toBe("world");
  });
  it("falls back to details JSON when there is no text", () => {
    expect(normalizeToolOutput({ content: [], details: { todos: [] } })).toBe(
      '{"todos":[]}',
    );
  });
  it("returns undefined for nothing showable", () => {
    expect(normalizeToolOutput(undefined)).toBeUndefined();
    expect(normalizeToolOutput({})).toBeUndefined();
  });
  it("treats an empty text item as no output (no metadata fallback)", () => {
    // e.g. a successful sudo_exec with empty stdout: an empty text item
    // plus command metadata in details.
    expect(
      normalizeToolOutput({
        content: [{ type: "text", text: "" }],
        details: { command: "true", exitCode: 0 },
      }),
    ).toBeUndefined();
  });
  it("falls back to details for a FAILED call with empty text", () => {
    // A failed/timed-out sudo_exec has an empty text item and puts the
    // failure reason in details — the expanded card must show it.
    expect(
      normalizeToolOutput(
        {
          content: [{ type: "text", text: "" }],
          details: { command: "true", error: "timed out after 120s" },
        },
        true,
      ),
    ).toBe('{"command":"true","error":"timed out after 120s"}');
  });
});

describe("ToolCallCard (rendering)", () => {
  it("renders the summary next to the title", () => {
    render(
      <ToolCallCard
        title="bash"
        status="completed"
        rawInput={{ command: "ls -la" }}
      />,
    );
    expect(screen.getByText("bash")).toBeTruthy();
    expect(screen.getByText("ls -la")).toBeTruthy();
  });
  it("renders the output text when expanded", () => {
    render(
      <ToolCallCard
        title="bash"
        status="completed"
        rawInput={{ command: "ls" }}
        rawOutput={{ content: [{ type: "text", text: "fileA\nfileB" }] }}
      />,
    );
    fireEvent.click(screen.getByRole("button"));
    expect(screen.getByText(/fileA/)).toBeTruthy();
  });
  it("renders a bare-string rawOutput when expanded", () => {
    render(
      <ToolCallCard title="read" status="completed" rawOutput="file contents" />,
    );
    fireEvent.click(screen.getByRole("button"));
    expect(screen.getByText("file contents")).toBeTruthy();
  });
  it("caps output at 20k chars with a truncation note", () => {
    render(
      <ToolCallCard
        title="bash"
        status="completed"
        rawOutput={"x".repeat(20001)}
      />,
    );
    fireEvent.click(screen.getByRole("button"));
    expect(screen.getByText(/truncated — 20001 chars total/)).toBeTruthy();
  });
  it("renders (no output) when there is no rawOutput", () => {
    render(<ToolCallCard title="bash" status="completed" />);
    fireEvent.click(screen.getByRole("button"));
    expect(screen.getByText("(no output)")).toBeTruthy();
  });
  it("renders (no output) for a result with only an empty text item", () => {
    render(
      <ToolCallCard
        title="sudo_exec"
        status="completed"
        rawInput={{ command: "true" }}
        rawOutput={{
          content: [{ type: "text", text: "" }],
          details: { command: "true", exitCode: 0 },
        }}
      />,
    );
    fireEvent.click(screen.getByRole("button"));
    expect(screen.getByText("(no output)")).toBeTruthy();
  });
  it("shows the failure reason from details for a failed call with empty text", () => {
    render(
      <ToolCallCard
        title="sudo_exec"
        status="failed"
        rawInput={{ command: "true" }}
        rawOutput={{
          content: [{ type: "text", text: "" }],
          details: { command: "true", error: "timed out after 120s" },
        }}
      />,
    );
    fireEvent.click(screen.getByRole("button"));
    expect(screen.getByText(/timed out after 120s/)).toBeTruthy();
  });
  it("defers output normalization until the card is expanded", () => {
    const spy = vi.spyOn(toolOutput, "normalizeToolOutput");
    render(
      <ToolCallCard
        title="bash"
        status="completed"
        rawOutput={"x".repeat(50000)}
      />,
    );
    expect(spy).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button"));
    expect(spy).toHaveBeenCalledTimes(1);
    spy.mockRestore();
  });
});
