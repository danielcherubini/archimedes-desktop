import { afterAll, beforeEach, describe, expect, it, vi } from "vitest";
import {
  applySessionUpdate,
  autoSelectActive,
  discardSessionMessages,
  finalizeSessionMessages,
  rowToMessages,
  spaceViewFor,
  useSessions,
  type AcpSessionUpdate,
  type Message,
} from "./sessions";
import { useSubagents } from "./subagents";
import { expandMentions } from "../lib/skills";
import type { AgentDefinitionDto, McpServerInfo } from "../lib/tauri";
import { loadHistory, setSessionArchived, setSpaceTrusted, deleteSpace } from "../lib/tauri";
import type {
  CloseReasonStr,
  MessageRow,
  SessionInfo,
  SpaceRow,
} from "../lib/tauri";

vi.mock("../lib/tauri", async () => {
  const actual = await vi.importActual<Record<string, unknown>>("../lib/tauri");
  return {
    ...actual,
    resumeSession: vi.fn().mockResolvedValue({
      sessionId: "s1",
      cwd: "/x",
      capabilities: {},
      archived: false,
      configOptions: [{ id: "model", name: "Model", type: "select", currentValue: "gpt-4" }],
    }),
    loadHistory: vi.fn().mockResolvedValue([]),
    // The store's `archiveSession` / `unarchiveSession` / `deleteSession` call
    // these: mock them (the REAL wrappers would `invoke` and reject in
    // jsdom — no global Tauri mock exists).
    setSessionArchived: vi.fn().mockResolvedValue(true),
    deleteSession: vi.fn().mockResolvedValue(undefined),
    deleteSpace: vi.fn().mockResolvedValue(undefined),
    setSpaceTrusted: vi.fn().mockResolvedValue(undefined),
  };
});

/**
 * Fixtures use EXACTLY the five `SessionInfo` wire fields — `SessionInfo`
 * over IPC has NO `createdAt` (see the ordering note in `sessions.ts`), so
 * the pure helpers must rely on input order, not invented timestamps.
 */
const info = (sessionId: string, cwd: string): SessionInfo => ({
  sessionId,
  cwd,
  capabilities: {},
  archived: false,
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
  trusted: false,
});

/**
 * `MessageRow` helper: override for `MessageRow` fixtures, to distinguish
 * it from the `SpaceRow` helper `row` above.
 */
const msgRow = (
  overrides: Partial<MessageRow> & { kind: MessageRow["kind"] },
): MessageRow => ({
  id: 1,
  sessionId: "s1",
  messageKey: null,
  payloadJson: "{}",
  createdAt: 1000,
  ...overrides,
});

const chunk = (messageId: string, text: string): AcpSessionUpdate => ({
  sessionUpdate: "agent_message_chunk",
  content: { type: "text", text },
  messageId,
});

describe("applySessionUpdate — agent_message_chunk", () => {
  const thought = (messageId: string, text: string): AcpSessionUpdate => ({
    sessionUpdate: "agent_thought_chunk",
    content: { type: "text", text },
    messageId,
  });

  it("accumulates contiguous agent_thought_chunk chunks into one agent-thought message", () => {
    let messages: Message[] = [];
    messages = applySessionUpdate(messages, thought("m1", "think"), 1);
    messages = applySessionUpdate(messages, thought("m1", "ing"), 2);
    expect(messages).toHaveLength(1);
    expect(messages[0]).toMatchObject({
      kind: "agent-thought",
      messageId: "m1",
      text: "thinking",
    });
  });

  it("starts a new agent-thought message when messageId changes", () => {
    let messages: Message[] = [];
    messages = applySessionUpdate(messages, thought("m1", "think"), 1);
    messages = applySessionUpdate(messages, thought("m2", "ing"), 2);
    expect(messages).toHaveLength(2);
    expect(messages[0]).toMatchObject({ kind: "agent-thought", messageId: "m1", text: "think" });
    expect(messages[1]).toMatchObject({ kind: "agent-thought", messageId: "m2", text: "ing" });
  });

  it("starts a new agent-thought message after an intervening text chunk", () => {
    let messages: Message[] = [];
    messages = applySessionUpdate(messages, thought("m1", "t1"), 1);
    messages = applySessionUpdate(messages, chunk("m1", "text"), 2);
    messages = applySessionUpdate(messages, thought("m1", "t2"), 3);
    const thoughts = messages.filter((m) => m.kind === "agent-thought");
    expect(thoughts).toHaveLength(2);
    expect(thoughts[0]).toMatchObject({ text: "t1" });
    expect(thoughts[1]).toMatchObject({ text: "t2" });
  });

  it("ignores non-text or empty thought chunks", () => {
    let messages: Message[] = [];
    messages = applySessionUpdate(
      messages,
      {
        sessionUpdate: "agent_thought_chunk",
        content: { type: "image", data: "x" },
        messageId: "m1",
      },
      1,
    );
    expect(messages).toHaveLength(0);
    messages = applySessionUpdate(messages, thought("m1", ""), 2);
    expect(messages).toHaveLength(0);
  });
});

