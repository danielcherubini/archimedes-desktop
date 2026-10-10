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
import { listCompletionEntries, listSpaceFiles } from "./tauri";

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

describe("listCompletionEntries (the out-of-Space `?` IPC boundary, ADR 0035)", () => {
  beforeEach(() => {
    mockedInvoke.mockReset();
  });

  it("invokes `list_completion_entries` with the `query` argument key", async () => {
    mockedInvoke.mockResolvedValue({ entries: [], truncated: false });
    await listCompletionEntries("~/.config/ht");
    // Same trap as above: Tauri maps the camelCase arg KEY to the snake_case
    // Rust parameter, and a typo in the name or the key is a runtime-only
    // failure in the desktop app that no Rust test and no hook test can see.
    expect(mockedInvoke).toHaveBeenCalledWith("list_completion_entries", {
      query: "~/.config/ht",
    });
  });

  it("passes null through as `{ query: null }` (no token: the empty listing, no error)", async () => {
    // PASSED THROUGH, not short-circuited client-side — the Rust `None` arm is
    // the one place that guarantees "no token costs no filesystem access", and a
    // wrapper-level early return would move that guarantee somewhere no Rust
    // test covers. This is the `listSpaceFiles` shape, kept deliberately.
    mockedInvoke.mockResolvedValue({ entries: [], truncated: false });
    await listCompletionEntries(null);
    expect(mockedInvoke).toHaveBeenCalledWith("list_completion_entries", {
      query: null,
    });
  });

  it("returns the `{ entries, truncated }` payload UNCHANGED (no reshaping)", async () => {
    // The row shape is pinned here rather than left to Task 4, because the DTO
    // crosses IPC as camelCase (`isDir` for Rust's `is_dir`): a rename on either
    // side is invisible to both test suites and shows up as every row looking
    // like a file.
    const payload = {
      entries: [
        {
          name: "htop",
          insert: "/home/u/.config/htop",
          display: "~/.config/htop",
          isDir: true,
        },
        {
          name: "config",
          insert: "/home/u/.config/htop/config",
          display: "~/.config/htop/config",
          isDir: false,
        },
      ],
      truncated: true,
    };
    mockedInvoke.mockResolvedValue(payload);
    await expect(listCompletionEntries("~/.config/")).resolves.toEqual(payload);
  });
});
