import { act, fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";
import SubagentDelegatingCard from "./SubagentDelegatingCard";
import { useSubagents } from "../store/subagents";
import { useSubagentSelection } from "../store/subagentSelection";
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
  it("renders one nested row per subagent entry for the parent (name + task)", () => {
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
        rawInput={{ task: "kick off the review" }}
        sessionId="main1"
      />,
    );
    // Both agent names + both tasks render (the card is OPEN by default).
    expect(screen.getByText("reviewer")).toBeTruthy();
    expect(screen.getByText("review the diff")).toBeTruthy();
    expect(screen.getByText("explorer")).toBeTruthy();
    expect(screen.getByText("explore the codebase")).toBeTruthy();
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

  it("renders NO activity line when no details entry matches (a sibling card's row)", () => {
    useSubagents.getState().addSession(
      entry({ sessionId: "sub1", task: "review the diff" }),
    );
    // The `details` carry a DIFFERENT subagent — this card's row must
    // render without an activity line.
    render(
      <SubagentDelegatingCard
        title="subagent"
        status="pending"
        sessionId="main1"
        rawOutput={{
          details: {
            progress: [
              {
                agent: "someone-else",
                task: "a different task",
                status: "running",
                currentTool: "read",
                currentToolArgs: "docs/other.md",
              },
            ],
          },
        }}
      />,
    );
    expect(screen.getByText("reviewer")).toBeTruthy();
    expect(screen.queryByText("read: docs/other.md")).toBeNull();
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
});
