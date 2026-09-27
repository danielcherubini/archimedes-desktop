import { beforeEach, describe, expect, it } from "vitest";
import { useSubagentSelection } from "./subagentSelection";

beforeEach(() => {
  useSubagentSelection.getState().select(null);
});

describe("useSubagentSelection (which subagent is open in the modal)", () => {
  it("starts closed (selectedSessionId = null)", () => {
    expect(useSubagentSelection.getState().selectedSessionId).toBeNull();
  });

  it("select(sessionId) opens the modal for that subagent", () => {
    useSubagentSelection.getState().select("sub1");
    expect(useSubagentSelection.getState().selectedSessionId).toBe("sub1");
  });

  it("select(null) closes the modal", () => {
    useSubagentSelection.getState().select("sub1");
    useSubagentSelection.getState().select(null);
    expect(useSubagentSelection.getState().selectedSessionId).toBeNull();
  });

  it("select(sessionId) replaces the previously open subagent", () => {
    useSubagentSelection.getState().select("sub1");
    useSubagentSelection.getState().select("sub2");
    expect(useSubagentSelection.getState().selectedSessionId).toBe("sub2");
  });
});
