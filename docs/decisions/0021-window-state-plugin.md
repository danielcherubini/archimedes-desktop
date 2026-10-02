---
status: accepted
date: 2026-10-02
superseded-by:
---

# Use the official `tauri-plugin-window-state` for window state persistence

The Client is a Tauri 2 app (2.11.5) that opens at a fixed 800×600 and does not remember the user's window size/position/maximized state across restarts. We decided to persist full window state (size + position + maximized, `StateFlags::all()`) using the **official `tauri-plugin-window-state` plugin** registered in `run()`, with the `main` window set to `visible: false` in `tauri.conf.json` so the user does not flash the default size before the saved state is applied. We chose the plugin over a manual Rust implementation (saving to the existing `settings.json`) because it is the first-party, officially-recommended mechanism, needs zero frontend code and zero capability changes for the core save-on-exit/restore-on-launch behavior, and handles multi-window/maximized/fullscreen edge cases we would otherwise have to hand-roll.

## Considered Options

- **A — Official `tauri-plugin-window-state` (chosen).** ~4 lines: a Cargo dep, one `.plugin(...)` call, and `"visible": false`. Restores on `on_window_ready`, saves on `RunEvent::Exit`. Verified from the plugin source that a `visible: false` window is reliably shown on first run (no saved state), so there is no "invisible app" risk.
- **B — Manual Rust via `settings.json`.** Add a `#[serde(default)]` `window_state` field and a debounced `on_window_event` handler that writes size/position/maximized to the existing settings store. More code (~50 lines) and you own the DPI/inner-vs-outer/debounce edge cases, but it sidesteps the plugin's open bugs. Rejected in favor of the official path, but kept as the escalation fallback (see Consequences).
- **C — Frontend-driven (no Rust changes).** React `getCurrentWindow().onResized()` → `saveSettings`; restore on `window-ready`. Rejected: the window is already visible by the time React hydrates, so the user sees a flash of 800×600, and it needs a new `core:window:allow-resize` capability.

## Consequences

- The plugin has **open, platform-specific bugs on our Tauri version (2.11.5)**: macOS Retina can double the geometry per launch (#3521), Linux can drift size/position per launch (#3553 — partly mitigated by our `decorations: false`), and a resize-time deadlock exists (#3594). We accept this risk and gate it with a manual acceptance test (two relaunches on macOS/Linux must show a stable size, no flash).
- The state file lands in **Tauri's** app data dir (`~/.local/share/codes.archimedes.desktop/.window-state.json` on Linux), **not** the `archimedes` dir the app deliberately uses via the `dirs` crate. The plugin's directory is not configurable (only the filename is); the file is plugin-managed, so this is harmless but worth knowing.
- The plugin **saves only on `RunEvent::Exit`**, so a crash or `kill -9` skips the save and the next launch falls back to the last clean-quit state or the default. Accepted; no mitigation.
- **No monitor is remembered and no off-screen clamping is done** — a saved position can come back off-screen after a monitor change. Known plugin limitation, out of scope.
- **Rollback is one clean commit:** remove the Cargo dep, the `.plugin(...)` line, and the `"visible": false` line (and delete any user `.window-state.json`); the app returns to the exact current 800×600-every-launch behavior. **Escalation path** if a platform bug bites: bump the plugin to the latest 2.x (2.5.0) and re-run the acceptance test; failing that, fall back to approach B.
