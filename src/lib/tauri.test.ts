import { describe, expect, it, vi, beforeEach } from "vitest";

// Mock the Tauri IPC core (the `version.test.ts` pattern): the wrappers in
// this module are ONE-LINE `invoke` calls, so the only thing worth pinning is
// the WIRE CONTRACT — the command name and the argument keys. Nothing else
// does: the hook tests mock this WRAPPER (so they cannot see a renamed command
// or a re-typed arg key), and no Rust test registers `commands::files`, so a
// drift here — e.g. `spacePath` vs Rust's `space_path`, or
// `list_space_files` renamed — would pass every test on BOTH sides and fail
// only at runtime, in the desktop app.
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

import { invoke } from "@tauri-apps/api/core";
import { listSpaceFiles } from "./tauri";

const mockedInvoke = vi.mocked(invoke);

describe("listSpaceFiles (the `?` File-completion IPC boundary, ADR 0033)", () => {
  beforeEach(() => {
    mockedInvoke.mockReset();
  });

  it("invokes `list_space_files` with the `spacePath` argument key", async () => {
    mockedInvoke.mockResolvedValue({ entries: [], truncated: false });
    await listSpaceFiles("/x");
    // The command NAME and the arg KEY are both load-bearing: Tauri maps the
    // camelCase `spacePath` here to the snake_case `space_path` Rust
    // parameter, and a typo in either is a runtime-only failure.
    expect(mockedInvoke).toHaveBeenCalledWith("list_space_files", {
      spacePath: "/x",
    });
  });

  it("passes null through as `{ spacePath: null }` (no Space: the empty listing, not an error)", async () => {
    mockedInvoke.mockResolvedValue({ entries: [], truncated: false });
    await listSpaceFiles(null);
    expect(mockedInvoke).toHaveBeenCalledWith("list_space_files", {
      spacePath: null,
    });
  });

  it("returns the `{ entries, truncated }` payload UNCHANGED (no reshaping)", async () => {
    // `truncated` is the flag the picker's note is built from, so a wrapper
    // that dropped or renamed it would silently kill the note.
    const payload = {
      entries: ["README.md", "src/main.rs"],
      truncated: true,
    };
    mockedInvoke.mockResolvedValue(payload);
    await expect(listSpaceFiles("/x")).resolves.toEqual(payload);
    await expect(listSpaceFiles("/x")).resolves.toEqual({
      entries: ["README.md", "src/main.rs"],
      truncated: true,
    });
  });
});
