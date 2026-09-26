import { render } from "@testing-library/react";
import { describe, it, expect } from "vitest";
import FlipMetricValue from "./FlipMetricValue";

describe("FlipMetricValue", () => {
  it("renders the number with the flip class", () => {
    const { container } = render(<FlipMetricValue value={5} />);
    const el = container.firstElementChild as HTMLElement | null;
    expect(el).not.toBeNull();
    expect(el!.textContent).toBe("5");
    expect(el!.className).toContain("flip-in");
  });
  it("re-mounts (re-triggers the animation) when the value changes", () => {
    const { container, rerender } = render(<FlipMetricValue value={5} />);
    const before = container.firstElementChild;
    rerender(<FlipMetricValue value={7} />);
    const after = container.firstElementChild as HTMLElement | null;
    expect(after).not.toBeNull();
    expect(after!.textContent).toBe("7");
    expect(after).not.toBe(before);
  });
});
