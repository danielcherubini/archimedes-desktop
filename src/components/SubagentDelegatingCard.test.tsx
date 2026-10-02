import { act, fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";
import SubagentDelegatingCard from "./SubagentDelegatingCard";
import { useSubagents } from "../store/subagents";
import { useSubagentSelection } from "../store/subagentSelection";
import { useSessions } from "../store/sessions";
import type { SubagentEntry } from "../store/subagents";

const entry = (over: Partial<SubagentEntry>): SubagentEntry => ({
  sessionId: "sub1",
  parentSessionId: "main1",
  agentName: "reviewer",
  task: "review the diff",
  status: "running",
  startedAt: Date.now(),
  ...over,
});

beforeEach(() => {
  for (const id of Object.keys(useSubagents.getState().entries)) {
    useSubagents.getState().dismiss(id);
  }
  useSubagentSelection.getState().select(null);
});

describe("SubagentDelegatingCard (the subagent tool card with the nested list)", () => {
  it("renders nested rows for subagents delegated in the tool call (name + task)", () => {
    useSubagents.getState().addSession(
      entry({ sessionId: "sub1", agentName: "reviewer", task: "review the diff" }),
    );
    useSubagents.getState().addSession(
      entry({
        sessionId: "sub2",
        agentName: "explorer",
        task: "explore the codebase",
      }),
    );
    render(
      <SubagentDelegatingCard
        title="subagent"
        status="pending"
        rawInput={{
          tasks: [
            { task: "review the diff" },
            { task: "explore the codebase" },
          ],
        }}
        sessionId="main1"
      />,
    );
    // Both agent names + both tasks render (the card is OPEN by default).
    expect(screen.getByText("reviewer")).toBeTruthy();
    expect(screen.getByText("review the diff")).toBeTruthy();
    expect(screen.getByText("explorer")).toBeTruthy();
    expect(screen.getByText("explore the codebase")).toBeTruthy();
  });

  it("does NOT show subagent entries from previous tool calls in the same parent session", () => {
    // S1 was dispatched in a previous tool call (Task 1)
    useSubagents.getState().addSession(
      entry({ sessionId: "sub1", agentName: "coder", task: "Task 1: UserPartyRestrictionDao fix" }),
    );
    // S2 was dispatched in a previous tool call (Task 2)
    useSubagents.getState().addSession(
      entry({ sessionId: "sub2", agentName: "coder", task: "Task 2: SqlLikeEscaper utility" }),
    );
    // S3 is dispatched in the current tool call (Task 3)
    useSubagents.getState().addSession(
      entry({ sessionId: "sub3", agentName: "coder", task: "Task 3: ClaimHeaderAutoDao registration" }),
    );

    render(
      <SubagentDelegatingCard
        title="subagent"
        status="pending"
        rawInput={{ task: "Task 3: ClaimHeaderAutoDao registration" }}
        sessionId="main1"
      />,
    );

    // Only Task 3 from this tool call must be displayed (header + nested row)
    expect(screen.getAllByText("Task 3: ClaimHeaderAutoDao registration")).toHaveLength(2);
    expect(screen.queryByText("Task 1: UserPartyRestrictionDao fix")).toBeNull();
    expect(screen.queryByText("Task 2: SqlLikeEscaper utility")).toBeNull();
  });

  it("renders model, thinking level, and tokens/stats in the metadata line (no redundant 'running' text)", () => {
    useSubagents.getState().addSession(
      entry({
        sessionId: "sub1",
        agentName: "reviewer",
        task: "review the diff",
        model: "anthropic/claude-3-7-sonnet",
        thinkingLevel: "high",
        status: "running",
      }),
    );
    render(
      <SubagentDelegatingCard
        title="subagent"
        status="pending"
        rawInput={{ task: "review the diff" }}
        sessionId="main1"
        rawOutput={{
          details: {
            progress: [
              {
                agent: "reviewer",
                task: "review the diff",
                status: "running",
                model: "claude-3-7-sonnet",
                tokens: 15400,
                percent: 8,
                currentTool: "read",
                currentToolArgs: "docs/foo.md",
              },
            ],
          },
        }}
      />,
    );

    // Shows model and formatted thinking indicator
    expect(screen.getByText("claude-3-7-sonnet")).toBeTruthy();
    expect(screen.getByText("◕ high")).toBeTruthy();
    // Shows context percentage / tokens
    expect(screen.getByText("8%")).toBeTruthy();
    // Does NOT render "running" chip text
    expect(screen.queryByText("running")).toBeNull();
  });

  it("renders NO rows for entries whose parentSessionId does not match", () => {
    useSubagents.getState().addSession(
      entry({ sessionId: "sub1", parentSessionId: "other-main" }),
    );
    render(
      <SubagentDelegatingCard
        title="subagent"
        status="pending"
        sessionId="main1"
      />,
    );
    expect(screen.queryByText("reviewer")).toBeNull();
    expect(screen.queryByText("review the diff")).toBeNull();
  });

  it("renders the empty state when there are no matching entries", () => {
    render(
      <SubagentDelegatingCard
        title="subagent"
        status="pending"
        sessionId="main1"
      />,
    );
    expect(screen.getByText("No subagents yet.")).toBeTruthy();
  });

  it("updates the activity preview live when the tool call's details change", () => {
    useSubagents.getState().addSession(
      entry({
        sessionId: "sub1",
        task: "review the diff",
        status: "running",
      }),
    );
    const { rerender } = render(
      <SubagentDelegatingCard
        title="subagent"
        status="pending"
        sessionId="main1"
        rawInput={{ task: "review the diff" }}
        rawOutput={{
          details: {
            progress: [
              {
                agent: "reviewer",
                task: "review the diff",
                status: "running",
                currentTool: "read",
                currentToolArgs: "docs/foo.md",
              },
            ],
          },
        }}
      />,
    );
    // The one-line activity renders for the matching entry.
    expect(screen.getByText("read: docs/foo.md")).toBeTruthy();
    // The details change (a new tool) → the preview re-renders.
    rerender(
      <SubagentDelegatingCard
        title="subagent"
        status="pending"
        sessionId="main1"
        rawInput={{ task: "review the diff" }}
        rawOutput={{
          details: {
            progress: [
              {
                agent: "reviewer",
                task: "review the diff",
                status: "running",
                currentTool: "grep",
                currentToolArgs: "TODO",
              },
            ],
          },
        }}
      />,
    );
    expect(screen.getByText("grep: TODO")).toBeTruthy();
    expect(screen.queryByText("read: docs/foo.md")).toBeNull();
  });

  it("clicking a row selects the subagent in useSubagentSelection", () => {
    useSubagents.getState().addSession(
      entry({ sessionId: "sub1", task: "review the diff" }),
    );
    useSubagents.getState().addSession(
      entry({
        sessionId: "sub2",
        agentName: "explorer",
        task: "explore the codebase",
      }),
    );
    render(
      <SubagentDelegatingCard
        title="subagent"
        status="pending"
        rawInput={{
          tasks: [
            { task: "review the diff" },
            { task: "explore the codebase" },
          ],
        }}
        sessionId="main1"
      />,
    );
    // Nothing is selected yet.
    expect(useSubagentSelection.getState().selectedSessionId).toBeNull();
    act(() => {
      fireEvent.click(
        screen.getByRole("button", { name: "Open reviewer transcript" }),
      );
    });
    expect(useSubagentSelection.getState().selectedSessionId).toBe("sub1");
    // Selecting another row replaces the selection.
    act(() => {
      fireEvent.click(
        screen.getByRole("button", { name: "Open explorer transcript" }),
      );
    });
    expect(useSubagentSelection.getState().selectedSessionId).toBe("sub2");
  });

  it("renders live tool duration and tool calls in the activity line", () => {
    useSubagents.getState().addSession(
      entry({
        sessionId: "sub1",
        agentName: "coder",
        task: "run tests",
        status: "running",
      }),
    );
    render(
      <SubagentDelegatingCard
        title="subagent"
        status="pending"
        rawInput={{ task: "run tests" }}
        sessionId="main1"
        rawOutput={{
          details: {
            progress: [
              {
                agent: "coder",
                task: "run tests",
                status: "running",
                currentTool: "bash",
                currentToolArgs: "cargo test",
                currentToolStartedAt: Date.now() - 4000,
              },
            ],
          },
        }}
      />,
    );

    // Shows current tool and args
    expect(screen.getByText("bash: cargo test")).toBeTruthy();
    // Shows live duration in the activity line
    expect(screen.getByText("· 4s")).toBeTruthy();
  });

  it("renders completed Done status in activity line when finished", () => {
    useSubagents.getState().addSession(
      entry({
        sessionId: "sub1",
        agentName: "coder",
        task: "run tests",
        status: "completed",
      }),
    );
    render(
      <SubagentDelegatingCard
        title="subagent"
        status="completed"
        rawInput={{ task: "run tests" }}
        sessionId="main1"
        rawOutput={{
          details: {
            results: [
              {
                agent: "coder",
                task: "run tests",
                childSessionId: "sub1",
                exitCode: 0,
              },
            ],
          },
        }}
      />,
    );

    expect(screen.getByText("Done")).toBeTruthy();
  });

  it("falls back to 'subagent' when agent name is empty string or absent", () => {
    useSubagents.getState().addSession(
      entry({
        sessionId: "sub1",
        agentName: "",
        task: "Task 5: UserDao case normalization",
        status: "running",
      }),
    );
    render(
      <SubagentDelegatingCard
        title="subagent"
        status="pending"
        rawInput={{ task: "Task 5: UserDao case normalization", agent: "" }}
        sessionId="main1"
      />,
    );

    expect(screen.getByText("subagent")).toBeTruthy();
    expect(screen.getAllByText("Task 5: UserDao case normalization")).toHaveLength(2);
  });

  it("reads live tool activity directly from subagent's session transcript in useSessions", () => {
    useSubagents.getState().addSession(
      entry({
        sessionId: "sub1",
        agentName: "coder",
        task: "Task 5: UserDao case normalization",
        status: "running",
      }),
    );

    // Populate subagent's session messages with an active tool call
    useSessions.setState({
      messages: {
        sub1: [
          {
            kind: "tool-call",
            id: "tc1",
            title: "read",
            status: "pending",
            rawInput: { path: "src/UserDao.java" },
            at: Date.now() - 3000,
          },
        ],
      },
    });

    render(
      <SubagentDelegatingCard
        title="subagent"
        status="pending"
        rawInput={{ task: "Task 5: UserDao case normalization" }}
        sessionId="main1"
      />,
    );

    // Resolves tool call from useSessions messages
    expect(screen.getByText("read: src/UserDao.java")).toBeTruthy();
    expect(screen.getByText("· 3s")).toBeTruthy();
  });

  it("reads thinking activity from subagent's session transcript in useSessions", () => {
    useSubagents.getState().addSession(
      entry({
        sessionId: "sub1",
        agentName: "coder",
        task: "Task 5: UserDao case normalization",
        status: "running",
      }),
    );

    useSessions.setState({
      messages: {
        sub1: [
          {
            kind: "agent-thought",
            messageId: "m1",
            text: "Analyzing database query case sensitivity\nChecking UserDao implementation",
            at: Date.now() - 1000,
          },
        ],
      },
    });

    render(
      <SubagentDelegatingCard
        title="subagent"
        status="pending"
        rawInput={{ task: "Task 5: UserDao case normalization" }}
        sessionId="main1"
      />,
    );

    expect(screen.getByText("[thinking] Checking UserDao implementation")).toBeTruthy();
  });
});
