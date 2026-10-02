---
status: approved
done-when: The app window restores its previous size, position, and maximized state on every launch (all three platforms), with no flash of the default 800x600, and the standard gates (cargo test / clippy / fmt / pnpm test / pnpm build) are green.
---

# Window state persistence (remember window size/position/maximized)

## Context

The Client (Tauri 2, v2.11.5) opens at a fixed 800×600 every launch. Tauri core has **no built-in window-state persistence**: the window size comes solely from `src-tauri/tauri.conf.json` (`width: 800`, `height: 600`, `decorations: false`, `transparent: true`), and nothing in the codebase observes or saves resize events (no `on_window_event` / `WindowEvent::Resized` in Rust; no `onResized` / `window-resized` subscriptions in the frontend). So every launch is a fresh 800×600.

## Decision

Persist full window state (size + position + maximized, `StateFlags::all()`) via the **official `tauri-plugin-window-state` plugin**, registered in `run()`, with the `main` window set to `visible: false` to prevent a flash of the default size before the saved state is applied. Full rationale, rejected alternatives, and accepted risks: `docs/decisions/0021-window-state-plugin.md` (ADR 0021).

## Changes (3 files, zero frontend changes)

1. **`src-tauri/Cargo.toml`** — add the dependency:
   ```toml
   tauri-plugin-window-state = "2"
   ```
2. **`src-tauri/src/lib.rs`** (`run()`, alongside the existing `dialog`/`opener` plugins at lines 85-86):
   ```rust
   .plugin(tauri_plugin_window_state::Builder::default().build())
   ```
   `Builder::default()` = `StateFlags::all()` (size + position + maximized + visible + decorations + fullscreen) and the default filename `.window-state.json`. No custom builder config.
3. **`src-tauri/tauri.conf.json`** (window config, lines 13-20) — add one field:
   ```json
   "visible": false
   ```
   Keep `width: 800` / `height: 600` as the first-run default.

### Why `visible: false` is safe on first run

Verified from the plugin source (`plugins/window-state/src/lib.rs`, v2 branch): in `restore_state`, `should_show` is initialized to `true` (line 196) and is only overwritten by the cached `state.visible` when saved state **exists** (line 244). On first run there is no cache entry, so `should_show` stays `true` and the plugin calls `self.show()?` + `self.set_focus()?` (lines 279-282). A `visible: false` window therefore cannot stay hidden forever on a fresh install.

## Runtime behavior

- **Launch:** the plugin's `on_window_ready` hook auto-restores the saved state per window, then shows it. First run (no state file) → shows the 800×600 default.
- **Use:** the plugin tracks `Moved`/`Resized`/maximize events into an in-memory cache; it skips size/position capture while the window is maximized or minimized, so the underlying size is preserved.
- **Quit:** `RunEvent::Exit` → the plugin writes the state file. The app is single-window with a custom close button (`WindowControls` → `close()`), so a normal quit always reaches `Exit` and saves.

## Accepted limitations (no mitigation in this change)

- **Plugin open bugs on Tauri 2.11.5:** macOS Retina can double the geometry per launch (plugins-workspace #3521); Linux can drift size/position per launch (#3553 — partly mitigated by `decorations: false`); a resize-time deadlock exists (#3594). Gated by the acceptance test below.
- **Save only on `RunEvent::Exit`:** a crash or `kill -9` skips the save; the next launch falls back to the last clean-quit state or the default (#3474, #1111). Accepted.
- **State file location:** the file lands in **Tauri's** app data dir (`~/.local/share/codes.archimedes.desktop/.window-state.json` on Linux, `%APPDATA%/codes.archimedes.desktop/` on Windows, `~/Library/Application Support/codes.archimedes.desktop/` on macOS) — **not** the `archimedes` dir the app deliberately uses via the `dirs` crate. The plugin's directory is not configurable (only the filename is); the file is plugin-managed, so this is harmless but worth knowing.
- **No monitor memory, no off-screen clamping:** a saved position can come back off-screen after a monitor change (#1282, #1988). Accepted; out of scope.

## Verification

- **Gates (all green):** `cargo test`, `cargo clippy --all-targets` (0 warnings), `cargo fmt --check` (from `src-tauri/`), `pnpm test`, `pnpm build` (from the repo root).
- **Manual acceptance test** (the TDD analogue — the headless `tests/ipc.rs` cannot exercise real windows; confirm each step before shipping):
  1. Delete the state file → launch → the window appears at 800×600 (first-run show works).
  2. Resize + move + maximize, quit normally → the state file exists in the Tauri app data dir.
  3. Relaunch → same size/position/maximized, with **no flash** of 800×600.
  4. On macOS (Retina) and on Linux: repeat steps 2–3 twice; the size must be **stable** (regression watch for #3521/#3553).

## Rollback

One commit: remove the Cargo dep, the `.plugin(...)` line, and the `"visible": false` line (delete any user `.window-state.json` if present). The app returns to the exact current 800×600-every-launch behavior. No schema, migration, or capability changes to unwind.

**Escalation path** if a platform bug bites: bump `tauri-plugin-window-state` to the latest 2.x (currently 2.5.0) and re-run the acceptance test; failing that, fall back to the manual `settings.json`-based approach (ADR 0021, option B — add a `#[serde(default)]` `window_state` field to `Settings` in `src-tauri/src/commands/settings.rs`, a debounced `on_window_event` handler in `run()`, and restore via `app.get_window("main")` in `.setup`).
