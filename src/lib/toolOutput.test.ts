import { describe, it, expect } from "vitest";
import {
  toolVerb,
  toolIcon,
  fileSummaries,
  editChangeStat,
  failureText,
} from "./toolOutput";
import {
  SquareTerminalIcon,
  PencilIcon,
  WrenchIcon,
} from "lucide-react";

describe("toolVerb", () => {
  it("returns the past-tense verb for completed calls", () => {
    expect(toolVerb("bash", "completed")).toBe("Ran");
    expect(toolVerb("read", "completed")).toBe("Read");
    expect(toolVerb("edit", "completed")).toBe("Edited");
    expect(toolVerb("ls", "completed")).toBe("Listed");
    expect(toolVerb("web_search", "completed")).toBe("Searched");
    expect(toolVerb("ask", "completed")).toBe("Asked");
    expect(toolVerb("manage_todo_list", "completed")).toBe("Todos");
  });
  it("returns the present-tense verb for pending calls", () => {
    expect(toolVerb("bash", "pending")).toBe("Running");
    expect(toolVerb("write", "pending")).toBe("Writing");
    expect(toolVerb("grep", "pending")).toBe("Searching");
    expect(toolVerb("fetch_content", "pending")).toBe("Fetching");
    expect(toolVerb("subagent", "pending")).toBe("Delegating");
  });
  it("returns the past tense for failed calls (the Failed word is the signal)", () => {
    expect(toolVerb("bash", "failed")).toBe("Ran");
  });
  it("returns the same word for `mcp` on any status", () => {
    expect(toolVerb("mcp", "pending")).toBe("MCP");
    expect(toolVerb("mcp", "completed")).toBe("MCP");
    expect(toolVerb("mcp", "failed")).toBe("MCP");
  });
  it("returns undefined for unknown tools", () => {
    expect(toolVerb("mystery_tool", "completed")).toBeUndefined();
  });
});

describe("toolIcon", () => {
  it("returns the per-tool icon", () => {
    expect(toolIcon("bash")).toBe(SquareTerminalIcon);
    expect(toolIcon("edit")).toBe(PencilIcon);
  });
  it("falls back to the wrench icon for unknown tools", () => {
    expect(toolIcon("mystery_tool")).toBe(WrenchIcon);
  });
});

describe("fileSummaries", () => {
  it("returns the file for `read` with a path", () => {
    expect(fileSummaries("read", { path: "/a/b/c.ts" })).toEqual([
      { path: "/a/b/c.ts", fileName: "c.ts" },
    ]);
  });
  it("returns the file for `write` with a path (basename = full name)", () => {
    expect(fileSummaries("write", { path: "x.md" })).toEqual([
      { path: "x.md", fileName: "x.md" },
    ]);
  });
  it("returns one entry for `edit` with a path", () => {
    const out = fileSummaries("edit", { path: "/a/b/c.ts", edits: [] });
    expect(out).toHaveLength(1);
    expect(out[0]).toEqual({ path: "/a/b/c.ts", fileName: "c.ts" });
  });
  it("returns [] for tools that do not carry a path", () => {
    expect(fileSummaries("bash", { command: "ls" })).toEqual([]);
  });
  it("returns [] for `read` without a path", () => {
    expect(fileSummaries("read", {})).toEqual([]);
  });
});

describe("editChangeStat", () => {
  it("sums line counts of newText / oldText over one edit", () => {
    expect(
      editChangeStat("edit", {
        path: "/a/b/c.ts",
        edits: [{ oldText: "a\nb", newText: "x\ny\nz" }],
      }),
    ).toEqual({ added: 3, removed: 2 });
  });
  it("sums across multiple edits", () => {
    expect(
      editChangeStat("edit", {
        path: "/a/b/c.ts",
        edits: [
          { oldText: "a", newText: "x" },
          { oldText: "b\nc", newText: "y\nz" },
        ],
      }),
    ).toEqual({ added: 3, removed: 3 });
  });
  it("returns undefined for an empty edits array", () => {
    expect(editChangeStat("edit", { path: "/a/b/c.ts", edits: [] })).toBeUndefined();
  });
  it("returns undefined when all texts are empty", () => {
    expect(
      editChangeStat("edit", {
        path: "/a/b/c.ts",
        edits: [{ oldText: "", newText: "" }],
      }),
    ).toBeUndefined();
  });
  it("returns undefined for other tools", () => {
    expect(
      editChangeStat("write", { path: "/a/b/c.ts", edits: [{ oldText: "a", newText: "x" }] }),
    ).toBeUndefined();
  });
  it("returns undefined for input without edits", () => {
    expect(editChangeStat("edit", {})).toBeUndefined();
  });
});

describe("failureText", () => {
  it("prefers details.error over content text", () => {
    expect(
      failureText({
        details: { error: "boom" },
        content: [{ type: "text", text: "t" }],
      }),
    ).toBe("boom");
  });
  it("falls back to the first NON-EMPTY content text item", () => {
    expect(
      failureText({
        content: [
          { type: "text", text: "" },
          { type: "text", text: "reason" },
        ],
      }),
    ).toBe("reason");
  });
  it("returns a bare-string result as-is", () => {
    expect(failureText("bare error")).toBe("bare error");
  });
  it("returns undefined for an empty string", () => {
    expect(failureText("")).toBeUndefined();
  });
  it("returns undefined for an object with nothing usable", () => {
    expect(failureText({})).toBeUndefined();
  });
  it("returns undefined for undefined", () => {
    expect(failureText(undefined)).toBeUndefined();
  });
});
