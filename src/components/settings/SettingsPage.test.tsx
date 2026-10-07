import { describe, expect, it, vi, beforeAll, afterEach } from "vitest";
import {
  render,
  screen,
  fireEvent,
  waitFor,
  cleanup,
  within,
} from "@testing-library/react";
import SettingsPage from "./SettingsPage";
import {
  authMcpServer,
  getSettings,
  listAgentDefinitions,
  listModels,
  saveSettings,
  shellSandboxAvailable,
  testMcpServer,
  type AppSettings,
} from "@/lib/tauri";
import { brailleLoaderVariants } from "@/lib/braille-loader";
import { getAppInfo } from "@/lib/version";
import { humanizeVariant } from "./primitives";

// The page's single source of truth (the `getSettings` fixture): a full
// `AppSettings` document — dark theme, font defaults, one provider, one
// MCP server.
const baseSettings: AppSettings = {
  theme: "dark",
  palette: null,
  paneLayout: {},
  defaultTrustNewSpaces: false,
  defaultModel: null,
  defaultThinkingLevel: null,
  enabledTools: [],
  providers: [
    {
      id: "tama",
      name: "Tama",
      baseUrl: "https://tama.wizards.town/v1",
      apiKey: "k",
      api: "openai-completions",
      keyUrl: null,
    },
  ],
  mcpServers: {
    tama: { url: "https://tama/mcp" },
  },
  font: { sizePx: 14, uiFamily: null, codeFamily: null },
  defaultThinkingLevels: {},
  subagentModels: {},
  spinnerStyle: null,
  filePolicy: { reads: "allow", writes: "allow", shell: "allow" },
};

vi.mock("@/lib/version", () => ({
  // The OS the app runs on (the Shell `Sandboxed` tier is Linux-only). Linux
  // by default so the option is offered; the non-Linux test overrides it
  // with `mockResolvedValueOnce`.
  getAppInfo: vi
    .fn()
    .mockResolvedValue({ version: "0.1.0", platform: "linux" }),
}));

