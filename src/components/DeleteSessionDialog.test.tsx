import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import DeleteSessionDialog from "./DeleteSessionDialog";

/**
 * The delete-confirm modal (the `SudoConfirmModal` structure — the
 * `ui/dialog` primitive with `showCloseButton={false}` + a
 * Cancel/`destructive` Delete footer). The dialog portals to
 * `document.body`; `screen` queries the whole document.
 */
describe("DeleteSessionDialog", () => {
  it("renders the title and body copy", () => {
    render(
      <DeleteSessionDialog
        title="Fix the login bug"
        onConfirm={vi.fn()}
        onClose={vi.fn()}
      />,
    );
    expect(screen.getByText("Delete session?")).toBeTruthy();
    // The session's title (subtle, truncated).
    expect(screen.getByText("Fix the login bug")).toBeTruthy();
    expect(
      screen.getByText(
        "This permanently removes the session's transcript from Archimedes' storage. This can't be undone.",
      ),
    ).toBeTruthy();
  });

  it("Cancel calls onClose (and NOT onConfirm)", () => {
    const onClose = vi.fn();
    const onConfirm = vi.fn();
    render(
      <DeleteSessionDialog title="t" onConfirm={onConfirm} onClose={onClose} />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(onConfirm).not.toHaveBeenCalled();
  });

  it("Delete calls onConfirm", () => {
    const onConfirm = vi.fn();
    const onClose = vi.fn();
    render(
      <DeleteSessionDialog title="t" onConfirm={onConfirm} onClose={onClose} />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    expect(onConfirm).toHaveBeenCalledTimes(1);
    expect(onClose).not.toHaveBeenCalled();
  });
});
