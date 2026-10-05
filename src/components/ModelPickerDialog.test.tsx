import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import {
  modelItemsFromCatalog,
  type ModelPickerItem,
} from "./ModelPickerDialog";
import ModelPickerDialog from "./ModelPickerDialog";

// The dialog is PROP-DRIVEN (like `SkillsDialog` — the data comes from the
// caller: the composer's config option or the settings' model catalog).
function item(overrides: Partial<ModelPickerItem> = {}): ModelPickerItem {
  return {
    value: "tama/Qwen3.8",
    name: "Qwen3.8",
    provider: "tama",
    ...overrides,
  };
}

function renderDialog(
  items: ModelPickerItem[],
  onSelect: (value: string) => void = vi.fn(),
): void {
  render(
    <ModelPickerDialog
      open
      onOpenChange={vi.fn()}
      items={items}
      onSelect={onSelect}
    />,
  );
}

describe("modelItemsFromCatalog (the ONE model-row derivation)", () => {
  const providers = [{ id: "wizards", name: "Wizards" }];

  it("the row NAME is the BARE model id — the provider prefix is stripped (it would otherwise be printed twice: once in the name, once in the cue)", () => {
    expect(
      modelItemsFromCatalog(["wizards/Qwen/Qwen3.8-27B"], providers),
    ).toEqual([
      {
        value: "wizards/Qwen/Qwen3.8-27B",
        name: "Qwen/Qwen3.8-27B",
        provider: "Wizards",
      },
    ]);
    // The VALUE still carries the full composed key (the selection is
    // unchanged — only the DISPLAY drops the prefix).
    expect(
      modelItemsFromCatalog(["wizards/Qwen/Qwen3.8-27B"], providers)[0].value,
    ).toBe("wizards/Qwen/Qwen3.8-27B");
  });

  it("splits on the FIRST `/` (model ids may contain `/`)", () => {
    expect(modelItemsFromCatalog(["wizards/deepseek/deepseek-v4-flash"], providers)[0].name).toBe(
      "deepseek/deepseek-v4-flash",
    );
  });

  it("an unconfigured provider key is the cue verbatim; a value with no `/` has no cue and keeps its name", () => {
    expect(modelItemsFromCatalog(["tama/m1"], providers)[0]).toEqual({
      value: "tama/m1",
      name: "m1",
      provider: "tama",
    });
    expect(modelItemsFromCatalog(["bare-model"], providers)[0]).toEqual({
      value: "bare-model",
      name: "bare-model",
      provider: undefined,
    });
  });
});

