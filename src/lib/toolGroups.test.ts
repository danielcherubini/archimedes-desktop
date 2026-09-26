import { describe, it, expect } from "vitest";
import type { Message } from "../store/sessions";
import { groupConsecutiveFileWrites } from "./toolGroups";

const tool = (
  id: string,
  title: string,
  at: number,
  status: "pending" | "completed" | "failed" = "completed",
): Message => ({ kind: "tool-call", id, title, status, at });

const text = (at: number): Message => ({
  kind: "agent-text",
  messageId: "m1",
  text: "x",
  at,
});

describe("groupConsecutiveFileWrites", () => {
  it("a single write stays a single unit", () => {
    const units = groupConsecutiveFileWrites([tool("t1", "write", 1)]);
    expect(units).toHaveLength(1);
    expect(units[0].kind).toBe("single");
  });

  it("a write + edit run folds into one changes-group with 2 messages", () => {
    const units = groupConsecutiveFileWrites([
      tool("t1", "write", 1),
      tool("t2", "edit", 2),
    ]);
    expect(units).toHaveLength(1);
    expect(units[0].kind).toBe("changes-group");
    if (units[0].kind === "changes-group") {
      expect(units[0].messages).toHaveLength(2);
      expect(units[0].messages.map((m) => m.title)).toEqual([
        "write",
        "edit",
      ]);
    }
  });

  it("an agent-text between two writes breaks the run (three single units)", () => {
    const units = groupConsecutiveFileWrites([
      tool("t1", "write", 1),
      text(2),
      tool("t2", "write", 3),
    ]);
    expect(units).toHaveLength(3);
    for (const u of units) expect(u.kind).toBe("single");
  });

  it("a run of three writes folds into one changes-group with 3 messages", () => {
    const units = groupConsecutiveFileWrites([
      tool("t1", "write", 1),
      tool("t2", "write", 2),
      tool("t3", "write", 3),
    ]);
    expect(units).toHaveLength(1);
    expect(units[0].kind).toBe("changes-group");
    if (units[0].kind === "changes-group") {
      expect(units[0].messages).toHaveLength(3);
    }
  });

  it("write + edit + write + bash → one changes-group (3) + one single (bash)", () => {
    const units = groupConsecutiveFileWrites([
      tool("t1", "write", 1),
      tool("t2", "edit", 2),
      tool("t3", "write", 3),
      tool("t4", "bash", 4),
    ]);
    expect(units).toHaveLength(2);
    expect(units[0].kind).toBe("changes-group");
    if (units[0].kind === "changes-group") {
      expect(units[0].messages).toHaveLength(3);
    }
    expect(units[1].kind).toBe("single");
    if (units[1].kind === "single" && units[1].message.kind === "tool-call") {
      expect(units[1].message.title).toBe("bash");
    }
  });

  it("non-file tools never group (bash + read → two single units)", () => {
    const units = groupConsecutiveFileWrites([
      tool("t1", "bash", 1),
      tool("t2", "read", 2),
    ]);
    expect(units).toHaveLength(2);
    for (const u of units) expect(u.kind).toBe("single");
  });

  it("an empty array yields no units", () => {
    expect(groupConsecutiveFileWrites([])).toEqual([]);
  });
});
