import { describe, expect, it, vi, beforeAll, afterEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import SettingsPage from "./SettingsPage";
import {
  listModels,
  saveSettings,
  type AppSettings,
} from "@/lib/tauri";

// The page's single source of truth (the `getSettings` fixture): a full
// `AppSettings` document — dark theme, font defaults, one provider.
const baseSettings: AppSettings = {
  theme: "dark",
  paneLayout: {},
  defaultAgent: null,
  defaultTrustNewSpaces: false,
  defaultModel: null,
  providers: [
    { id: "tama", name: "Tama", baseUrl: "https://tama.wizards.town/v1", apiKey: "k" },
  ],
  font: { sizePx: 14, uiFamily: null, codeFamily: null },
};

vi.mock("../../lib/tauri", async () => {
  const actual = await vi.importActual<Record<string, unknown>>("../../lib/tauri");
  return {
    ...actual,
    // Inlined (NOT the `baseSettings` const): the factory is hoisted above
    // the const's initializer (a TDZ reference would throw at import time).
    getSettings: vi.fn().mockResolvedValue({
      theme: "dark",
      paneLayout: {},
      defaultAgent: null,
      defaultTrustNewSpaces: false,
      defaultModel: null,
      providers: [
        { id: "tama", name: "Tama", baseUrl: "https://tama.wizards.town/v1", apiKey: "k" },
      ],
      font: { sizePx: 14, uiFamily: null, codeFamily: null },
    }),
    listAgents: vi.fn().mockResolvedValue([
      { id: "pi", name: "Pi" },
      { id: "archimedes", name: "Archimedes" },
    ]),
    // The effective catalog (Task 2's `ModelDto` camelCase shape).
    listModels: vi.fn().mockResolvedValue([
      {
        id: "Qwen3.8",
        provider: "tama",
        contextWindow: 128000,
        supportsThinking: false,
        thinkingLevels: [],
      },
    ]),
    saveSettings: vi.fn().mockResolvedValue(undefined),
  };
});

// The page's selects are the Radix `ui/select` port — opening one in jsdom
// throws `hasPointerCapture is not a function` without the pointer-capture
// stubs (the SessionConfigSelect.test.tsx beforeAll, verbatim) + `matchMedia`.
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
  vi.clearAllMocks();
});

/** Wait for the page's `getSettings` load to land in state (a marker row). */
async function loaded(): Promise<void> {
  await screen.findByText("Default agent");
}

/** Navigate to a section (click its sidebar button) and wait for it. */
async function go(section: "Appearance" | "Providers"): Promise<void> {
  fireEvent.click(screen.getByRole("button", { name: section }));
  if (section === "Appearance") await screen.findByText("Theme");
  // The "Add provider" button is always in the Providers section (below the card).
  else await screen.findByRole("button", { name: "Add provider" });
}