describe("ModelPickerDialog", () => {
  it("lists every model, ALPHABETICALLY (case-insensitive — the input order is ignored)", async () => {
    renderDialog([
      item({ value: "z/zebra", name: "Zebra", provider: "z" }),
      item({ value: "a/alpha", name: "alpha", provider: "a" }),
      item({ value: "m/mid", name: "Mid", provider: "m" }),
    ]);
    // `alpha` < `Mid` < `Zebra` (case-insensitive), regardless of the
    // input order. (The rows render `name` + the inline provider cue —
    // match on the name PREFIX.)
    const names = screen
      .getAllByText(/^(alpha|Mid|Zebra)/)
      .map((el) => el.textContent);
    expect(names).toEqual(["alpha (a)", "Mid (m)", "Zebra (z)"]);
  });

  it("shows the provider INLINE, after the name, in muted grey (the `Qwen/Qwen3.8-27B (Tama)` shape)", async () => {
    renderDialog([
      item({ value: "Qwen/Qwen3.8-27B", name: "Qwen/Qwen3.8-27B", provider: "Tama" }),
    ]);
    // The row renders `name` + ` (provider)` — the provider part in the
    // muted-grey token.
    const row = await screen.findByText(/Qwen\/Qwen3\.8-27B/);
    expect(row.textContent).toBe("Qwen/Qwen3.8-27B (Tama)");
    const providerPart = row.querySelector("span") as HTMLElement;
    expect(providerPart.textContent).toBe(" (Tama)");
    expect(providerPart.className).toContain("text-foreground-subtlest");
  });

  it("a row without a provider shows ONLY the name (no empty parens)", async () => {
    renderDialog([item({ provider: undefined })]);
    const row = await screen.findByText("Qwen3.8");
    expect(row.textContent).toBe("Qwen3.8");
  });

  it("the fuzzy search filters by subsequence (name OR provider OR the full value)", async () => {
    renderDialog([
      item({ value: "tama/Qwen3.8", name: "Qwen3.8", provider: "tama" }),
      item({ value: "openai/gpt-4", name: "GPT-4", provider: "openai" }),
      item({ value: "anthropic/claude", name: "Claude", provider: "anthropic" }),
    ]);
    // `qwen` is a subsequence of the name only.
    fireEvent.change(screen.getByPlaceholderText("Search models…"), {
      target: { value: "qwen" },
    });
    expect(screen.getByText("Qwen3.8")).toBeTruthy();
    expect(screen.queryByText("GPT-4")).toBeNull();
    expect(screen.queryByText("Claude")).toBeNull();
    // A provider-only subsequence matches too (`oa` ⊂ `openai`).
    fireEvent.change(screen.getByPlaceholderText("Search models…"), {
      target: { value: "oa" },
    });
    expect(screen.getByText("GPT-4")).toBeTruthy();
    expect(screen.queryByText("Qwen3.8")).toBeNull();
  });

  it("a query spanning provider AND model matches the composed VALUE (the row text no longer contains the joined shape)", async () => {
    renderDialog([item({ value: "wizards/Qwen3.8", name: "Qwen3.8", provider: "Wizards" })]);
    fireEvent.change(screen.getByPlaceholderText("Search models…"), {
      target: { value: "wizards/q" },
    });
    expect(screen.getByText("Qwen3.8")).toBeTruthy();
  });

  it("clicking a row selects its value and closes the dialog", async () => {
    const onSelect = vi.fn();
    const onOpenChange = vi.fn();
    render(
      <ModelPickerDialog
        open
        onOpenChange={onOpenChange}
        items={[
          item({ value: "tama/Qwen3.8", name: "Qwen3.8", provider: "tama" }),
          item({ value: "openai/gpt-4", name: "GPT-4", provider: "openai" }),
        ]}
        onSelect={onSelect}
      />,
    );
    fireEvent.click(await screen.findByText("GPT-4"));
    expect(onSelect).toHaveBeenCalledWith("openai/gpt-4");
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });

  it("a disabled row is NOT selectable (the stale-override pattern)", async () => {
    const onSelect = vi.fn();
    const onOpenChange = vi.fn();
    render(
      <ModelPickerDialog
        open
        onOpenChange={onOpenChange}
        items={[
          item({ value: "gone/m1", name: "gone/m1", disabled: true }),
          item({ value: "tama/Qwen3.8", name: "Qwen3.8", provider: "tama" }),
        ]}
        onSelect={onSelect}
      />,
    );
    const stale = (await screen.findByText("gone/m1")).closest(
      "[role=button]",
    ) as HTMLElement;
    expect(stale.getAttribute("aria-disabled")).toBe("true");
    fireEvent.click(stale);
    expect(onSelect).not.toHaveBeenCalled();
    expect(onOpenChange).not.toHaveBeenCalled();
    // The enabled row still works.
    fireEvent.click(screen.getByText("Qwen3.8"));
    expect(onSelect).toHaveBeenCalledWith("tama/Qwen3.8");
  });

  it("renders the empty states", async () => {
    render(
      <ModelPickerDialog open onOpenChange={vi.fn()} items={[]} onSelect={vi.fn()} />,
    );
    expect(await screen.findByText("No models found.")).toBeTruthy();
  });

  it("the no-match state names the query", async () => {
    renderDialog([item()]);
    fireEvent.change(screen.getByPlaceholderText("Search models…"), {
      target: { value: "zzz" },
    });
    expect(screen.getByText('No models match "zzz".')).toBeTruthy();
  });

  it("is bounded like the skills dialog (max-w-3xl + the min-w-0 chain + pr-2)", async () => {
    renderDialog([item()]);
    const title = await screen.findByText("Models");
    const content = title.closest("[data-slot=dialog-content]");
    expect(content?.className).toContain("max-w-3xl");
    const desc = screen.getByText("Qwen3.8");
    expect(desc.className).toContain("min-w-0");
    const row = desc.closest("[role=button]");
    expect(row?.className).toContain("min-w-0");
    const scroll = row?.parentElement;
    expect(scroll?.className).toContain("min-w-0");
    expect(scroll?.className).toContain("overflow-y-auto");
    expect(scroll?.className).toContain("pr-2");
    const wrapper = scroll?.parentElement;
    expect(wrapper?.className).toContain("min-w-0");
  });
});
