/**
 * Auto-update wrappers over @tauri-apps/plugin-updater (Task 6).
 *
 * The backend (Rust) verifies every update package with the minisign
 * public key embedded in `tauri.conf.json` before the frontend is even
 * allowed to install it — these wrappers only orchestrate the
 * check/download/install flow and surface the result to the UI.
 */

import {
  check,
  type DownloadEvent,
  type Update,
} from "@tauri-apps/plugin-updater";

/** Result of an update check (UI-friendly, no plugin types leak out). */
export interface UpdateCheck {
  /** `true` when a newer signed release is available. */
  available: boolean;
  /** The newer release version, or null when up to date. */
  version: string | null;
  /** The running version (null when no update was found). */
  currentVersion: string | null;
}

/**
 * Ask the updater whether a newer release exists.
 *
 * @returns `available: false` when `check()` resolves `null` (up to date).
 */
export async function checkForUpdate(): Promise<UpdateCheck> {
  const update: Update | null = await check();
  if (update === null) {
    return { available: false, version: null, currentVersion: null };
  }
  return {
    available: true,
    version: update.version,
    currentVersion: update.currentVersion,
  };
}

/** Callback receiving raw download events (Started / Progress / Finished). */
export type UpdateProgress = (event: DownloadEvent) => void;

/**
 * Check for an update and, if one exists, download and install it.
 *
 * Platform notes (from the plugin): on Windows the app exits after
 * launching the updater installer; on macOS/Linux the app must be
 * relaunched to run the new binary.
 */
export async function installUpdate(onProgress?: UpdateProgress): Promise<void> {
  const update: Update | null = await check();
  if (update === null) {
    return;
  }
  await update.downloadAndInstall(onProgress);
}
