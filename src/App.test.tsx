import { beforeAll, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import App from "./App";

// Mock the Tauri IPC layer (the `importActual` pattern from
// `NewSpaceDialog.test.tsx`): the boot `useEffect` loads `listSessions` /
// `listSpaces` (resolving `[]` so the workspace renders its empty state),
// every `listen*` registration resolves a no-op unlisten (the App's
// listener `useEffect` awaits them in cleanup), and `listSkills` resolves
// `[]` (the `useSkillCatalog` hook in both `SpacesList` and `ChatStream`
// calls it on mount — the hook catches rejections, but the mock avoids
// noisy failures with a bare `vi.mock` factory).
vi.mock("./lib/tauri", async () => {
  const actual = await vi.importActual<Record<string, unknown>>("./lib/tauri");
  return {
    ...actual,
    listSessions: vi.fn().mockResolvedValue([]),
    listSpaces: vi.fn().mockResolvedValue([]),
    listSkills: vi.fn().mockResolvedValue([]),
    // `SettingsPage`'s data deps: the settings document + the agent/model
    // catalogs.
    getSettings: vi.fn().mockResolvedValue({
      theme: "dark",
      paneLayout: {},
      defaultAgent: null,
      defaultTrustNewSpaces: false,
      defaultModel: null,
      providers: [],
      font: { sizePx: 14, uiFamily: null, codeFamily: null },
    }),
    saveSettings: vi.fn().mockResolvedValue(undefined),
    listAgents: vi.fn().mockResolvedValue([]),
    listModels: vi.fn().mockResolvedValue([]),
    // Every `listen*` registration resolves a no-op unlisten.
    listenSessionUpdate: vi.fn().mockResolvedValue(() => {}),
    listenSessionClosed: vi.fn().mockResolvedValue(() => {}),
    listenPermissionRequest: vi.fn().mockResolvedValue(() => {}),
    listenBridgeRequest: vi.fn().mockResolvedValue(() => {}),
    listenBridgeRequestClose: vi.fn().mockResolvedValue(() => {}),
    listenBridgeEvent: vi.fn().mockResolvedValue(() => {}),
    listenSubagentSessionStarted: vi.fn().mockResolvedValue(() => {}),
    listenSubagentClosed: vi.fn().mockResolvedValue(() => {}),
  };
});

// `App` renders `WindowControls`, which calls `getCurrentWindow()` from
// `@tauri-apps/api/window` on EVERY render (its own test file documents
// this and mocks the module); in jsdom `window.__TAURI_INTERNALS__` is
// undefined → `TypeError` during render → every case fails without it.
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    minimize: vi.fn(),
    toggleMaximize: vi.fn(),
    close: vi.fn(),
  }),
}));

beforeAll(() => {
  vi.stubGlobal(
    "matchMedia",
    (q: string) => ({
      matches: false,
      media: q,
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
      addListener: vi.fn(),
      removeListener: vi.fn(),
      dispatchEvent: vi.fn(),
    }),
  );
});

describe("App (the gear icon + settings view swap)", () => {
  it("the_gear_icon_swaps_to_the_settings_view", () => {
    render(<App />);
    // The workspace is visible (the `SpacesList`'s "Sessions" heading).
    expect(screen.getByText("Sessions")).toBeTruthy();
    // The gear icon (the left sidebar footer) opens the settings view.
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    // The settings view is visible (the `SettingsPage`'s section nav —
    // it is the left edge; the `SpacesList` is NOT rendered in the
    // settings view).
    expect(screen.getByText("General")).toBeTruthy();
    expect(screen.getByText("Appearance")).toBeTruthy();
    expect(screen.getByText("Providers")).toBeTruthy();
    // ...and the workspace is gone (it UNMOUNTS — the stores are the
    // source of truth and re-hydrate on remount, so the ZCode
    // `opacity-0` + `inert` pattern is not needed).
    expect(screen.queryByText("Sessions")).toBeNull();
  });

  it("the_back_button_returns_to_the_workspace", () => {
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    expect(screen.getByText("General")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Back" }));
    expect(screen.getByText("Sessions")).toBeTruthy();
  });
});
