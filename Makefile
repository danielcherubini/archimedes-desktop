# Archimedes Desktop — build & validation entry points.
#
# The project validates from TWO roots (see AGENTS.md): the frontend from
# the repo root (`pnpm`) and the Rust backend from `src-tauri/` (`cargo`).
# A branch is ready to merge when `make check` is green.
#
# One-time setup (system packages, NOT done by `setup`):
#   - Rust: pinned in src-tauri/rust-toolchain.toml (rustup honours it
#     automatically in `src-tauri/`; `make` reads it for the root recipes)
#   - Node >= 22.19
#   - Linux: libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev
#
# `build` (release) signs the updater artifacts: it needs
# TAURI_SIGNING_PRIVATE_KEY_PATH pointing at the secret key (a clean
# single-line base64 file — no trailing newline) and prompts for the
# key password. On Fedora (41+) it also auto-sets NO_STRIP=1 — the
# AppImage bundling step (linuxdeploy) fails there because the old
# `strip` frozen inside the linuxdeploy AppImage can't parse the newer
# `.relr.dyn` ELF sections in Fedora's system libraries (see README);
# override with `make build NO_STRIP=` (force off) or `NO_STRIP=1` (force on).

PNPM     := pnpm
RUST_DIR := src-tauri

# The Rust toolchain is pinned by `$(RUST_DIR)/rust-toolchain.toml`, but rustup
# resolves that file from the CWD and the recipes below run from the repo root
# (`--manifest-path`), which would silently use whatever `rustup default` is —
# exactly the local/CI drift the pin exists to prevent. Read the channel out of
# the file and name it, so `make check` lints and tests on the compiler CI does.
RUST_CHANNEL := $(shell sed -n 's/^channel *= *"\([^"]*\)".*/\1/p' $(RUST_DIR)/rust-toolchain.toml)
CARGO        := $(if $(RUST_CHANNEL),cargo +$(RUST_CHANNEL),cargo)

# The Fedora AppImage workaround (see header): auto-detected from
# /etc/os-release. A command-line `NO_STRIP=…` always wins (GNU make
# gives command-line variables precedence over this `?=`), and the var
# is only exported when NON-EMPTY — a non-Fedora build is untouched
# regardless of how Tauri interprets an empty value.
NO_STRIP ?= $(shell grep -q '^ID=fedora' /etc/os-release 2>/dev/null && echo 1)

.DEFAULT_GOAL := help

# ── Setup / dev ───────────────────────────────────────────────────────

.PHONY: install-desktop
install-desktop: ## Install desktop entry and icons to ~/.local/share (Linux only)
	@if [ "$$(uname -s)" = "Linux" ]; then \
		mkdir -p ~/.local/share/icons/hicolor/32x32/apps ~/.local/share/icons/hicolor/64x64/apps ~/.local/share/icons/hicolor/128x128/apps ~/.local/share/icons/hicolor/256x256/apps ~/.local/share/icons/hicolor/512x512/apps ~/.local/share/applications; \
		cp $(RUST_DIR)/icons/32x32.png ~/.local/share/icons/hicolor/32x32/apps/archimedes.png 2>/dev/null || true; \
		cp $(RUST_DIR)/icons/64x64.png ~/.local/share/icons/hicolor/64x64/apps/archimedes.png 2>/dev/null || true; \
		cp $(RUST_DIR)/icons/128x128.png ~/.local/share/icons/hicolor/128x128/apps/archimedes.png 2>/dev/null || true; \
		cp $(RUST_DIR)/icons/128x128@2x.png ~/.local/share/icons/hicolor/256x256/apps/archimedes.png 2>/dev/null || true; \
		cp $(RUST_DIR)/icons/icon.png ~/.local/share/icons/hicolor/512x512/apps/archimedes.png 2>/dev/null || true; \
		cp $(RUST_DIR)/icons/icon.png ~/.local/share/icons/hicolor/512x512/apps/codes.archimedes.desktop.png 2>/dev/null || true; \
		printf '[Desktop Entry]\nName=Archimedes\nComment=Archimedes Desktop Coding Agent\nExec=archimedes %%U\nTerminal=false\nType=Application\nIcon=archimedes\nCategories=Development;\nStartupWMClass=archimedes\n' > ~/.local/share/applications/archimedes.desktop; \
		printf '[Desktop Entry]\nName=Archimedes\nComment=Archimedes Desktop Coding Agent\nExec=archimedes %%U\nTerminal=false\nType=Application\nIcon=archimedes\nCategories=Development;\nStartupWMClass=codes.archimedes.desktop\n' > ~/.local/share/applications/codes.archimedes.desktop.desktop; \
		update-desktop-database ~/.local/share/applications 2>/dev/null || true; \
		gtk-update-icon-cache -f ~/.local/share/icons/hicolor 2>/dev/null || true; \
	fi

.PHONY: setup
setup: install-desktop ## Install JS dependencies (one-time; system deps are manual)
	$(PNPM) install

.PHONY: dev
dev: install-desktop ## Run the app in the Tauri shell (Vite HMR + cargo build)
	$(PNPM) tauri dev

.PHONY: dev-web
dev-web: ## Vite dev server only (UI in a browser, no Tauri shell)
	$(PNPM) dev

# ── Frontend (repo root) ──────────────────────────────────────────────

.PHONY: build-frontend
build-frontend: ## Type-check (tsc) + build the frontend (vite)
	$(PNPM) build

.PHONY: test-frontend
test-frontend: ## Frontend unit tests (vitest)
	$(PNPM) test

# ── Rust backend (src-tauri/) ─────────────────────────────────────────

.PHONY: test-rust
test-rust: ## Rust tests
	$(CARGO) test --manifest-path $(RUST_DIR)/Cargo.toml

.PHONY: lint-rust
lint-rust: ## Rust lint (must be 0 warnings)
	$(CARGO) clippy --all-targets --manifest-path $(RUST_DIR)/Cargo.toml

.PHONY: fmt
fmt: ## Format the Rust code
	$(CARGO) fmt --manifest-path $(RUST_DIR)/Cargo.toml

.PHONY: fmt-check
fmt-check: ## Rust format check (the CI gate)
	$(CARGO) fmt --manifest-path $(RUST_DIR)/Cargo.toml --check

# ── Release ───────────────────────────────────────────────────────────

.PHONY: build
build: ## Release build → src-tauri/target/release/bundle/ (signs the updater artifacts; prompts for the key password; auto NO_STRIP=1 on Fedora)
	$(if $(NO_STRIP),NO_STRIP=$(NO_STRIP) ,)$(PNPM) tauri build

.PHONY: clean
clean: ## Remove build artifacts (cargo target + dist)
	$(CARGO) clean --manifest-path $(RUST_DIR)/Cargo.toml
	rm -rf dist

# ── Aggregates ────────────────────────────────────────────────────────

.PHONY: test
test: test-frontend test-rust ## All unit tests (both roots)

.PHONY: check
check: ## Full AGENTS.md validation bar (the merge gate) — sequential, fast-fail
	$(CARGO) fmt --manifest-path $(RUST_DIR)/Cargo.toml --check
	$(CARGO) clippy --all-targets --manifest-path $(RUST_DIR)/Cargo.toml
	$(CARGO) test --manifest-path $(RUST_DIR)/Cargo.toml
	$(PNPM) build
	$(PNPM) test

# ── Help ──────────────────────────────────────────────────────────────

.PHONY: help
help: ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) \
		| awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-15s\033[0m %s\n", $$1, $$2}'
