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
  defaultAgent: null,
  defaultTrustNewSpaces: false,
  defaultModel: null,
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
      defaultAgent: null,
      defaultTrustNewSpaces: false,
      defaultModel: null,
      providers: [
        { id: "tama", name: "Tama", baseUrl: "https://tama.wizards.town/v1", apiKey: "k" },
      ],
      mcpServers: {
        tama: { url: "https://tama/mcp" },
      },
      font: { sizePx: 14, uiFamily: null, codeFamily: null },
      defaultThinkingLevels: {},
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
  await screen.findByText("Default agent");
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
