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
  it("renders the current model's BARE id + the provider as a muted suffix (never the redundant `provider/id` prefix)", () => {
    render(<SessionConfigSelect option={mockOption} onSet={vi.fn()} />);
    // The trigger shows the shared `modelItemsFromCatalog` derivation — the
    // value's provider PREFIX is dropped (the provider is the suffix), so
    // the composer's trigger is not wasted on `acme/…`.
    const trigger = screen.getByRole("button", { name: "Model" });
    expect(trigger.textContent).toBe("alpha · acme");
    expect(trigger.textContent).not.toContain("acme/alpha");
  });

  it("the composer's model trigger sizes to its text up to a WIDE cap (a long model id is not chopped to `Qwen/Qwen3.8-27…`)", () => {
    render(<SessionConfigSelect option={mockOption} onSet={vi.fn()} />);
    const trigger = screen.getByRole("button", { name: "Model" });
    // CONTENT-SIZED (auto-resizing): no fixed `w-*`, so a short name gets a
    // short trigger and `truncate` + the cap only kick in past the cap.
    const fixedWidth = trigger.className
      .split(/\s+/)
      .filter((cls) => /(^|:)w-/.test(cls));
    expect(fixedWidth).toEqual([]);
    // The cap is wide enough for `<model id> · <provider>` (the old
    // `max-w-48` chopped `Qwen/Qwen3.8-27B (Tama)` to an unreadable stub).
    expect(trigger.className).toContain("max-w-72");
    expect(trigger.className).not.toContain("max-w-48");
    // It also GIVES width back when the row is tight (the Button base's
    // `shrink-0` would otherwise push the send button out of the composer).
    expect(trigger.className).toContain("min-w-0");
    expect(trigger.className).toContain("shrink");
    // Its SEAM edge (the chevron side, facing the thinking trigger) drops to
    // `pr-1`: the trigger's text-side `px-2` inset is right, but 8px on both
    // sides of the pair's 4px gap reads as one dead 22px band.
    const tokens = trigger.className.split(/\s+/);
    expect(tokens).toContain("pr-1");
    expect(tokens).toContain("px-2");
  });

  it("the thinking trigger's SEAM edge (the glyph side, facing the model trigger) is tight too", () => {
    render(
      <SessionConfigSelect option={thinkingOption} onSet={vi.fn()} kind="thinking" />,
    );
    const tokens = screen
      .getByRole("combobox", { name: "Thinking" })
      .className.split(/\s+/);
    expect(tokens).toContain("pl-1");
    expect(tokens).not.toContain("pl-2");
  });

  it("the null-option stubs keep the same width contract as the live triggers", () => {
    const { unmount } = render(
      <SessionConfigSelect kind="model" option={null} onSet={vi.fn()} />,
    );
    const model = screen.getByRole("button", { name: "Model" });
    expect(model.className).toContain("max-w-72");
    expect(model.className).not.toContain("max-w-48");
    expect(model.className.split(/\s+/)).toContain("pr-1");
    unmount();

    render(
      <SessionConfigSelect kind="thinking" option={null} onSet={vi.fn()} />,
    );
    const thinking = screen.getByRole("button", { name: "Thinking" });
    expect(thinking.className).toContain("max-w-72");
    expect(thinking.className).not.toContain("max-w-48");
    expect(thinking.className.split(/\s+/)).toContain("pl-1");
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

    // Pick the row (the row name is the BARE id — the shared
    // `modelItemsFromCatalog` derivation; the VALUE sent is still the
    // full composed key).
    const beta = screen.getByText("beta");
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
    fireEvent.click(screen.getByText("beta"));

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

    // Open the dialog + pick the row (the row name is the bare id).
    fireEvent.click(screen.getByRole("button", { name: "Model" }));
    fireEvent.click(screen.getByText("beta"));

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
    // to their values) — the row name is the value MINUS the provider
    // prefix, and the provider cue is that prefix (no configured provider
    // here → the raw key, inline in muted grey).
    fireEvent.click(screen.getByRole("button", { name: "Model" }));
    expect(screen.getByText("gpt-4")).toBeTruthy();
    expect(screen.getByText("gpt-4-turbo")).toBeTruthy();
    // The trigger's current value and the dialog's `claude` row are the two
    // places `claude` now appears (the composed key is only the VALUE).
    expect(screen.getAllByText(/^claude/)).toHaveLength(2);
    // The provider cue (`openai` on both `openai` rows + the trigger's
    // `anthropic` suffix).
    expect(screen.getAllByText(/\(openai\)/)).toHaveLength(2);
    expect(screen.getByText(/·\s*anthropic/)).toBeTruthy();

    // Select a row: the full composed VALUE is sent.
    fireEvent.click(screen.getByText("gpt-4"));
    await waitFor(() => expect(onSet).toHaveBeenCalledWith("model", "openai/gpt-4"));
  });
});
