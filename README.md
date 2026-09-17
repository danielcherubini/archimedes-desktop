# Archimedes Desktop

A cross-platform (Windows / macOS / Linux) desktop app, built with
[Tauri 2](https://tauri.app), that connects to coding agents over the
[Agent Client Protocol (ACP)](https://agentclientprotocol.com). Pi is the
first-class agent in v1; other ACP agents follow.

See [CONTEXT.md](CONTEXT.md) for the project's language and terminology.

## What it does

- Spawns ACP-speaking agent processes as subprocesses and speaks stdio
  JSON-RPC with them. The default agent is `pi` (via `pi-acp`); the
  space's folder is the conversation's working directory.
- Presents **spaces** — each space is a folder; your conversations live
  inside spaces, and the agent's file access is sandboxed to the space's
  folder.
- One conversation is live at a time; starting a conversation in another
  space pauses the current one (it stays resumable). This sidesteps a
  known two-session runtime constraint
  ([docs/decisions/0002](docs/decisions/0002-one-live-acp-session.md)).
- Streams the conversation: agent text, tool calls, and file diffs.
- Permission prompts: the agent asks before each tool call; you approve
  or deny in the UI.
- Conversations survive restarts — history is persisted in a local SQLite
  database and can be resumed.
- Auto-update: signed updates (minisign) delivered through GitHub
  Releases; the updater verifies the signature before installing.

## Prerequisites

**Building:**

- Rust ≥ 1.88 (stable)
- Node.js ≥ 22.19 and [pnpm](https://pnpm.io)
- Platform system dependencies:
  - **Linux:** `libwebkit2gtk-4.1-dev` (webkit2gtk 4.1),
    `libappindicator3-dev`, `librsvg2-dev`, `patchelf`, `xdg-utils`
    (Debian/Ubuntu names; on Fedora: `webkit2gtk4.1-devel`,
    `gtk3-devel`, `librsvg2-devel`, `patchelf`, `xdg-utils`)
  - **macOS:** Xcode command-line tools
  - **Windows:** the [Windows prerequisites](https://tauri.app/start/prerequisites/)
    (WebView2, MSVC build tools)

**Running (the app needs an ACP agent on PATH):**

- `pi` — the coding agent
- `pi-acp` — the ACP adapter that wraps pi

## Getting started

```sh
pnpm install
pnpm tauri dev
```

## Building installers

```sh
pnpm tauri build
```

Artifacts land in `src-tauri/target/release/bundle/`:

| Platform | Bundles |
| -------- | ------- |
| macOS    | `.app` + `.dmg` (Developer ID signed + notarized in CI) |
| Windows  | `.exe` (NSIS; unsigned in v1 — expect SmartScreen warnings) |
| Linux    | `.AppImage` + `.deb` |

With `createUpdaterArtifacts` enabled, each build also produces the
updater payload (`.app.tar.gz` / `.nsis.zip`) and its `.sig` signature.

## Auto-update

The updater plugin checks a static `latest.json` on GitHub Releases and
verifies every update with the minisign public key embedded in
`tauri.conf.json` before installing. The "Check for updates" menu item
triggers a check.

**First-release steps (before the first tag):**

1. Replace the `<owner>` placeholder in `plugins.updater.endpoints` in
   `src-tauri/tauri.conf.json` with the real GitHub owner.
2. Create the `TAURI_SIGNING_PRIVATE_KEY` (and optional
   `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`) repository secret — the value is
   the minisign private key generated with `pnpm tauri signer generate`
   (keep it outside the repo, e.g. `~/.archimedes/tauri-signing.key`).
3. Add `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID` secrets for macOS
   notarization.
4. Tag `v0.1.0` (or later) — `.github/workflows/release.yml` builds all
   three platforms and publishes the release with `latest.json`.

## Linux + NVIDIA note

On Linux with an NVIDIA GPU, the app may crash on startup with a DMABUF
error. Work around it by launching with:

```sh
WEBKIT_DISABLE_DMABUF_RENDERER=1 ./Archimedes\ Desktop.AppImage
```

(or set the variable in your environment before starting the app).

### Building AppImages on recent Fedora

On recent Fedora releases (41+), the AppImage bundling step can fail with
`failed to run linuxdeploy`: the old `strip` bundled inside the
linuxdeploy AppImage cannot parse the newer `.relr.dyn` ELF sections in
system libraries. Build with `NO_STRIP=1` to skip the (optimization-only)
strip pass:

```sh
NO_STRIP=1 pnpm tauri build
```

This only affects the local AppImage; CI (Ubuntu) is unaffected.

## Development

- `pnpm dev` — Vite dev server
- `pnpm tauri dev` — full app in dev mode
- `pnpm test` — frontend tests (Vitest)
- `cargo test` (in `src-tauri/`) — Rust tests
- CI (`.github/workflows/ci.yml`) runs fmt, clippy, and both test suites
  on every push/PR.
