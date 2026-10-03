import { create } from "zustand";

import { getSettings, type AppSettings } from "../lib/tauri";

interface SettingsState {
  /** The last loaded settings (`null` until the first successful load). */
  settings: AppSettings | null;
  /** True once a load attempt finished (success OR failure — the UI can stop waiting). */
  loaded: boolean;
  /**
   * Fetch the settings from the backend and store them. A failure is LOGGED
   * and leaves `settings` at its current value (the boot path's theme/font
   * fallback in `lib/settings.ts` handles the document side; the store just
   * records what the backend handed over).
   */
  loadSettings: () => Promise<void>;
  /** Overwrite the stored settings (the settings UI's optimistic write — the save happens in the caller). */
  setSettings: (settings: AppSettings) => void;
}

export const useSettings = create<SettingsState>()((set) => ({
  settings: null,
  loaded: false,
  loadSettings: async () => {
    try {
      const settings = await getSettings();
      set({ settings, loaded: true });
    } catch (error) {
      console.error("Failed to load settings into the store", error);
      set({ loaded: true });
    }
  },
  setSettings: (settings) => set({ settings, loaded: true }),
}));
