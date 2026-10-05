import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { ModelPickerItem } from "./ModelPickerDialog";
import ModelPicker from "./ModelPicker";

const items: ModelPickerItem[] = [
  { value: "tama/Qwen3.8", name: "Qwen3.8", provider: "Tama" },
  { value: "", name: "System default" },
];

describe("ModelPicker (the SHARED trigger + dialog — every model picker uses it)", () => {
  it("the trigger shows the selected item's NAME + the provider as a muted suffix, and opens the dialog", () => {
    render(
      <ModelPicker
        label="Default model"
        value="tama/Qwen3.8"
        items={items}
        onSelect={vi.fn()}
      />,
    );
    const trigger = screen.getByRole("button", { name: "Default model" });
    // NOT the raw `provider/id` value (the prefix is redundant next to
    // the provider name) — `Qwen3.8 · Tama`.
    expect(trigger.textContent).toContain("Qwen3.8");
    expect(trigger.textContent).not.toContain("tama/Qwen3.8");
    // The provider suffix is the muted token, prefixed by the separator.
    const suffix = trigger.querySelector("span > span") as HTMLElement;
    expect(suffix.textContent).toBe(" · Tama");
    expect(suffix.className).toContain("text-foreground-subtlest");
    // Closed by default.
    expect(screen.queryByText("System default")).toBeNull();
    fireEvent.click(trigger);
    expect(screen.getByText("System default")).toBeTruthy();
  });

  it("a selected value that matches no item falls back to the raw value (a value outside the derived list)", () => {
    render(
      <ModelPicker
        label="Default model"
        value="other/x"
        items={items}
        onSelect={vi.fn()}
      />,
    );
    const trigger = screen.getByRole("button", { name: "Default model" });
    expect(trigger.textContent).toContain("other/x");
    // No provider suffix.
    expect(trigger.querySelector("span > span")).toBeNull();
  });

  it("an unset value shows the muted placeholder (the SelectValue placeholder pattern)", () => {
    render(
      <ModelPicker
        label="Default model"
        value=""
        placeholder="System default"
        items={items}
        onSelect={vi.fn()}
      />,
    );
    const trigger = screen.getByRole("button", { name: "Default model" });
    expect(trigger.textContent).toContain("System default");
    // The placeholder is the muted token.
    const span = trigger.querySelector("span") as HTMLElement;
    expect(span.className).toContain("text-foreground-subtlest");
  });

  it("clicking a row selects its value and closes the dialog", async () => {
    const onSelect = vi.fn();
    render(
      <ModelPicker
        label="Default model"
        value=""
        placeholder="—"
        items={items}
        onSelect={onSelect}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Default model" }));
    // "System default" is unique here (the placeholder is "—").
    fireEvent.click(await screen.findByText("System default"));
    expect(onSelect).toHaveBeenCalledWith("");
    // The dialog closed (the row is gone; the trigger shows the placeholder).
    expect(screen.queryByText("System default")).toBeNull();
  });

  it("a disabled trigger does not open the dialog (the composer's pending state)", () => {
    render(
      <ModelPicker
        label="Model"
        value="tama/Qwen3.8"
        disabled
        items={items}
        onSelect={vi.fn()}
      />,
    );
    const trigger = screen.getByRole("button", { name: "Model" });
    expect(trigger.hasAttribute("disabled")).toBe(true);
    fireEvent.click(trigger);
    expect(screen.queryByText("System default")).toBeNull();
  });

  it("renders the leading icon slot (the composer's bot icon)", () => {
    render(
      <ModelPicker
        label="Model"
        value="tama/Qwen3.8"
        icon={<span data-testid="picker-icon" />}
        items={items}
        onSelect={vi.fn()}
      />,
    );
    const trigger = screen.getByRole("button", { name: "Model" });
    expect(trigger.querySelector("[data-testid=picker-icon]")).toBeTruthy();
  });

  it("the ghost variant (the composer) and the outline variant (the settings) trigger looks", () => {
    const { rerender } = render(
      <ModelPicker
        label="Model"
        value="x"
        variant="ghost"
        items={items}
        onSelect={vi.fn()}
      />,
    );
    const ghost = screen.getByRole("button", { name: "Model" });
    expect(ghost.getAttribute("data-variant")).toBe("ghost");
    // The ghost trigger is borderless (no `border-input-border`).
    expect(ghost.className).not.toContain("border-input-border");
    rerender(
      <ModelPicker
        label="Model"
        value="x"
        variant="outline"
        items={items}
        onSelect={vi.fn()}
      />,
    );
    const outline = screen.getByRole("button", { name: "Model" });
    expect(outline.getAttribute("data-variant")).toBe("outline");
    // The outline trigger mirrors the SelectTrigger's `input` variant
    // (a bordered input-look box).
    expect(outline.className).toContain("border-input-border");
  });
});
