import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import WindowControls from "./WindowControls";

// The component calls getCurrentWindow() on every render, so the mock must
// return a STABLE object (hoisted) that we can clear between tests.
const mockWindow = vi.hoisted(() => ({
  minimize: vi.fn(),
  toggleMaximize: vi.fn(),
  close: vi.fn(),
}));

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => mockWindow,
}));

beforeEach(() => {
  vi.clearAllMocks();
});

describe("WindowControls", () => {
  it("renders minimize, maximize, and close controls", () => {
    render(<WindowControls />);
    expect(screen.getByTitle("Minimize")).toBeTruthy();
    expect(screen.getByTitle("Maximize / Restore")).toBeTruthy();
    expect(screen.getByTitle("Close")).toBeTruthy();
  });

  it("calls window.minimize() on the minimize button", () => {
    render(<WindowControls />);
    fireEvent.click(screen.getByTitle("Minimize"));
    expect(mockWindow.minimize).toHaveBeenCalledTimes(1);
  });

  it("calls window.toggleMaximize() on the maximize button", () => {
    render(<WindowControls />);
    fireEvent.click(screen.getByTitle("Maximize / Restore"));
    expect(mockWindow.toggleMaximize).toHaveBeenCalledTimes(1);
  });

  it("calls window.close() on the close button", () => {
    render(<WindowControls />);
    fireEvent.click(screen.getByTitle("Close"));
    expect(mockWindow.close).toHaveBeenCalledTimes(1);
  });
});
