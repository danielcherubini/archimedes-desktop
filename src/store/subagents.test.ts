import { beforeEach, describe, expect, it } from "vitest";
import { MAX_CLOSED_ENTRIES, useSubagents } from "./subagents";

const A1 = {
  sessionId: "sub1",
  parentSessionId: "main1",
  agentName: "reviewer",
  task: "review the diff",
  status: "running" as const,
};

const A2 = {
  sessionId: "sub2",
  parentSessionId: "main1",
  agentName: "explorer",
  task: "explore the codebase",
  status: "running" as const,
};

beforeEach(() => {
  // Reset: dismiss every entry a previous test may have left behind.
  for (const id of Object.keys(useSubagents.getState().entries)) {
    useSubagents.getState().dismiss(id);
  }
});

describe("addSession", () => {
  it("adds an entry keyed by the subagent session id", () => {
    useSubagents.getState().addSession(A1);
    expect(useSubagents.getState().entries["sub1"]).toMatchObject({
      parentSessionId: "main1",
      agentName: "reviewer",
      task: "review the diff",
      status: "running",
    });
  });

  it("replaces the entry on re-delivery (latest wins)", () => {
    const add = useSubagents.getState().addSession;
    add(A1);
    add({ ...A1, task: "review v2" });
    expect(useSubagents.getState().entries["sub1"]).toMatchObject({
      task: "review v2",
      status: "running",
    });
  });

  it("keeps entries in arrival order", () => {
    const add = useSubagents.getState().addSession;
    add(A1);
    add(A2);
    expect(Object.keys(useSubagents.getState().entries)).toEqual([
      "sub1",
      "sub2",
    ]);
  });

  it("keeps entries for different sessions separate", () => {
    const add = useSubagents.getState().addSession;
    add(A1);
    add(A2);
    expect(useSubagents.getState().entries["sub1"].agentName).toBe("reviewer");
    expect(useSubagents.getState().entries["sub2"].agentName).toBe("explorer");
  });
});

describe("markClosed", () => {
  it("marks an entry completed", () => {
    useSubagents.getState().addSession(A1);
    useSubagents.getState().markClosed("sub1", "completed");
    expect(useSubagents.getState().entries["sub1"].status).toBe("completed");
  });

  it("marks an entry failed with the error", () => {
    useSubagents.getState().addSession(A1);
    useSubagents.getState().markClosed("sub1", "failed", "boom");
    expect(useSubagents.getState().entries["sub1"]).toMatchObject({
      status: "failed",
      error: "boom",
    });
  });

  it("stores the metrics snapshot (the `subagent-closed` payload)", () => {
    useSubagents.getState().addSession(A1);
    const metrics = { inputTokens: 0, outputTokens: 0, cost: 0, durationMs: 1234 };
    useSubagents.getState().markClosed("sub1", "completed", undefined, metrics);
    expect(useSubagents.getState().entries["sub1"].metrics).toEqual(metrics);
  });

  it("is a no-op for an unknown session id", () => {
    useSubagents.getState().markClosed("unknown", "completed");
    expect(useSubagents.getState().entries["unknown"]).toBeUndefined();
    expect(Object.keys(useSubagents.getState().entries)).toEqual([]);
  });

  it("keeps the entry's identity fields (only status/error/metrics change)", () => {
    useSubagents.getState().addSession(A1);
    useSubagents.getState().markClosed("sub1", "failed", "e");
    expect(useSubagents.getState().entries["sub1"]).toMatchObject({
      sessionId: "sub1",
      parentSessionId: "main1",
      agentName: "reviewer",
      task: "review the diff",
    });
  });
});

describe("dismiss", () => {
  it("removes the entry", () => {
    useSubagents.getState().addSession(A1);
    useSubagents.getState().dismiss("sub1");
    expect(useSubagents.getState().entries["sub1"]).toBeUndefined();
  });

  it("is a no-op for an unknown session id", () => {
    useSubagents.getState().addSession(A1);
    useSubagents.getState().dismiss("unknown");
    expect(useSubagents.getState().entries["sub1"]).toBeDefined();
  });

  it("a later `markClosed` does not resurrect a dismissed entry", () => {
    useSubagents.getState().addSession(A1);
    useSubagents.getState().dismiss("sub1");
    useSubagents.getState().markClosed("sub1", "completed");
    expect(useSubagents.getState().entries["sub1"]).toBeUndefined();
  });
});

