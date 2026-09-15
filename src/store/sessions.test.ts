import { describe, expect, it } from "vitest";
import {
  applySessionUpdate,
  finalizeSessionMessages,
  type AcpSessionUpdate,
  type Message,
} from "./sessions";

const chunk = (messageId: string, text: string): AcpSessionUpdate => ({
  sessionUpdate: "agent_message_chunk",
  content: { type: "text", text },
  messageId,
});

describe("applySessionUpdate — agent_message_chunk", () => {
  it("appends a chunk to the agent-text message with the same messageId", () => {
    let messages: Message[] = [];
    messages = applySessionUpdate(messages, chunk("m1", "hello"), 1);
    messages = applySessionUpdate(messages, chunk("m1", " world"), 2);
    expect(messages).toHaveLength(1);
    expect(messages[0]).toMatchObject({
      kind: "agent-text",
      messageId: "m1",
      text: "hello world",
    });
  });

  it("starts a new message when the messageId changes", () => {
    let messages: Message[] = [];
    messages = applySessionUpdate(messages, chunk("m1", "hello"), 1);
    messages = applySessionUpdate(messages, chunk("m2", "second"), 2);
    expect(messages).toHaveLength(2);
    expect(messages[0]).toMatchObject({ kind: "agent-text", text: "hello" });
    expect(messages[1]).toMatchObject({
      kind: "agent-text",
      messageId: "m2",
      text: "second",
    });
  });

  it("does not merge chunks across a tool call with the same messageId", () => {
    let messages: Message[] = [];
    messages = applySessionUpdate(messages, chunk("m1", "pre"), 1);
    messages = applySessionUpdate(
      messages,
      { sessionUpdate: "tool_call", toolCallId: "tc1", title: "tool" },
      2,
    );
    messages = applySessionUpdate(messages, chunk("m1", "post"), 3);
    // The tool call in between breaks the run: a new agent-text message is
    // created rather than appending to the earlier one.
    const texts = messages.filter((m) => m.kind === "agent-text");
    expect(texts).toHaveLength(2);
    expect(texts[1]).toMatchObject({ text: "post" });
  });

  it("ignores non-text chunk content", () => {
    const messages = applySessionUpdate(
      [],
      {
        sessionUpdate: "agent_message_chunk",
        content: { type: "image", data: "x" },
        messageId: "m1",
      },
      1,
    );
    expect(messages).toHaveLength(0);
  });
});

describe("applySessionUpdate — tool calls", () => {
  it("creates a tool-call message on tool_call", () => {
    const messages = applySessionUpdate(
      [],
      {
        sessionUpdate: "tool_call",
        toolCallId: "tc1",
        title: "edit file",
        status: "in_progress",
      },
      1,
    );
    expect(messages).toHaveLength(1);
    expect(messages[0]).toMatchObject({
      kind: "tool-call",
      id: "tc1",
      title: "edit file",
      status: "pending",
    });
  });

  it("updates the existing tool call on tool_call_update", () => {
    let messages = applySessionUpdate(
      [],
      { sessionUpdate: "tool_call", toolCallId: "tc1", title: "edit" },
      1,
    );
    messages = applySessionUpdate(
      messages,
      {
        sessionUpdate: "tool_call_update",
        toolCallId: "tc1",
        status: "completed",
      },
      2,
    );
    expect(messages).toHaveLength(1);
    expect(messages[0]).toMatchObject({ kind: "tool-call", status: "completed" });
  });

  it("updates the title when the update carries one", () => {
    let messages = applySessionUpdate(
      [],
      { sessionUpdate: "tool_call", toolCallId: "tc1", title: "edit" },
      1,
    );
    messages = applySessionUpdate(
      messages,
      {
        sessionUpdate: "tool_call_update",
        toolCallId: "tc1",
        title: "edit (done)",
      },
      2,
    );
    expect(messages[0]).toMatchObject({ title: "edit (done)" });
  });

  it("extracts a diff from a tool_call's content into a diff message", () => {
    const messages = applySessionUpdate(
      [],
      {
        sessionUpdate: "tool_call",
        toolCallId: "tc1",
        title: "edit",
        content: [
          {
            type: "diff",
            path: "/tmp/x.txt",
            oldText: "a\nb\n",
            newText: "a\nc\n",
          },
        ],
      },
      1,
    );
    const diff = messages.find((m) => m.kind === "diff");
    expect(diff).toBeDefined();
    if (diff?.kind !== "diff") throw new Error("no diff message");
    expect(diff.path).toBe("/tmp/x.txt");
    expect(diff.patch).toContain("-b");
    expect(diff.patch).toContain("+c");
    const toolCall = messages.find((m) => m.kind === "tool-call");
    if (toolCall?.kind !== "tool-call") throw new Error("no tool-call message");
    expect(toolCall.diff?.path).toBe("/tmp/x.txt");
  });

  it("extracts diffs from tool_call_update content", () => {
    let messages = applySessionUpdate(
      [],
      { sessionUpdate: "tool_call", toolCallId: "tc1", title: "edit" },
      1,
    );
    messages = applySessionUpdate(
      messages,
      {
        sessionUpdate: "tool_call_update",
        toolCallId: "tc1",
        content: [
          { type: "diff", path: "/tmp/y.txt", newText: "new\n" },
        ],
      },
      2,
    );
    const diff = messages.find((m) => m.kind === "diff");
    expect(diff).toBeDefined();
    if (diff?.kind !== "diff") throw new Error("no diff message");
    expect(diff.path).toBe("/tmp/y.txt");
    expect(diff.patch).toContain("+new");
  });

  it("ignores unknown update types", () => {
    const messages = applySessionUpdate(
      [],
      { sessionUpdate: "plan", plan: [] } as unknown as AcpSessionUpdate,
      1,
    );
    expect(messages).toHaveLength(0);
  });
});

describe("finalizeSessionMessages (session-closed cleanup)", () => {
  it("marks pending tool calls failed and leaves finished ones alone", () => {
    let messages = applySessionUpdate(
      [],
      { sessionUpdate: "tool_call", toolCallId: "a", title: "a" },
      1,
    );
    messages = applySessionUpdate(
      messages,
      { sessionUpdate: "tool_call_update", toolCallId: "a", status: "completed" },
      2,
    );
    messages = applySessionUpdate(
      messages,
      { sessionUpdate: "tool_call", toolCallId: "b", title: "b" },
      3,
    );
    const finalized = finalizeSessionMessages(messages);
    const a = finalized.find((m) => m.kind === "tool-call" && m.id === "a");
    const b = finalized.find((m) => m.kind === "tool-call" && m.id === "b");
    if (a?.kind !== "tool-call" || b?.kind !== "tool-call")
      throw new Error("missing tool calls");
    expect(a.status).toBe("completed");
    expect(b.status).toBe("failed");
  });
});
