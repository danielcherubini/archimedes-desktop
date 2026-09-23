import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import PermissionPrompt from "./PermissionPrompt";
import { usePermissions } from "../store/permissions";

// Mock the Tauri IPC layer; everything else (stores) is the real code.
vi.mock("../lib/tauri", async () => {
  const actual = await vi.importActual<Record<string, unknown>>("../lib/tauri");
  return {
    ...actual,
    respondPermission: vi.fn().mockResolvedValue(undefined),
  };
});

import { respondPermission } from "../lib/tauri";

const mockedRespond = vi.mocked(respondPermission);

const request = {
  toolCall: { toolCallId: "tc1", title: "run a tool" },
  options: [
    { optionId: "opt-1", name: "Allow", kind: "allow_once" },
    { optionId: "opt-2", name: "Deny", kind: "reject_once" },
  ],
};

describe("PermissionPrompt", () => {
  beforeEach(() => {
    usePermissions.getState().dismissSessionPrompts("s1");
    mockedRespond.mockReset();
    mockedRespond.mockResolvedValue(undefined);
  });

  it("renders the tool title and every offered option", () => {
    usePermissions.getState().addPrompt("s1", "r1", request);
    render(<PermissionPrompt sessionId="s1" requestId="r1" />);
    expect(screen.getByText("run a tool")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Allow" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Deny" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Cancel" })).toBeTruthy();
  });

  it("renders the confirmation treatment with one button per option + Cancel", () => {
    usePermissions.getState().addPrompt("s1", "r1", request);
    const { container } = render(
      <PermissionPrompt sessionId="s1" requestId="r1" />,
    );
    // The green confirmation treatment: a `bg-interaction-confirmation-surface`
    // header strip with the tool name in the confirmation foreground.
    const header = container.querySelector(
      ".bg-interaction-confirmation-surface",
    )!;
    expect(header).toBeTruthy();
    const title = header.querySelector(
      ".text-interaction-confirmation-foreground",
    )!;
    expect(title.textContent).toContain("run a tool");
    // The footer renders ONE button per fixture option (the fixture has 2)
    // + a Cancel button.
    expect(screen.getByRole("button", { name: "Allow" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Deny" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Cancel" })).toBeTruthy();
  });

  it("renders nothing when no prompt is pending for the request id", () => {
    const { container } = render(
      <PermissionPrompt sessionId="s1" requestId="missing" />,
    );
    expect(container.firstChild).toBeNull();
  });

  it("calls respondPermission with the selected option id", async () => {
    usePermissions.getState().addPrompt("s1", "r1", request);
    render(<PermissionPrompt sessionId="s1" requestId="r1" />);
    fireEvent.click(screen.getByRole("button", { name: "Allow" }));
    await waitFor(() =>
      expect(mockedRespond).toHaveBeenCalledWith("s1", "r1", {
        selected: { option_id: "opt-1" },
      }),
    );
  });

  it("sends the second option id when a different option is chosen", async () => {
    usePermissions.getState().addPrompt("s1", "r1", request);
    render(<PermissionPrompt sessionId="s1" requestId="r1" />);
    fireEvent.click(screen.getByRole("button", { name: "Deny" }));
    await waitFor(() =>
      expect(mockedRespond).toHaveBeenCalledWith("s1", "r1", {
        selected: { option_id: "opt-2" },
      }),
    );
  });

  it("cancels via the Cancel button", async () => {
    usePermissions.getState().addPrompt("s1", "r1", request);
    render(<PermissionPrompt sessionId="s1" requestId="r1" />);
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    await waitFor(() =>
      expect(mockedRespond).toHaveBeenCalledWith("s1", "r1", "cancelled"),
    );
  });

  it("removes the prompt from the store after answering", async () => {
    usePermissions.getState().addPrompt("s1", "r1", request);
    render(<PermissionPrompt sessionId="s1" requestId="r1" />);
    fireEvent.click(screen.getByRole("button", { name: "Allow" }));
    await waitFor(() =>
      expect(usePermissions.getState().prompts["s1"]).toHaveLength(0),
    );
  });
});
