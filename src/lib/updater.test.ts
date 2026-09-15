import { describe, expect, it, vi, beforeEach } from "vitest";

vi.mock("@tauri-apps/plugin-updater", () => ({
  check: vi.fn(),
}));

import { check, type Update } from "@tauri-apps/plugin-updater";
import { checkForUpdate, installUpdate } from "./updater";

const mockedCheck = vi.mocked(check);

/** Build a stand-in for the plugin's `Update` resource. */
function fakeUpdate(
  overrides: Partial<Update> = {},
): Update & { downloadAndInstall: ReturnType<typeof vi.fn> } {
  const downloadAndInstall = vi.fn().mockResolvedValue(undefined);
  return {
    available: true,
    currentVersion: "0.1.0",
    version: "0.2.0",
    download: vi.fn().mockResolvedValue(undefined),
    install: vi.fn().mockResolvedValue(undefined),
    close: vi.fn().mockResolvedValue(undefined),
    downloadAndInstall,
    ...overrides,
  } as unknown as Update & { downloadAndInstall: ReturnType<typeof vi.fn> };
}

describe("checkForUpdate", () => {
  beforeEach(() => {
    mockedCheck.mockReset();
  });

  it("reports no update when check() resolves null", async () => {
    mockedCheck.mockResolvedValue(null);
    const result = await checkForUpdate();
    expect(result).toEqual({
      available: false,
      version: null,
      currentVersion: null,
    });
  });

  it("reports the new and current version when an Update is returned", async () => {
    mockedCheck.mockResolvedValue(fakeUpdate());
    const result = await checkForUpdate();
    expect(result.available).toBe(true);
    expect(result.version).toBe("0.2.0");
    expect(result.currentVersion).toBe("0.1.0");
  });
});

describe("installUpdate", () => {
  beforeEach(() => {
    mockedCheck.mockReset();
  });

  it("downloads and installs when an update is available", async () => {
    const update = fakeUpdate();
    mockedCheck.mockResolvedValue(update);
    await installUpdate();
    expect(update.downloadAndInstall).toHaveBeenCalledTimes(1);
  });

  it("is a no-op (no throw) when there is no update", async () => {
    mockedCheck.mockResolvedValue(null);
    await expect(installUpdate()).resolves.toBeUndefined();
    expect(mockedCheck).toHaveBeenCalledTimes(1);
  });

  it("forwards the progress callback to downloadAndInstall", async () => {
    const update = fakeUpdate();
    mockedCheck.mockResolvedValue(update);
    const onProgress = vi.fn();
    await installUpdate(onProgress);
    expect(update.downloadAndInstall).toHaveBeenCalledWith(onProgress);
  });
});
