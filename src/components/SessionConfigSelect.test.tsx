import { describe, expect, it, vi, beforeAll, afterEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import SessionConfigSelect from "./SessionConfigSelect";
import { SessionConfigOption } from "../lib/tauri";

beforeAll(() => {
  Element.prototype.scrollIntoView = vi.fn();
  Element.prototype.hasPointerCapture = vi.fn(() => false);
  Element.prototype.setPointerCapture = vi.fn();
  Element.prototype.releasePointerCapture = vi.fn();
  vi.stubGlobal("matchMedia", (q: string) => ({
    matches: false,
    media: q,
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
    addListener: vi.fn(),
    removeListener: vi.fn(),
    dispatchEvent: vi.fn(),
  }));
});

afterEach(() => {
  vi.useRealTimers();
});

const mockOption: SessionConfigOption = {
  id: "model",
  name: "Model",
  type: "select",
  currentValue: "acme/alpha",
  options: [
    { value: "acme/alpha", name: "acme/Alpha" },
    { value: "acme/beta", name: "acme/Beta" },
  ],
};

describe("SessionConfigSelect", () => {
  it("renders the current option's name as the trigger value", () => {
    render(<SessionConfigSelect option={mockOption} onSet={vi.fn()} />);
    expect(screen.getByText("acme/Alpha")).toBeTruthy();
  });

  it("selecting a different option calls onSet with that option's value", async () => {
    const onSet = vi.fn().mockResolvedValue(undefined);
    render(<SessionConfigSelect option={mockOption} onSet={onSet} />);

    // Open select
    fireEvent.click(screen.getByRole("combobox", { name: "Model" }));
    
    // Pick item
    const beta = screen.getByRole("option", { name: "acme/Beta" });
    fireEvent.click(beta);

    await waitFor(() => expect(onSet).toHaveBeenCalledWith("acme/beta"));
  });

  it("the trigger is disabled while onSet is pending", async () => {
    let resolve: (value: void | PromiseLike<void>) => void;
    const promise = new Promise<void>((r) => { resolve = r; });
    const onSet = vi.fn(() => promise);

    render(<SessionConfigSelect option={mockOption} onSet={onSet} />);
    const trigger = screen.getByRole("combobox", { name: "Model" });
    
    fireEvent.click(trigger);
    fireEvent.click(screen.getByRole("option", { name: "acme/Beta" }));

    expect(trigger.getAttribute("data-disabled")).toBe("");

    await resolve!();

    await waitFor(() => expect(trigger.getAttribute("data-disabled")).toBeNull());
  });

  it("an onSet rejection shows the error text and clears after 5s", async () => {
    const onSet = vi.fn().mockRejectedValue(new Error("boom"));
    render(<SessionConfigSelect option={mockOption} onSet={onSet} />);

    // Open select + pick item
    fireEvent.click(screen.getByRole("combobox", { name: "Model" }));
    fireEvent.click(screen.getByRole("option", { name: "acme/Beta" }));

    // Error should appear
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toBe("boom");

    // Real-timer approach: wait for the 5s timer
    const { waitForElementToBeRemoved } = await import("@testing-library/react");
    await waitForElementToBeRemoved(() => screen.queryByRole("alert"), { timeout: 8000 });
  }, 10000);
});
