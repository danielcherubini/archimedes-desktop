import { describe, expect, it, vi, beforeAll, afterEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import NewSpaceDialog from "./NewSpaceDialog";
import * as tauri from "../lib/tauri";
import type { AppSettings } from "../lib/tauri";

// The dialog's `getSettings` fixture default: a full `AppSettings` with
// `defaultAgent: null` (the existing tests' `agents[0]` behavior holds).
const baseSettings: AppSettings = {
  theme: "dark",
  paneLayout: {},
  defaultAgent: null,
  defaultTrustNewSpaces: false,
  defaultModel: null,
  providers: [],
  font: { sizePx: 14, uiFamily: null, codeFamily: null },
};

// Mock the Tauri IPC layer: the agent picker's data source is `list_agents`
// (the backend's `Registry::load` merge appends the built-in `archimedes`
// native entry AFTER the user entries — `agents[0]` stays the user's first
// entry, so `pi` remains the default).
vi.mock("../lib/tauri", async () => {
  const actual = await vi.importActual<Record<string, unknown>>("../lib/tauri");
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
      providers: [],
      font: { sizePx: 14, uiFamily: null, codeFamily: null },
    }),
    listAgents: vi.fn().mockResolvedValue([
      { id: "pi", name: "Pi" },
      { id: "archimedes", name: "Archimedes" },
    ]),
    spaceForPath: vi.fn().mockResolvedValue({ canonicalPath: "/tmp/ws", isSpace: false }),
    startSession: vi.fn().mockResolvedValue({
      sessionId: "native-1",
      agentId: "archimedes",
      cwd: "/tmp/ws",
      capabilities: { loadSession: true },
      configOptions: [
        {
          id: "model",
          name: "Model",
          type: "select",
          currentValue: "test/m1",
          options: [{ value: "test/m1", name: "m1" }],
        },
      ],
    }),
  };
});

// The directory picker plugin is unavailable outside Tauri (jsdom).
vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn().mockResolvedValue(null),
}));

beforeAll(() => {
  Element.prototype.scrollIntoView = vi.fn();
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

/** Type a cwd + validate it (the dialog's `changeCwd` flow — the Radix
 * Dialog portals to `document.body`, so query there). */
async function setFolder(path: string) {
  const input = document.querySelector("input[placeholder='/path/to/project']") as HTMLInputElement;
  fireEvent.change(input, { target: { value: path } });
  // `spaceForPath` resolves → the folder check clears the error.
  await waitFor(() => expect(tauri.spaceForPath).toHaveBeenCalled());
}

/** Wait for the registry fetch to land, then return the agent `<select>`. */
async function waitForSelect(): Promise<HTMLSelectElement> {
  await waitFor(() => expect(tauri.listAgents).toHaveBeenCalled());
  const select = document.querySelector("select") as HTMLSelectElement;
  expect(select).toBeTruthy();
  return select;
}

describe("NewSpaceDialog (the agent picker, native-agent-harness Task 7)", () => {
  it("shows the 'Archimedes' native agent entry (it flows via list_agents)", async () => {
    render(<NewSpaceDialog onClose={vi.fn()} />);
    // Both registry entries render (the native entry is a plain option —
    // the picker is unchanged; it is data-driven).
    await waitFor(() => expect(screen.getByRole("option", { name: "Pi" })).toBeTruthy());
    expect(screen.getByRole("option", { name: "Archimedes" })).toBeTruthy();
    // The native entry is SELECTABLE (opt-in): selecting it + starting
    // calls `start_session` with the native agent id. (The Radix Dialog
    // portals to `document.body` — query there, not the render container.)
    const select = document.querySelector("select") as HTMLSelectElement;
    fireEvent.change(select, { target: { value: "archimedes" } });
    await setFolder("/tmp/ws");
    fireEvent.click(screen.getByRole("button", { name: "Start" }));
    await waitFor(() =>
      expect(tauri.startSession).toHaveBeenCalledWith("archimedes", "/tmp/ws")
    );
  });

  it("keeps `pi` the default (agents[0] — the built-in is appended, never prepended)", async () => {
    render(<NewSpaceDialog onClose={vi.fn()} />);
    await waitFor(() => expect(tauri.listAgents).toHaveBeenCalled());
    const select = document.querySelector("select") as HTMLSelectElement;
    // No explicit selection: the effective agent is `agents[0]` — the
    // user's first entry (`pi`), NOT the built-in (a prepended built-in
    // would silently flip the default to native). The controlled select
    // displays the effective id.
    expect(select.value).toBe("pi");
    // Simulate the default start (no agent picked): `startSession` gets
    // `pi` (the effective id).
    const input = document.querySelector(
      "input[placeholder='/path/to/project']"
    ) as HTMLInputElement;
    fireEvent.change(input, { target: { value: "/tmp/ws" } });
    await waitFor(() => expect(tauri.spaceForPath).toHaveBeenCalled());
    fireEvent.click(screen.getByRole("button", { name: "Start" }));
    await waitFor(() => expect(tauri.startSession).toHaveBeenCalledWith("pi", "/tmp/ws"));
  });

  it("the_settings_default_agent_wins_over_agents_0", async () => {
    vi.mocked(tauri.getSettings).mockResolvedValueOnce({
      ...baseSettings,
      defaultAgent: "archimedes",
    });
    render(<NewSpaceDialog onClose={vi.fn()} />);
    const select = await waitForSelect();
    // `settings.defaultAgent` (in the registry) beats `agents[0]`.
    await waitFor(() => expect(select.value).toBe("archimedes"));
  });

  it("an_unknown_settings_default_agent_falls_back_to_agents_0", async () => {
    // The agent was removed from `agents.json` — `defaultAgent` is not in
    // the registry, so the effective selection falls back to `agents[0]`.
    vi.mocked(tauri.getSettings).mockResolvedValueOnce({
      ...baseSettings,
      defaultAgent: "gone",
    });
    render(<NewSpaceDialog onClose={vi.fn()} />);
    const select = await waitForSelect();
    await waitFor(() => expect(select.value).toBe("pi"));
  });

  it("has NO `get_models` command (the native model picker is the synthesized configOptions)", () => {
    // The native session's model picker is the EXISTING `SessionConfigSelect`
    // pipeline (SessionInfo.configOptions → setSessionConfigOption) — a
    // separate `get_models` command would branch the picker on session kind
    // (a UI change). The tauri surface must not gain one.
    expect("getModels" in tauri).toBe(false);
    expect("get_models" in tauri).toBe(false);
  });
});
