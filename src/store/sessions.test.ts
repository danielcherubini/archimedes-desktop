import { describe, expect, it } from "vitest";
import {
  applySessionUpdate,
  autoSelectActive,
  finalizeSessionMessages,
  rowToMessages,
  spaceViewFor,
  type AcpSessionUpdate,
  type Message,
} from "./sessions";
import type {
  CloseReasonStr,
  MessageRow,
  SessionInfo,
  SpaceRow,
} from "../lib/tauri";

/**
 * Fixtures use EXACTLY the four `SessionInfo` wire fields — `SessionInfo`
 * over IPC has NO `createdAt` (see the ordering note in `sessions.ts`), so
 * the pure helpers must rely on input order, not invented timestamps.
 */
const info = (sessionId: string, cwd: string): SessionInfo => ({
  sessionId,
  agentId: "clack-1.0",
  cwd,
  capabilities: {},
});

/**
 * `SpaceRow` carries `createdAt`/`lastOpenedAt` — these are the bookkeeping
 * timestamps of the `spaces` table itself (not of individual sessions). The
 * helpers never read `createdAt`, only `path`; the timestamps exist on the
 * fixture so the shape is full and it'd be obvious if a helper started
 * sorting on them (it must not — see the ordering note in `sessions.ts`).
 */
