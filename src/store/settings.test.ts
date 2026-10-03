import { beforeEach, describe, expect, it, vi } from "vitest";

import { getSettings } from "../lib/tauri";
import type { AppSettings } from "../lib/tauri";
import { useSettings } from "./settings";

vi.mock("../lib/tauri", async () => {
  const actual = await vi.importActual<typeof import("../lib/tauri")>(
    "../lib/tauri",
  );
  return { ...actual, getSettings: vi.fn() };
});

const mockGetSettings = vi.mocked(getSettings);

/** A full settings fixture (every field — the store carries the whole shape). */
const FULL_SETTINGS: AppSettings = {
  theme: "dark",
  paneLayout: {},
  defaultTrustNewSpaces: false,
  defaultModel: null,
  defaultThinkingLevel: null,
  enabledTools: [],
  providers: [
    {
      id: "my-gateway",
      name: "My Gateway",
      baseUrl: "http://localhost:8080/v1",
      apiKey: "sk-test",
      api: "openai-completions",
    },
  ],
  mcpServers: {},
  font: { sizePx: 14, uiFamily: null, codeFamily: null },
  defaultThinkingLevels: {},
  subagentModels: {},
  spinnerStyle: null,
};

describe("useSettings store", () => {
  beforeEach(() => {
    useSettings.setState({ settings: null, loaded: false });
    mockGetSettings.mockReset();
  });

  it("loadSettings stores the backend settings and marks loaded", async () => {
    mockGetSettings.mockResolvedValue(FULL_SETTINGS);

    await useSettings.getState().loadSettings();

    expect(useSettings.getState().settings).toBe(FULL_SETTINGS);
    expect(useSettings.getState().loaded).toBe(true);
  });

  it("loadSettings tolerates a getSettings failure (settings stay null, loaded marks the attempt)", async () => {
    mockGetSettings.mockRejectedValue(new Error("backend down"));

    await useSettings.getState().loadSettings();

    expect(useSettings.getState().settings).toBeNull();
    expect(useSettings.getState().loaded).toBe(true);
  });

  it("setSettings overwrites the current settings (the settings UI's optimistic write)", () => {
    const next: AppSettings = { ...FULL_SETTINGS, spinnerStyle: "pendulum" };

    useSettings.getState().setSettings(next);

    expect(useSettings.getState().settings).toBe(next);
    expect(useSettings.getState().settings?.spinnerStyle).toBe("pendulum");
  });
});
