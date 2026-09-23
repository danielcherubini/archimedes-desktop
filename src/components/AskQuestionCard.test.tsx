import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import AskQuestionCard from "./AskQuestionCard";
import { useBridge } from "../store/bridge";

// Mock the Tauri IPC layer; everything else (stores) is the real code.
vi.mock("../lib/tauri", async () => {
  const actual = await vi.importActual<Record<string, unknown>>("../lib/tauri");
  return {
    ...actual,
    respondBridgeRequest: vi.fn().mockResolvedValue(undefined),
  };
});

import { respondBridgeRequest } from "../lib/tauri";

const mockedRespond = vi.mocked(respondBridgeRequest);

const singleRequest = {
  requestId: "r1",
  method: "ask" as const,
  source: "main",
  params: {
    questions: [
      {
        id: "q1",
        question: "Which color?",
        options: [{ label: "Red" }, { label: "Blue" }],
      },
    ],
  },
};

beforeEach(() => {
  useBridge.getState().dismissSession("s1");
  mockedRespond.mockReset();
  mockedRespond.mockResolvedValue(undefined);
});

describe("AskQuestionCard", () => {
  it("renders the question and its options", () => {
    useBridge.getState().addRequest("s1", singleRequest);
    render(<AskQuestionCard sessionId="s1" requestId="r1" />);
    expect(screen.getByText("Which color?")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Red" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Blue" })).toBeTruthy();
    expect(
      screen.getByRole("button", { name: "Other (type your own)" }),
    ).toBeTruthy();
  });

  it("renders nothing when no request is pending for the id", () => {
    const { container } = render(
      <AskQuestionCard sessionId="s1" requestId="missing" />,
    );
    expect(container.firstChild).toBeNull();
  });

  it("submits the selected option via respondBridgeRequest and collapses", async () => {
    useBridge.getState().addRequest("s1", singleRequest);
    render(<AskQuestionCard sessionId="s1" requestId="r1" />);
    fireEvent.click(screen.getByRole("button", { name: "Red" }));
    fireEvent.click(screen.getByRole("button", { name: "Submit" }));
    await waitFor(() =>
      expect(mockedRespond).toHaveBeenCalledWith("s1", "r1", {
        cancelled: false,
        results: [{ id: "q1", selectedOptions: ["Red"] }],
      }),
    );
    // The card collapses: the request is removed from the store.
    expect(useBridge.getState().requests["s1"]).toHaveLength(0);
  });

  it("collects multiple selections for a multi question", async () => {
    useBridge.getState().addRequest("s1", {
      requestId: "r1",
      method: "ask",
      source: "main",
      params: {
        questions: [
          {
            id: "q1",
            question: "Pick some",
            multi: true,
            options: [{ label: "Red" }, { label: "Blue" }, { label: "Green" }],
          },
        ],
      },
    });
    render(<AskQuestionCard sessionId="s1" requestId="r1" />);
    fireEvent.click(screen.getByRole("button", { name: "Red" }));
    fireEvent.click(screen.getByRole("button", { name: "Blue" }));
    fireEvent.click(screen.getByRole("button", { name: "Submit" }));
    await waitFor(() =>
      expect(mockedRespond).toHaveBeenCalledWith("s1", "r1", {
        cancelled: false,
        results: [{ id: "q1", selectedOptions: ["Red", "Blue"] }],
      }),
    );
  });

  it("renders the ask treatment (tint header, question + options, fill on select)", () => {
    useBridge.getState().addRequest("s1", singleRequest);
    const { container } = render(<AskQuestionCard sessionId="s1" requestId="r1" />);
    // The `--color-interaction-ask-*` treatment: a `bg-interaction-ask-surface`
    // tint header with the label in the ask foreground.
    const header = container.querySelector(
      ".bg-interaction-ask-surface",
    )!;
    expect(header).toBeTruthy();
    expect(
      header.querySelector(".text-interaction-ask-foreground")?.textContent,
    ).toBe("Agent question");
    // The option list renders the question + options.
    expect(screen.getByText("Which color?")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Red" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Blue" })).toBeTruthy();
    // No option is selected yet: no `bg-interaction-ask-fill`.
    expect(container.querySelector(".bg-interaction-ask-fill")).toBeNull();
    // Selecting an option gets the ask-fill treatment.
    fireEvent.click(screen.getByRole("button", { name: "Red" }));
    expect(container.querySelector(".bg-interaction-ask-fill")).toBeTruthy();
  });

  it("receives focus on mount when unanchored (Enter/Esc are live)", () => {
    useBridge.getState().addRequest("s1", singleRequest);
    const { container } = render(<AskQuestionCard sessionId="s1" requestId="r1" />);
    // A `main` ask without a `toolCallId` is unanchored: the card takes
    // focus on mount so the keyboard shortcuts work immediately (a
    // `focus-visible` ring is the visible indicator — `outline-none` would
    // suppress it).
    expect(document.activeElement).toBe(container.firstChild);
  });

  it("labels a subagent request by its source", () => {
    useBridge.getState().addRequest("s1", {
      requestId: "r1",
      method: "ask",
      source: "subagent:x",
      params: {
        questions: [
          {
            id: "q1",
            question: "Which color?",
            options: [{ label: "Red" }],
          },
        ],
      },
    });
    render(<AskQuestionCard sessionId="s1" requestId="r1" />);
    expect(screen.getByText("subagent:x")).toBeTruthy();
  });

  it("cancels with cancelled=true and empty results, and collapses", async () => {
    useBridge.getState().addRequest("s1", singleRequest);
    render(<AskQuestionCard sessionId="s1" requestId="r1" />);
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    await waitFor(() =>
      expect(mockedRespond).toHaveBeenCalledWith("s1", "r1", {
        cancelled: true,
        results: [],
      }),
    );
    expect(useBridge.getState().requests["s1"]).toHaveLength(0);
  });

  it("sends customInput for the Other option", async () => {
    useBridge.getState().addRequest("s1", singleRequest);
    render(<AskQuestionCard sessionId="s1" requestId="r1" />);
    fireEvent.click(
      screen.getByRole("button", { name: "Other (type your own)" }),
    );
    fireEvent.change(screen.getByPlaceholderText("Type your own answer"), {
      target: { value: "purple" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Submit" }));
    await waitFor(() =>
      expect(mockedRespond).toHaveBeenCalledWith("s1", "r1", {
        cancelled: false,
        results: [{ id: "q1", selectedOptions: [], customInput: "purple" }],
      }),
    );
  });

  it("sends the option note as a 'label - note' entry", async () => {
    useBridge.getState().addRequest("s1", singleRequest);
    render(<AskQuestionCard sessionId="s1" requestId="r1" />);
    fireEvent.click(screen.getByRole("button", { name: "Red" }));
    fireEvent.change(screen.getByPlaceholderText("note (optional)"), {
      target: { value: "the warm one" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Submit" }));
    await waitFor(() =>
      expect(mockedRespond).toHaveBeenCalledWith("s1", "r1", {
        cancelled: false,
        results: [{ id: "q1", selectedOptions: ["Red - the warm one"] }],
      }),
    );
  });

  it("shows a (Recommended) suffix on the recommended option", () => {
    useBridge.getState().addRequest("s1", {
      requestId: "r1",
      method: "ask",
      source: "main",
      params: {
        questions: [
          {
            id: "q1",
            question: "Which color?",
            recommended: 0,
            options: [{ label: "Red" }, { label: "Blue" }],
          },
        ],
      },
    });
    render(<AskQuestionCard sessionId="s1" requestId="r1" />);
    expect(screen.getByText("Red (Recommended)")).toBeTruthy();
  });

  it("sends one result per question for a multi-question ask", async () => {
    useBridge.getState().addRequest("s1", {
      requestId: "r1",
      method: "ask",
      source: "main",
      params: {
        questions: [
          {
            id: "q1",
            question: "Which color?",
            options: [{ label: "Red" }, { label: "Blue" }],
          },
          {
            id: "q2",
            question: "Which shape?",
            options: [{ label: "Square" }, { label: "Circle" }],
          },
        ],
      },
    });
    render(<AskQuestionCard sessionId="s1" requestId="r1" />);
    fireEvent.click(screen.getByRole("button", { name: "Blue" }));
    fireEvent.click(screen.getByRole("button", { name: "Circle" }));
    fireEvent.click(screen.getByRole("button", { name: "Submit" }));
    await waitFor(() =>
      expect(mockedRespond).toHaveBeenCalledWith("s1", "r1", {
        cancelled: false,
        results: [
          { id: "q1", selectedOptions: ["Blue"] },
          { id: "q2", selectedOptions: ["Circle"] },
        ],
      }),
    );
  });
});