const row = (path: string, lastOpenedAt: number): SpaceRow => ({
  path,
  createdAt: lastOpenedAt - 86_400_000,
  lastOpenedAt,
});

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

  it("keeps rawInput on the tool-call message (the todo board's rawInput fallback reads it)", () => {
    const messages = applySessionUpdate(
      [],
      {
        sessionUpdate: "tool_call",
        toolCallId: "tc1",
        title: "manage_todo_list",
        rawInput: { operation: "write", todoList: [{ content: "a", status: "pending" }] },
      },
      1,
    );
    const toolCall = messages.find((m) => m.kind === "tool-call");
    if (toolCall?.kind !== "tool-call") throw new Error("no tool-call message");
    expect(toolCall.rawInput).toEqual({
      operation: "write",
      todoList: [{ content: "a", status: "pending" }],
    });
  });

  it("keeps the rawInput from a tool_call_update when the update carries one", () => {
    let messages = applySessionUpdate(
      [],
      { sessionUpdate: "tool_call", toolCallId: "tc1", title: "manage_todo_list" },
      1,
    );
    messages = applySessionUpdate(
      messages,
      {
        sessionUpdate: "tool_call_update",
        toolCallId: "tc1",
        rawInput: { operation: "write", todoList: [{ content: "b", status: "completed" }] },
      },
      2,
    );
    const toolCall = messages.find((m) => m.kind === "tool-call");
    if (toolCall?.kind !== "tool-call") throw new Error("no tool-call message");
    expect(toolCall.rawInput).toEqual({
      operation: "write",
      todoList: [{ content: "b", status: "completed" }],
    });
  });

  it("preserves an earlier rawInput when a later update carries none", () => {
    let messages = applySessionUpdate(
      [],
      {
        sessionUpdate: "tool_call",
        toolCallId: "tc1",
        title: "manage_todo_list",
        rawInput: { operation: "write", todoList: [{ content: "a", status: "pending" }] },
      },
      1,
    );
    messages = applySessionUpdate(
      messages,
      { sessionUpdate: "tool_call_update", toolCallId: "tc1", status: "completed" },
      2,
    );
    const toolCall = messages.find((m) => m.kind === "tool-call");
    if (toolCall?.kind !== "tool-call") throw new Error("no tool-call message");
    expect(toolCall.rawInput).toEqual({
      operation: "write",
      todoList: [{ content: "a", status: "pending" }],
    });
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

describe("spaceViewFor (per-space grouping, pure)", () => {
  // Spaces arrive `lastOpenedAt`-desc from `list_spaces`; `historySessions`
  // newest-first from `list_sessions` — input order is the semantics.
  const spaces = [
    row("/workspaces/alpha", 3000),
    row("/workspaces/bravo", 2000),
    row("/workspaces/charlie", 1000),
  ];
  const sessions = [info("live-alpha", "/workspaces/alpha")];
  // Passed in a SPECIFIC order to prove no re-sort by a nonexistent field.
  const historySessions = [
    info("stored-alpha", "/workspaces/alpha"),
    info("idX", "/workspaces/bravo"),
    info("idY", "/workspaces/bravo"),
  ];
  const closeReasons: Record<string, CloseReasonStr> = {
    "live-alpha": "replaced",
    "stored-alpha": "replaced",
    idX: "agent-exited",
  };

  it("groups a space with a live session and a stored session", () => {
    const view = spaceViewFor(spaces[0], sessions, historySessions, closeReasons);
    expect(view).toEqual({
      path: "/workspaces/alpha",
      title: "alpha",
      liveSessionId: "live-alpha",
      storedSessionIds: ["stored-alpha"],
      // The newest session's reason: the live one — recorded as "replaced"
      // (the one-live-policy closed it when a newer session started). The
      // stored session carries the same reason here, so this proves the
      // lookup does not silently fall through to it.
      lastReason: "replaced",
    });
  });

  it("returns storedSessionIds in the given input order (no re-sort)", () => {
    const view = spaceViewFor(spaces[1], sessions, historySessions, closeReasons);
    expect(view).toEqual({
      path: "/workspaces/bravo",
      title: "bravo",
      liveSessionId: null,
      storedSessionIds: ["idX", "idY"],
      // No live session: the head of the stored subsequence's reason.
      lastReason: "agent-exited",
    });
  });

  it("returns an empty shape for a space with no sessions", () => {
    const view = spaceViewFor(spaces[2], sessions, historySessions, closeReasons);
    expect(view).toEqual({
      path: "/workspaces/charlie",
      title: "charlie",
      liveSessionId: null,
      storedSessionIds: [],
      lastReason: undefined,
    });
  });
});

describe("autoSelectActive (boot auto-select, pure)", () => {
  // `spaces` arrives `lastOpenedAt`-desc — that IS input order; the function
  // must NOT re-sort.
  const spaces = [
    row("/a/space-one", 3000),
    row("/a/space-two", 2000),
    row("/a/space-three", 1000),
    row("/a/space-four", 500),
  ];

  it("returns null when there is nothing at all", () => {
    expect(autoSelectActive([], [], [])).toBeNull();
    expect(autoSelectActive(spaces, [], [])).toBeNull();
  });

  it("prefers the first space's live session", () => {
    const live = [info("live-1", "/a/space-one")];
    const stored = [
      info("old-1", "/a/space-one"),
      info("old-2", "/a/space-two"),
    ];
    // Even if the other space's stored session is older/newer, the live
    // session in the most recent space wins.
    expect(autoSelectActive(spaces, live, stored)).toBe("live-1");
  });

  it("falls back to the head of the first space's stored subsequence", () => {
    const stored = [
      info("newest-1", "/a/space-one"),
      info("older-1", "/a/space-one"),
    ];
    // `historySessions` is newest-first (server order): the head wins.
    expect(autoSelectActive(spaces, [], stored)).toBe("newest-1");
  });

  it("skips a space with no sessions and advances to the next", () => {
    const stored = [
      info("newest-2", "/a/space-two"),
      info("older-2", "/a/space-two"),
    ];
    expect(autoSelectActive(spaces, [], stored)).toBe("newest-2");
  });

  it("the first (most recent) space wins over a newer session in another", () => {
    const stored = [
      info("head-1", "/a/space-one"),
      info("brand-new-2", "/a/space-two"), // delivered first in input order
      info("older-2", "/a/space-two"),
    ];
    // Input/recency order wins: the FIRST space's head, even though the
    // second space's stored session was created more recently. (We cannot
    // check timestamps — `SessionInfo` has no `createdAt` — so input order
    // is THE ordering.)
    expect(autoSelectActive(spaces, [], stored)).toBe("head-1");
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

  it("maps a tool-call row's rawInput onto the message", () => {
    const [msg] = rowToMessages(
      row({
        kind: "tool-call",
        messageKey: "tc1",
        payloadJson: JSON.stringify({
          toolCallId: "tc1",
          title: "manage_todo_list",
          status: "completed",
          rawInput: { operation: "write", todoList: [{ content: "a", status: "pending" }] },
        }),
      }),
    );
    if (msg?.kind !== "tool-call") throw new Error("no tool-call message");
    expect(msg.rawInput).toEqual({
      operation: "write",
      todoList: [{ content: "a", status: "pending" }],
    });
  });

  it("ignores rows with unparseable payloads or unknown kinds", () => {
    expect(
      rowToMessages(row({ kind: "user", payloadJson: "not json" })),
    ).toHaveLength(0);
    expect(rowToMessages(row({ kind: "mystery" }))).toHaveLength(0);
  });
});
