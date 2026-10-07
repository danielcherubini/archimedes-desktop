import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen } from "@testing-library/react";
import App from "./App";
import { getLeftPaneCollapsed, LEFT_PANE_RAIL, setLeftPaneCollapsed } from "./lib/leftPaneState";
import { getSidePaneCollapsed, setSidePaneCollapsed } from "./lib/sidePaneState";

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
    // `SettingsPage`'s data deps: the settings document + the model catalog.
    getSettings: vi.fn().mockResolvedValue({
      theme: "dark",
      paneLayout: {},
      defaultTrustNewSpaces: false,
      defaultModel: null,
      defaultThinkingLevel: null,
      enabledTools: [],
      providers: [],
      mcpServers: {},
      font: { sizePx: 14, uiFamily: null, codeFamily: null },
      defaultThinkingLevels: {},
      subagentModels: {},
      spinnerStyle: null,
      filePolicy: { reads: "allow", writes: "allow", shell: "allow" },
    }),
    saveSettings: vi.fn().mockResolvedValue(undefined),
    listModels: vi.fn().mockResolvedValue([]),
    listTools: vi.fn().mockResolvedValue([]),
    // Every `listen*` registration resolves a no-op unlisten.
    listenSessionUpdate: vi.fn().mockResolvedValue(() => {}),
    listenSessionClosed: vi.fn().mockResolvedValue(() => {}),
    listenPermissionRequest: vi.fn().mockResolvedValue(() => {}),
    listenInteractiveRequest: vi.fn().mockResolvedValue(() => {}),
    listenInteractiveRequestClose: vi.fn().mockResolvedValue(() => {}),
    listenInteractiveEvent: vi.fn().mockResolvedValue(() => {}),
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
  // The pane collapsed flags are module-scoped: reset them between tests.
  beforeEach(() => {
    setLeftPaneCollapsed(false);
    setSidePaneCollapsed(false);
  });

  it("the header shows the app icon (app-icon-large) and NO 'Archimedes' text (the word was removed — icon only)", () => {
    render(<App />);
    // The top-left header-bar icon (the label is gone — icon only).
    const icon = screen.getByAltText("Archimedes");
    expect(icon.getAttribute("src")).toBe("/app-icon-large.png");
    expect(screen.queryByText("Archimedes")).toBeNull();
  });

  it("the chrome bar is the drag region (the whole bar — draggable + double-click to maximize; the inner buttons stay interactive)", () => {
    render(<App />);
    const chrome = document.querySelector('[data-testid="chrome-bar"]')!;
    // The attribute on the CONTAINER with the value "deep": Tauri's
    // drag-region script honors a BARE attribute only on the element a
    // mousedown lands on DIRECTLY (a click on a child of a bare-attribute
    // element does NOT drag — the bar's logo segment / tab spacer would
    // be dead zones). "deep" extends the region to the whole subtree: the
    // bar's empty areas drag the window (double-click maximizes) while the
    // inner buttons/tabs (interactive elements without the attribute) still
    // block it and stay clickable.
    expect(chrome.getAttribute("data-tauri-drag-region")).toBe("deep");
  });

  it("renders the Space tabs in the top chrome bar (browser-style — NOT in the center column)", () => {
    render(<App />);
    const tabs = screen.getByTestId("space-tabs");
    const chrome = document.querySelector('[data-testid="chrome-bar"]')!;
    // The tabs are a direct child of the chrome bar (the top row) — the
    // old position was the center column's top row (above the chat).
    expect(chrome.contains(tabs)).toBe(true);
    expect(tabs.parentElement).toBe(chrome);
    // …and the content row (the second child) does NOT contain them.
    const content = document.querySelector('[data-testid="content-row"]')!;
    expect(content.contains(tabs)).toBe(false);
  });

  it("the chrome bar's logo segment follows the sidebar's width (the tabs start where the center column begins)", () => {
    render(<App />);
    const logo = screen.getByAltText("Archimedes").parentElement!;
    // Expanded sidebar (260px): the logo segment is 260px wide — the
    // tabs start at the center column's left edge.
    expect(logo.style.width).toBe("260px");
    // Collapsed sidebar (width 0 — the chrome bar's button is the
    // re-expand control, so no rail is needed): the segment follows
    // (0px — the icon is clipped away with it).
    act(() => {
      setLeftPaneCollapsed(true);
    });
    expect(logo.style.width).toBe(`${LEFT_PANE_RAIL}px`);
  });

  it("the chrome bar NEVER holds a pane control — expanded or collapsed (the toggles stay at the bottom of their panes in both states)", () => {
    render(<App />);
    const chrome = document.querySelector('[data-testid="chrome-bar"]')!;
    // Query by a STABLE hook, not the label: the label FLIPS with the state
    // (`Collapse sidebar` ↔ `Expand sidebar`) for screen-reader correctness,
    // so a name-based query would silently stop matching on one side of the
    // collapse — the exact vacuity this file has been bitten by twice.
    const TOGGLES = ["left-pane-toggle", "right-pane-toggle"];
    for (const id of TOGGLES) {
      expect(screen.getByTestId(id), `${id} belongs to the content row`).toBeTruthy();
      expect(chrome.contains(screen.getByTestId(id)), id).toBe(false);
    }
    // Collapsed: STILL nothing in the chrome bar. This is the regression this
    // pins — the collapsed state used to hand the control back to the top.
    act(() => {
      setLeftPaneCollapsed(true);
      setSidePaneCollapsed(true);
    });
    expect(chrome.querySelectorAll("[data-testid='left-pane-toggle']").length).toBe(0);
    expect(chrome.querySelectorAll("[data-testid='right-pane-toggle']").length).toBe(0);
    // …and each pane still exposes its OWN control at the bottom, collapsed,
    // now labelled for the direction it performs.
    expect(screen.getByTestId("left-pane-toggle").getAttribute("aria-label")).toBe("Expand sidebar");
    expect(screen.getByTestId("right-pane-toggle").getAttribute("aria-label")).toBe("Expand side pane");
    expect(chrome.querySelectorAll('[aria-label="Settings"]').length).toBe(0);
  });

  it("one toggle does both jobs from the same place, and its aria-label flips with the state", () => {
    render(<App />);
    const left = screen.getByTestId("left-pane-toggle");
    expect(left.getAttribute("aria-label")).toBe("Collapse sidebar");
    expect(left.getAttribute("aria-pressed")).toBe("true"); // pressed = open
    fireEvent.click(left);
    expect(getLeftPaneCollapsed()).toBe(true);
    // The SAME element — no second control to hand off to, and no keyboard
    // shortcut exists, so one control must do both jobs from the bottom.
    expect(screen.getByTestId("left-pane-toggle")).toBe(left);
    expect(left.getAttribute("aria-label")).toBe("Expand sidebar");
    expect(left.getAttribute("aria-pressed")).toBe("false");
    fireEvent.click(left);
    expect(getLeftPaneCollapsed()).toBe(false);
    expect(left.getAttribute("aria-label")).toBe("Collapse sidebar");
  });

  it("the right pane's toggle does both jobs from the same place too", () => {
    render(<App />);
    const right = screen.getByTestId("right-pane-toggle");
    expect(right.getAttribute("aria-label")).toBe("Collapse side pane");
    fireEvent.click(right);
    expect(getSidePaneCollapsed()).toBe(true);
    expect(screen.getByTestId("right-pane-toggle")).toBe(right);
    fireEvent.click(right);
    expect(getSidePaneCollapsed()).toBe(false);
  });

  it("the settings view renders NO tabs and NO collapse buttons in the chrome bar (the logo segment spans full width)", () => {
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    expect(screen.queryByTestId("space-tabs")).toBeNull();
    // EVERY pane-control name, expanded or collapsed: the old version of this
    // test queried a name that had been renamed, so it passed forever without
    // checking anything. A rename must be chased into these negatives too.
    for (const id of ["left-pane-toggle", "right-pane-toggle"]) {
      expect(screen.queryByTestId(id), id).toBeNull();
    }
    const logo = screen.getByAltText("Archimedes").parentElement!;
    // No inline width — the segment is `flex-1` (full width, the old
    // chrome-bar behavior), icon only (the word was removed).
    expect(logo.style.width).toBe("");
    expect(screen.queryByText("Archimedes")).toBeNull();
  });

  it("the tabs start 1px right of the sidebar's right edge (the center column's left edge — the chrome bar's left padding lives INSIDE the logo segment, not before it)", () => {
    render(<App />);
    const chrome = document.querySelector('[data-testid="chrome-bar"]')!;
    const tabs = screen.getByTestId("space-tabs");
    // The chrome bar has NO left padding (a `pl-*` here would offset the
    // logo segment — and the tabs after it — right of the sidebar's
    // 260px edge): the 12px icon margin lives INSIDE the logo segment.
    expect(chrome.className).not.toContain("pl-");
    expect(chrome.className).not.toContain("px-");
    const logo = screen.getByAltText("Archimedes").parentElement!;
    expect(logo.className).toContain("pl-3");
    // …and the tabs container has a 1px left offset (`pl-px` — the user's
    // "1px too far left" nudge: the first tab sits 1px right of the
    // segment's edge = the center column's left edge) and NO larger left
    // padding (a `pl-1`/`pl-2`/… would push it further right).
    expect(tabs.className).toContain("pl-px");
    expect(tabs.className).not.toContain("pl-1");
    expect(tabs.className).not.toContain("pl-2");
    expect(tabs.className).not.toContain("pl-3");
    expect(tabs.className).not.toContain("px-");
  });

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
