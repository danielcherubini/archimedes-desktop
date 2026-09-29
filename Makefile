# Archimedes Desktop — build & validation entry points.
#
# The project validates from TWO roots (see AGENTS.md): the frontend from
# the repo root (`pnpm`) and the Rust backend from `src-tauri/` (`cargo`).
# A branch is ready to merge when `make check` is green.
#
# One-time setup (system packages, NOT done by `setup`):
#   - Rust >= 1.88, Node >= 22.19
#   - Linux: libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev
#     (on Fedora the webkit2gtk build additionally needs NO_STRIP=1 —
#     see README)
#
# `build` (release) signs the updater artifacts: it needs
# TAURI_SIGNING_PRIVATE_KEY_PATH pointing at the secret key (a clean
# single-line base64 file — no trailing newline) and prompts for the
# key password.

PNPM     := pnpm
CARGO    := cargo
RUST_DIR := src-tauri

.DEFAULT_GOAL := help

# ── Setup / dev ───────────────────────────────────────────────────────

.PHONY: setup
setup: ## Install JS dependencies (one-time; system deps are manual)
	$(PNPM) install

.PHONY: dev
dev: ## Run the app in the Tauri shell (Vite HMR + cargo build)
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
build: ## Release build → src-tauri/target/release/bundle/ (signs the updater artifacts; prompts for the key password)
	$(PNPM) tauri build

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
