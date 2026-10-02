import { beforeEach, describe, expect, it } from "vitest";
import { useInteractive } from "./interactive";

const S1 = "s1";
const S2 = "s2";

beforeEach(() => {
  const s = useInteractive.getState();
  // Reset everything a previous test may have left behind.
  for (const id of new Set([
    ...Object.keys(s.requests),
    ...Object.keys(s.todos),
    ...Object.keys(s.agentState),
    ...Object.keys(s.cost),
    ...Object.keys(s.session),
  ])) {
    s.dismissSession(id);
  }
});

describe("addRequest / removeRequest", () => {
  it("adds a request keyed by session id", () => {
    useInteractive.getState().addRequest(S1, {
      requestId: "r1",
      method: "ask",
      source: "main",
      params: { questions: [] },
    });
    const list = useInteractive.getState().requests[S1];
    expect(list).toHaveLength(1);
    expect(list?.[0]).toMatchObject({
      requestId: "r1",
      method: "ask",
      source: "main",
    });
  });

  it("dedups by requestId (a re-delivered frame replaces, not duplicates)", () => {
    const add = useInteractive.getState().addRequest;
    add(S1, {
      requestId: "r1",
      method: "confirm",
      source: "main",
      params: { command: "a", reason: "b" },
    });
    add(S1, {
      requestId: "r1",
      method: "confirm",
      source: "main",
      params: { command: "a2", reason: "b2" },
    });
    const list = useInteractive.getState().requests[S1];
    expect(list).toHaveLength(1);
    expect(list?.[0].params).toEqual({ command: "a2", reason: "b2" });
  });

  it("keeps requests for different sessions separate", () => {
    const add = useInteractive.getState().addRequest;
    add(S1, { requestId: "r1", method: "ask", source: "main", params: {} });
    add(S2, { requestId: "r1", method: "confirm", source: "main", params: {} });
    expect(useInteractive.getState().requests[S1]).toHaveLength(1);
    expect(useInteractive.getState().requests[S2]).toHaveLength(1);
  });

  it("removes a request by requestId", () => {
    useInteractive
      .getState()
      .addRequest(S1, {
        requestId: "r1",
        method: "ask",
        source: "main",
        params: {},
      });
    useInteractive.getState().removeRequest(S1, "r1");
    expect(useInteractive.getState().requests[S1]).toHaveLength(0);
  });
});

describe("todos", () => {
  it("applyTodoUpdate routes source=main to the main column", () => {
    useInteractive.getState().applyTodoUpdate(S1, {
      source: "main",
      todos: [{ content: "a", status: "in_progress" }],
    });
    expect(useInteractive.getState().todos[S1]?.main).toEqual([
      { content: "a", status: "in_progress" },
    ]);
  });

  it("applyTodoUpdate routes a subagent source to its own column", () => {
    useInteractive.getState().applyTodoUpdate(S1, {
      source: "subagent:x",
      todos: [{ content: "child", status: "pending" }],
    });
    const column = useInteractive.getState().todos[S1];
    expect(column?.subagents["subagent:x"]).toEqual([
      { content: "child", status: "pending" },
    ]);
    expect(column?.main).toEqual([]);
  });

  it("applyTodoUpdate replaces the column on re-delivery (latest wins)", () => {
    useInteractive.getState().applyTodoUpdate(S1, {
      source: "main",
      todos: [{ content: "a", status: "pending" }],
    });
    useInteractive.getState().applyTodoUpdate(S1, {
      source: "main",
      todos: [{ content: "b", status: "completed" }],
    });
    expect(useInteractive.getState().todos[S1]?.main).toEqual([
      { content: "b", status: "completed" },
    ]);
  });

  it("applyTodoClear deletes the subagent column", () => {
    useInteractive.getState().applyTodoUpdate(S1, {
      source: "subagent:x",
      todos: [{ content: "child", status: "pending" }],
    });
    useInteractive.getState().applyTodoClear(S1, { source: "subagent:x" });
    expect(useInteractive.getState().todos[S1]?.subagents).toEqual({});
  });

  it("applyTodoClear clears the main column", () => {
    useInteractive.getState().applyTodoUpdate(S1, {
      source: "main",
      todos: [{ content: "a", status: "pending" }],
    });
    useInteractive.getState().applyTodoClear(S1, { source: "main" });
    expect(useInteractive.getState().todos[S1]?.main).toEqual([]);
  });
});

describe("wired-but-not-rendered state", () => {
  it("applyState stores the agent state", () => {
    useInteractive.getState().applyState(S1, { state: "blocked" });
    expect(useInteractive.getState().agentState[S1]).toBe("blocked");
  });

  it("applyCost stores the cost payload", () => {
    useInteractive.getState().applyCost(S1, { inputTokens: 10 });
    expect(useInteractive.getState().cost[S1]).toEqual({ inputTokens: 10 });
  });

  it("applySession stores the session payload", () => {
    useInteractive.getState().applySession(S1, { bridgeSession: "abc" });
    expect(useInteractive.getState().session[S1]).toEqual({ bridgeSession: "abc" });
  });
});

describe("dismissSession", () => {
  it("clears requests, todos, agentState, cost, and session for the session", () => {
    const s = useInteractive.getState();
    s.addRequest(S1, {
      requestId: "r1",
      method: "ask",
      source: "main",
      params: {},
    });
    s.applyTodoUpdate(S1, {
      source: "main",
      todos: [{ content: "a", status: "pending" }],
    });
    s.applyState(S1, { state: "working" });
    s.applyCost(S1, { inputTokens: 1 });
    s.applySession(S1, { bridgeSession: "abc" });
    s.dismissSession(S1);
    const after = useInteractive.getState();
    expect(after.requests[S1]).toBeUndefined();
    expect(after.todos[S1]).toBeUndefined();
    expect(after.agentState[S1]).toBeUndefined();
    expect(after.cost[S1]).toBeUndefined();
    expect(after.session[S1]).toBeUndefined();
  });

  it("leaves other sessions alone", () => {
    const s = useInteractive.getState();
    s.addRequest(S1, {
      requestId: "r1",
      method: "ask",
      source: "main",
      params: {},
    });
    s.addRequest(S2, {
      requestId: "r2",
      method: "ask",
      source: "main",
      params: {},
    });
    s.dismissSession(S1);
    expect(useInteractive.getState().requests[S2]).toHaveLength(1);
  });
});