vi.mock("../../lib/tauri", async () => {
  const actual =
    await vi.importActual<Record<string, unknown>>("../../lib/tauri");
  return {
    ...actual,
    // Inlined (NOT the `baseSettings` const): the factory is hoisted above
    // the const's initializer (a TDZ reference would throw at import time).
    getSettings: vi.fn().mockResolvedValue({
      theme: "dark",
      palette: null,
      paneLayout: {},
      defaultTrustNewSpaces: false,
      defaultModel: null,
      defaultThinkingLevel: null,
      enabledTools: [],
      providers: [
        {
          id: "tama",
          name: "Tama",
          baseUrl: "https://tama.wizards.town/v1",
          apiKey: "k",
          api: "openai-completions",
          keyUrl: null,
        },
      ],
      mcpServers: {
        tama: { url: "https://tama/mcp" },
      },
      font: { sizePx: 14, uiFamily: null, codeFamily: null },
      defaultThinkingLevels: {},
      subagentModels: {},
      spinnerStyle: null,
      filePolicy: { reads: "allow", writes: "allow", shell: "allow" },
    }),
    // The discovered agent definitions (ADR 0023 — the Subagents section's
    // data source): empty by default (the tests override per case).
    listAgentDefinitions: vi.fn().mockResolvedValue([]),
    // (ADR 0030) Whether THIS kernel can confine a shell (Landlock).
    // AVAILABLE by default so the existing tests keep their meaning (the
    // tier is offered); the grey-out test overrides with
    // `mockResolvedValueOnce(false)`.
    shellSandboxAvailable: vi.fn().mockResolvedValue(true),
    // The effective catalog (Task 2's `ModelDto` camelCase shape): two
    // models advertising OVERLAPPING thinking levels (the union is
    // deduped in FIRST-SEEN order — `medium` / `high` appear in both;
    // the first model's levels are deliberately NOT alphabetical so the
    // order assertion discriminates first-seen from lexicographic).
    listModels: vi.fn().mockResolvedValue([
      {
        id: "Qwen3.8",
        provider: "tama",
        contextWindow: 128000,
        supportsThinking: true,
        thinkingLevels: ["high", "medium", "low"],
      },
      {
        id: "GPT-5",
        provider: "openai",
        contextWindow: 400000,
        supportsThinking: true,
        thinkingLevels: ["medium", "high", "xhigh"],
      },
    ]),
    // The known-providers catalog (ADR 0024 — the Providers section's
    // picker's data source): the 20 ZCode builtin templates (the camelCase
    // `KnownProviderDto` wire shape). Inlined (the factory hoisting rule —
    // see the `getSettings` note above).
    listKnownProviders: vi.fn().mockResolvedValue([
      {
        id: "zai",
        name: "Z.ai",
        baseUrl: "https://api.z.ai/api/anthropic",
        api: "anthropic-messages",
        keyUrl: "https://z.ai/manage-apikey/apikey-list",
      },
      {
        id: "zai-api",
        name: "Z.ai API",
        baseUrl: "https://api.z.ai/api/paas/v4",
        api: "openai-completions",
        keyUrl: "https://z.ai/manage-apikey/apikey-list",
      },
      {
        id: "bigmodel",
        name: "BigModel",
        baseUrl: "https://open.bigmodel.cn/api/anthropic",
        api: "anthropic-messages",
        keyUrl: "https://bigmodel.cn/coding-plan/personal/overview",
      },
      {
        id: "bigmodel-api",
        name: "BigModel API",
        baseUrl: "https://open.bigmodel.cn/api/paas/v4",
        api: "openai-completions",
        keyUrl: "https://bigmodel.cn/usercenter/proj-mgmt/apikeys",
      },
      {
        id: "kimi",
        name: "Kimi",
        baseUrl: "https://api.moonshot.cn/anthropic",
        api: "anthropic-messages",
        keyUrl: "https://platform.kimi.com/console/api-keys",
      },
      {
        id: "minimax",
        name: "MiniMax",
        baseUrl: "https://api.minimaxi.com/anthropic",
        api: "anthropic-messages",
        keyUrl: "https://platform.minimaxi.com/console/access?tab=api-keys",
      },
      {
        id: "deepseek",
        name: "DeepSeek",
        baseUrl: "https://api.deepseek.com/anthropic",
        api: "anthropic-messages",
        keyUrl: "https://platform.deepseek.com/api_keys",
      },
      {
        id: "alibaba-cn",
        name: "Alibaba Cloud (China)",
        baseUrl: "https://dashscope.aliyuncs.com/apps/anthropic",
        api: "anthropic-messages",
        keyUrl: "https://bailian.console.aliyun.com/cn-beijing?tab=model",
      },
      {
        id: "alibaba-intl",
        name: "Alibaba Cloud (Global)",
        baseUrl: "https://dashscope-intl.aliyuncs.com/compatible-mode/v1",
        api: "openai-completions",
        keyUrl:
          "https://modelstudio.console.aliyun.com/ap-southeast-1?tab=dashboard",
      },
      {
        id: "xiaomi-mimo",
        name: "Xiaomi MiMo",
        baseUrl: "https://api.xiaomimimo.com/anthropic",
        api: "anthropic-messages",
        keyUrl: "https://platform.xiaomimimo.com/",
      },
      {
        id: "openai",
        name: "OpenAI",
        baseUrl: "https://api.openai.com/v1",
        api: "openai-responses",
        keyUrl: "https://platform.openai.com/api-keys",
      },
      {
        id: "anthropic",
        name: "Anthropic",
        baseUrl: "https://api.anthropic.com/v1",
        api: "anthropic-messages",
        keyUrl: "https://console.anthropic.com/settings/keys",
      },
      {
        id: "xai",
        name: "xAI",
        baseUrl: "https://api.x.ai/v1",
        api: "openai-responses",
        keyUrl: "https://console.x.ai",
      },
      {
        id: "openrouter",
        name: "OpenRouter",
        baseUrl: "https://openrouter.ai/api",
        api: "anthropic-messages",
        keyUrl: "https://openrouter.ai/keys",
      },
      {
        id: "opencode-go-chat",
        name: "OpenCode Go (Chat)",
        baseUrl: "https://opencode.ai/zen/go/v1",
        api: "openai-completions",
        keyUrl: "https://opencode.ai/auth",
      },
      {
        id: "opencode-go-anthropic",
        name: "OpenCode Go (Anthropic)",
        baseUrl: "https://opencode.ai/zen/go/v1",
        api: "anthropic-messages",
        keyUrl: "https://opencode.ai/auth",
      },
      {
        id: "opencode-go-responses",
        name: "OpenCode Go (Responses)",
        baseUrl: "https://opencode.ai/zen/go/v1",
        api: "openai-responses",
        keyUrl: "https://opencode.ai/auth",
      },
      {
        id: "opencode-zen-chat",
        name: "OpenCode Zen (Chat)",
        baseUrl: "https://opencode.ai/zen/v1",
        api: "openai-completions",
        keyUrl: "https://opencode.ai/auth",
      },
      {
        id: "opencode-zen-anthropic",
        name: "OpenCode Zen (Anthropic)",
        baseUrl: "https://opencode.ai/zen/v1",
        api: "anthropic-messages",
        keyUrl: "https://opencode.ai/auth",
      },
      {
        id: "opencode-zen-responses",
        name: "OpenCode Zen (Responses)",
        baseUrl: "https://opencode.ai/zen/v1",
        api: "openai-responses",
        keyUrl: "https://opencode.ai/auth",
      },
    ]),
    // The native harness's tool names (the enabled-tools checkbox list).
    listTools: vi
      .fn()
      .mockResolvedValue(["bash", "read", "write", "edit", "subagent"]),
    saveSettings: vi.fn().mockResolvedValue(undefined),
    // The one-shot MCP test (ADR 0019): 0 tools by default (the tests
    // override per case).
    testMcpServer: vi.fn().mockResolvedValue(0),
    authMcpServer: vi.fn().mockResolvedValue("authenticated"),
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
  await screen.findByText("Default model");
}

/** Navigate to a section (click its sidebar button) and wait for it. */
async function go(
  section: "Appearance" | "Providers" | "Subagents" | "MCP",
): Promise<void> {
  fireEvent.click(screen.getByRole("button", { name: section }));
  if (section === "Appearance") await screen.findByText("Theme");
  else if (section === "Providers")
    await screen.findByRole("button", { name: "Add provider" });
  // The Subagents card has no always-present button/heading (an empty
  // agents list renders a nearly empty card) — the per-test `findByText`
  // on a row handles the wait.
  else if (section === "Subagents") return;
  // The "Add server" button is always in the MCP section (below the card).
  else await screen.findByRole("button", { name: "Add server" });
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
    expect(saved.defaultThinkingLevel).toBeNull();
    expect(saved.enabledTools).toEqual([]);
    expect(saved.defaultModel).toBeNull();
    expect(saved.providers).toHaveLength(1);
    expect(saved.font).toEqual(baseSettings.font);
  });

  it("the UI font picker offers Noto Sans, Fira Sans, and Martel Sans (immediate save of the quoted family)", async () => {
    render(<SettingsPage onBack={vi.fn()} />);
    await loaded();
    await go("Appearance");
    const trigger = await screen.findByRole("combobox", { name: "UI font" });
    fireEvent.click(trigger);
    // The three web-font families are offered (alongside the existing
    // System / Serif / Monospace options).
    for (const name of ["Noto Sans", "Fira Sans", "Martel Sans"]) {
      expect(screen.getByRole("option", { name })).toBeTruthy();
    }
    // Pick one: the saved `uiFamily` is the QUOTED CSS family name
    // (`applySettingsFont` splices it into the `--font-sans` stack — an
    // unquoted multi-word name would not be a valid CSS family there).
    fireEvent.click(screen.getByRole("option", { name: "Fira Sans" }));
    await waitFor(() => expect(saveSettings).toHaveBeenCalledTimes(1));
    const saved = vi.mocked(saveSettings).mock.calls[0][0] as AppSettings;
    expect(saved.font.uiFamily).toBe('"Fira Sans"');
  });

  it("the font pickers' default option is the app default (Noto Sans for UI, Fira Code for Code)", async () => {
    render(<SettingsPage onBack={vi.fn()} />);
    await loaded();
    await go("Appearance");
    // The `""` (null) option is the APP DEFAULT — Noto Sans for the UI
    // font, Fira Code for the code font (the `index.css` stacks resolve
    // to them first, with the system tail as the offline fallback).
    // (One select at a time: an open Radix Select `aria-hidden`s the rest
    // of the document, so the other trigger is unreachable until it
    // closes.)
    const uiTrigger = await screen.findByRole("combobox", { name: "UI font" });
    fireEvent.click(uiTrigger); // open
    expect(
      screen.getByRole("option", { name: "Default (Noto Sans)" }),
    ).toBeTruthy();
    fireEvent.keyDown(uiTrigger, { key: "Escape" }); // close (Radix's escape)
    await new Promise((resolve) => setTimeout(resolve, 0)); // let Radix un-`aria-hide` the document
    const codeTrigger = await screen.findByRole("combobox", {
      name: "Code font",
    });
    fireEvent.click(codeTrigger); // open
    expect(
      screen.getByRole("option", { name: "Default (Fira Code)" }),
    ).toBeTruthy();
  });

  it("selecting a default option shows its label in the trigger (not empty — Radix renders nothing for an empty-string value)", async () => {
    render(<SettingsPage onBack={vi.fn()} />);
    await loaded();
    await go("Appearance");
    const uiTrigger = await screen.findByRole("combobox", { name: "UI font" });
    // A fresh `null` family: the default option is selected — the trigger
    // shows its label (with the `""` value it would show nothing — Radix
    // treats an empty value as "no value").
    expect(
      uiTrigger.querySelector("[data-slot=select-value]")?.textContent,
    ).toBe("Default (Noto Sans)");
    // Pick a web font, then go BACK to the default: the label comes back
    // (and the saved document maps the sentinel to `null`).
    fireEvent.click(uiTrigger);
    fireEvent.click(screen.getByRole("option", { name: "Fira Sans" }));
    await waitFor(() => expect(saveSettings).toHaveBeenCalledTimes(1));
    expect(
      uiTrigger.querySelector("[data-slot=select-value]")?.textContent,
    ).toBe("Fira Sans");
    fireEvent.click(uiTrigger);
    fireEvent.click(
      screen.getByRole("option", { name: "Default (Noto Sans)" }),
    );
    await waitFor(() => expect(saveSettings).toHaveBeenCalledTimes(2));
    expect(
      uiTrigger.querySelector("[data-slot=select-value]")?.textContent,
    ).toBe("Default (Noto Sans)");
    const saved = vi.mocked(saveSettings).mock.calls[1][0] as AppSettings;
    expect(saved.font.uiFamily).toBeNull();
  });

  it("a_provider_field_commits_on_blur_and_refreshes_discovery", async () => {
    render(<SettingsPage onBack={vi.fn()} />);
    await loaded();
    await go("Providers");
    const nameInput = await screen.findByDisplayValue("Tama");
    fireEvent.change(nameInput, { target: { value: "Tama2" } });
    // Commit on blur: the id is derived from the name (`Tama` → `Tama2`
    // re-identifies the provider `tama` → `tama2`) + re-run the discovery
    // under the NEW id (the `listModels` force-refresh bypasses the cache).
    fireEvent.blur(nameInput);
    await waitFor(() => expect(saveSettings).toHaveBeenCalled());
    const saved = vi.mocked(saveSettings).mock.calls[0][0] as AppSettings;
    expect(saved.providers[0].name).toBe("Tama2");
    expect(saved.providers[0].id).toBe("tama2");
    expect(listModels).toHaveBeenCalledWith("tama2");
  });

  it("add_provider_appends_a_row_and_the_name_commit_derives_the_id", async () => {
    render(<SettingsPage onBack={vi.fn()} />);
    await loaded();
    await go("Providers");
    // The existing provider row is loaded (one "Provider name" input).
    await screen.findByDisplayValue("Tama");
    fireEvent.click(screen.getByRole("button", { name: "Add provider" }));
    // The new empty row: a second "Provider name" input, empty, editable
    // (a placeholder id until a name is committed).
    const nameInputs = screen.getAllByPlaceholderText("Provider name");
    expect(nameInputs).toHaveLength(2);
    const newInput = nameInputs[1] as HTMLInputElement;
    expect(newInput.value).toBe("");
    // Type the SAME name as the existing provider + commit: the id is
    // derived from the name (the slug `tama`) and de-duped against the
    // existing provider's id (`tama` → `tama-2`). The ADD already saved
    // the document (immediate save — call 0, name `""`); the blur commit
    // is call 1.
    fireEvent.change(newInput, { target: { value: "Tama" } });
    fireEvent.blur(newInput);
    await waitFor(() => expect(saveSettings).toHaveBeenCalledTimes(2));
    const saved = vi.mocked(saveSettings).mock.calls[1][0] as AppSettings;
    expect(saved.providers).toHaveLength(2);
    expect(saved.providers[1].name).toBe("Tama");
    expect(saved.providers[1].id).toBe("tama-2");
  });

  it("a_name_commit_reidentifies_the_provider_and_remaps_the_model_refs", async () => {
    // The loaded document: the default model + a remembered thinking level
    // reference the provider by its CURRENT id (`tama/Qwen3.8`).
    vi.mocked(getSettings).mockResolvedValueOnce({
      ...baseSettings,
      defaultModel: "tama/Qwen3.8",
      defaultThinkingLevels: { "tama/Qwen3.8": "xhigh" },
      subagentModels: { scout: "tama/m1" },
    });
    render(<SettingsPage onBack={vi.fn()} />);
    await loaded();
    await go("Providers");
    const nameInput = await screen.findByDisplayValue("Tama");
    // Rename: `Tama` → `Another Name` → the id is the slug (`another-name`).
    fireEvent.change(nameInput, { target: { value: "Another Name" } });
    fireEvent.blur(nameInput);
    await waitFor(() => expect(saveSettings).toHaveBeenCalledTimes(1));
    const saved = vi.mocked(saveSettings).mock.calls[0][0] as AppSettings;
    expect(saved.providers[0].name).toBe("Another Name");
    expect(saved.providers[0].id).toBe("another-name");
    // The settings-level references ride along re-mapped (old id → new id —
    // they must NOT orphan).
    expect(saved.defaultModel).toBe("another-name/Qwen3.8");
    expect(saved.defaultThinkingLevels).toEqual({
      "another-name/Qwen3.8": "xhigh",
    });
    // (ADR 0023) The per-agent subagent overrides ride along re-mapped by
    // VALUE (the agent name is the key — the model ref is the value; a
    // non-matching value stays untouched).
    expect(saved.subagentModels).toEqual({ scout: "another-name/m1" });
    // The discovery re-runs under the new id.
    expect(listModels).toHaveBeenCalledWith("another-name");
  });

  it("a_blank_name_keeps_the_provider_id", async () => {
    render(<SettingsPage onBack={vi.fn()} />);
    await loaded();
    await go("Providers");
    const nameInput = await screen.findByDisplayValue("Tama");
    fireEvent.change(nameInput, { target: { value: "" } });
    fireEvent.blur(nameInput);
    await waitFor(() => expect(saveSettings).toHaveBeenCalledTimes(1));
    const saved = vi.mocked(saveSettings).mock.calls[0][0] as AppSettings;
    expect(saved.providers[0].name).toBe("");
    expect(saved.providers[0].id).toBe("tama");
  });

  it("the_provider_row_shows_all_its_fields", async () => {
    render(<SettingsPage onBack={vi.fn()} />);
    await loaded();
    await go("Providers");
    // The labeled fields (the fix for the clipped single-line layout —
    // the Name / Base URL / API key fields are all visible, not
    // overflowed out of the 280px control column).
    expect(screen.getByText("Name")).toBeTruthy();
    expect(screen.getByText("Base URL")).toBeTruthy();
    expect(screen.getByText("API key")).toBeTruthy();
    expect(
      await screen.findByDisplayValue("https://tama.wizards.town/v1"),
    ).toBeTruthy();
    // The status + the refresh + the remove remain in the row.
    expect(screen.getByRole("button", { name: "Refresh models" })).toBeTruthy();
    expect(
      screen.getByRole("button", { name: "Remove provider" }),
    ).toBeTruthy();
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

  it("the_default_model_picker_dialog_lists_the_catalog_and_system_default", async () => {
    render(<SettingsPage onBack={vi.fn()} />);
    await loaded();
    // The model pickers are DIALOGS (the catalog is too long for a Radix
    // dropdown) — the trigger is a button, not a combobox.
    const trigger = screen.getByRole("button", { name: "Default model" });
    fireEvent.click(trigger);
    // The catalog entry (the BARE model id + the provider's display-name
    // cue — `tama` → `Tama` from the configured providers; the composed
    // `tama/Qwen3.8` key is the VALUE, never the row text) + the
    // "System default" row.
    expect(screen.getByText(/Qwen3\.8/)).toBeTruthy();
    expect(screen.getByText(/\(Tama\)/)).toBeTruthy();
    expect(screen.queryByText(/tama\/Qwen3\.8/)).toBeNull();
    // "System default" appears TWICE — the trigger's placeholder AND the
    // dialog's row — so the row is picked scoped to the dialog.
    const dialog = within(
      document.querySelector("[data-slot=dialog-content]") as HTMLElement,
    );
    expect(dialog.getByText("System default")).toBeTruthy();
    // Choosing the model saves the COMPOSED key (the row's text is the
    // bare id, the value is `provider/id`).
    fireEvent.click(dialog.getByText(/Qwen3\.8/));
    await waitFor(() =>
      expect(saveSettings).toHaveBeenLastCalledWith(
        expect.objectContaining({ defaultModel: "tama/Qwen3.8" }),
      ),
    );
    // Choosing "System default" saves `defaultModel: null`. Re-capture the
    // dialog content — Radix REMOUNTS it on the second open (the first
    // selection closed the dialog and unmounted the old content node, so
    // the reference captured above is detached and inert).
    fireEvent.click(trigger);
    const reopenedDialog = within(
      document.querySelector("[data-slot=dialog-content]") as HTMLElement,
    );
    fireEvent.click(reopenedDialog.getByText("System default"));
    await waitFor(() =>
      expect(saveSettings).toHaveBeenLastCalledWith(
        expect.objectContaining({ defaultModel: null }),
      ),
    );
  });

  it("the_default_thinking_level_select_offers_the_union_of_the_models_levels_and_commits_the_choice", async () => {
    render(<SettingsPage onBack={vi.fn()} />);
    await loaded();
    const trigger = await screen.findByRole("combobox", {
      name: "Default thinking level",
    });
    fireEvent.click(trigger);
    // The union across the two models (deduped, FIRST-SEEN order — NOT
    // lexicographic): the overlapping `medium` / `high` appear once, in
    // the first model's advertised order, then `xhigh` from the second.
    expect(
      await screen.findByRole("option", { name: "Model default" }),
    ).toBeTruthy();
    const options = await screen.findAllByRole("option");
    expect(options.map((o) => o.textContent)).toEqual([
      "Model default",
      "high",
      "medium",
      "low",
      "xhigh",
    ]);
    // Choosing a level saves it.
    fireEvent.click(screen.getByRole("option", { name: "xhigh" }));
    await waitFor(() =>
      expect(saveSettings).toHaveBeenLastCalledWith(
        expect.objectContaining({ defaultThinkingLevel: "xhigh" }),
      ),
    );
  });

  it("the_default_thinking_level_select_commits_null_on_model_default", async () => {
    // The loaded document remembers a level; choosing "Model default"
    // commits `null` (the model's own default).
    vi.mocked(getSettings).mockResolvedValueOnce({
      ...baseSettings,
      defaultThinkingLevel: "high",
    });
    render(<SettingsPage onBack={vi.fn()} />);
    await loaded();
    const trigger = await screen.findByRole("combobox", {
      name: "Default thinking level",
    });
    fireEvent.click(trigger);
    fireEvent.click(
      await screen.findByRole("option", { name: "Model default" }),
    );
    await waitFor(() =>
      expect(saveSettings).toHaveBeenLastCalledWith(
        expect.objectContaining({ defaultThinkingLevel: null }),
      ),
    );
  });

  it("a_stored_default_thinking_level_outside_the_union_renders_as_its_own_option", async () => {
    // A stored level no model currently advertises (a provider removed it /
    // the level sets changed — the select offers the UNION): the select
    // must show the ACTUAL stored value as its own selectable option, not
    // fall back to the "Model default" placeholder (which would lie about
    // the stored value).
    vi.mocked(getSettings).mockResolvedValueOnce({
      ...baseSettings,
      defaultThinkingLevel: "ultra",
    });
    render(<SettingsPage onBack={vi.fn()} />);
    await loaded();
    const trigger = await screen.findByRole("combobox", {
      name: "Default thinking level",
    });
    fireEvent.click(trigger);
    // The raw stored value is offered (alongside the union) — and it is the
    // SELECTED option: the trigger displays the actual stored level, not
    // the "Model default" placeholder (the old fall-back, which lied about
    // the stored value).
    const ultra = await screen.findByRole("option", { name: "ultra" });
    expect(ultra.getAttribute("data-state")).toBe("checked");
    expect(trigger.textContent).toContain("ultra");
    expect(trigger.textContent).not.toContain("Model default");
  });

  it("a_blank_stored_default_thinking_level_renders_no_extra_option", async () => {
    // A hand-edited `settings.json` may carry `"defaultThinkingLevel": ""`
    // (the frontend never SAVES `""`): the select must behave as for `null`
    // — the "Model default" placeholder — NOT render a second `value=""`
    // item alongside it (two items with the same value break Radix).
    vi.mocked(getSettings).mockResolvedValueOnce({
      ...baseSettings,
      defaultThinkingLevel: "",
    });
    render(<SettingsPage onBack={vi.fn()} />);
    await loaded();
    const trigger = await screen.findByRole("combobox", {
      name: "Default thinking level",
    });
    // The trigger shows the "Model default" placeholder (blank = absent).
    expect(trigger.textContent).toContain("Model default");
    fireEvent.click(trigger);
    // Exactly ONE "Model default" option — the union, unchanged.
    const modelDefaults = await screen.findAllByRole("option", {
      name: "Model default",
    });
    expect(modelDefaults).toHaveLength(1);
    const options = screen.getAllByRole("option");
    expect(options.map((o) => o.textContent)).toEqual([
      "Model default",
      "high",
      "medium",
      "low",
      "xhigh",
    ]);
  });

  it("the_enabled_tools_checkboxes_commit_the_checked_names", async () => {
    render(<SettingsPage onBack={vi.fn()} />);
    await loaded();
    // One row per native harness tool (the `listTools` fixture).
    const bash = await screen.findByRole("checkbox", { name: /bash/ });
    const edit = screen.getByRole("checkbox", { name: /edit/ });
    // Toggle two on (the initial `enabledTools` is `[]`): each toggle
    // commits the checked names (immediate save — two saves).
    fireEvent.click(bash);
    fireEvent.click(edit);
    await waitFor(() => expect(saveSettings).toHaveBeenCalledTimes(2));
    const saved = vi.mocked(saveSettings).mock.calls[1][0] as AppSettings;
    expect(saved.enabledTools).toEqual(["bash", "edit"]);
  });

  it("an_unchecked_all_enabled_tools_list_commits_an_empty_list", async () => {
    // The loaded document explicitly enables two tools; unchecking both
    // commits `[]` (= all tools — the backend's convention).
    vi.mocked(getSettings).mockResolvedValueOnce({
      ...baseSettings,
      enabledTools: ["bash", "read"],
    });
    render(<SettingsPage onBack={vi.fn()} />);
    await loaded();
    fireEvent.click(await screen.findByRole("checkbox", { name: /bash/ }));
    fireEvent.click(screen.getByRole("checkbox", { name: /read/ }));
    await waitFor(() => expect(saveSettings).toHaveBeenCalledTimes(2));
    const saved = vi.mocked(saveSettings).mock.calls[1][0] as AppSettings;
    expect(saved.enabledTools).toEqual([]);
  });

  // ---------------------------------------------------------------------
  // The file-access policy (ADR 0030): three selects — Reads / Writes /
  // Shell — above the Trust switch. All three DEFAULT to `allow`
  // ("Don't ask me"), so the row copy is the safety story: it must state
  // the free pass plainly (never phrase the default as a restriction).
  // ---------------------------------------------------------------------

  /** The `SettingsRow` hosting the named control (label + description +
   * control share one row element — `SettingsRow` renders the control as
   * the trigger's accessible name source). */
  function rowOf(controlName: string): HTMLElement {
    const trigger = screen.getByRole("combobox", { name: controlName });
    const row = trigger.closest(".border-t");
    expect(row).toBeTruthy();
    return row as HTMLElement;
  }

  /** Open the named select and return its offered option labels. */
  async function openSelect(controlName: string): Promise<HTMLElement> {
    const trigger = await screen.findByRole("combobox", { name: controlName });
    fireEvent.click(trigger);
    await screen.findAllByRole("option");
    return trigger;
  }

  /** Close the open Radix select (one at a time — an open select
   * `aria-hidden`s the rest of the document, so the trigger must be the
   * element reference captured BEFORE it opened). */
  async function closeSelect(trigger: HTMLElement): Promise<void> {
    fireEvent.keyDown(trigger, { key: "Escape" });
    await new Promise((resolve) => setTimeout(resolve, 0)); // let Radix un-`aria-hide`
  }

  /** The labels of the currently open select's options. */
  function optionLabels(): string[] {
    return screen.getAllByRole("option").map((o) => o.textContent ?? "");
  }

  it("the_three_file_access_selects_default_to_don_t_ask_me", async () => {
    render(<SettingsPage onBack={vi.fn()} />);
    await loaded();
    for (const name of ["Reads", "Writes", "Shell"]) {
      const trigger = await screen.findByRole("combobox", { name });
      // The trigger DISPLAYS the selected policy (a fresh install is
      // all-`allow` — nothing may fall back to a placeholder).
      expect(
        trigger.querySelector("[data-slot=select-value]")?.textContent,
      ).toBe("Don't ask me");
    }
    // The three rows sit above the Trust switch (the plan's layout).
    const rows = document.querySelectorAll(".border-t");
    const index = (label: string): number =>
      [...rows].findIndex((r) =>
        [...r.querySelectorAll("div")].some((d) => d.textContent === label),
      );
    expect(index("Reads")).toBeGreaterThanOrEqual(0);
    expect(index("Reads")).toBeLessThan(index("Writes"));
    expect(index("Writes")).toBeLessThan(index("Shell"));
    expect(index("Shell")).toBeLessThan(index("Trust new Spaces by default"));
  });

  it("the_file_access_rows_state_the_honest_default (the copy carries the all-allow posture)", async () => {
    render(<SettingsPage onBack={vi.fn()} />);
    await loaded();
    // Writes + Shell: the default is a FREE PASS — the copy must name
    // "anywhere" and the absence of a prompt (a user who only ever sees
    // the default must not mistake it for a restriction).
    for (const name of ["Writes", "Shell"]) {
      const text = rowOf(name).textContent ?? "";
      expect(text).toContain("anywhere");
      expect(text).toContain("with no prompt");
    }
    // Writes also names the ONE carve-out that survives every policy.
    expect(rowOf("Writes").textContent ?? "").toContain("agent-definition");
    // Reads: `Don't ask me` means no prompt AND no log.
    expect(rowOf("Reads").textContent ?? "").toContain("no prompt and no log");
    // The boundary-relative promise for `Ask me` / `Sandboxed` (the
    // policy decides ONLY the beyond-boundary case).
    expect(rowOf("Reads").textContent ?? "").toContain("outside the boundary");
  });

  it("changing_the_reads_select_to_ask_me_saves_the_complete_document", async () => {
    render(<SettingsPage onBack={vi.fn()} />);
    await loaded();
    fireEvent.click(await screen.findByRole("combobox", { name: "Reads" }));
    fireEvent.click(await screen.findByRole("option", { name: "Ask me" }));
    await waitFor(() => expect(saveSettings).toHaveBeenCalledTimes(1));
    const saved = vi.mocked(saveSettings).mock.calls[0][0] as AppSettings;
    // The saved document carries the new policy AND every other field
    // (the whole-document save pattern — the backend passes it through).
    expect(saved.filePolicy.reads).toBe("ask");
    expect(saved.filePolicy.writes).toBe("allow");
    expect(saved.filePolicy.shell).toBe("allow");
    expect(saved.theme).toBe("dark");
    expect(saved.providers).toHaveLength(1);
    expect(saved.font).toEqual(baseSettings.font);
    // The select reloads from the saved document (the trigger shows the
    // choice — the optimistic state matches what was written).
    const trigger = screen.getByRole("combobox", { name: "Reads" });
    expect(trigger.querySelector("[data-slot=select-value]")?.textContent).toBe(
      "Ask me",
    );
  });

  it("the_shell_sandboxed_option_is_hidden_off_linux", async () => {
    vi.mocked(getAppInfo).mockResolvedValueOnce({
      version: "0.1.0",
      platform: "windows",
    });
    render(<SettingsPage onBack={vi.fn()} />);
    await loaded();
    // Reads still offers all three tiers…
    const reads = await openSelect("Reads");
    expect(optionLabels()).toEqual(["Sandboxed", "Ask me", "Don't ask me"]);
    await closeSelect(reads);
    // …the Shell row offers only two (there is no sandbox to choose —
    // Landlock is Linux-only).
    const shell = await openSelect("Shell");
    expect(optionLabels()).toEqual(["Ask me", "Don't ask me"]);
    expect(screen.queryByRole("option", { name: "Sandboxed" })).toBeNull();
    await closeSelect(shell);
    // Nothing was saved by merely looking.
    expect(saveSettings).not.toHaveBeenCalled();
  });

  it("the_shell_sandboxed_option_is_greyed_out_when_the_kernel_has_no_landlock", async () => {
    // Linux (so the tier IS offered — hiding it would hide a real choice)
    // but the kernel cannot confine: the option stays VISIBLE and greyed,
    // with the reason stated in one line. A silently-selectable tier whose
    // commands all fail closed is the failure mode this guards.
    vi.mocked(shellSandboxAvailable).mockResolvedValueOnce(false);
    render(<SettingsPage onBack={vi.fn()} />);
    await loaded();
    // The row is captured BEFORE the select opens (an open Radix select
    // `aria-hidden`s the rest of the document, so the trigger is
    // unqueryable while it is open).
    const shellRow = rowOf("Shell");
    const shell = await openSelect("Shell");
    const option = await screen.findByRole("option", { name: "Sandboxed" });
    expect(option.getAttribute("data-disabled")).not.toBeNull();
    expect(option.getAttribute("aria-disabled")).toBe("true");
    // The reason is in the row (not only in the disabled option).
    expect(shellRow.textContent ?? "").toContain("Landlock");
    // Clicking the greyed option changes nothing.
    fireEvent.click(option);
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(saveSettings).not.toHaveBeenCalled();
    await closeSelect(shell);
  });

  it("the_shell_sandboxed_option_is_selectable_when_the_kernel_can_confine", async () => {
    // CONTRAST (so the grey-out above cannot pass vacuously): with the
    // probe reporting Landlock ( the suite default), the tier is enabled.
    render(<SettingsPage onBack={vi.fn()} />);
    await loaded();
    const shellRow = rowOf("Shell");
    const shell = await openSelect("Shell");
    const option = await screen.findByRole("option", { name: "Sandboxed" });
    // The probe has to have SETTLED before this is meaningful (the page's
    // pre-probe state is deliberately "not supported"), hence `waitFor`.
    await waitFor(() =>
      expect(option.getAttribute("data-disabled")).toBeNull(),
    );
    expect(option.getAttribute("aria-disabled")).not.toBe("true");
    expect(shellRow.textContent ?? "").not.toContain("cannot run");
    fireEvent.keyDown(shell, { key: "Escape" });
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(saveSettings).not.toHaveBeenCalled();
  });

  it("a_stored_sandboxed_shell_on_non_linux_does_not_render_an_empty_select", async () => {
    // Off Linux the `Sandboxed` item is HIDDEN, but a stored
    // `shell: "sandboxed"` value survives the platform change — the trigger
    // must never render blank for a value whose option is gone (a
    // placeholder stands in; the value itself is preserved on save).
    vi.mocked(getAppInfo).mockResolvedValueOnce({
      version: "0.1.0",
      platform: "windows",
    });
    vi.mocked(getSettings).mockResolvedValueOnce({
      ...baseSettings,
      filePolicy: { reads: "allow", writes: "allow", shell: "sandboxed" },
    });
    render(<SettingsPage onBack={vi.fn()} />);
    await loaded();
    const trigger = await screen.findByRole("combobox", { name: "Shell" });
    const value = trigger
      .querySelector("[data-slot=select-value]")
      ?.textContent?.trim();
    expect(value).toBe("Sandboxed (unavailable here)");
    // NOT the freer-sounding label: a stored `sandboxed` fails every
    // command closed, and "Don't ask me" would hide that.
    expect(value).not.toBe("Don't ask me");
  });

  it("a_pre_feature_document_without_file_policy_renders_don_t_ask_me (never an empty select)", async () => {
    // A `settings.json` written before ADR 0030 has no `filePolicy` key;
    // the backend fills the default, but the UI must not render an empty
    // select if the key is ever absent.
    const preFeature = { ...baseSettings } as Partial<AppSettings>;
    delete preFeature.filePolicy;
    vi.mocked(getSettings).mockResolvedValueOnce(preFeature as AppSettings);
    render(<SettingsPage onBack={vi.fn()} />);
    await loaded();
    for (const name of ["Reads", "Writes", "Shell"]) {
      const trigger = await screen.findByRole("combobox", { name });
      expect(
        trigger.querySelector("[data-slot=select-value]")?.textContent,
      ).toBe("Don't ask me");
    }
  });

  it("the_default_agent_select_is_gone", async () => {
    render(<SettingsPage onBack={vi.fn()} />);
    await loaded();
    // The removed UI: there is one harness — no agent to choose, so the
    // "Default agent" control is absent (and the agent-catalog data source
    // is gone from the module — nothing to fetch).
    expect(screen.queryByText("Default agent")).toBeNull();
    expect(
      screen.queryByRole("combobox", { name: "Default agent" }),
    ).toBeNull();
  });

  it("the_update_round_trip_preserves_the_default_thinking_levels", async () => {
    // (ADR 0015) The loaded document remembers a per-model thinking level
    // (`"<provider>/<id>"` → level).
    vi.mocked(getSettings).mockResolvedValueOnce({
      ...baseSettings,
      defaultThinkingLevels: { "tama/Qwen3.8": "xhigh" },
    });
    render(<SettingsPage onBack={vi.fn()} />);
    await loaded();
    // ANY update (theme → Light) = one save of the complete document.
    fireEvent.click(screen.getByRole("button", { name: "Appearance" }));
    const trigger = await screen.findByRole("combobox", { name: "Theme" });
    fireEvent.click(trigger);
    fireEvent.click(screen.getByRole("option", { name: "Light" }));
    await waitFor(() => expect(saveSettings).toHaveBeenCalledTimes(1));
    const saved = vi.mocked(saveSettings).mock.calls[0][0] as AppSettings;
    // The `{ ...settings, ...patch }` round-trip loses no field — the
    // per-model memory rides along untouched (no new UI, ADR 0015).
    expect(saved.defaultThinkingLevels).toEqual({ "tama/Qwen3.8": "xhigh" });
  });
});

describe("SettingsPage (the MCP section — ADR 0019)", () => {
  it("the_mcp_section_lists_the_servers_with_their_actions", async () => {
    render(<SettingsPage onBack={vi.fn()} />);
    await loaded();
    await go("MCP");
    // The server row: the name + a type badge (HTTP) + a one-line summary
    // (the url) + the on-demand Test / Edit / Remove actions.
    expect(await screen.findByText("tama")).toBeTruthy();
    expect(screen.getByText("HTTP")).toBeTruthy();
    expect(screen.getByText("https://tama/mcp")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Add server" })).toBeTruthy();
    expect(
      screen.getByRole("button", { name: "Test MCP server tama" }),
    ).toBeTruthy();
    expect(
      screen.getByRole("button", { name: "Edit MCP server tama" }),
    ).toBeTruthy();
    expect(
      screen.getByRole("button", { name: "Remove MCP server tama" }),
    ).toBeTruthy();
  });

  it("add_mcp_http_server_saves_the_entry_in_pi_shape", async () => {
    render(<SettingsPage onBack={vi.fn()} />);
    await loaded();
    await go("MCP");
    fireEvent.click(screen.getByRole("button", { name: "Add server" }));
    // The structured form (HTTP is the default kind).
    expect(await screen.findByRole("dialog")).toBeTruthy();
    const nameInput = screen.getByPlaceholderText("Server name");
    fireEvent.change(nameInput, { target: { value: "My Gateway" } });
    const urlInput = screen.getByPlaceholderText("https://example.com/mcp");
    fireEvent.change(urlInput, {
      target: { value: "https://gw.example.com/mcp" },
    });
    const headers = screen.getByPlaceholderText("Authorization=Bearer $TOKEN");
    fireEvent.change(headers, { target: { value: "Authorization=Bearer k" } });
    const bearer = screen.getByPlaceholderText("MY_TOKEN");
    fireEvent.change(bearer, { target: { value: "GW_TOKEN" } });
    fireEvent.click(
      screen.getByRole("switch", { name: "OAuth (interactive flow)" }),
    );
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(saveSettings).toHaveBeenCalled());
    const saved = vi.mocked(saveSettings).mock.calls[0][0] as AppSettings;
    // The entry is pi's shape VERBATIM (an entry copy-pastes to mcp.json).
    expect(saved.mcpServers["My Gateway"]).toEqual({
      url: "https://gw.example.com/mcp",
      headers: { Authorization: "Bearer k" },
      bearerTokenEnv: "GW_TOKEN",
      auth: "oauth",
    });
    // The new row appears + the dialog closed.
    expect(await screen.findByText("My Gateway")).toBeTruthy();
  });

  it("add_mcp_stdio_server_saves_the_entry", async () => {
    render(<SettingsPage onBack={vi.fn()} />);
    await loaded();
    await go("MCP");
    fireEvent.click(screen.getByRole("button", { name: "Add server" }));
    await screen.findByRole("dialog");
    // Switch to stdio (the command / args / env / cwd fields appear).
    const kindTrigger = screen.getByRole("combobox", { name: "Type" });
    fireEvent.click(kindTrigger);
    fireEvent.click(
      await screen.findByRole("option", { name: "stdio (a spawned process)" }),
    );
    const nameInput = screen.getByPlaceholderText("Server name");
    fireEvent.change(nameInput, { target: { value: "Local" } });
    const command = screen.getByPlaceholderText("npx");
    fireEvent.change(command, { target: { value: "npx" } });
    // The args placeholder is multi-line (one arg per line) — the query is
    // the NORMALIZED form (a space; `getByPlaceholderText` normalizes the
    // element's text but not the query).
    const args = screen.getByPlaceholderText("-y example-mcp");
    fireEvent.change(args, { target: { value: "-y\nexample-mcp" } });
    const env = screen.getByPlaceholderText("KEY=value");
    fireEvent.change(env, { target: { value: "MY_VAR=1" } });
    const cwd = screen.getByPlaceholderText("/home/user/project");
    fireEvent.change(cwd, { target: { value: "/tmp/proj" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(saveSettings).toHaveBeenCalled());
    const saved = vi.mocked(saveSettings).mock.calls[0][0] as AppSettings;
    expect(saved.mcpServers["Local"]).toEqual({
      command: "npx",
      args: ["-y", "example-mcp"],
      env: { MY_VAR: "1" },
      cwd: "/tmp/proj",
    });
  });

  it("edit_mcp_server_pre_fills_the_form_and_updates_the_entry", async () => {
    render(<SettingsPage onBack={vi.fn()} />);
    await loaded();
    await go("MCP");
    await screen.findByText("tama");
    fireEvent.click(
      screen.getByRole("button", { name: "Edit MCP server tama" }),
    );
    // The form is pre-filled (the entry's values).
    expect(await screen.findByDisplayValue("https://tama/mcp")).toBeTruthy();
    fireEvent.change(screen.getByDisplayValue("https://tama/mcp"), {
      target: { value: "https://tama2/mcp" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(saveSettings).toHaveBeenCalled());
    const saved = vi.mocked(saveSettings).mock.calls[0][0] as AppSettings;
    expect(saved.mcpServers["tama"]).toEqual({ url: "https://tama2/mcp" });
  });

  it("remove_mcp_server_confirms_then_saves", async () => {
    render(<SettingsPage onBack={vi.fn()} />);
    await loaded();
    await go("MCP");
    await screen.findByText("tama");
    fireEvent.click(
      screen.getByRole("button", { name: "Remove MCP server tama" }),
    );
    // The confirm dialog (the server name in the copy).
    expect(await screen.findByRole("alertdialog")).toBeTruthy();
    expect(screen.getByText("Remove MCP server tama?")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Remove" }));
    await waitFor(() =>
      expect(saveSettings).toHaveBeenCalledWith(
        expect.objectContaining({ mcpServers: {} }),
      ),
    );
  });

  it("test_mcp_server_runs_the_one_shot_test_and_reports_the_count", async () => {
    vi.mocked(testMcpServer).mockResolvedValueOnce(3);
    render(<SettingsPage onBack={vi.fn()} />);
    await loaded();
    await go("MCP");
    await screen.findByText("tama");
    fireEvent.click(
      screen.getByRole("button", { name: "Test MCP server tama" }),
    );
    // The one-shot test (the entry as saved) + the result badge.
    expect(await screen.findByText("3 tools")).toBeTruthy();
    expect(testMcpServer).toHaveBeenCalledWith(
      { url: "https://tama/mcp" },
      "tama",
    );
  });

  it("test_mcp_server_surfaces_the_error", async () => {
    vi.mocked(testMcpServer).mockRejectedValueOnce("connect timed out");
    render(<SettingsPage onBack={vi.fn()} />);
    await loaded();
    await go("MCP");
    await screen.findByText("tama");
    fireEvent.click(
      screen.getByRole("button", { name: "Test MCP server tama" }),
    );
    // The error badge (the message in the tooltip).
    const badge = await screen.findByText("error");
    expect(badge).toBeTruthy();
  });

  it("auth_mcp_server_runs_the_interactive_flow_and_retests", async () => {
    vi.mocked(getSettings).mockResolvedValueOnce({
      ...baseSettings,
      mcpServers: {
        oauth_srv: { url: "https://auth.example/mcp", auth: "oauth" },
      },
    });
    vi.mocked(authMcpServer).mockResolvedValueOnce("oauth_srv: authenticated");
    vi.mocked(testMcpServer).mockResolvedValueOnce(5);
    render(<SettingsPage onBack={vi.fn()} />);
    await loaded();
    await go("MCP");
    await screen.findByText("oauth_srv");
    const authBtn = screen.getByRole("button", {
      name: "Authenticate MCP server oauth_srv",
    });
    fireEvent.click(authBtn);
    expect(await screen.findByText("5 tools")).toBeTruthy();
    expect(authMcpServer).toHaveBeenCalledWith("oauth_srv", {
      url: "https://auth.example/mcp",
      auth: "oauth",
    });
    expect(testMcpServer).toHaveBeenCalledWith(
      { url: "https://auth.example/mcp", auth: "oauth" },
      "oauth_srv",
    );
  });
});

describe("SettingsPage (the Subagents section — ADR 0023)", () => {
  it("renders_the_subagents_section_rows_for_discovered_agents", async () => {
    vi.mocked(getSettings).mockResolvedValueOnce({
      ...baseSettings,
      subagentModels: {},
    });
    vi.mocked(listAgentDefinitions).mockResolvedValueOnce([
      {
        name: "scout",
        description: "Fast recon",
        model: "p/m1",
        scope: "user",
      },
      { name: "builder", description: "", model: null, scope: "user" },
    ]);
    render(<SettingsPage onBack={vi.fn()} />);
    await loaded();
    await go("Subagents");
    // One row per discovered agent: the name + the description (with the
    // file's model — read-only; `null` = the agent inherits the parent
    // model).
    expect(await screen.findByText("scout")).toBeTruthy();
    expect(screen.getByText("builder")).toBeTruthy();
    expect(screen.getByText(/Fast recon · file: p\/m1/)).toBeTruthy();
    expect(
      screen.getByText(/No description · file: — \(inherits parent model\)/),
    ).toBeTruthy();
    // Both triggers default to "File value (no override)" (the
    // placeholder — no override stored). The model pickers are DIALOGS
    // (the triggers are buttons, not comboboxes).
    const scoutTrigger = screen.getByRole("button", {
      name: "Subagent model for scout",
    });
    expect(scoutTrigger.textContent).toContain("File value (no override)");
    const builderTrigger = screen.getByRole("button", {
      name: "Subagent model for builder",
    });
    expect(builderTrigger.textContent).toContain("File value (no override)");
  });

  it("a_stale_override_value_renders_as_its_own_option", async () => {
    // A stored override whose model key is no longer in the catalog (a
    // renamed/removed provider): the select must show the ACTUAL stored
    // value as its own (disabled) option — NOT fall back to the "File
    // value (no override)" placeholder (which would lie about the stored
    // value — the `storedLevelOutsideUnion` pattern).
    vi.mocked(getSettings).mockResolvedValueOnce({
      ...baseSettings,
      subagentModels: { scout: "gone/m1" },
    });
    vi.mocked(listAgentDefinitions).mockResolvedValueOnce([
      {
        name: "scout",
        description: "Fast recon",
        model: "p/m1",
        scope: "user",
      },
    ]);
    render(<SettingsPage onBack={vi.fn()} />);
    await loaded();
    await go("Subagents");
    const trigger = await screen.findByRole("button", {
      name: "Subagent model for scout",
    });
    // The trigger displays the stored value, not the placeholder.
    expect(trigger.textContent).toContain("gone/m1");
    expect(trigger.textContent).not.toContain("File value (no override)");
    fireEvent.click(trigger);
    // The raw stored value is offered (alongside the catalog) as its own
    // DISABLED row (it is not a live catalog model) — clicking it does
    // nothing (the `storedLevelOutsideUnion` pattern).
    const dialog = document.querySelector(
      "[data-slot=dialog-content]",
    ) as HTMLElement;
    const stale = dialog.querySelector("[aria-disabled=true]");
    expect(stale).toBeTruthy();
    expect(stale?.textContent).toContain("gone/m1");
    fireEvent.click(stale!);
    expect(saveSettings).not.toHaveBeenCalled();
  });

  it("selecting_a_model_saves_the_subagent_models_entry", async () => {
    vi.mocked(listAgentDefinitions).mockResolvedValueOnce([
      {
        name: "scout",
        description: "Fast recon",
        model: "p/m1",
        scope: "user",
      },
    ]);
    render(<SettingsPage onBack={vi.fn()} />);
    await loaded();
    await go("Subagents");
    const trigger = await screen.findByRole("button", {
      name: "Subagent model for scout",
    });
    fireEvent.click(trigger);
    // Pick a catalog model (the `listModels` fixture's first model).
    fireEvent.click(await screen.findByText("Qwen3.8"));
    // Immediate save of the COMPLETE document: `subagentModels` gains the
    // entry; every other field is the loaded document, unchanged.
    await waitFor(() =>
      expect(saveSettings).toHaveBeenLastCalledWith({
        ...baseSettings,
        subagentModels: { scout: "tama/Qwen3.8" },
      }),
    );
  });

  it("selecting_file_value_deletes_the_subagent_models_entry", async () => {
    // The loaded document carries an override; picking "File value (no
    // override)" deletes the key (the agent falls back to its file's
    // model).
    vi.mocked(getSettings).mockResolvedValueOnce({
      ...baseSettings,
      subagentModels: { scout: "p/m1" },
    });
    vi.mocked(listAgentDefinitions).mockResolvedValueOnce([
      {
        name: "scout",
        description: "Fast recon",
        model: "p/m1",
        scope: "user",
      },
    ]);
    render(<SettingsPage onBack={vi.fn()} />);
    await loaded();
    await go("Subagents");
    const trigger = await screen.findByRole("button", {
      name: "Subagent model for scout",
    });
    expect(trigger.textContent).toContain("p/m1");
    fireEvent.click(trigger);
    fireEvent.click(await screen.findByText("File value (no override)"));
    await waitFor(() =>
      expect(saveSettings).toHaveBeenLastCalledWith(
        expect.objectContaining({ subagentModels: {} }),
      ),
    );
    const saved = vi.mocked(saveSettings).mock.calls[
      vi.mocked(saveSettings).mock.calls.length - 1
    ]?.[0] as AppSettings;
    // The key is ABSENT (not `null` / `""` — deleted).
    expect("scout" in saved.subagentModels).toBe(false);
    expect(saved.subagentModels).toEqual({});
  });

  it("an_orphaned_override_renders_with_a_remove_button", async () => {
    // The loaded document carries an override for an agent that is no
    // longer discovered (its file was removed/renamed): a muted row with
    // a remove button — the ONLY cleanup path (no confirm: a stale entry
    // is inert).
    vi.mocked(getSettings).mockResolvedValueOnce({
      ...baseSettings,
      subagentModels: { ghost: "p/m1" },
    });
    vi.mocked(listAgentDefinitions).mockResolvedValueOnce([
      {
        name: "scout",
        description: "Fast recon",
        model: "p/m1",
        scope: "user",
      },
    ]);
    render(<SettingsPage onBack={vi.fn()} />);
    await loaded();
    await go("Subagents");
    // The orphan row: the muted name + the stale copy.
    expect(await screen.findByText("ghost")).toBeTruthy();
    expect(
      screen.getByText("No longer discovered (stale override)"),
    ).toBeTruthy();
    // The discovered agent is still listed (the orphan is ADDITIONAL).
    expect(screen.getByText("scout")).toBeTruthy();
    // Remove: deletes the key (immediate save, the complete document).
    fireEvent.click(
      screen.getByRole("button", { name: "Remove subagent override ghost" }),
    );
    await waitFor(() =>
      expect(saveSettings).toHaveBeenLastCalledWith(
        expect.objectContaining({ subagentModels: {} }),
      ),
    );
    const saved = vi.mocked(saveSettings).mock.calls[
      vi.mocked(saveSettings).mock.calls.length - 1
    ]?.[0] as AppSettings;
    expect("ghost" in saved.subagentModels).toBe(false);
  });

  it("a_mixed_case_stored_override_renders_in_the_row", async () => {
    // The backend resolves `subagentModels` keys case-insensitively (`eq_ignore_ascii_case`),
    // so a hand-edited key `"Scout"` for a discovered agent `scout` IS the stored
    // override — the row must show the ACTUAL stored value (here no longer in the
    // catalog → its own disabled option), NOT the "File value (no override)" placeholder
    // (an exact-case lookup would misrepresent the saved state).
    vi.mocked(getSettings).mockResolvedValueOnce({
      ...baseSettings,
      subagentModels: { Scout: "gone/m1" },
    });
    vi.mocked(listAgentDefinitions).mockResolvedValueOnce([
      {
        name: "scout",
        description: "Fast recon",
        model: "p/m1",
        scope: "user",
      },
    ]);
    render(<SettingsPage onBack={vi.fn()} />);
    await loaded();
    await go("Subagents");
    const trigger = await screen.findByRole("button", {
      name: "Subagent model for scout",
    });
    // The trigger displays the stored value, not the placeholder.
    expect(trigger.textContent).toContain("gone/m1");
    expect(trigger.textContent).not.toContain("File value (no override)");
    fireEvent.click(trigger);
    // The raw stored value is offered (alongside the catalog) as its own
    // DISABLED row (the case-insensitive lookup found it — the
    // `storedLevelOutsideUnion` pattern).
    const dialog = document.querySelector(
      "[data-slot=dialog-content]",
    ) as HTMLElement;
    const stale = dialog.querySelector("[aria-disabled=true]");
    expect(stale).toBeTruthy();
    expect(stale?.textContent).toContain("gone/m1");
  });

  it("a_case_variant_key_is_not_treated_as_an_orphan", async () => {
    // The orphan filter matches case-insensitively, so a hand-edited key
    // `"Ghost"` for a discovered agent `ghost` is NOT an orphan (it is the
    // agent's stored override) — and the agent's row shows the stored value
    // (no longer in the catalog → its own disabled option), not the placeholder.
    vi.mocked(getSettings).mockResolvedValueOnce({
      ...baseSettings,
      subagentModels: { Ghost: "p/m1" },
    });
    vi.mocked(listAgentDefinitions).mockResolvedValueOnce([
      {
        name: "ghost",
        description: "Fast recon",
        model: "p/m1",
        scope: "user",
      },
    ]);
    render(<SettingsPage onBack={vi.fn()} />);
    await loaded();
    await go("Subagents");
    // No orphan row for the case-variant key (it matches the def).
    expect(screen.queryByText("Ghost")).toBeNull();
    expect(
      screen.queryByText("No longer discovered (stale override)"),
    ).toBeNull();
    // The def's row shows the stored value (the case-insensitive lookup
    // finds it — an exact-case lookup would show the placeholder).
    const trigger = await screen.findByRole("button", {
      name: "Subagent model for ghost",
    });
    expect(trigger.textContent).toContain("p/m1");
    expect(trigger.textContent).not.toContain("File value (no override)");
  });

  it("saving_drops_case_variant_twin_keys", async () => {
    // A case-variant twin (`"Scout"` alongside `"scout"` — a pre-fix save could
    // create one): picking a model must leave EXACTLY ONE key for the agent
    // (the canonical `def.name` key) — the case-variant twin is dropped, so the
    // backend's case-insensitive `.find` never picks between twins in
    // nondeterministic HashMap iteration order.
    vi.mocked(getSettings).mockResolvedValueOnce({
      ...baseSettings,
      subagentModels: { Scout: "gone/m1", scout: "p/m1" },
    });
    vi.mocked(listAgentDefinitions).mockResolvedValueOnce([
      {
        name: "scout",
        description: "Fast recon",
        model: "p/m1",
        scope: "user",
      },
    ]);
    render(<SettingsPage onBack={vi.fn()} />);
    await loaded();
    await go("Subagents");
    const trigger = await screen.findByRole("button", {
      name: "Subagent model for scout",
    });
    // Pick a different catalog model.
    fireEvent.click(trigger);
    fireEvent.click(await screen.findByText("GPT-5"));
    await waitFor(() =>
      expect(saveSettings).toHaveBeenLastCalledWith(
        expect.objectContaining({ subagentModels: { scout: "openai/GPT-5" } }),
      ),
    );
    const saved = vi.mocked(saveSettings).mock.calls[
      vi.mocked(saveSettings).mock.calls.length - 1
    ]?.[0] as AppSettings;
    // EXACTLY ONE key for the agent — the canonical `def.name` key (no
    // `Scout` twin left behind).
    expect(Object.keys(saved.subagentModels)).toEqual(["scout"]);
    expect("Scout" in saved.subagentModels).toBe(false);
  });

  it("a_non_ascii_case_variant_key_is_not_matched", async () => {
    // The backend folds ASCII A–Z only (`eq_ignore_ascii_case`) — a non-ASCII
    // case pair (a hand-edited key `"É-claude"` for a discovered agent
    // `é-claude`) is NOT matched by the backend, so the UI must not claim the
    // stored value either (a Unicode `toLowerCase()` fold would — the two
    // foldings agree on ASCII input only, and the divergence is reachable
    // only with non-ASCII case pairs). Under the ASCII-only fold the key
    // matches no def, so it is ALSO an orphan (a muted row with the remove
    // button — the only cleanup path).
    vi.mocked(getSettings).mockResolvedValueOnce({
      ...baseSettings,
      subagentModels: { "É-claude": "p/m1" },
    });
    vi.mocked(listAgentDefinitions).mockResolvedValueOnce([
      {
        name: "é-claude",
        description: "Fast recon",
        model: null,
        scope: "user",
      },
    ]);
    render(<SettingsPage onBack={vi.fn()} />);
    await loaded();
    await go("Subagents");
    // The def's row shows the PLACEHOLDER (the ASCII-only fold does not
    // match the non-ASCII pair — mirroring the backend, which ignores the
    // key), NOT the stored value (a `toLowerCase()` fold would show it).
    const trigger = await screen.findByRole("button", {
      name: "Subagent model for é-claude",
    });
    expect(trigger.textContent).toContain("File value (no override)");
    // The key matches no def under the ASCII-only fold → a muted orphan
    // row (the stale copy).
    expect(await screen.findByText("É-claude")).toBeTruthy();
    expect(
      screen.getByText("No longer discovered (stale override)"),
    ).toBeTruthy();
  });
});

describe("SettingsPage (the known-providers picker + the provider api field — ADR 0024)", () => {
  it("picker_renders_the_20_known_providers", async () => {
    render(<SettingsPage onBack={vi.fn()} />);
    await loaded();
    await go("Providers");
    // Radix `Select` options render only when the popover is OPEN.
    const trigger = screen.getByRole("combobox", { name: "Known provider" });
    fireEvent.click(trigger);
    // The 20 catalog entries + the leading (disabled) placeholder.
    const options = await screen.findAllByRole("option");
    expect(options).toHaveLength(21);
    expect(options[0].textContent).toBe("Add a known provider…");
    // The label is `name — apiLabel(api)` (the wire's human label).
    expect(
      screen.getByRole("option", { name: "Anthropic — Anthropic" }),
    ).toBeTruthy();
    expect(
      screen.getByRole("option", { name: "OpenAI — OpenAI Responses" }),
    ).toBeTruthy();
  });

  it("picker_adds_a_prefilled_row", async () => {
    render(<SettingsPage onBack={vi.fn()} />);
    await loaded();
    await go("Providers");
    const trigger = screen.getByRole("combobox", { name: "Known provider" });
    // Pick the `anthropic` template.
    fireEvent.click(trigger);
    fireEvent.click(
      await screen.findByRole("option", { name: "Anthropic — Anthropic" }),
    );
    // The Add button is enabled (a template is selected) → the pre-filled
    // row (name / base URL / wire / key URL, EMPTY key — the user pastes
    // the key) is saved via the normal `update` path (immediate save); the
    // id is the slug of the template's name.
    fireEvent.click(screen.getByRole("button", { name: "Add" }));
    await waitFor(() =>
      expect(saveSettings).toHaveBeenCalledWith(
        expect.objectContaining({
          providers: expect.arrayContaining([
            expect.objectContaining({
              id: "anthropic",
              name: "Anthropic",
              baseUrl: "https://api.anthropic.com/v1",
              api: "anthropic-messages",
              apiKey: "",
              keyUrl: "https://console.anthropic.com/settings/keys",
            }),
          ]),
        }),
      ),
    );
    // The picker resets to the placeholder.
    expect(trigger.textContent).toContain("Add a known provider…");
  });

  it("add_provider_button_still_adds_an_empty_row", async () => {
    render(<SettingsPage onBack={vi.fn()} />);
    await loaded();
    await go("Providers");
    // The existing `Add provider` button (no picker selection): an empty
    // row with the wire defaults (`openai-completions` / `keyUrl: null`).
    fireEvent.click(screen.getByRole("button", { name: "Add provider" }));
    await waitFor(() =>
      expect(saveSettings).toHaveBeenCalledWith(
        expect.objectContaining({
          providers: expect.arrayContaining([
            expect.objectContaining({
              name: "",
              baseUrl: "",
              apiKey: "",
              api: "openai-completions",
              keyUrl: null,
            }),
          ]),
        }),
      ),
    );
  });

  it("provider_row_api_select_commits_api", async () => {
    // A row on the `openai-completions` wire (the default).
    vi.mocked(getSettings).mockResolvedValueOnce({
      ...baseSettings,
      providers: [
        {
          id: "tama",
          name: "Tama",
          baseUrl: "https://tama.wizards.town/v1",
          apiKey: "k",
          api: "openai-completions",
          keyUrl: null,
        },
      ],
    });
    render(<SettingsPage onBack={vi.fn()} />);
    await loaded();
    await go("Providers");
    // Switch the wire to `anthropic-messages` (the three-wire select).
    const trigger = await screen.findByRole("combobox", { name: "API" });
    fireEvent.click(trigger);
    fireEvent.click(await screen.findByRole("option", { name: "Anthropic" }));
    // Immediate save (the `commitProviderField` pattern — the patch is a
    // `Partial<ProviderConfig>`).
    await waitFor(() =>
      expect(saveSettings).toHaveBeenCalledWith(
        expect.objectContaining({
          providers: expect.arrayContaining([
            expect.objectContaining({ api: "anthropic-messages" }),
          ]),
        }),
      ),
    );

    // Switch the wire again — to `litellm` (the discovery mode, ADR 0026).
    fireEvent.click(trigger);
    fireEvent.click(await screen.findByRole("option", { name: "LiteLLM" }));
    await waitFor(() =>
      expect(saveSettings).toHaveBeenCalledWith(
        expect.objectContaining({
          providers: expect.arrayContaining([
            expect.objectContaining({ api: "litellm" }),
          ]),
        }),
      ),
    );
  });

  it("provider_row_api_select_offers_litellm", async () => {
    // A row on the `openai-completions` wire (the default) — the select
    // must offer the `litellm` api as a fourth option.
    vi.mocked(getSettings).mockResolvedValueOnce({
      ...baseSettings,
      providers: [
        {
          id: "tama",
          name: "Tama",
          baseUrl: "https://tama.wizards.town/v1",
          apiKey: "k",
          api: "openai-completions",
          keyUrl: null,
        },
      ],
    });
    render(<SettingsPage onBack={vi.fn()} />);
    await loaded();
    await go("Providers");
    const trigger = await screen.findByRole("combobox", { name: "API" });
    fireEvent.click(trigger);
    await screen.findByRole("option", { name: "LiteLLM" }); // the await is the assertion (throws if absent)
  });

  it("provider_row_get_key_link_renders_when_key_url_set", async () => {
    // A row with a `keyUrl` (a template's key-management page): the
    // "Get key" link beside the API key field.
    vi.mocked(getSettings).mockResolvedValueOnce({
      ...baseSettings,
      providers: [
        {
          id: "anthropic",
          name: "Anthropic",
          baseUrl: "https://api.anthropic.com/v1",
          apiKey: "",
          api: "anthropic-messages",
          keyUrl: "https://x",
        },
      ],
    });
    render(<SettingsPage onBack={vi.fn()} />);
    await loaded();
    await go("Providers");
    const link = await screen.findByRole("link", { name: "Get key" });
    expect(link.getAttribute("href")).toBe("https://x");

    // A row with `keyUrl: null` (a hand-typed provider): NO "Get key" link.
    vi.mocked(getSettings).mockResolvedValueOnce({
      ...baseSettings,
      providers: [
        {
          id: "tama",
          name: "Tama",
          baseUrl: "https://tama.wizards.town/v1",
          apiKey: "k",
          api: "openai-completions",
          keyUrl: null,
        },
      ],
    });
    // Drop the first render (two renders would leave two "Providers"
    // sidebar buttons — the `go` helper's `getByRole` would be ambiguous).
    cleanup();
    render(<SettingsPage onBack={vi.fn()} />);
    await loaded();
    await go("Providers");
    // The row is rendered (the name field) — and there is no "Get key" link.
    await screen.findByDisplayValue("Tama");
    expect(screen.queryByRole("link", { name: "Get key" })).toBeNull();
  });
});

describe("Appearance: palette", () => {
  // (ADR 0027) The palette is a second, orthogonal axis beside Theme. The UI
  // stores the default as the `null` sentinel (like `spinnerStyle` / the font
  // families), so a Zai selection leaves `settings.json` minimal — the shape a
  // pre-feature file already has.
  it("offers Zai and Dracula, with Zai preselected for a `null` palette", async () => {
    render(<SettingsPage onBack={vi.fn()} />);
    await loaded();
    await go("Appearance");
    const trigger = await screen.findByRole("combobox", { name: "Palette" });
    fireEvent.click(trigger);
    expect(screen.getByRole("option", { name: "Zai (default)" })).toBeTruthy();
    expect(screen.getByRole("option", { name: "Dracula" })).toBeTruthy();
    // The row states the pin (Dracula overrides the Theme setting).
    expect(
      screen.getByText(/Dracula is dark-only .* overrides the Theme setting/i),
    ).toBeTruthy();
  });

  it("saves Dracula as the palette, and Zai as the null default", async () => {
    render(<SettingsPage onBack={vi.fn()} />);
    await loaded();
    await go("Appearance");
    fireEvent.click(await screen.findByRole("combobox", { name: "Palette" }));
    fireEvent.click(screen.getByRole("option", { name: "Dracula" }));
    await waitFor(() => expect(saveSettings).toHaveBeenCalledTimes(1));
    const saved = vi.mocked(saveSettings).mock.calls[0][0] as AppSettings;
    expect(saved.palette).toBe("dracula");
    // The complete document (the other fields intact — the mode unchanged).
    expect(saved.theme).toBe("dark");
    expect(saved.font).toEqual(baseSettings.font);
  });

  it("saves Zai as the null sentinel, not as the string zai", async () => {
    // The OTHER direction, which was asserted nowhere: the select's option value
    // is the `"zai"` SENTINEL, and `onValueChange` must translate it to `null`
    // on the way out (the same pattern as `spinnerStyle` and the font families)
    // so a default palette leaves `settings.json` minimal and a pre-feature file
    // (no `palette` key at all) and an explicit Zai choice stay the SAME
    // document. Save `"zai"` instead of `null` and the two documents fork — the
    // file grows a key that means nothing, and any other reader of `palette`
    // that only knows `null` as "default" now disagrees with the UI.
    //
    // Reached from a document that is ALREADY Dracula: starting from `null` the
    // select is already on Zai, Radix fires no change, and the test would assert
    // nothing.
    vi.mocked(getSettings).mockResolvedValueOnce({
      ...baseSettings,
      palette: "dracula",
    });
    render(<SettingsPage onBack={vi.fn()} />);
    await loaded();
    await go("Appearance");
    fireEvent.click(await screen.findByRole("combobox", { name: "Palette" }));
    fireEvent.click(
      await screen.findByRole("option", { name: "Zai (default)" }),
    );
    await waitFor(() => expect(saveSettings).toHaveBeenCalledTimes(1));
    const saved = vi.mocked(saveSettings).mock.calls[0][0] as AppSettings;
    expect(saved.palette).toBeNull();
    expect(saved.palette).not.toBe("zai");
    // And it is still a COMPLETE document — the sentinel must not be sent alone.
    expect(saved.theme).toBe(baseSettings.theme);
    expect(saved.providers).toEqual(baseSettings.providers);
  });

  it("shows the stored palette in the trigger for both values", async () => {
    // The trigger is what the user reads as "which palette am I on", and it is
    // driven by `settings.palette ?? "zai"` — so the `null` default has to render
    // a LABEL, not nothing (the failure mode the font pickers document for an
    // empty-string Radix value: `SelectValue` renders nothing and the control
    // goes blank).
    for (const [palette, expected] of [
      [null, "Zai (default)"],
      ["dracula", "Dracula"],
    ] as const) {
      vi.mocked(getSettings).mockResolvedValueOnce({
        ...baseSettings,
        palette,
      });
      const { unmount } = render(<SettingsPage onBack={vi.fn()} />);
      await loaded();
      await go("Appearance");
      const trigger = await screen.findByRole("combobox", { name: "Palette" });
      expect(
        trigger.querySelector("[data-slot=select-value]")?.textContent,
        `palette: ${JSON.stringify(palette)}`,
      ).toBe(expected);
      unmount();
      cleanup();
    }
  });
});

describe("Appearance: thinking spinner", () => {
  it("offers every braille variant as a live preview (the default typing is preselected)", async () => {
    render(<SettingsPage onBack={vi.fn()} />);
    await loaded();
    await go("Appearance");
    // Every variant in the gallery is offered, named by its humanized name
    // (the picker's `aria-label`), with a live `BrailleLoader` preview inside.
    for (const variant of brailleLoaderVariants) {
      expect(
        screen.getByRole("radio", { name: humanizeVariant(variant) }),
      ).toBeTruthy();
    }
    // The default (`typing` — the fixture's `spinnerStyle` is null) is
    // preselected; the others are not.
    const typing = screen.getByRole("radio", {
      name: humanizeVariant("typing"),
    }) as HTMLButtonElement;
    expect(typing.getAttribute("aria-checked")).toBe("true");
    const pendulum = screen.getByRole("radio", {
      name: humanizeVariant("pendulum"),
    }) as HTMLButtonElement;
    expect(pendulum.getAttribute("aria-checked")).toBe("false");
  });

  it("saves the picked spinner style (immediate save, complete document)", async () => {
    render(<SettingsPage onBack={vi.fn()} />);
    await loaded();
    await go("Appearance");
    fireEvent.click(
      screen.getByRole("radio", { name: humanizeVariant("pendulum") }),
    );
    await waitFor(() => expect(saveSettings).toHaveBeenCalledTimes(1));
    const saved = vi.mocked(saveSettings).mock.calls[0][0] as AppSettings;
    expect(saved.spinnerStyle).toBe("pendulum");
    // The complete document (the other fields intact).
    expect(saved.theme).toBe("dark");
    expect(saved.font).toEqual(baseSettings.font);
  });
});

describe("a stored value the option list does not offer", () => {
  // The settings document is user-editable and the backend round-trips
  // `palette` / `theme` / the font families UNVALIDATED, so any string can
  // reach these selects. Radix renders NOTHING for a value that has no
  // matching item, so an out-of-vocabulary value used to blank the trigger —
  // the user could not see what was active, and could not find their way back
  // to an option. These cases pin the legible rendering; they deliberately
  // assert that merely OPENING the page saves nothing (viewing the page must
  // not rewrite the user's file).
  /** The text the trigger's value slot renders (the picker's "what is
   * active" readout — the same probe the other trigger tests use). */
  function triggerText(name: string): string | undefined {
    return screen
      .getByRole("combobox", { name })
      .querySelector("[data-slot=select-value]")?.textContent;
  }

  it("renders the Zai default for a palette outside the vocabulary, not a blank trigger", async () => {
    // `"solarized"` is the reported defect. The app applies Zai for anything
    // it does not recognise (`applySettingsTheme` matches `palette ===
    // "dracula"` exactly), so "Zai (default)" is not merely a placeholder — it
    // is the truth about what is on screen, and the option the user can pick
    // to get back to a clean document.
    vi.mocked(getSettings).mockResolvedValueOnce({
      ...baseSettings,
      palette: "solarized" as AppSettings["palette"],
    });
    render(<SettingsPage onBack={vi.fn()} />);
    await loaded();
    await go("Appearance");
    const trigger = await screen.findByRole("combobox", { name: "Palette" });
    expect(trigger.querySelector("[data-slot=select-value]")?.textContent).toBe(
      "Zai (default)",
    );
    // And the default is the SELECTED option — the way back is visible.
    fireEvent.click(trigger);
    const zai = await screen.findByRole("option", { name: "Zai (default)" });
    expect(zai.getAttribute("data-state")).toBe("checked");
    fireEvent.keyDown(trigger, { key: "Escape" });
    // Viewing the page rewrote nothing.
    expect(saveSettings).not.toHaveBeenCalled();
  });

  it("renders the Zai default for a blank palette (Radix renders nothing for an empty value)", async () => {
    // A hand-edited `"palette": ""`. Blank means absent (the same rule the
    // blank `defaultThinkingLevel` case establishes), so it reads as the
    // default rather than as a blank control.
    vi.mocked(getSettings).mockResolvedValueOnce({
      ...baseSettings,
      palette: "" as AppSettings["palette"],
    });
    render(<SettingsPage onBack={vi.fn()} />);
    await loaded();
    await go("Appearance");
    await screen.findByRole("combobox", { name: "Palette" });
    expect(triggerText("Palette")).toBe("Zai (default)");
  });

  it("renders the mode an out-of-vocabulary theme actually applies, not a blank trigger", async () => {
    // The Theme picker has the identical blank-trigger hole (`"sepia"` used to
    // render `""`). Its fallback is NOT the field's documented default (`dark`)
    // but `light`, because that is what the resolver really applies: every
    // value outside the vocabulary falls through `resolveTheme`'s final branch
    // (`theme === "dark" ? zai-dark : zai-light`). A label of "Dark" would have
    // described the default while the window rendered light — a lie of the same
    // kind as the blank it replaces.
    vi.mocked(getSettings).mockResolvedValueOnce({
      ...baseSettings,
      theme: "sepia" as AppSettings["theme"],
    });
    render(<SettingsPage onBack={vi.fn()} />);
    await loaded();
    await go("Appearance");
    const trigger = await screen.findByRole("combobox", { name: "Theme" });
    expect(trigger.querySelector("[data-slot=select-value]")?.textContent).toBe(
      "Light",
    );
    fireEvent.click(trigger);
    expect(
      (await screen.findByRole("option", { name: "Light" })).getAttribute(
        "data-state",
      ),
    ).toBe("checked");
    fireEvent.keyDown(trigger, { key: "Escape" });
    expect(saveSettings).not.toHaveBeenCalled();
  });

  it("shows an off-list font family in the trigger as its own option (both pickers)", async () => {
    // The font pickers share the blank-trigger symptom, but their vocabulary is
    // OPEN: `applySettingsFont` splices ANY non-null family into the CSS stack,
    // so a hand-edited `"Georgia"` is genuinely applied. Coercing it to "Default
    // (Noto Sans)" would misreport the font on screen, so the stored value is
    // surfaced as its own option — the app's established pattern for a stored
    // value outside a select's list (the default-thinking-level and
    // subagent-model pickers).
    vi.mocked(getSettings).mockResolvedValueOnce({
      ...baseSettings,
      font: { sizePx: 14, uiFamily: "Georgia", codeFamily: "Iosevka" },
    });
    render(<SettingsPage onBack={vi.fn()} />);
    await loaded();
    await go("Appearance");
    const uiTrigger = await screen.findByRole("combobox", { name: "UI font" });
    expect(
      uiTrigger.querySelector("[data-slot=select-value]")?.textContent,
    ).toBe("Georgia");
    // It is a real, selected option — and the list still offers the default, so
    // the user can leave the off-list family.
    fireEvent.click(uiTrigger);
    const georgia = await screen.findByRole("option", { name: "Georgia" });
    expect(georgia.getAttribute("data-state")).toBe("checked");
    expect(
      screen.getByRole("option", { name: "Default (Noto Sans)" }),
    ).toBeTruthy();
    // Picking an offered option saves it normally (the escape hatch works).
    fireEvent.click(screen.getByRole("option", { name: "Fira Sans" }));
    await waitFor(() => expect(saveSettings).toHaveBeenCalledTimes(1));
    expect(
      (vi.mocked(saveSettings).mock.calls[0][0] as AppSettings).font.uiFamily,
    ).toBe('"Fira Sans"');
    cleanup();
    // The code font, same treatment (a second render: an open Radix Select
    // `aria-hidden`s the rest of the document).
    vi.mocked(getSettings).mockResolvedValueOnce({
      ...baseSettings,
      font: { sizePx: 14, uiFamily: null, codeFamily: "Iosevka" },
    });
    render(<SettingsPage onBack={vi.fn()} />);
    await loaded();
    await go("Appearance");
    const codeTrigger = await screen.findByRole("combobox", {
      name: "Code font",
    });
    expect(
      codeTrigger.querySelector("[data-slot=select-value]")?.textContent,
    ).toBe("Iosevka");
  });

  it("treats a blank stored font family as absent (the default label, one option)", async () => {
    // `"uiFamily": ""` is not a font; it must read as the default (and must not
    // add a second empty-valued option, which Radix rejects).
    vi.mocked(getSettings).mockResolvedValueOnce({
      ...baseSettings,
      font: { sizePx: 14, uiFamily: "", codeFamily: null },
    });
    render(<SettingsPage onBack={vi.fn()} />);
    await loaded();
    await go("Appearance");
    const trigger = await screen.findByRole("combobox", { name: "UI font" });
    expect(trigger.querySelector("[data-slot=select-value]")?.textContent).toBe(
      "Default (Noto Sans)",
    );
    fireEvent.click(trigger);
    expect(saveSettings).not.toHaveBeenCalled();
  });

  it("labels a provider api outside the wire list as the wire it actually speaks", async () => {
    // The same class of hole, in the Providers section: a hand-edited
    // `"api": "grpc-gateway"` blanked the row's API select. Both consumers
    // parse the api leniently (`WireApi::parse(..).unwrap_or(OpenAi
    // Completions)` for the wire, ditto discovery), so an unrecognised api
    // really does speak the OpenAI-compatible protocol — that is the label
    // shown, and the option the user can move the row to.
    vi.mocked(getSettings).mockResolvedValueOnce({
      ...baseSettings,
      providers: [
        {
          ...baseSettings.providers[0],
          api: "grpc-gateway" as AppSettings["providers"][number]["api"],
        },
      ],
    });
    render(<SettingsPage onBack={vi.fn()} />);
    await loaded();
    await go("Providers");
    const trigger = await screen.findByRole("combobox", { name: "API" });
    expect(trigger.querySelector("[data-slot=select-value]")?.textContent).toBe(
      "OpenAI-compatible",
    );
    fireEvent.click(trigger);
    expect(
      (
        await screen.findByRole("option", { name: "OpenAI-compatible" })
      ).getAttribute("data-state"),
    ).toBe("checked");
    // Every offered wire is still there to switch to.
    expect(screen.getByRole("option", { name: "Anthropic" })).toBeTruthy();
    fireEvent.keyDown(trigger, { key: "Escape" });
    expect(saveSettings).not.toHaveBeenCalled();
  });
});