describe("rowToMessages — agent-thought", () => {
  it("maps an agent-thought row to an agent-thought message", () => {
    const [msg] = rowToMessages(
      msgRow({
        kind: "agent-thought",
        messageKey: "m9#1",
        payloadJson: JSON.stringify({ text: "recalled" }),
        createdAt: 123,
      }),
    );
    expect(msg).toEqual({
      kind: "agent-thought",
      messageId: "m9#1",
      text: "recalled",
      at: 123,
    });
  });

  it("returns empty for an agent-thought row with a non-string payload.text", () => {
    const messages = rowToMessages(
      msgRow({
        kind: "agent-thought",
        payloadJson: JSON.stringify({ text: 123 }),
      }),
    );
    expect(messages).toHaveLength(0);
  });
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

  it("keeps rawOutput on the tool-call message", () => {
    const messages = applySessionUpdate(
      [],
      {
        sessionUpdate: "tool_call",
        toolCallId: "tc1",
        title: "bash",
        rawOutput: { content: [{ type: "text", text: "hello" }] },
      },
      1,
    );
    const toolCall = messages.find((m) => m.kind === "tool-call");
    if (toolCall?.kind !== "tool-call") throw new Error("no tool-call message");
    expect(toolCall.rawOutput).toEqual({ content: [{ type: "text", text: "hello" }] });
  });

  it("keeps the rawOutput from a tool_call_update", () => {
    let messages = applySessionUpdate(
      [],
      { sessionUpdate: "tool_call", toolCallId: "tc1", title: "bash" },
      1,
    );
    messages = applySessionUpdate(
      messages,
      {
        sessionUpdate: "tool_call_update",
        toolCallId: "tc1",
        rawOutput: { content: [{ type: "text", text: "world" }] },
      },
      2,
    );
    const toolCall = messages.find((m) => m.kind === "tool-call");
    if (toolCall?.kind !== "tool-call") throw new Error("no tool-call message");
    expect(toolCall.rawOutput).toEqual({ content: [{ type: "text", text: "world" }] });
  });

  it("preserves an earlier rawOutput when a later update carries none", () => {
    let messages = applySessionUpdate(
      [],
      {
        sessionUpdate: "tool_call",
        toolCallId: "tc1",
        title: "bash",
        rawOutput: { content: [{ type: "text", text: "hello" }] },
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
    expect(toolCall.rawOutput).toEqual({ content: [{ type: "text", text: "hello" }] });
  });

  it("stores rawOutput when a tool_call_update arrives for a tool call we never saw", () => {
    const messages = applySessionUpdate(
      [],
      {
        sessionUpdate: "tool_call_update",
        toolCallId: "tc1",
        rawOutput: { content: [{ type: "text", text: "late" }] },
      },
      1,
    );
    const toolCall = messages.find((m) => m.kind === "tool-call");
    if (toolCall?.kind !== "tool-call") throw new Error("no tool-call message");
    expect(toolCall.rawOutput).toEqual({ content: [{ type: "text", text: "late" }] });
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
    "live-alpha": "user",
    "stored-alpha": "error",
    idX: "agent-exited",
  };

  it("groups a space with a live session and a stored session", () => {
    const view = spaceViewFor(spaces[0], sessions, historySessions, closeReasons, []);
    expect(view).toEqual({
      path: "/workspaces/alpha",
      title: "alpha",
      liveSessionId: "live-alpha",
      storedSessionIds: ["stored-alpha"],
      archivedSessionIds: [],
      trusted: false,
      // The newest session's reason: the live one's (`live-alpha` →
      // `user`), NOT the stored session's (`stored-alpha` → `error`) —
      // proves the lookup does not fall through to the stored one.
      lastReason: "user",
    });
  });

  it("returns storedSessionIds in the given input order (no re-sort)", () => {
    const view = spaceViewFor(spaces[1], sessions, historySessions, closeReasons, []);
    expect(view).toEqual({
      path: "/workspaces/bravo",
      title: "bravo",
      liveSessionId: null,
      storedSessionIds: ["idX", "idY"],
      archivedSessionIds: [],
      trusted: false,
      // No live session: the head of the stored subsequence's reason.
      lastReason: "agent-exited",
    });
  });

  it("returns an empty shape for a space with no sessions", () => {
    const view = spaceViewFor(spaces[2], sessions, historySessions, closeReasons, []);
    expect(view).toEqual({
      path: "/workspaces/charlie",
      title: "charlie",
      liveSessionId: null,
      storedSessionIds: [],
      archivedSessionIds: [],
      trusted: false,
      lastReason: undefined,
    });
  });

  // The one-live cap is LIFTED: a space may hold MORE than one
  // live session. `sessions` is in insertion order (newest appended by
  // `addSession` / `resumeSession`), so the space row points at the
  // MOST-RECENTLY-STARTED one.
  it("a space with TWO coexisting live sessions points at the most-recently-started one (the one-live cap lift)", () => {
    const twoLive = [
      info("first", "/workspaces/alpha"),
      info("second", "/workspaces/alpha"),
    ];
    const view = spaceViewFor(spaces[0], twoLive, historySessions, closeReasons, []);
    expect(view.liveSessionId).toBe("second");
    expect(view.storedSessionIds).toEqual(["stored-alpha"]);
  });

  it("autoSelectActive prefers the most-recently-started live session in the space (the one-live cap lift)", () => {
    const twoLive = [
      info("first", "/workspaces/alpha"),
      info("second", "/workspaces/alpha"),
    ];
    expect(autoSelectActive(spaces, twoLive, historySessions)).toBe("second");
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

describe("discardSessionMessages (ephemeral subagent cleanup)", () => {
  // The store is global — reset the subagent entries and transcripts the
  // previous test may have left behind.
  beforeEach(() => {
    for (const id of Object.keys(useSubagents.getState().entries)) {
      useSubagents.getState().dismiss(id);
    }
    useSessions.setState({ messages: {} });
  });

  it("deletes a closed subagent session's messages (ephemeral: no history to preserve)", () => {
    useSubagents.getState().addSession({
      sessionId: "sub1",
      parentSessionId: "main1",
      agentName: "reviewer",
      task: "review the diff",
      status: "completed",
    });
    const sub: Message[] = [
      { kind: "agent-text", messageId: "m1", text: "hi", at: 1 },
      {
        kind: "tool-call",
        id: "t1",
        title: "Bash",
        status: "pending",
        at: 1,
      },
    ];
    const main: Message[] = [
      { kind: "agent-text", messageId: "m2", text: "main", at: 1 },
    ];
    useSessions.setState({ messages: { sub1: sub, main1: main } });

    useSessions.getState().handleSessionClosed("sub1", "user");
    discardSessionMessages("sub1");
    expect(useSessions.getState().messages["sub1"]).toBeUndefined();
    // The MAIN session's transcript is kept (finalized by
    // `handleSessionClosed`, NOT deleted — its history survives in the
    // database and the in-memory record stays).
    useSessions.getState().handleSessionClosed("main1", "user");
    discardSessionMessages("main1");
    expect(useSessions.getState().messages["main1"]).toEqual(
      finalizeSessionMessages(main),
    );
  });

  it("is a no-op for a main session id (no subagent entry)", () => {
    const main: Message[] = [{ kind: "user", text: "hello", at: 1 }];
    useSessions.setState({ messages: { main1: main } });
    discardSessionMessages("main1");
    expect(useSessions.getState().messages["main1"]).toEqual(main);
  });

  it("is a no-op when the subagent has no transcript yet", () => {
    useSubagents.getState().addSession({
      sessionId: "sub1",
      parentSessionId: "main1",
      agentName: "reviewer",
      task: "review the diff",
      status: "running",
    });
    discardSessionMessages("sub1");
    expect(useSessions.getState().messages).toEqual({});
  });

  it("deletes the transcript when the `subagent-closed` path runs `discardSessionMessages` + `markClosed` (the `listenSubagentClosed` handler's call sequence — the `session-closed`-beats-`subagent-session-started` ordering-race guarantee)", () => {
    // WHY this exists: `subagent-session-started` is emitted by the
    // WORKER task (after `drive_session` returns) while `session-closed`
    // is emitted by the DRIVER task (after teardown) — different tasks.
    // If the agent dies (or a cancel lands) immediately
    // post-establishment, `session-closed` can beat `started`: the
    // `session-closed` handler's `discardSessionMessages` no-ops (no
    // subagent entry yet) while `handleSessionClosed` creates
    // `messages[sid]`, and nothing else re-runs the discard — a small
    // unreclaimed leak (at most a partial transcript) per occurrence.
    // The `subagent-closed` handler therefore discards too: the entry
    // exists from `started` (the worker task emits `started` before
    // `subagent-closed` sequentially) and `running` entries are never
    // evicted, so the discard-first order is deterministic. (Discard
    // must run BEFORE `markClosed`: `markClosed` can evict this entry
    // synchronously — the oldest of 21+ closed entries — and a discard
    // running after would then no-op its guard.)
    useSubagents.getState().addSession({
      sessionId: "sub2",
      parentSessionId: "main1",
      agentName: "reviewer",
      task: "review the diff",
      status: "running",
    });
    // `session-closed` beat `started` and created the transcript:
    useSessions.setState({
      messages: {
        sub2: [{ kind: "agent-text", messageId: "m1", text: "partial", at: 1 }],
      },
    });
    // The `listenSubagentClosed` handler's call sequence (App.tsx) —
    // discard FIRST, then `markClosed`:
    discardSessionMessages("sub2");
    useSubagents.getState().markClosed("sub2", "failed", "died");
    expect(useSessions.getState().messages["sub2"]).toBeUndefined();
  });
});

describe("contextUsage (the context-percentage state)", () => {
  // The store is global — reset the maps the previous test may have left
  // behind (and the `afterAll` cleans up AFTER this block, so the next
  // block's assumption of a fresh store holds).
  beforeEach(() => {
    useSessions.setState({ contextUsage: {}, messages: {} });
  });
  afterAll(() => {
    useSessions.setState({ contextUsage: {}, messages: {} });
  });

  it("applySessionUpdates stores a context_usage_update frame per session", () => {
    useSessions
      .getState()
      .applySessionUpdates([
        {
          sessionId: "s1",
          update: {
            sessionUpdate: "context_usage_update",
            usedTokens: 12000,
            windowTokens: 128000,
          },
        },
      ]);
    expect(useSessions.getState().contextUsage["s1"]).toEqual({
      used: 12000,
      window: 128000,
    });
  });

  it("a later frame replaces the earlier one (last-wins)", () => {
    useSessions.getState().applySessionUpdates([
      {
        sessionId: "s1",
        update: {
          sessionUpdate: "context_usage_update",
          usedTokens: 12000,
          windowTokens: 128000,
        },
      },
      {
        sessionId: "s1",
        update: {
          sessionUpdate: "context_usage_update",
          usedTokens: 9000,
          windowTokens: 128000,
        },
      },
    ]);
    expect(useSessions.getState().contextUsage["s1"]).toEqual({
      used: 9000,
      window: 128000,
    });
  });

  it("one frame per session does not clobber another session's entry", () => {
    useSessions.getState().applySessionUpdates([
      {
        sessionId: "s1",
        update: {
          sessionUpdate: "context_usage_update",
          usedTokens: 12000,
          windowTokens: 128000,
        },
      },
      {
        sessionId: "s2",
        update: {
          sessionUpdate: "context_usage_update",
          usedTokens: 3000,
          windowTokens: 64000,
        },
      },
    ]);
    expect(useSessions.getState().contextUsage["s1"]).toEqual({
      used: 12000,
      window: 128000,
    });
    expect(useSessions.getState().contextUsage["s2"]).toEqual({
      used: 3000,
      window: 64000,
    });
  });

  it("a context_usage_update frame does not touch the session's messages", () => {
    useSessions.setState({
      messages: { s1: [{ kind: "agent-text", messageId: "m1", text: "hi", at: 1 }] },
    });
    const before = useSessions.getState().messages["s1"];
    useSessions.getState().applySessionUpdates([
      {
        sessionId: "s1",
        update: {
          sessionUpdate: "context_usage_update",
          usedTokens: 12000,
          windowTokens: 128000,
        },
      },
    ]);
    expect(useSessions.getState().messages["s1"]).toBe(before);
  });

  it("handleSessionClosed KEEPS the session's config options + context usage (a pause, not a discard)", () => {
    useSessions.getState().applySessionUpdates([
      {
        sessionId: "s1",
        update: {
          sessionUpdate: "config_option_update",
          configOptions: [
            {
              id: "model",
              name: "Model",
              type: "select",
              category: "model",
              currentValue: "tama/m1",
              options: [{ value: "tama/m1", name: "m1" }],
            },
          ],
        },
      },
      {
        sessionId: "s1",
        update: {
          sessionUpdate: "context_usage_update",
          usedTokens: 12000,
          windowTokens: 128000,
        },
      },
    ]);
    useSessions.getState().handleSessionClosed("s1", "user");
    // A close is a PAUSE: the last known values STAY (the stored session's
    // bottom bar is populated — the selectors render disabled; a resume's
    // `config_option_update` / `context_usage_update` frames refresh them).
    expect(useSessions.getState().contextUsage["s1"]).toEqual({
      used: 12000,
      window: 128000,
    });
    expect(useSessions.getState().configOptions["s1"]?.[0]?.id).toBe("model");
  });
});

describe("user message with image attachments", () => {
  it("(a) stores a user message with image attachments", () => {
    useSessions.getState().addUserMessage("s1", "hi", [{ name: "a.png", mimeType: "image/png", sizeBytes: 3, data: "QUJD" }]);
    const messages = useSessions.getState().messages["s1"];
    expect(messages).toHaveLength(1);
    expect(messages[0]).toMatchObject({
      kind: "user",
      text: "hi",
      images: [{ name: "a.png", mimeType: "image/png", sizeBytes: 3, data: "QUJD" }],
    });
  });

  it("(b) stores a user message without images without the key", () => {
    // Reset messages for this test
    useSessions.setState({ messages: { s1: [] } });
    useSessions.getState().addUserMessage("s1", "hi");
    const messages = useSessions.getState().messages["s1"];
    expect(messages).toHaveLength(1);
    expect("images" in messages[0]).toBe(false);
  });

  it("(b2) omits the images key when an empty array is passed", () => {
    useSessions.setState({ messages: { s1: [] } });
    useSessions.getState().addUserMessage("s1", "hi", []);
    const messages = useSessions.getState().messages["s1"];
    expect(messages).toHaveLength(1);
    expect("images" in messages[0]).toBe(false);
  });

  it("(c) hydrates a user row with image attachments (round-trip)", () => {
    const payload = JSON.stringify({
      text: "old",
      images: [{ name: "a.png", mimeType: "image/png", sizeBytes: 3, data: "QUJD" }],
    });
    const [msg] = rowToMessages(msgRow({ kind: "user", payloadJson: payload }));
    expect(msg).toMatchObject({
      kind: "user",
      text: "old",
      images: [{ name: "a.png", mimeType: "image/png", sizeBytes: 3, data: "QUJD" }],
    });
  });

  it("(d) hydrates a user row without images", () => {
    const payload = JSON.stringify({ text: "old" });
    const [msg] = rowToMessages(msgRow({ kind: "user", payloadJson: payload }));
    expect(msg).toMatchObject({ kind: "user", text: "old" });
    expect("images" in msg).toBe(false);
  });

  it("(e) drops malformed image entries during hydration", () => {
    const payload = JSON.stringify({
      text: "x",
      images: [{ data: 42 }, { name: "a.png", mimeType: "image/png", sizeBytes: 3, data: "QUJD" }],
    });
    const [msg] = rowToMessages(msgRow({ kind: "user", payloadJson: payload }));
    expect(msg).toMatchObject({
      kind: "user",
      text: "x",
      images: [{ name: "a.png", mimeType: "image/png", sizeBytes: 3, data: "QUJD" }],
    });
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

  it("maps a tool-call row's rawOutput onto the message", () => {
    const [msg] = rowToMessages(
      row({
        kind: "tool-call",
        messageKey: "tc1",
        payloadJson: JSON.stringify({
          toolCallId: "tc1",
          title: "bash",
          status: "completed",
          rawOutput: { content: [{ type: "text", text: "done" }] },
        }),
      }),
    );
    if (msg?.kind !== "tool-call") throw new Error("no tool-call message");
    expect(msg.rawOutput).toEqual({ content: [{ type: "text", text: "done" }] });
  });

  it("ignores rows with unparseable payloads or unknown kinds", () => {
    expect(
      rowToMessages(row({ kind: "user", payloadJson: "not json" })),
    ).toHaveLength(0);
    expect(rowToMessages(row({ kind: "mystery" }))).toHaveLength(0);
  });
});

describe("configOptions state", () => {
  beforeEach(() => {
    useSessions.setState({
      sessions: [],
      historySessions: [],
      activeSessionId: null,
      messages: {},
      configOptions: {},
    });
  });

  it("seeds configOptions from a started session's SessionInfo (and not when absent)", () => {
    useSessions.getState().addSession({
      sessionId: "s1",
      cwd: "/x",
      capabilities: {},
      archived: false,
      configOptions: [{ id: "model", name: "Model", type: "select", currentValue: "gpt-4" }],
    });
    expect(useSessions.getState().configOptions.s1).toEqual([
      { id: "model", name: "Model", type: "select", currentValue: "gpt-4" },
    ]);

    useSessions.getState().addSession({
      sessionId: "s2",
      cwd: "/x",
      capabilities: {},
      archived: false,
    });
    expect("s2" in useSessions.getState().configOptions).toBe(false);
  });

  it("seeds configOptions from a resumed session's SessionInfo", async () => {
    useSessions.setState({
      historySessions: [
        { sessionId: "s1", cwd: "/x", capabilities: {}, archived: false },
      ],
    });
    await useSessions.getState().resumeSession("s1");
    expect(useSessions.getState().configOptions.s1).toEqual([
      { id: "model", name: "Model", type: "select", currentValue: "gpt-4" },
    ]);
  });

  it("replaces a session's configOptions on a config_option_update update", () => {
    useSessions.getState().applyConfigOptions("s1", [
      { id: "model", name: "Model", type: "select", currentValue: "gpt-3.5" },
    ]);
    useSessions.getState().applySessionUpdate("s1", {
      sessionUpdate: "config_option_update",
      configOptions: [
        { id: "model", name: "Model", type: "select", currentValue: "gpt-4" },
      ],
    });
    expect(useSessions.getState().configOptions.s1).toEqual([
      { id: "model", name: "Model", type: "select", currentValue: "gpt-4" },
    ]);

    // Non-config update should not change it
    useSessions.getState().applySessionUpdate("s1", {
      sessionUpdate: "agent_message_chunk",
      content: { type: "text", text: "hi" },
    });
    expect(useSessions.getState().configOptions.s1).toEqual([
      { id: "model", name: "Model", type: "select", currentValue: "gpt-4" },
    ]);
  });

  it("replaces a session's configOptions via applyConfigOptions", () => {
    useSessions.getState().applyConfigOptions("s1", [
      { id: "model", name: "Model", type: "select", currentValue: "gpt-3.5" },
    ]);
    useSessions.getState().applyConfigOptions("s1", [
      { id: "model", name: "Model", type: "select", currentValue: "gpt-4" },
    ]);
    expect(useSessions.getState().configOptions.s1).toEqual([
      { id: "model", name: "Model", type: "select", currentValue: "gpt-4" },
    ]);
  });

});

describe("applySessionUpdates (batch)", () => {
  // A local agent at xhigh thinking emits ~1000 `session-update` events/sec.
  // The listener coalesces them into one batch per frame; the batch must
  // apply them in ONE `setState` (one React render per frame, not one per
  // event) or the webview main thread stays saturated and the UI renders
  // at ~1fps.
  const thought = (text: string): AcpSessionUpdate => ({
    sessionUpdate: "agent_thought_chunk",
    content: { type: "text", text },
  });

  it("applies a burst of updates for one session in a single store update", () => {
    useSessions.getState().addSession(info("s1", "/x"));
    let notifications = 0;
    const unsub = useSessions.subscribe(() => {
      notifications += 1;
    });

    useSessions
      .getState()
      .applySessionUpdates([
        { sessionId: "s1", update: thought("a") },
        { sessionId: "s1", update: thought("b") },
        { sessionId: "s1", update: thought("c") },
      ]);
    unsub();

    expect(notifications).toBe(1);
    const messages = useSessions.getState().messages.s1;
    expect(messages).toHaveLength(1);
    expect(messages[0]).toMatchObject({
      kind: "agent-thought",
      text: "abc",
    });
  });

  it("updates several sessions in the same single store update", () => {
    useSessions.getState().addSession(info("s1", "/x"));
    useSessions.getState().addSession(info("s2", "/y"));
    let notifications = 0;
    const unsub = useSessions.subscribe(() => {
      notifications += 1;
    });

    useSessions
      .getState()
      .applySessionUpdates([
        { sessionId: "s1", update: thought("one") },
        { sessionId: "s2", update: thought("two") },
      ]);
    unsub();

    expect(notifications).toBe(1);
    expect((useSessions.getState().messages.s1[0] as { text: string }).text).toBe("one");
    expect((useSessions.getState().messages.s2[0] as { text: string }).text).toBe("two");
  });

  it("keeps the config_option_update side effect inside a batch", () => {
    useSessions.getState().addSession(info("s1", "/x"));
    useSessions
      .getState()
      .applySessionUpdates([
        {
          sessionId: "s1",
          update: {
            sessionUpdate: "config_option_update",
            configOptions: [
              { id: "model", name: "Model", type: "select", currentValue: "m1" },
            ],
          },
        },
        { sessionId: "s1", update: thought("after") },
      ]);

    expect(useSessions.getState().configOptions.s1).toEqual([
      { id: "model", name: "Model", type: "select", currentValue: "m1" },
    ]);
    // A `config_option_update` is ignored by the reducer (no message), but
    // it still updated `configOptions` — the batch applied it in order.
    expect((useSessions.getState().messages.s1[0] as { text: string }).text).toBe("after");
  });

  it("does not notify when the batch is a no-op", () => {
    useSessions.getState().addSession(info("s1", "/x"));
    const before = useSessions.getState();
    let notifications = 0;
    const unsub = useSessions.subscribe(() => {
      notifications += 1;
    });

    // Empty chunks are dropped by the reducer (same reference returned),
    // and unknown types fall through to `default` — the batch must not
    // notify at all (same state object back, no render).
    useSessions
      .getState()
      .applySessionUpdates([
        { sessionId: "s1", update: thought("") },
        { sessionId: "s1", update: { sessionUpdate: "some_future_type" } as unknown as AcpSessionUpdate },
      ]);
    unsub();

    expect(notifications).toBe(0);
    expect(useSessions.getState()).toBe(before);
  });
});

describe("resumeSession (history reload race)", () => {
  beforeEach(() => {
    useSessions.setState({
      sessions: [],
      historySessions: [],
      activeSessionId: null,
      messages: {},
      configOptions: {},
    });
  });

  it("does not wipe a user message added while the history reload is in flight (merge, not overwrite)", async () => {
    useSessions.setState({
      activeSessionId: "s1",
      historySessions: [
        {
          sessionId: "s1",
          cwd: "/x",
          capabilities: { loadSession: true },
          archived: false,
        },
      ],
      messages: {
        s1: [
          { kind: "user", text: "one", at: 1 },
          { kind: "agent-text", messageId: "m1", text: "two", at: 2 },
        ],
      },
    });
    // The history reload is slow (the resume IPC spawns the agent): it is
    // still in flight when `send()` adds the user message — the race the
    // merge exists for.
    let resolveRows: (rows: MessageRow[]) => void = () => {};
    vi.mocked(loadHistory).mockImplementationOnce(
      () => new Promise<MessageRow[]>((r) => (resolveRows = r)),
    );
    // `resumeSession` resolves once the resume IPC settles — the reload is
    // fire-and-forget (NOT awaited), so it is still in flight.
    await useSessions.getState().resumeSession("s1");
    // The user message is added while the reload is in flight (the
    // `send()` flow: `await resume()` → `addUserMessage` → `sendPrompt`,
    // which is the FIRST `record_message` of the user message — the
    // reloaded rows predate it, so the fresh row is NOT in them).
    useSessions.getState().addUserMessage("s1", "three");
    // The reload settles with the persisted history.
    resolveRows([
      {
        id: 1,
        sessionId: "s1",
        kind: "user",
        messageKey: null,
        payloadJson: JSON.stringify({ text: "one" }),
        createdAt: 1,
      },
      {
        id: 2,
        sessionId: "s1",
        kind: "agent-text",
        messageKey: "m1",
        payloadJson: JSON.stringify({ text: "two" }),
        createdAt: 2,
      },
    ]);
    // Let the fire-and-forget reload's continuation run.
    await new Promise((r) => setTimeout(r, 0));
    // The reloaded history is MERGED with the locally-added message (not
    // wiped): all three are present, the added one appended after.
    const msgs = useSessions.getState().messages.s1;
    expect(msgs).toHaveLength(3);
    expect(msgs[2]).toMatchObject({ kind: "user", text: "three" });
  });

  it("does not render a resumed prompt twice when record_message commits before the history query reads (dedup)", async () => {
    useSessions.setState({
      activeSessionId: "s1",
      historySessions: [
        {
          sessionId: "s1",
          cwd: "/x",
          capabilities: { loadSession: true },
          archived: false,
        },
      ],
      messages: {
        s1: [{ kind: "user", text: "one", at: 1 }],
      },
    });
    // The history reload is slow (the resume IPC spawns the agent): it is
    // still in flight when `send()` adds the user message.
    let resolveRows: (rows: MessageRow[]) => void = () => {};
    vi.mocked(loadHistory).mockImplementationOnce(
      () => new Promise<MessageRow[]>((r) => (resolveRows = r)),
    );
    await useSessions.getState().resumeSession("s1");
    // `send()` adds the user message while the reload is in flight, then
    // `send_prompt`'s `record_message` COMMITS it BEFORE the history query
    // reads — so the reloaded rows INCLUDE the new user message (the race
    // that made the prompt render twice, attachments included). The reloaded
    // row's `createdAt` is >= the local message's creation time (`at`):
    // the DB commit happens AFTER the client-side `addUserMessage` (which
    // stamps `at = Date.now()`), so the reloaded row is the SAME message
    // committed slightly later — the timestamp rule dedups it (equal or
    // newer timestamps both dedup: the safe direction).
    useSessions.getState().addUserMessage("s1", "two", [
      { name: "shot.png", mimeType: "image/png", sizeBytes: 12, data: "AAAA" },
    ]);
    const added = useSessions.getState().messages.s1;
    const addedAt = added[added.length - 1].at;
    resolveRows([
      {
        id: 1,
        sessionId: "s1",
        kind: "user",
        messageKey: null,
        payloadJson: JSON.stringify({ text: "one" }),
        createdAt: 1,
      },
      {
        id: 2,
        sessionId: "s1",
        kind: "user",
        messageKey: null,
        payloadJson: JSON.stringify({
          text: "two",
          images: [
            { name: "shot.png", mimeType: "image/png", sizeBytes: 12, data: "AAAA" },
          ],
        }),
        createdAt: addedAt,
      },
    ]);
    // Let the fire-and-forget reload's continuation run.
    await new Promise((r) => setTimeout(r, 0));
    // The prompt appears ONCE, not twice: the locally-added copy is
    // deduped against the reloaded row (text + images match AND the
    // reloaded row's timestamp is >= the local creation time).
    const msgs = useSessions.getState().messages.s1;
    expect(msgs).toHaveLength(2);
    expect(msgs.filter((m) => m.kind === "user" && m.text === "two")).toHaveLength(1);
  });

  it("keeps a re-sent identical prompt when the matching reloaded row is OLDER (repeated prompt, not a duplicate)", async () => {
    useSessions.setState({
      activeSessionId: "s1",
      historySessions: [
        {
          sessionId: "s1",
          cwd: "/x",
          capabilities: { loadSession: true },
          archived: false,
        },
      ],
      messages: {
        s1: [{ kind: "user", text: "hi", at: 1 }],
      },
    });
    // The history reload is slow (the resume IPC spawns the agent): it is
    // still in flight when the user re-sends the SAME prompt.
    let resolveRows: (rows: MessageRow[]) => void = () => {};
    vi.mocked(loadHistory).mockImplementationOnce(
      () => new Promise<MessageRow[]>((r) => (resolveRows = r)),
    );
    await useSessions.getState().resumeSession("s1");
    // The user re-sent the identical prompt while the reload is in flight:
    // the persisted (reloaded) "hi" is the OLDER one (committed long ago,
    // `createdAt = 1`) and the new local "hi" (`at = Date.now()`) is NOT
    // in the snapshot yet. The content match is a false positive for the
    // new turn — it must NOT be deduped.
    useSessions.getState().addUserMessage("s1", "hi");
    resolveRows([
      {
        id: 1,
        sessionId: "s1",
        kind: "user",
        messageKey: null,
        payloadJson: JSON.stringify({ text: "hi" }),
        createdAt: 1,
      },
    ]);
    // Let the fire-and-forget reload's continuation run.
    await new Promise((r) => setTimeout(r, 0));
    // BOTH the older reloaded "hi" AND the new local "hi" survive: the
    // matching reloaded row's timestamp (1) is EARLIER than the new
    // message's creation time, so it is the older prompt, not a duplicate.
    const msgs = useSessions.getState().messages.s1;
    const hi = msgs.filter((m) => m.kind === "user" && m.text === "hi");
    expect(hi).toHaveLength(2);
    // The new local copy (created just now, NOT the reloaded row's `at: 1`)
    // is present.
    expect(hi.some((m) => m.at > 1)).toBe(true);
  });

  it("dedupes a resumed prompt whose text carries <agent>/<mcp> mention blocks (the merge key is byte-identical across a reload)",
    async () => {
      // The `send()` REFINEMENT: the live bubble, the persisted record, and
      // the agent's input all carry the SAME EXPANDED text, so
      // `mergeDedupeKey`'s user case (`user|<text>|<images>`) matches across
      // a resume reload EVEN for the multi-line `<agent>` / `<mcp>` blocks
      // (em dashes, curly quotes, blank lines). Any normalization on either
      // side (a trim, a `splitMentionBlocks` render-then-persist, a
      // re-join) would change the key and render the prompt TWICE.
      const agent: AgentDefinitionDto = {
        name: "scout",
        description: "Fast recon.",
        model: null,
        scope: "user",
      };
      const mcp: McpServerInfo = {
        name: "postgres",
        kind: "stdio",
        summary: "npx -y x-mcp",
      };
      // The REAL expanded shape (built by the production builder, not a
      // hand-written literal).
      const expanded = expandMentions("ping @scout and #postgres", {
        skills: [],
        agents: [agent],
        mcpServers: [mcp],
      });
      // Precondition: the fixture really is the block shape (two blocks, one
      // per new kind) — otherwise this test would degrade to the plain-text
      // case already covered above.
      expect(expanded).toContain('<agent name="scout">');
      expect(expanded).toContain('<mcp name="postgres">');

      useSessions.setState({
        activeSessionId: "s1",
        historySessions: [
          {
            sessionId: "s1",
            cwd: "/x",
            capabilities: { loadSession: true },
            archived: false,
          },
        ],
        messages: { s1: [{ kind: "user", text: "one", at: 1 }] },
      });
      let resolveRows: (rows: MessageRow[]) => void = () => {};
      vi.mocked(loadHistory).mockImplementationOnce(
        () => new Promise<MessageRow[]>((r) => (resolveRows = r)),
      );
      await useSessions.getState().resumeSession("s1");
      // The live row the send appended (the EXPANDED text — `send()` calls
      // `addUserMessage(id, expandMentions(rawText, …))`).
      useSessions.getState().addUserMessage("s1", expanded);
      const addedAt =
        useSessions.getState().messages.s1[
          useSessions.getState().messages.s1.length - 1
        ].at;
      // The persisted row `record_message` committed for the SAME message
      // (`payloadJson.text` is the sent text verbatim — the persistence path
      // stores what it is handed, no reshaping).
      resolveRows([
        {
          id: 1,
          sessionId: "s1",
          kind: "user",
          messageKey: null,
          payloadJson: JSON.stringify({ text: "one" }),
          createdAt: 1,
        },
        {
          id: 2,
          sessionId: "s1",
          kind: "user",
          messageKey: null,
          payloadJson: JSON.stringify({ text: expanded }),
          createdAt: addedAt,
        },
      ]);
      await new Promise((r) => setTimeout(r, 0));
      const msgs = useSessions.getState().messages.s1;
      // DEDUPED: the locally-added copy is dropped against the reloaded row
      // (same `mergeDedupeKey`, reloaded `createdAt` >= the local `at`), so
      // the mention-expanded prompt renders ONCE.
      expect(msgs).toHaveLength(2);
      const ping = msgs.filter(
        (m) => m.kind === "user" && m.text.startsWith("ping @scout"),
      );
      expect(ping).toHaveLength(1);
      // The surviving row is byte-identical to the expansion (the transcript
      // keeps the blocks — the split is DISPLAY-only).
      expect(ping[0]!.kind === "user" ? ping[0]!.text : null).toBe(expanded);
    });

  it("does NOT dedupe a mention prompt when the reloaded row holds the UNEXPANDED text (the key is byte-exact)",
    async () => {
      // The negative control for the test above (and the reason it is not a
      // tautology): a live row carrying the EXPANDED text and a persisted row
      // carrying the RAW `@`/`#` text are DIFFERENT keys, so both survive.
      // If the dedupe key were normalized (e.g. the blocks stripped by
      // `splitMentionBlocks` before hashing), this pair would collapse and
      // the test above would pass for the wrong reason.
      const agent: AgentDefinitionDto = {
        name: "scout",
        description: "Fast recon.",
        model: null,
        scope: "user",
      };
      const expanded = expandMentions("ping @scout", {
        skills: [],
        agents: [agent],
        mcpServers: [],
      });
      useSessions.setState({
        activeSessionId: "s1",
        historySessions: [
          {
            sessionId: "s1",
            cwd: "/x",
            capabilities: { loadSession: true },
            archived: false,
          },
        ],
        messages: { s1: [] },
      });
      let resolveRows: (rows: MessageRow[]) => void = () => {};
      vi.mocked(loadHistory).mockImplementationOnce(
        () => new Promise<MessageRow[]>((r) => (resolveRows = r)),
      );
      await useSessions.getState().resumeSession("s1");
      useSessions.getState().addUserMessage("s1", expanded);
      const addedAt =
        useSessions.getState().messages.s1[
          useSessions.getState().messages.s1.length - 1
        ].at;
      resolveRows([
        {
          id: 1,
          sessionId: "s1",
          kind: "user",
          messageKey: null,
          // The RAW draft (NOT what `send()` persisted) — a divergence the
          // dedupe must catch.
          payloadJson: JSON.stringify({ text: "ping @scout" }),
          createdAt: addedAt,
        },
      ]);
      await new Promise((r) => setTimeout(r, 0));
      const msgs = useSessions.getState().messages.s1;
      expect(msgs).toHaveLength(2);
      expect(
        msgs.filter((m) => m.kind === "user" && m.text === expanded),
      ).toHaveLength(1);
    });
});

describe("archivedSessions (sticky, ADR 0016)", () => {
  beforeEach(() => {
    useSessions.setState({
      sessions: [],
      historySessions: [],
      archivedSessions: [],
      activeSessionId: null,
      closeReasons: {},
      messages: {},
      configOptions: {},
    });
  });

  it("setHistorySessions splits rows by the archived flag (live ids in neither list)", () => {
    useSessions.setState({
      sessions: [info("live-1", "/x")],
    });
    useSessions.getState().setHistorySessions([
      info("live-1", "/x"), // live id → excluded from BOTH lists
      { ...info("arch-1", "/x"), archived: true },
      info("hist-1", "/x"),
    ]);
    const st = useSessions.getState();
    expect(st.historySessions.map((s) => s.sessionId)).toEqual(["hist-1"]);
    expect(st.archivedSessions.map((s) => s.sessionId)).toEqual(["arch-1"]);
  });

  it("archiveSession moves the entry historySessions → archivedSessions and calls the backend", async () => {
    useSessions.setState({
      historySessions: [info("h1", "/x")],
    });
    await useSessions.getState().archiveSession("h1");
    expect(vi.mocked(setSessionArchived)).toHaveBeenCalledWith("h1", true);
    const st = useSessions.getState();
    expect(st.historySessions).toEqual([]);
    expect(st.archivedSessions).toEqual([info("h1", "/x")]);
  });

  it("archiveSession is a no-op for an unknown id (does not call the backend)", async () => {
    useSessions.setState({ historySessions: [] });
    await useSessions.getState().archiveSession("missing");
    expect(vi.mocked(setSessionArchived)).not.toHaveBeenCalled();
  });

  it("unarchiveSession is the mirror", async () => {
    useSessions.setState({
      archivedSessions: [info("h1", "/x")],
    });
    await useSessions.getState().unarchiveSession("h1");
    expect(vi.mocked(setSessionArchived)).toHaveBeenCalledWith("h1", false);
    const st = useSessions.getState();
    expect(st.archivedSessions).toEqual([]);
    expect(st.historySessions).toEqual([info("h1", "/x")]);
  });

  it("archiveSession is idempotent under concurrent invocation (double-click)", async () => {
    // Two invocations both pass the pre-await membership check; the move
    // must still land the id in `archivedSessions` EXACTLY ONCE.
    useSessions.setState({
      historySessions: [info("h1", "/x")],
    });
    const p1 = useSessions.getState().archiveSession("h1");
    const p2 = useSessions.getState().archiveSession("h1");
    await Promise.all([p1, p2]);
    const st = useSessions.getState();
    expect(st.historySessions.map((s) => s.sessionId)).not.toContain("h1");
    expect(
      st.archivedSessions.map((s) => s.sessionId).filter((id) => id === "h1"),
    ).toHaveLength(1);
  });

  it("unarchiveSession is idempotent under concurrent invocation (double-click)", async () => {
    useSessions.setState({
      archivedSessions: [info("h1", "/x")],
    });
    const p1 = useSessions.getState().unarchiveSession("h1");
    const p2 = useSessions.getState().unarchiveSession("h1");
    await Promise.all([p1, p2]);
    const st = useSessions.getState();
    expect(st.archivedSessions.map((s) => s.sessionId)).not.toContain("h1");
    expect(
      st.historySessions.map((s) => s.sessionId).filter((id) => id === "h1"),
    ).toHaveLength(1);
  });

  it("a delete that completes mid-archive-await leaves no ghost row", async () => {
    useSessions.setState({
      historySessions: [info("h1", "/x")],
    });
    // Manual deferred resolution: the delete's `set` must land BEFORE the
    // archive's `set`. With plain `mockResolvedValue` the mock resolves in
    // call order (archive first — it called the backend first), which would
    // let the delete clear the row and hide the bug.
    let resolveArchived: (v: boolean) => void = () => {};
    vi.mocked(setSessionArchived).mockImplementationOnce(
      () =>
        new Promise<boolean>((resolve) => {
          resolveArchived = resolve;
        }),
    );
    const pArchive = useSessions.getState().archiveSession("h1");
    const pDelete = useSessions.getState().deleteSession("h1");
    await pDelete; // delete's `set` clears both lists first
    resolveArchived(true);
    await pArchive; // archive's `set` must re-check and no-op
    const st = useSessions.getState();
    expect(st.historySessions.map((s) => s.sessionId)).not.toContain("h1");
    expect(st.archivedSessions.map((s) => s.sessionId)).not.toContain("h1");
  });

  it("an archive that lands after a resume of the same session keeps client and DB in agreement (sticky live)", async () => {
    // The mocked `resumeSession` resolves to a FIXED `SessionInfo` with
    // `sessionId: "s1"` — seed the history list with that id.
    useSessions.setState({
      historySessions: [info("s1", "/x")],
    });
    // Manual deferred resolution: the resume's `set` must land BEFORE the
    // archive's `set` (the row's Archive hover button stays clickable
    // until the resume's `set` lands). With plain `mockResolvedValue`
    // the archive's backend resolves first (it was called first) and the
    // race would not be exercised.
    let resolveArchived: (v: boolean) => void = () => {};
    vi.mocked(setSessionArchived).mockImplementationOnce(
      () =>
        new Promise<boolean>((resolve) => {
          resolveArchived = resolve;
        }),
    );
    const pArchive = useSessions.getState().archiveSession("s1");
    const pResume = useSessions.getState().resumeSession("s1");
    await pResume; // resume's `set` removes the id from `historySessions`,
                   // adds it to `sessions`, leaves it OUT of
                   // `archivedSessions` (the pre-sticky state)
    resolveArchived(true);
    await pArchive; // the DB flag is now `true` — the client must land
                    // the id in `archivedSessions` (sticky live), not no-op
    const st = useSessions.getState();
    expect(st.sessions.map((s) => s.sessionId)).toContain("s1");
    expect(st.archivedSessions.map((s) => s.sessionId)).toContain("s1");
    expect(st.historySessions.map((s) => s.sessionId)).not.toContain("s1");
  });

  it("an unarchive that lands after a resume of the same session does not duplicate the live id into historySessions", async () => {
    useSessions.setState({
      archivedSessions: [info("s1", "/x")],
    });
    // Manual deferred resolution: the resume's `set` must land BEFORE the
    // unarchive's `set` (see the archive race above).
    let resolveArchived: (v: boolean) => void = () => {};
    vi.mocked(setSessionArchived).mockImplementationOnce(
      () =>
        new Promise<boolean>((resolve) => {
          resolveArchived = resolve;
        }),
    );
    const pUnarchive = useSessions.getState().unarchiveSession("s1");
    const pResume = useSessions.getState().resumeSession("s1");
    await pResume; // the id is now live: in `sessions` AND (sticky)
                   // `archivedSessions`
    resolveArchived(false);
    await pUnarchive; // must remove it from `archivedSessions` but NOT
                      // append it to `historySessions` (the close handler
                      // lands it there on close)
    const st = useSessions.getState();
    expect(st.sessions.map((s) => s.sessionId)).toContain("s1");
    expect(st.archivedSessions.map((s) => s.sessionId)).not.toContain("s1");
    expect(st.historySessions.map((s) => s.sessionId)).not.toContain("s1");
  });

  it("resumeSession finds an archived session and keeps it sticky (in BOTH live and archived)", async () => {
    // The mocked `resumeSession` (tauri) resolves to a FIXED `SessionInfo`
    // with `sessionId: "s1"` — seed the archived list with that id.
    useSessions.setState({
      archivedSessions: [info("s1", "/x")],
    });
    await useSessions.getState().resumeSession("s1");
    const st = useSessions.getState();
    expect(st.sessions.map((s) => s.sessionId)).toContain("s1");
    // STICKY: the entry stays in `archivedSessions` while the session is
    // live (the Archived view filters out live ids).
    expect(st.archivedSessions.map((s) => s.sessionId)).toContain("s1");
    expect(st.historySessions.map((s) => s.sessionId)).not.toContain("s1");
  });

  it("a resumed-then-closed archived session stays archived (NOT in historySessions)", async () => {
    useSessions.setState({
      archivedSessions: [info("s1", "/x")],
    });
    await useSessions.getState().resumeSession("s1");
    useSessions.getState().handleSessionClosed("s1", "user");
    const st = useSessions.getState();
    expect(st.archivedSessions.map((s) => s.sessionId)).toContain("s1");
    expect(st.historySessions.map((s) => s.sessionId)).not.toContain("s1");
  });

  it("deleteSession removes the id from all three lists", async () => {
    useSessions.setState({
      sessions: [info("d1", "/x")],
      historySessions: [info("d1", "/x")],
      archivedSessions: [info("d1", "/x")],
      activeSessionId: "d1",
    });
    await useSessions.getState().deleteSession("d1");
    const st = useSessions.getState();
    expect(st.sessions).toEqual([]);
    expect(st.historySessions).toEqual([]);
    expect(st.archivedSessions).toEqual([]);
    expect(st.activeSessionId).toBeNull();
  });
});

describe("spaceViewFor — archivedSessionIds (view membership only)", () => {
  it("reports archivedSessionIds without polluting storedSessionIds, and lastReason falls back to the archived head", () => {
    const space = row("/w/alpha", 1000);
    const live = info("live-a", "/w/alpha");
    const stored = info("stored-a", "/w/alpha");
    const archived = { ...info("arch-a", "/w/alpha"), archived: true };
    const reasons: Record<string, CloseReasonStr> = {
      "live-a": "user",
      "stored-a": "error",
      "arch-a": "agent-exited",
    };
    const view = spaceViewFor(space, [live], [stored], reasons, [archived]);
    expect(view.archivedSessionIds).toEqual(["arch-a"]);
    // `storedSessionIds` is UNCHANGED: stored sessions only (the archived
    // id must NOT appear — the Space group renders stored sessions only).
    expect(view.storedSessionIds).toEqual(["stored-a"]);
    // The live session is the newest: its reason wins.
    expect(view.lastReason).toBe("user");

    // No live session, no stored sessions: the ARCHIVED head is still the
    // space's newest stored session — the close-reason banner must not
    // silently vanish for an archived-only space.
    const archivedOnly = spaceViewFor(space, [], [], reasons, [archived]);
    expect(archivedOnly.archivedSessionIds).toEqual(["arch-a"]);
    expect(archivedOnly.storedSessionIds).toEqual([]);
    expect(archivedOnly.lastReason).toBe("agent-exited");
  });
});

describe("activeSpacePath (the selected Space — the top tabs' state)", () => {
  beforeEach(() => {
    useSessions.setState({
      sessions: [],
      historySessions: [],
      archivedSessions: [],
      activeSessionId: null,
      spaces: [],
      activeSpacePath: null,
      closeReasons: {},
      messages: {},
      configOptions: {},
    });
  });

  const space = (path: string): SpaceRow => ({
    path,
    createdAt: 0,
    lastOpenedAt: 0,
    trusted: false,
  });

  it("boot (setSpaces, nothing active): the auto-selected session's space becomes active; no sessions → the most recent space", () => {
    // A stored session in /a (the input order is newest-first).
    useSessions.getState().setSpaces([space("/a"), space("/b")]);
    // No sessions at all → the most recent space (the FIRST row —
    // `list_spaces` order is `lastOpenedAt` desc).
    expect(useSessions.getState().activeSpacePath).toBe("/a");
    expect(useSessions.getState().activeSessionId).toBeNull();

    useSessions.setState({ spaces: [], activeSpacePath: null });
    useSessions.getState().setHistorySessions([
      { ...info("h1", "/a"), archived: false },
      { ...info("h2", "/b"), archived: false },
    ]);
    useSessions.getState().setSpaces([space("/a"), space("/b")]);
    // `autoSelectActive` lands on the most recent space's newest session
    // (the /a session — first space, first stored session).
    expect(useSessions.getState().activeSessionId).toBe("h1");
    expect(useSessions.getState().activeSpacePath).toBe("/a");
  });

  it("openSession follows the session's space (live or stored)", () => {
    useSessions.setState({
      spaces: [space("/a"), space("/b")],
      activeSpacePath: "/a",
    });
    useSessions.getState().addSession(info("live-1", "/a"));
    expect(useSessions.getState().activeSpacePath).toBe("/a");
    useSessions.getState().setHistorySessions([
      { ...info("hist-1", "/b"), archived: false },
    ]);
    useSessions.getState().openSession("hist-1");
    expect(useSessions.getState().activeSessionId).toBe("hist-1");
    expect(useSessions.getState().activeSpacePath).toBe("/b");
  });

  it("openSession for a session whose cwd is NO space leaves activeSpacePath alone", () => {
    useSessions.setState({
      spaces: [space("/a")],
      activeSpacePath: "/a",
    });
    useSessions.getState().setHistorySessions([
      { ...info("legacy-1", "/not-a-space"), archived: false },
    ]);
    useSessions.getState().openSession("legacy-1");
    expect(useSessions.getState().activeSpacePath).toBe("/a");
  });

  it("selectSpace: live session preferred, then the newest stored, then nothing", () => {
    useSessions.setState({
      spaces: [space("/a"), space("/b")],
      activeSpacePath: "/a",
    });
    // /b holds a live + a stored session: the live wins.
    useSessions.getState().addSession(info("live-1", "/b"));
    useSessions.getState().setHistorySessions([
      { ...info("hist-1", "/b"), archived: false },
    ]);
    useSessions.getState().selectSpace("/b");
    expect(useSessions.getState().activeSpacePath).toBe("/b");
    expect(useSessions.getState().activeSessionId).toBe("live-1");

    // No live in /b: the NEWEST stored (input order — first wins).
    useSessions.getState().handleSessionClosed("live-1", "user");
    useSessions.getState().selectSpace("/b");
    expect(useSessions.getState().activeSessionId).toBe("hist-1");

    // An empty space: no session (the chat's empty state).
    useSessions.getState().selectSpace("/a");
    expect(useSessions.getState().activeSpacePath).toBe("/a");
    expect(useSessions.getState().activeSessionId).toBeNull();
  });

  it("selectSpace is a no-op for an unknown path", () => {
    useSessions.setState({
      spaces: [space("/a")],
      activeSpacePath: "/a",
    });
    useSessions.getState().selectSpace("/nowhere");
    expect(useSessions.getState().activeSpacePath).toBe("/a");
  });

  it("the start flow (addSession + addSpace) makes the new space active", () => {
    useSessions.setState({
      spaces: [space("/a")],
      activeSpacePath: "/a",
    });
    // The start flow: `addSession` (the fresh session) + `addSpace`
    // (the space row upsert) — the new space becomes the active one.
    useSessions.getState().addSession(info("live-1", "/new"));
    useSessions.getState().addSpace("/new");
    expect(useSessions.getState().activeSessionId).toBe("live-1");
    expect(useSessions.getState().activeSpacePath).toBe("/new");
  });

  it("removing the active space falls back to the first remaining space (or null)", async () => {
    useSessions.setState({
      spaces: [space("/a"), space("/b")],
      activeSpacePath: "/a",
    });
    // `removeSpace` awaits the backend command (the mock resolves).
    await useSessions.getState().removeSpace("/a");
    expect(useSessions.getState().activeSpacePath).toBe("/b");
    await useSessions.getState().removeSpace("/b");
    expect(useSessions.getState().activeSpacePath).toBeNull();
  });
});

describe("setSpaceTrusted (the trust flag — the sidebar's shield UI is gone; the behavior lives here)", () => {
  beforeEach(() => {
    useSessions.setState({
      sessions: [],
      historySessions: [],
      archivedSessions: [],
      activeSessionId: null,
      activeSpacePath: null,
      spaces: [
        { path: "/tmp/alpha", createdAt: 0, lastOpenedAt: 0, trusted: false },
        { path: "/tmp/beta", createdAt: 0, lastOpenedAt: 0, trusted: true },
      ],
      closeReasons: {},
      messages: {},
      configOptions: {},
    });
    // Reset the Tauri mocks (clears any `mockImplementationOnce` queues
    // left by an earlier test in this file — the mock is module-scoped).
    vi.mocked(setSpaceTrusted).mockReset();
    vi.mocked(setSpaceTrusted).mockResolvedValue(undefined);
    vi.mocked(deleteSpace).mockReset();
    vi.mocked(deleteSpace).mockResolvedValue(undefined);
  });

  it("flips the flag optimistically and calls the set_space_trusted wrapper", async () => {
    const toggling = useSessions.getState().setSpaceTrusted("/tmp/alpha", true);
    // Optimistic: the store flipped BEFORE the (async) command resolves —
    // the spaces store has no live refresh, so the flag must not wait for
    // a round-trip.
    expect(
      useSessions.getState().spaces.find((s) => s.path === "/tmp/alpha")!.trusted,
    ).toBe(true);
    expect(vi.mocked(setSpaceTrusted)).toHaveBeenCalledWith("/tmp/alpha", true);
    await toggling;
  });

  it("rolls the flag back to the previous value when the command rejects", async () => {
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
    vi.mocked(setSpaceTrusted).mockRejectedValueOnce(new Error("boom"));
    const toggling = useSessions.getState().setSpaceTrusted("/tmp/beta", false);
    // Optimistic flip first (beta trusted → untrusted)...
    expect(
      useSessions.getState().spaces.find((s) => s.path === "/tmp/beta")!.trusted,
    ).toBe(false);
    // ...then rolled back once the command rejects.
    await toggling;
    expect(
      useSessions.getState().spaces.find((s) => s.path === "/tmp/beta")!.trusted,
    ).toBe(true);
    expect(consoleError).toHaveBeenCalledWith(
      "Failed to set trusted for /tmp/beta:",
      expect.anything(),
    );
    consoleError.mockRestore();
  });

  it("serializes rapid trust toggles for a space (the second command runs only after the first settles)", async () => {
    // The first command stays pending on a deferred promise: without a
    // per-path queue, the second click's command would be issued
    // immediately (two overlapping Tauri invokes could commit in either
    // order and the DB could end on the OPPOSITE value from the
    // optimistic UI).
    let resolveFirst!: () => void;
    vi.mocked(setSpaceTrusted).mockImplementationOnce(
      () =>
        new Promise<void>((resolve) => {
          resolveFirst = resolve;
        }),
    );
    vi.mocked(setSpaceTrusted).mockImplementationOnce(() => Promise.resolve());
    // `/tmp/gamma` is a FRESH path (no earlier test queued a command
    // for it): the first command is issued synchronously.
    useSessions.getState().setSpaces([
      { path: "/tmp/gamma", createdAt: 0, lastOpenedAt: 0, trusted: false },
    ]);
    // Toggle trust on...
    const first = useSessions.getState().setSpaceTrusted("/tmp/gamma", true);
    expect(vi.mocked(setSpaceTrusted)).toHaveBeenCalledTimes(1);
    // ...then immediately off (the optimistic flip re-labeled the UI).
    const second = useSessions.getState().setSpaceTrusted("/tmp/gamma", false);
    // The second command is NOT issued until the first settles.
    expect(vi.mocked(setSpaceTrusted)).toHaveBeenCalledTimes(1);
    resolveFirst();
    await first;
    await second;
    expect(vi.mocked(setSpaceTrusted)).toHaveBeenCalledTimes(2);
    expect(vi.mocked(setSpaceTrusted)).toHaveBeenNthCalledWith(1, "/tmp/gamma", true);
    expect(vi.mocked(setSpaceTrusted)).toHaveBeenNthCalledWith(2, "/tmp/gamma", false);
    // The final state ends on the last click's value.
    expect(
      useSessions.getState().spaces.find((s) => s.path === "/tmp/gamma")!.trusted,
    ).toBe(false);
  });

  it("settles on the DB-committed value when BOTH of two rapid toggles reject (not the first click's optimistic value)", async () => {
    // Seed through `setSpaces` (the production load path) so the store's
    // committed-trusted baseline is seeded from the DB rows like in
    // production. `delta` is a fresh path (no baseline entry from an
    // earlier test) committed as `trusted: false`.
    const now = Date.now();
    useSessions.getState().setSpaces([
      { path: "/tmp/delta", createdAt: now, lastOpenedAt: now, trusted: false },
    ]);
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
    // Both commands reject, on deferred promises so the two rollbacks
    // land in click order: the first click's rollback first, the
    // second's LAST — the last rollback is what the UI settles on.
    let rejectFirst!: (err: Error) => void;
    let rejectSecond!: (err: Error) => void;
    vi.mocked(setSpaceTrusted).mockImplementationOnce(
      () =>
        new Promise<void>((_, reject) => {
          rejectFirst = reject;
        }),
    );
    vi.mocked(setSpaceTrusted).mockImplementationOnce(
      () =>
        new Promise<void>((_, reject) => {
          rejectSecond = reject;
        }),
    );
    // Click 1 (false → true): the optimistic flip lands before the
    // command settles.
    const click1 = useSessions.getState().setSpaceTrusted("/tmp/delta", true);
    expect(
      useSessions.getState().spaces.find((s) => s.path === "/tmp/delta")!.trusted,
    ).toBe(true);
    // Click 2 (true → false) while click 1's command is still pending:
    // the per-path queue defers the second command.
    const click2 = useSessions.getState().setSpaceTrusted("/tmp/delta", false);
    expect(
      useSessions.getState().spaces.find((s) => s.path === "/tmp/delta")!.trusted,
    ).toBe(false);
    // Reject click 1's command: its rollback lands first, and the queue
    // then issues click 2's (still pending) command — in a microtask
    // that runs AFTER `await click1`'s continuation, so wait for it.
    rejectFirst(new Error("boom1"));
    await click1;
    await vi.waitFor(() => {
      expect(typeof rejectSecond).toBe("function");
    });
    // Reject click 2's command: its rollback lands LAST — the final
    // value is the DB-committed baseline (false), NOT the first
    // click's optimistic value (true).
    rejectSecond(new Error("boom2"));
    await click2;
    expect(
      useSessions.getState().spaces.find((s) => s.path === "/tmp/delta")!.trusted,
    ).toBe(false);
    consoleError.mockRestore();
  });

  it("does NOT leave a stale committed-trusted baseline after a remove (a late in-flight toggle success must not re-insert the pruned entry)", async () => {
    // Seed through `setSpaces` (the production load path) so the store's
    // committed-trusted baseline is seeded from the DB rows: `epsilon`
    // is committed as `trusted: false`.
    const now = Date.now();
    useSessions.getState().setSpaces([
      { path: "/tmp/epsilon", createdAt: now, lastOpenedAt: now, trusted: false },
    ]);
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
    // A toggle in flight on a deferred command...
    let resolveToggle!: () => void;
    vi.mocked(setSpaceTrusted).mockImplementationOnce(
      () =>
        new Promise<void>((resolve) => {
          resolveToggle = resolve;
        }),
    );
    const toggling = useSessions.getState().setSpaceTrusted("/tmp/epsilon", true);
    // ...and a removal while it is in flight (deferred delete): the
    // store prunes the path's baseline + queue once the delete settles.
    let resolveDelete!: () => void;
    vi.mocked(deleteSpace).mockImplementationOnce(
      () =>
        new Promise<void>((resolve) => {
          resolveDelete = resolve;
        }),
    );
    const removing = useSessions.getState().removeSpace("/tmp/epsilon");
    resolveDelete();
    await removing;
    // The in-flight toggle's command succeeds AFTER the prune: its
    // success handler must NOT re-insert a baseline entry for the
    // removed path (it would survive the `addSpace` guard and poison a
    // fresh re-add).
    resolveToggle();
    await toggling;
    // Re-add the path: a fresh DB row is committed as `trusted: false`,
    // so the (pruned) baseline must re-seed to false.
    useSessions.getState().addSpace("/tmp/epsilon");
    // A failed toggle must roll back to the FRESH baseline (false), not
    // a stale in-flight value (true).
    let rejectToggle!: (err: Error) => void;
    vi.mocked(setSpaceTrusted).mockImplementationOnce(
      () =>
        new Promise<void>((_, reject) => {
          rejectToggle = reject;
        }),
    );
    const toggle2 = useSessions.getState().setSpaceTrusted("/tmp/epsilon", true);
    rejectToggle(new Error("boom"));
    await toggle2;
    expect(
      useSessions.getState().spaces.find((s) => s.path === "/tmp/epsilon")!.trusted,
    ).toBe(false);
    consoleError.mockRestore();
  });
});
