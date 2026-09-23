import { describe, it, expect, vi } from "vitest";
import {
  createBatchedSessionUpdate,
  type SessionUpdateBatchItem,
} from "./batchSessionUpdates";

/**
 * A manual scheduler: `schedule` pushes the flush callback into a queue
 * (standing in for `requestAnimationFrame`); the test drains the queue to
 * simulate the next frame.
 */
function makeScheduler() {
  const queue: Array<() => void> = [];
  return {
    schedule: (fn: () => void) => {
      queue.push(fn);
    },
    flushFrame: () => {
      const batch = queue.splice(0, queue.length);
      for (const fn of batch) fn();
    },
  };
}

const thought = (text: string) => ({
  sessionUpdate: "agent_thought_chunk" as const,
  content: { type: "text" as const, text },
});

describe("createBatchedSessionUpdate", () => {
  it("coalesces a burst of events into ONE apply per frame", () => {
    const apply = vi.fn();
    const { schedule, flushFrame } = makeScheduler();
    const push = createBatchedSessionUpdate(apply, schedule);

    // A local agent at xhigh thinking emits ~1000 chunks/sec; a full
    // frame's worth arrives before the next rAF.
    for (let i = 0; i < 1000; i++) {
      push({ sessionId: "s1", update: thought(`chunk ${i}`) });
    }
    expect(apply).not.toHaveBeenCalled(); // nothing applied mid-frame

    flushFrame();
    expect(apply).toHaveBeenCalledTimes(1);
    expect(apply.mock.calls[0][0]).toHaveLength(1000);
    expect(apply.mock.calls[0][0][999].update).toEqual(thought("chunk 999"));
  });

  it("batches the next frame's events separately", () => {
    const apply = vi.fn();
    const { schedule, flushFrame } = makeScheduler();
    const push = createBatchedSessionUpdate(apply, schedule);

    push({ sessionId: "s1", update: thought("a") });
    flushFrame();
    push({ sessionId: "s1", update: thought("b") });
    push({ sessionId: "s2", update: thought("c") });
    flushFrame();

    expect(apply).toHaveBeenCalledTimes(2);
    expect(apply.mock.calls[0][0]).toHaveLength(1);
    expect(apply.mock.calls[1][0]).toHaveLength(2);
  });

  it("delivers events in arrival order within a batch", () => {
    const apply = vi.fn();
    const { schedule, flushFrame } = makeScheduler();
    const push = createBatchedSessionUpdate(apply, schedule);

    const items: SessionUpdateBatchItem[] = [
      { sessionId: "s1", update: thought("1") },
      { sessionId: "s2", update: thought("2") },
      { sessionId: "s1", update: thought("3") },
    ];
    for (const item of items) push(item);
    flushFrame();

    expect(apply.mock.calls[0][0]).toEqual(items);
  });
});
