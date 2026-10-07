import { describe, it, expect } from "vitest";
import {
  toolVerb,
  toolIcon,
  fileSummaries,
  editChangeStat,
  failureText,
  readLineRange,
  normalizeToolOutput,
  summarizeSubagentFor,
  summarizeToolCall,
  subagentActivityFor,
  cleanModelName,
  extractThinkingFromModel,
  formatCost,
  formatDuration,
  formatThinkingIndicator,
  formatTokens,
  getModelContextWindow,
  subagentActivityLine,
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
  it("gives the skill tools an icon (not the unknown-tool wrench)", () => {
    expect(toolIcon("list_skills")).not.toBe(WrenchIcon);
    expect(toolIcon("read_skill")).not.toBe(WrenchIcon);
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

describe("readLineRange", () => {
  it("returns the inclusive range when both offset and limit are numbers", () => {
    expect(readLineRange({ offset: 5, limit: 50 })).toBe("L5–54");
    expect(readLineRange({ offset: 1, limit: 10 })).toBe("L1–10");
  });
  it("returns undefined when either is missing or not a number", () => {
    expect(readLineRange({})).toBeUndefined();
    expect(readLineRange({ offset: 5 })).toBeUndefined();
    expect(readLineRange({ limit: 5 })).toBeUndefined();
    expect(readLineRange({ offset: "5", limit: 50 })).toBeUndefined();
    expect(readLineRange("x")).toBeUndefined();
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

describe("normalizeToolOutput (the subagent progress envelope)", () => {
  const runningDetails = {
    mode: "single",
    results: [],
    progress: [
      {
        agent: "explore",
        status: "running",
        task: "Read the markdown documentation",
        currentTool: "read",
        currentToolArgs: "docs/decisions/0001-foo.md",
        currentToolStartedAt: Date.now() - 5000,
      },
    ],
  };
  it("renders a human-readable summary for the subagent details (not raw JSON)", () => {
    const out = normalizeToolOutput(
      { content: [], details: runningDetails },
      false,
      "subagent",
    );
    expect(out).toContain("explore");
    expect(out).toContain("Read the markdown documentation");
    expect(out).toContain("read");
    // Not a raw JSON dump (no `{"mode"` envelope marker).
    expect(out).not.toContain('{"mode"');
  });
  it("renders the LAST line of the subagent's streamed output when no current tool", () => {
    const out = normalizeToolOutput(
      {
        content: [],
        details: {
          mode: "single",
          progress: [
            {
              agent: "explore",
              status: "running",
              task: "Read the docs",
              recentOutput: ["line one", "line two"],
            },
          ],
        },
      },
      false,
      "subagent",
    );
    expect(out).toContain("line two");
    expect(out).not.toContain("line one");
  });
  it("returns the content text when present (not the details summary)", () => {
    const out = normalizeToolOutput(
      {
        content: [{ type: "text", text: "final output" }],
        details: runningDetails,
      },
      false,
      "subagent",
    );
    expect(out).toBe("final output");
  });
  it("still JSON-dumps a non-subagent details", () => {
    const out = normalizeToolOutput(
      { content: [], details: { foo: "bar" } },
      false,
      "some_other_tool",
    );
    expect(out).toBe('{"foo":"bar"}');
  });
});

describe("summarizeSubagentFor (one subagent's summary)", () => {
  it("filters to the matching subagent by childSessionId (results)", () => {
    const out = summarizeSubagentFor(
      {
        mode: "single",
        results: [
          {
            agent: "a",
            task: "task-a",
            childSessionId: "sub-a",
            exitCode: 0,
            finalOutput: "done-a",
          },
          {
            agent: "b",
            task: "task-b",
            childSessionId: "sub-b",
            exitCode: 0,
            finalOutput: "done-b",
          },
        ],
      },
      "sub-a",
      "task-a",
    );
    expect(out).toContain("task-a");
    expect(out).not.toContain("task-b");
  });
  it("filters to the matching subagent by task (progress)", () => {
    const out = summarizeSubagentFor(
      {
        mode: "parallel",
        progress: [
          { agent: "a", task: "task-a", status: "running" },
          { agent: "b", task: "task-b", status: "running" },
        ],
      },
      "sub-a",
      "task-a",
    );
    expect(out).toContain("task-a");
    expect(out).not.toContain("task-b");
  });
  it("returns undefined when no entry matches", () => {
    const out = summarizeSubagentFor(
      {
        mode: "single",
        results: [
          { agent: "a", task: "task-a", childSessionId: "sub-a" },
        ],
      },
      "sub-unknown",
      "task-unknown",
    );
    expect(out).toBeUndefined();
  });
});

describe("subagentActivityFor (the one-line activity)", () => {
  it("returns the one-line activity of a progress entry matching by task", () => {
    const out = subagentActivityFor(
      {
        progress: [
          {
            agent: "a",
            task: "task-a",
            status: "running",
            currentTool: "read",
            currentToolArgs: "docs/foo.md",
            currentToolStartedAt: Date.now() - 12000,
          },
        ],
      },
      "sub-a",
      "task-a",
    );
    // `currentTool` + `currentToolArgs` + the live duration.
    expect(out).toBe("read: docs/foo.md · 12s");
  });
  it("returns the one-line activity of a results entry matching by childSessionId", () => {
    const out = subagentActivityFor(
      {
        results: [
          {
            agent: "a",
            task: "task-a",
            childSessionId: "sub-a",
            exitCode: 0,
            finalOutput: "line1\nline2",
          },
        ],
      },
      "sub-a",
      "task-a",
    );
    // The LAST line of the final output (the `subagentActivityLine` treatment).
    expect(out).toBe("line2");
  });
  it("returns `Done` for a results entry with no final output", () => {
    const out = subagentActivityFor(
      {
        results: [
          {
            agent: "a",
            task: "task-a",
            childSessionId: "sub-a",
            exitCode: 0,
          },
        ],
      },
      "sub-a",
      "task-a",
    );
    expect(out).toBe("Done");
  });
  it("returns undefined when no entry matches the sessionId / task", () => {
    const out = subagentActivityFor(
      {
        progress: [
          { agent: "a", task: "task-a", status: "running" },
        ],
        results: [
          { agent: "a", task: "task-a", childSessionId: "sub-a" },
        ],
      },
      "sub-unknown",
      "task-unknown",
    );
    expect(out).toBeUndefined();
  });
  it("returns undefined when details is undefined / null / a non-object", () => {
    expect(subagentActivityFor(undefined, "sub-a", "task-a")).toBeUndefined();
    expect(subagentActivityFor(null, "sub-a", "task-a")).toBeUndefined();
    expect(subagentActivityFor("details", "sub-a", "task-a")).toBeUndefined();
  });
  it("filters FIRST (a finished subagent keeps its results entry while a sibling runs)", () => {
    // A's finished entry lives in `results`; B is still running in
    // `progress`. Prefer-then-filter would pick `progress` (B running) for
    // A's row — a stale preview. Filter-first picks A's `results` entry.
    const out = subagentActivityFor(
      {
        progress: [
          { agent: "a", task: "task-a", status: "completed" },
          {
            agent: "b",
            task: "task-b",
            status: "running",
            currentTool: "read",
            currentToolArgs: "docs/b.md",
          },
        ],
        results: [
          {
            agent: "a",
            task: "task-a",
            childSessionId: "sub-a",
            exitCode: 0,
            finalOutput: "done-a",
          },
        ],
      },
      "sub-a",
      "task-a",
    );
    expect(out).toBe("done-a");
  });
});

describe("summarizeToolCall", () => {
  it("shows the bash / sudo command", () => {
    expect(summarizeToolCall("bash", { command: "ls -la" })).toBe("ls -la");
    expect(summarizeToolCall("sudo_exec", { command: "apt install x" })).toBe(
      "apt install x",
    );
  });
  it("shows the read path (+ line range when offset + limit)", () => {
    expect(summarizeToolCall("read", { path: "/foo/bar.ts" })).toBe("/foo/bar.ts");
    expect(summarizeToolCall("read", { path: "/foo", offset: 1, limit: 10 })).toBe(
      "/foo (L1–10)",
    );
  });
  it("shows the grep / find pattern", () => {
    expect(summarizeToolCall("grep", { pattern: "foo" })).toBe("foo");
    expect(summarizeToolCall("grep", { pattern: "foo", path: "/bar" })).toBe(
      "foo in /bar",
    );
  });
  it("shows the subagent task (truncated)", () => {
    expect(summarizeToolCall("subagent", { task: "a".repeat(80) })).toBe(
      "a".repeat(60) + "…",
    );
  });
  it("returns undefined for an empty object (unknown tool)", () => {
    expect(summarizeToolCall("some_tool", {})).toBeUndefined();
  });
  it("falls back to a truncated JSON dump for unknown tools", () => {
    expect(summarizeToolCall("some_tool", { a: 1, b: 2 })).toBe('{"a":1,"b":2}');
  });
});

describe("summarizeToolCall (skill tools — the name is the handle)", () => {
  it("shows nothing for the bare list_skills call", () => {
    expect(summarizeToolCall("list_skills", {})).toBeUndefined();
  });
  it("shows the skill name", () => {
    expect(summarizeToolCall("read_skill", { name: "discuss" })).toBe("discuss");
  });
  it("shows the skill name with the bundled file", () => {
    expect(
      summarizeToolCall("read_skill", { name: "discuss", path: "./adr-format.md" }),
    ).toBe("discuss → adr-format.md");
  });
});

describe("summarizeToolCall (mcp — the server name on the side)", () => {
  it("shows the tool being called", () => {
    expect(summarizeToolCall("mcp", { tool: "echo" })).toBe("echo");
  });
  it("shows the tool + the server it is called on", () => {
    expect(summarizeToolCall("mcp", { tool: "echo", server: "postgres" })).toBe(
      "echo (postgres)",
    );
  });
  it("shows the server for a server-scoped action (list / auth)", () => {
    expect(summarizeToolCall("mcp", { server: "postgres" })).toBe("postgres");
    expect(summarizeToolCall("mcp", { action: "auth", server: "postgres" })).toBe(
      "postgres",
    );
  });
  it("shows the search / describe / connect queries", () => {
    expect(summarizeToolCall("mcp", { search: "echo" })).toBe("search: echo");
    expect(summarizeToolCall("mcp", { describe: "echo" })).toBe(
      "describe: echo",
    );
    expect(summarizeToolCall("mcp", { connect: "postgres" })).toBe(
      "connect postgres",
    );
  });
  it("shows the action when no server / tool is named (status)", () => {
    expect(summarizeToolCall("mcp", { action: "status" })).toBe("status");
  });
  it("returns undefined for the bare status call (mcp({}))", () => {
    expect(summarizeToolCall("mcp", {})).toBeUndefined();
  });
});

describe("subagent metadata and activity formatting helpers", () => {
  it("formats tokens into human-readable strings", () => {
    expect(formatTokens(450)).toBe("450");
    expect(formatTokens(12400)).toBe("12.4k");
    expect(formatTokens(12000)).toBe("12k");
    expect(formatTokens(1500000)).toBe("1.5M");
  });

  it("formats durations into human-readable spans", () => {
    expect(formatDuration(50)).toBe("<0.1s");
    expect(formatDuration(800)).toBe("0.8s");
    expect(formatDuration(4000)).toBe("4s");
    expect(formatDuration(75000)).toBe("1m15s");
    expect(formatDuration(120000)).toBe("2m");
  });

  it("formats costs into clean dollar strings", () => {
    expect(formatCost(0)).toBe("");
    expect(formatCost(0.0042)).toBe("$0.0042");
    expect(formatCost(0.15)).toBe("$0.15");
  });

  it("formats thinking indicators with glyphs", () => {
    expect(formatThinkingIndicator("high")).toBe("◕ high");
    expect(formatThinkingIndicator("medium")).toBe("◑ medium");
    expect(formatThinkingIndicator("low")).toBe("◔ low");
    expect(formatThinkingIndicator("off")).toBe("○ off");
    expect(formatThinkingIndicator("minimal")).toBe("○ minimal");
    expect(formatThinkingIndicator("xhigh")).toBe("● xhigh");
    expect(formatThinkingIndicator("max")).toBe("● max");
    // `none` (the LiteLLM verbatim vocabulary) is a thinking-OFF level —
    // the same glyph as `off` / `minimal`, not the mid fallback.
    expect(formatThinkingIndicator("none")).toBe("○ none");
    expect(formatThinkingIndicator(undefined)).toBeUndefined();
  });

  it("cleans model names and extracts thinking suffix", () => {
    expect(cleanModelName("claude-3-7-sonnet")).toBe("claude-3-7-sonnet");
    expect(cleanModelName("anthropic/claude-3-7-sonnet:high")).toBe("anthropic/claude-3-7-sonnet");
    expect(cleanModelName(undefined)).toBeUndefined();
    // A model id may contain a colon — the level is the trailing
    // segment after the LAST colon (mirrors Rust's `ModelRef::parse`).
    expect(cleanModelName("tama/m:1:high")).toBe("tama/m:1");

    expect(extractThinkingFromModel("claude-3-7-sonnet:high")).toBe("high");
    expect(extractThinkingFromModel("claude-3-7-sonnet")).toBeUndefined();
    expect(extractThinkingFromModel("tama/m:1:high")).toBe("high");
  });

  it("subagentActivityLine formats recent tool calls when no active tool is running", () => {
    const out = subagentActivityLine({
      status: "running",
      toolCalls: [
        { name: "read", argsPreview: "src/dao.rs", error: false },
      ],
    });
    expect(out).toBe("✓ read: src/dao.rs");
  });

  it("subagentActivityLine formats failed tool calls", () => {
    const out = subagentActivityLine({
      status: "running",
      toolCalls: [
        { name: "bash", argsPreview: "cargo test", error: true },
      ],
    });
    expect(out).toBe("✗ bash: cargo test");
  });

  it("resolves model context windows", () => {
    expect(getModelContextWindow("protector/gemini-3.8-flash")).toBe(1_000_000);
    expect(getModelContextWindow("anthropic/claude-3-7-sonnet")).toBe(200_000);
    expect(getModelContextWindow("openai/gpt-4o")).toBe(128_000);
    expect(getModelContextWindow("deepseek/deepseek-chat")).toBe(64_000);
    expect(getModelContextWindow(undefined)).toBe(200_000);
  });
});

