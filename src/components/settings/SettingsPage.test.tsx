import { describe, expect, it, vi, beforeAll, afterEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import SettingsPage from "./SettingsPage";
import {
  authMcpServer,
  getSettings,
  listModels,
  saveSettings,
  testMcpServer,
  type AppSettings,
} from "@/lib/tauri";

// The page's single source of truth (the `getSettings` fixture): a full
// `AppSettings` document — dark theme, font defaults, one provider, one
// MCP server.
const baseSettings: AppSettings = {
  theme: "dark",
  paneLayout: {},
  defaultTrustNewSpaces: false,
  defaultModel: null,
  defaultThinkingLevel: null,
  enabledTools: [],
  providers: [
    { id: "tama", name: "Tama", baseUrl: "https://tama.wizards.town/v1", apiKey: "k" },
  ],
  mcpServers: {
    tama: { url: "https://tama/mcp" },
  },
  font: { sizePx: 14, uiFamily: null, codeFamily: null },
  defaultThinkingLevels: {},
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
      defaultTrustNewSpaces: false,
      defaultModel: null,
      defaultThinkingLevel: null,
      enabledTools: [],
      providers: [
        { id: "tama", name: "Tama", baseUrl: "https://tama.wizards.town/v1", apiKey: "k" },
      ],
      mcpServers: {
        tama: { url: "https://tama/mcp" },
      },
      font: { sizePx: 14, uiFamily: null, codeFamily: null },
      defaultThinkingLevels: {},
    }),
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
    // The native harness's tool names (the enabled-tools checkbox list).
    listTools: vi.fn().mockResolvedValue(["bash", "read", "write", "edit", "subagent"]),
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
async function go(section: "Appearance" | "Providers" | "MCP"): Promise<void> {
  fireEvent.click(screen.getByRole("button", { name: section }));
  if (section === "Appearance") await screen.findByText("Theme");
  else if (section === "Providers")
    await screen.findByRole("button", { name: "Add provider" });
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
    expect(screen.getByRole("button", { name: "Remove provider" })).toBeTruthy();
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
    expect(await screen.findByRole("option", { name: "Model default" })).toBeTruthy();
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

  it("the_default_agent_select_is_gone", async () => {
    render(<SettingsPage onBack={vi.fn()} />);
    await loaded();
    // The removed UI: there is one harness — no agent to choose, so the
    // "Default agent" control is absent (and the agent-catalog data source
    // is gone from the module — nothing to fetch).
    expect(screen.queryByText("Default agent")).toBeNull();
    expect(screen.queryByRole("combobox", { name: "Default agent" })).toBeNull();
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
    fireEvent.change(urlInput, { target: { value: "https://gw.example.com/mcp" } });
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
    expect(testMcpServer).toHaveBeenCalledWith({ url: "https://tama/mcp" }, "tama");
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
