import { describe, expect, it, vi, beforeEach } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

import { invoke } from "@tauri-apps/api/core";
import { getAppInfo } from "./version";

const mockedInvoke = vi.mocked(invoke);

describe("getAppInfo", () => {
  beforeEach(() => {
    mockedInvoke.mockReset();
  });

  it("invokes the app_info command", async () => {
    mockedInvoke.mockResolvedValue({ version: "0.1.0", platform: "linux" });
    await getAppInfo();
    expect(mockedInvoke).toHaveBeenCalledWith("app_info");
  });

  it("returns version and platform from the IPC response", async () => {
    mockedInvoke.mockResolvedValue({ version: "1.2.3", platform: "linux" });
    const info = await getAppInfo();
    expect(info).toEqual({ version: "1.2.3", platform: "linux" });
    expect(typeof info.version).toBe("string");
    expect(typeof info.platform).toBe("string");
  });
});
