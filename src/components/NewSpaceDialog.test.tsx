import { describe, expect, it, vi, beforeAll, afterEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import NewSpaceDialog from "./NewSpaceDialog";
import * as tauri from "../lib/tauri";

// Mock the Tauri IPC layer: `start_session` is native-only (there is one
// harness — no agent to choose), so the invoke is `{ cwd }`.
vi.mock("../lib/tauri", async () => {
  const actual = await vi.importActual<Record<string, unknown>>("../lib/tauri");
  return {
    ...actual,
    spaceForPath: vi.fn().mockResolvedValue({ canonicalPath: "/tmp/ws", isSpace: false }),
    startSession: vi.fn().mockResolvedValue({
      sessionId: "native-1",
      cwd: "/tmp/ws",
      capabilities: { loadSession: true },
      archived: false,
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

describe("NewSpaceDialog (create the Space + start a native session)", () => {
  it("starts a native session in the picked folder (no agent selection anywhere — one harness)", async () => {
    render(<NewSpaceDialog onClose={vi.fn()} />);
    // The dialog has NO agent control (the picker is gone) — just the
    // folder field.
    expect(document.querySelector("select")).toBeNull();
    await setFolder("/tmp/ws");
    fireEvent.click(screen.getByRole("button", { name: "Start" }));
    // The invoke is `{ cwd }` — a single argument, no agent id.
    await waitFor(() =>
      expect(tauri.startSession).toHaveBeenCalledWith("/tmp/ws")
    );
  });

  it("the_start_button_is_disabled_until_a_folder_is_validated", () => {
    render(<NewSpaceDialog onClose={vi.fn()} />);
    const start = screen.getByRole("button", { name: "Start" }) as HTMLButtonElement;
    expect(start.disabled).toBe(true);
  });

  it("keeps the dialog open on a start error", async () => {
    vi.mocked(tauri.startSession).mockRejectedValueOnce("no such folder");
    render(<NewSpaceDialog onClose={vi.fn()} />);
    await setFolder("/tmp/ws");
    fireEvent.click(screen.getByRole("button", { name: "Start" }));
    // The error surfaces + the dialog stays open (the Cancel button is
    // still there).
    expect(await screen.findByText("no such folder")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Cancel" })).toBeTruthy();
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
