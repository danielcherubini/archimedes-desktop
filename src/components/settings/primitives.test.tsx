import { describe, expect, it, vi, beforeAll } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { SettingsIcon } from "lucide-react";
import {
  SettingsGroupCard,
  SettingsRow,
  SettingsSidebarButton,
  closedSelectValue,
  openSelectItems,
} from "./primitives";

// The primitives wrap Radix primitives (the `SettingsSidebarButton` tooltip) —
// jsdom lacks the pointer-capture API + `matchMedia` (same stubs as
// SessionConfigSelect.test.tsx).
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

describe("the select value helpers (a stored value outside the option list)", () => {
  // The settings document is user-editable and the backend round-trips these
  // fields unvalidated, so any string can reach a picker. Radix renders NO
  // label for a value with no matching item, which blanks the trigger.
  const PALETTE = ["zai", "dracula"] as const;

  it("closedSelectValue keeps an offered value and falls back otherwise", () => {
    expect(closedSelectValue("dracula", PALETTE, "zai")).toBe("dracula");
    expect(closedSelectValue("solarized", PALETTE, "zai")).toBe("zai");
    // Absent AND blank both mean "the default" (the blank `palette` is not a
    // palette, and Radix rejects an empty-valued item anyway).
    expect(closedSelectValue(null, PALETTE, "zai")).toBe("zai");
    expect(closedSelectValue(undefined, PALETTE, "zai")).toBe("zai");
    expect(closedSelectValue("", PALETTE, "zai")).toBe("zai");
    // Case-sensitive: `"Dracula"` is not `"dracula"` — the app would render
    // Zai for it, so the trigger must not claim otherwise.
    expect(closedSelectValue("Dracula", PALETTE, "zai")).toBe("zai");
  });

  it("openSelectItems surfaces an off-list value as its own option", () => {
    const options = [
      { value: "default", label: "Default (Noto Sans)" },
      { value: '"Fira Sans"', label: "Fira Sans" },
    ];
    // An offered value: the list is untouched and no duplicate is added.
    expect(openSelectItems('"Fira Sans"', options)).toEqual({
      value: '"Fira Sans"',
      items: options,
    });
    // An off-list value: selected, and appended as its own item so the trigger
    // has a label to render (the font family really IS applied).
    expect(openSelectItems("Georgia", options)).toEqual({
      value: "Georgia",
      items: [...options, { value: "Georgia", label: "Georgia" }],
    });
    // Absent / blank → the first option (the `"default"` sentinel) and the
    // ORIGINAL list — no empty-valued item.
    expect(openSelectItems(null, options)).toEqual({
      value: "default",
      items: options,
    });
    expect(openSelectItems("", options)).toEqual({ value: "default", items: options });
  });
});

describe("settings primitives (the ZCode port)", () => {
  it("SettingsRow_renders_label_description_and_control", () => {
    const { container } = render(
      <SettingsGroupCard>
        <SettingsRow
          label="Theme"
          description="The app theme"
          control={
            <button type="button" aria-label="theme-control">
              C
            </button>
          }
        />
      </SettingsGroupCard>,
    );
    expect(screen.getByText("Theme")).toBeTruthy();
    expect(screen.getByText("The app theme")).toBeTruthy();
    expect(screen.getByRole("button", { name: "theme-control" })).toBeTruthy();
    // The row is the `border-t` divider row (`first:border-t-0` for the top row).
    const row = container.querySelector(".border-t") as HTMLElement;
    expect(row).toBeTruthy();
    expect(row.className).toContain("first:border-t-0");
  });

  it("SettingsSidebarButton_active_state", () => {
    const onClick = vi.fn();
    render(
      <SettingsSidebarButton
        icon={SettingsIcon}
        label="General"
        active
        onClick={onClick}
      />,
    );
    const button = screen.getByRole("button", { name: "General" });
    // Active = the surface-hover fill; inactive would be `hover:bg-surface-hover`.
    expect(button.className).toContain("bg-surface-hover");
    expect(button.className).not.toContain("hover:bg-surface-hover");
    fireEvent.click(button);
    expect(onClick).toHaveBeenCalledTimes(1);
  });

  it("SettingsGroupCard_renders_children_in_a_card", () => {    const { container } = render(
      <SettingsGroupCard>
        <span>child-content</span>
      </SettingsGroupCard>,
    );
    expect(screen.getByText("child-content")).toBeTruthy();
    const card = container.querySelector('[data-slot="card"]') as HTMLElement;
    expect(card).toBeTruthy();
    expect(card.className).toContain("rounded-xl");
    expect(card.className).toContain("bg-card");
  });
});