describe("SettingsPage (the ZCode port — sections + immediate save)", () => {
  it("renders_the_three_sections_and_the_back_button", async () => {
    render(<SettingsPage onBack={vi.fn()} />);
    // The section nav + the back button.
    expect(screen.getByRole("button", { name: "General" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Appearance" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Providers" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Back" })).toBeTruthy();
    // The default section is General.
    await loaded();
    expect(screen.getByText("Default model")).toBeTruthy();

    // Appearance: the Theme + Font size rows.
    fireEvent.click(screen.getByRole("button", { name: "Appearance" }));
    expect(await screen.findByText("Theme")).toBeTruthy();
    expect(screen.getByText("Font size")).toBeTruthy();

    // Providers: the provider row (name "Tama").
    fireEvent.click(screen.getByRole("button", { name: "Providers" }));
    expect(await screen.findByDisplayValue("Tama")).toBeTruthy();
  });

  it("immediate_save_a_control_change_saves_the_complete_document", async () => {
    render(<SettingsPage onBack={vi.fn()} />);
    await loaded();
    fireEvent.click(screen.getByRole("button", { name: "Appearance" }));
    const trigger = await screen.findByRole("combobox", { name: "Theme" });
    fireEvent.click(trigger);
    fireEvent.click(screen.getByRole("option", { name: "Light" }));
    // ONE control change = ONE save of the COMPLETE document.
    await waitFor(() => expect(saveSettings).toHaveBeenCalledTimes(1));
    const saved = vi.mocked(saveSettings).mock.calls[0][0] as AppSettings;
    expect(saved.theme).toBe("light");
    // The other fields are intact (the complete document, not a patch).
    expect(saved.defaultAgent).toBeNull();
    expect(saved.defaultModel).toBeNull();
    expect(saved.providers).toHaveLength(1);
    expect(saved.font).toEqual(baseSettings.font);
  });

  it("a_provider_field_commits_on_blur_and_refreshes_discovery", async () => {
    render(<SettingsPage onBack={vi.fn()} />);
    await loaded();
    await go("Providers");
    const nameInput = await screen.findByDisplayValue("Tama");
    fireEvent.change(nameInput, { target: { value: "Tama2" } });
    // Commit on blur: save the updated provider + re-run its discovery
    // (the `listModels` force-refresh bypasses Task 2's cache).
    fireEvent.blur(nameInput);
    await waitFor(() => expect(saveSettings).toHaveBeenCalled());
    const saved = vi.mocked(saveSettings).mock.calls[0][0] as AppSettings;
    expect(saved.providers[0].name).toBe("Tama2");
    expect(listModels).toHaveBeenCalledWith("tama");
  });

  it("add_provider_appends_a_row_with_a_generated_id", async () => {
    render(<SettingsPage onBack={vi.fn()} />);
    await loaded();
    await go("Providers");
    // The existing provider row is loaded (one "Provider name" input).
    await screen.findByDisplayValue("Tama");
    fireEvent.click(screen.getByRole("button", { name: "Add provider" }));
    // The new empty row: a second "Provider name" input, empty, editable.
    const nameInputs = screen.getAllByPlaceholderText("Provider name");
    expect(nameInputs).toHaveLength(2);
    const newInput = nameInputs[1] as HTMLInputElement;
    expect(newInput.value).toBe("");
    // Type the SAME name as the existing provider + commit: the id is
    // generated ONCE at add time (`provider-2`) and a name commit must NOT
    // regenerate it (the `defaultModel` refs + discovery cache would orphan).
    // The ADD already saved the document (immediate save — call 0, name
    // `""`); the blur commit is call 1.
    fireEvent.change(newInput, { target: { value: "Tama" } });
    fireEvent.blur(newInput);
    await waitFor(() => expect(saveSettings).toHaveBeenCalledTimes(2));
    const saved = vi.mocked(saveSettings).mock.calls[1][0] as AppSettings;
    expect(saved.providers).toHaveLength(2);
    expect(saved.providers[1].name).toBe("Tama");
    expect(saved.providers[1].id).toBe("provider-2");
  });

  it("remove_provider_confirms_then_saves", async () => {
    render(<SettingsPage onBack={vi.fn()} />);
    await loaded();
    await go("Providers");
    await screen.findByDisplayValue("Tama");
    fireEvent.click(screen.getByRole("button", { name: "Remove provider" }));
    // The confirm dialog (the provider's model count in the copy).
    expect(await screen.findByRole("alertdialog")).toBeTruthy();
    expect(screen.getByText("Remove provider Tama?")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Remove" }));
    await waitFor(() =>
      expect(saveSettings).toHaveBeenCalledWith(
        expect.objectContaining({ providers: [] }),
      ),
    );
  });

  it("the_default_model_select_lists_the_catalog_and_system_default", async () => {
    render(<SettingsPage onBack={vi.fn()} />);
    await loaded();
    const trigger = screen.getByRole("combobox", { name: "Default model" });
    fireEvent.click(trigger);
    // The catalog entry (`{provider}/{id}`) + the "System default" option.
    expect(screen.getByRole("option", { name: "tama/Qwen3.8" })).toBeTruthy();
    expect(screen.getByRole("option", { name: "System default" })).toBeTruthy();
    // Choosing the model saves the composed key.
    fireEvent.click(screen.getByRole("option", { name: "tama/Qwen3.8" }));
    await waitFor(() =>
      expect(saveSettings).toHaveBeenLastCalledWith(
        expect.objectContaining({ defaultModel: "tama/Qwen3.8" }),
      ),
    );
    // Choosing "System default" saves `defaultModel: null`.
    fireEvent.click(trigger);
    fireEvent.click(screen.getByRole("option", { name: "System default" }));
    await waitFor(() =>
      expect(saveSettings).toHaveBeenLastCalledWith(
        expect.objectContaining({ defaultModel: null }),
      ),
    );
  });
});
