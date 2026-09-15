import { invoke } from "@tauri-apps/api/core";

export interface AppInfo {
  version: string;
  platform: string;
}

export async function getAppInfo(): Promise<AppInfo> {
  return invoke<AppInfo>("app_info");
}
