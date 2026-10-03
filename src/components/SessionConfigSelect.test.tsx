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

const thinkingOption: SessionConfigOption = {
  id: "thought_level",
  name: "Thinking",
  category: "thought_level",
  type: "select",
  currentValue: "medium",
  options: [
    { value: "low", name: "Low" },
    { value: "medium", name: "Medium" },
  ],
};

describe("SessionConfigSelect", () => {
  it("renders the current value as the trigger text (the FULL `provider/id` value — the `Qwen/Qwen3.8-27B` shape)", () => {
    render(<SessionConfigSelect option={mockOption} onSet={vi.fn()} />);
    // The trigger shows the value itself (the row name IS the value —
    // the shared `modelItemsFromCatalog` derivation).
    expect(screen.getByText("acme/alpha")).toBeTruthy();
  });

  it("renders a bot icon for a model option (category 'model')", () => {
    const { container } = render(
      <SessionConfigSelect option={mockOption} onSet={vi.fn()} />,
    );
    // The icon is INSIDE the trigger (before the value — the reference UI's
    // robot-icon model placement). The model option's trigger is a BUTTON
    // (it opens the model picker dialog — NOT a Radix dropdown).
    const trigger = screen.getByRole("button", { name: "Model" });
    expect(trigger.querySelector('[data-testid="model-icon"]')).toBeTruthy();
    expect(container.querySelector('[data-testid="model-icon"]')).toBeTruthy();
  });

  it("renders the level's fill-ramp glyph for a thinking-level option (category 'thought_level')", () => {
    const { container } = render(
      <SessionConfigSelect option={thinkingOption} onSet={vi.fn()} />,
    );
    // The glyph is INSIDE the trigger (before the value — the pi-archimedes
    // `thinkingLevelIcons` ramp: the fill is the level's magnitude).
    const trigger = screen.getByRole("combobox", { name: "Thinking" });
    const icon = trigger.querySelector(
      '[data-testid="thinking-level-icon"]',
    ) as HTMLElement | null;
    expect(icon).toBeTruthy();
    // `currentValue: "medium"` → the 50% glyph, in the medium hue.
    expect(icon!.textContent).toBe("◑");
    expect(icon!.className).toContain("text-indigo-500");
    expect(container.querySelector('[data-testid="thinking-level-icon"]')).toBeTruthy();
  });

  it("the thinking glyph's fill follows the level (◔ low, ◕ high, ● xhigh/max)", () => {
    for (const [level, glyph, hue] of [
      ["low", "◔", "text-blue-500"],
      ["high", "◕", "text-purple-500"],
      ["xhigh", "●", "text-pink-500"],
      ["max", "●", "text-red-500"],
    ] as const) {
      const { unmount } = render(
        <SessionConfigSelect
          option={{ ...thinkingOption, currentValue: level }}
          onSet={vi.fn()}
        />,
      );
      const icon = screen
        .getByRole("combobox", { name: "Thinking" })
        .querySelector('[data-testid="thinking-level-icon"]') as HTMLElement;
      expect(icon.textContent, `level ${level}`).toBe(glyph);
      expect(icon.className, `level ${level}`).toContain(hue);
      unmount();
    }
  });

  it("a thinking option without a set level renders the open circle (○) dimmed", () => {
    render(
      <SessionConfigSelect
        option={{ ...thinkingOption, currentValue: "" }}
        onSet={vi.fn()}
      />,
    );
    const icon = screen
      .getByRole("combobox", { name: "Thinking" })
      .querySelector('[data-testid="thinking-level-icon"]') as HTMLElement;
    expect(icon.textContent).toBe("○");
    expect(icon.className).toContain("text-foreground-subtle");
  });

  it("does NOT render the thinking glyph for a model option", () => {
    const { container } = render(
      <SessionConfigSelect option={mockOption} onSet={vi.fn()} />,
    );
    expect(
      container.querySelector('[data-testid="thinking-level-icon"]'),
    ).toBeNull();
  });

  it("does NOT render the bot icon for a thinking-level option", () => {
    const { container } = render(
      <SessionConfigSelect option={thinkingOption} onSet={vi.fn()} />,
    );
    expect(container.querySelector('[data-testid="model-icon"]')).toBeNull();
  });

  it("selecting a different option calls onSet with that option's value", async () => {
    const onSet = vi.fn().mockResolvedValue(undefined);
    render(<SessionConfigSelect option={mockOption} onSet={onSet} />);

    // Open the model picker dialog (the model option is a dialog trigger,
    // NOT a Radix dropdown — the catalog is too long for a menu).
    fireEvent.click(screen.getByRole("button", { name: "Model" }));

    // Pick the row (the row name is the FULL value — the shared
    // derivation).
    const beta = screen.getByText("acme/beta");
    fireEvent.click(beta);

    await waitFor(() => expect(onSet).toHaveBeenCalledWith("model", "acme/beta"));
  });

  it("the trigger is disabled while onSet is pending", async () => {
    let resolve: (value: void | PromiseLike<void>) => void;
    const promise = new Promise<void>((r) => { resolve = r; });
    const onSet = vi.fn(() => promise);

    render(<SessionConfigSelect option={mockOption} onSet={onSet} />);
    const trigger = screen.getByRole("button", { name: "Model" });

    fireEvent.click(trigger);
    fireEvent.click(screen.getByText("acme/beta"));

    expect(trigger.hasAttribute("disabled")).toBe(true);

    await resolve!();

    await waitFor(() => expect(trigger.hasAttribute("disabled")).toBe(false));
  });

  it("the disabled prop disables the trigger even with a real option (a stored session's selector is populated but read-only)", () => {
    render(
      <SessionConfigSelect
        option={mockOption}
        onSet={vi.fn().mockResolvedValue(undefined)}
        disabled
      />,
    );
    expect(screen.getByRole("button", { name: "Model" }).hasAttribute("disabled")).toBe(true);
  });

  it("the disabled prop disables the thinking trigger too", () => {
    render(
      <SessionConfigSelect
        option={thinkingOption}
        onSet={vi.fn().mockResolvedValue(undefined)}
        kind="thinking"
        disabled
      />,
    );
    expect(screen.getByRole("combobox").hasAttribute("disabled")).toBe(true);
  });

  it("an onSet rejection shows the error text and clears after 5s", async () => {
    const onSet = vi.fn().mockRejectedValue(new Error("boom"));
    render(<SessionConfigSelect option={mockOption} onSet={onSet} />);

    // Open the dialog + pick the row.
    fireEvent.click(screen.getByRole("button", { name: "Model" }));
    fireEvent.click(screen.getByText("acme/beta"));

    // Error should appear
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toBe("boom");

    // Real-timer approach: wait for the 5s timer
    const { waitForElementToBeRemoved } = await import("@testing-library/react");
    await waitForElementToBeRemoved(() => screen.queryByRole("alert"), { timeout: 8000 });
  }, 10000);

  it("a null model option renders a disabled stub (the control's identity, no data — the session's config is dropped on close)", () => {
    render(
      <SessionConfigSelect kind="model" option={null} onSet={vi.fn()} />,
    );
    // The trigger is DISABLED (there is no data to pick from — the config
    // re-emits on resume) but the identity is kept (the bot icon + the
    // muted placeholder).
    const trigger = screen.getByRole("button", { name: "Model" });
    expect(trigger.hasAttribute("disabled")).toBe(true);
    expect(trigger.querySelector('[data-testid="model-icon"]')).toBeTruthy();
    expect(screen.getByText("Model")).toBeTruthy();
  });

  it("a null thinking option renders a disabled stub (the open-circle glyph, no data)", () => {
    render(
      <SessionConfigSelect kind="thinking" option={null} onSet={vi.fn()} />,
    );
    const trigger = screen.getByRole("button", { name: "Thinking" });
    expect(trigger.hasAttribute("disabled")).toBe(true);
    const icon = trigger.querySelector(
      '[data-testid="thinking-level-icon"]',
    ) as HTMLElement;
    expect(icon.textContent).toBe("○");
  });

  it("the model picker dialog flattens grouped options (the value prefix is the provider cue)", async () => {
    const groupedOption: SessionConfigOption = {
      id: "model",
      name: "Model",
      type: "select",
      currentValue: "anthropic/claude",
      options: [
        {
          name: "OpenAI",
          options: [
            { value: "openai/gpt-4", name: "GPT-4" },
            { value: "openai/gpt-4-turbo", name: "GPT-4 Turbo" },
          ],
        },
        { value: "anthropic/claude", name: "Claude" },
      ],
    };
    const onSet = vi.fn().mockResolvedValue(undefined);
    render(<SessionConfigSelect option={groupedOption} onSet={onSet} />);

    // Open the dialog: the rows are FLAT (the grouped entries flattened
    // to their values) — the row name is the FULL `provider/id` value,
    // and the provider cue is the value's PREFIX (no configured provider
    // here → the raw key, inline in muted grey).
    fireEvent.click(screen.getByRole("button", { name: "Model" }));
    expect(screen.getByText("openai/gpt-4")).toBeTruthy();
    expect(screen.getByText("openai/gpt-4-turbo")).toBeTruthy();
    // `anthropic/claude` appears TWICE — the trigger's current value AND
    // the dialog's row.
    expect(screen.getAllByText("anthropic/claude")).toHaveLength(2);
    // The provider cue (both `openai` rows — the ` (openai)` spans).
    expect(screen.getAllByText(/\(openai\)/)).toHaveLength(2);

    // Select a row: the value is sent.
    fireEvent.click(screen.getByText("openai/gpt-4"));
    await waitFor(() => expect(onSet).toHaveBeenCalledWith("model", "openai/gpt-4"));
  });
});
