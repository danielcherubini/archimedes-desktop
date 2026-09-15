import { describe, expect, it } from "vitest";
import {
  applySessionUpdate,
  finalizeSessionMessages,
  rowToMessages,
  type AcpSessionUpdate,
  type Message,
} from "./sessions";
import type { MessageRow } from "../lib/tauri";

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

describe("rowToMessages (history replay from the database)", () => {
  const row = (
    overrides: Partial<MessageRow> & { kind: MessageRow["kind"] },
  ): MessageRow => ({
    id: 1,
    sessionId: "s1",
    messageKey: null,
    payloadJson: "{}",
    createdAt: 1000,
    ...overrides,
  });

  it("maps a user row to a user message", () => {
    const [msg] = rowToMessages(
      row({ kind: "user", payloadJson: JSON.stringify({ text: "hi" }) }),
    );
    expect(msg).toEqual({ kind: "user", text: "hi", at: 1000 });
  });

  it("maps an agent-text row to an agent-text message with its messageId", () => {
    const [msg] = rowToMessages(
      row({
        kind: "agent-text",
        messageKey: "m1",
        payloadJson: JSON.stringify({ text: "hello world" }),
      }),
    );
    expect(msg).toEqual({
      kind: "agent-text",
      messageId: "m1",
      text: "hello world",
      at: 1000,
    });
  });

  it("maps a tool-call row to a tool-call message plus its diff messages", () => {
    const messages = rowToMessages(
      row({
        kind: "tool-call",
        messageKey: "tc1",
        payloadJson: JSON.stringify({
          toolCallId: "tc1",
          title: "edit file",
          status: "completed",
          content: [
            { type: "diff", path: "/tmp/x.txt", oldText: "a\n", newText: "b\n" },
          ],
        }),
      }),
    );
    expect(messages).toHaveLength(2);
    expect(messages[0]).toMatchObject({
      kind: "tool-call",
      id: "tc1",
      title: "edit file",
      status: "completed",
    });
    const diff = messages.find((m) => m.kind === "diff");
    if (!diff || diff.kind !== "diff") throw new Error("no diff message");
    expect(diff.path).toBe("/tmp/x.txt");
    expect(diff.patch).toContain("+b");
  });

  it("ignores rows with unparseable payloads or unknown kinds", () => {
    expect(
      rowToMessages(row({ kind: "user", payloadJson: "not json" })),
    ).toHaveLength(0);
    expect(rowToMessages(row({ kind: "mystery" }))).toHaveLength(0);
  });
});