describe("bounded retention (MAX_CLOSED_ENTRIES)", () => {
  it("evicts the oldest closed entries beyond the cap, keeping the most recent K", () => {
    const { addSession, markClosed } = useSubagents.getState();
    for (let i = 0; i < MAX_CLOSED_ENTRIES + 1; i++) {
      const id = `cap${i}`;
      addSession({ ...A1, sessionId: id });
      markClosed(id, "completed");
    }
    // 21 closed > cap of 20: the OLDEST closed (cap0) is evicted, the most
    // recent 20 survive in insertion order.
    expect(Object.keys(useSubagents.getState().entries)).toEqual(
      Array.from({ length: MAX_CLOSED_ENTRIES }, (_, i) => `cap${i + 1}`),
    );
  });

  it("never evicts running entries", () => {
    const { addSession, markClosed } = useSubagents.getState();
    for (let i = 0; i < MAX_CLOSED_ENTRIES + 4; i++) {
      const id = `run${i}`;
      addSession({ ...A1, sessionId: id });
    }
    // Close the first 21 (beyond the cap): the oldest closed is evicted,
    // every running entry survives.
    for (let i = 0; i < MAX_CLOSED_ENTRIES + 1; i++) {
      markClosed(`run${i}`, "completed");
    }
    const keys = Object.keys(useSubagents.getState().entries);
    expect(keys).toEqual(
      Array.from({ length: MAX_CLOSED_ENTRIES + 3 }, (_, i) => `run${i + 1}`),
    );
    expect(keys).toContain("run23"); // the last running entry
  });

  it("`addSession` also enforces the cap (re-delivering a frame as closed)", () => {
    const { addSession, markClosed } = useSubagents.getState();
    for (let i = 0; i < MAX_CLOSED_ENTRIES + 1; i++) {
      const id = `k${i}`;
      addSession({ ...A1, sessionId: id });
    }
    // Close k1..k20 (20 = exactly the cap; k0 is still running):
    // nothing evicted.
    for (let i = 1; i <= MAX_CLOSED_ENTRIES; i++) {
      markClosed(`k${i}`, "failed", "e");
    }
    expect(Object.keys(useSubagents.getState().entries).length).toBe(
      MAX_CLOSED_ENTRIES + 1,
    );
    // Re-deliver k0 as CLOSED ("latest wins" replaces it): the closed
    // count is now 21 > the cap, and k0's INSERTION position is the
    // oldest — so k0 itself is evicted, the rest survive.
    addSession({ ...A1, sessionId: "k0", status: "failed" });
    const keys = Object.keys(useSubagents.getState().entries);
    expect(keys).toEqual(
      Array.from({ length: MAX_CLOSED_ENTRIES }, (_, i) => `k${i + 1}`),
    );
  });

  it("a user `dismiss` of a closed entry does not trigger a re-eviction", () => {
    const { addSession, markClosed, dismiss } = useSubagents.getState();
    for (let i = 0; i < MAX_CLOSED_ENTRIES + 2; i++) {
      const id = `d${i}`;
      addSession({ ...A1, sessionId: id });
      markClosed(id, "completed");
    }
    // Already evicted down to the cap by markClosed.
    expect(Object.keys(useSubagents.getState().entries)).toEqual(
      Array.from({ length: MAX_CLOSED_ENTRIES }, (_, i) => `d${i + 2}`),
    );
    dismiss("d2"); // user dismiss stays as-is: just removes the entry.
    expect(useSubagents.getState().entries["d2"]).toBeUndefined();
    expect(Object.keys(useSubagents.getState().entries).length).toBe(
      MAX_CLOSED_ENTRIES - 1,
    );
  });
});
