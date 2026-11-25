# Everyday tasks for working on and dogfooding Utopia. `make` lists them.

SHELL := /bin/bash
# rustup's cargo, even when ~/.cargo/bin is not on PATH (make runs simple recipes
# without a shell, so exporting PATH is not enough).
CARGO ?= $(shell command -v cargo 2>/dev/null || echo $(HOME)/.cargo/bin/cargo)
# Ask xcode-select where Xcode is: a version bump renames the bundle.
export DEVELOPER_DIR ?= $(shell xcode-select -p)
# Disk is tight here: incremental caches grow fast with GPUI (see AGENTS.md).
export CARGO_INCREMENTAL ?= 0

VERSION := $(shell sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
ARCH := $(shell uname -m)
APP := target/Utopia.app
ZIP := target/Utopia-$(VERSION)-$(ARCH).zip
INSTALL_DIR ?= /Applications
PREFIX ?= $(HOME)/.local
SCRATCH ?= $(CURDIR)/target/scratch

.DEFAULT_GOAL := help
.PHONY: help run dev check build test clippy fmt fmt-check comments bench relay-check sync-fuzz app zip install install-ut doctor clean-debug

help: ## List the targets
	@grep -E '^[a-z-]+:.*## ' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*## "} {printf "  %-12s %s\n", $$1, $$2}'

run: ## Run the app (debug build) on your usual graph
	$(CARGO) run -p utopia-app

dev: ## Run the app (debug build) on a throwaway graph in target/scratch
	mkdir -p "$(SCRATCH)"
	UTOPIA_CONFIG_DIR="$(SCRATCH)" $(CARGO) run -p utopia-app -- --graph "$(SCRATCH)/scratch.utopia"

check: fmt-check build test clippy ## Everything that must pass before a change is done

build: ## cargo build --workspace
	$(CARGO) build --workspace

test: ## cargo test --workspace
	$(CARGO) test --workspace

clippy: ## Clippy with warnings as errors
	$(CARGO) clippy --workspace --all-targets -- -D warnings

fmt: ## Format the code
	$(CARGO) fmt --all

fmt-check: ## Fail if the code is not formatted
	$(CARGO) fmt --all -- --check

comments: ## Comment ratchet (ADR 0001); ARGS=--update tightens it, ARGS="--list <path>" lists
	$(CARGO) run -q -p xtask -- comments $(ARGS)

# The same steps, in the same order, as .github/workflows/relay.yml. Needs Node and
# `npm ci` in relay-cloudflare once; runs wrangler dev --local only, never an account.
relay-check: ## Cloudflare relay: tsc, vitest, then the contract and differential tests against wrangler
	cd relay-cloudflare && npx --no-install tsc --noEmit
	cd relay-cloudflare && npm test
	$(CARGO) test -p utopia-syncd
	relay-cloudflare/scripts/test-against-rust-client.sh

# 2,000 seeds of crates/utopia-sync/tests/convergence.rs (make check runs 20). SEEDS=n changes
# the count; a failure names its seed, rerun one with UTOPIA_SYNC_FUZZ_SEED=<seed>.
sync-fuzz: ## Multi-device sync convergence, the long run (about 2 minutes)
	UTOPIA_SYNC_FUZZ_SEEDS=$(or $(SEEDS),2000) $(CARGO) test --release -p utopia-sync --test convergence -- --ignored

bench: ## Performance budgets (release build; opens a window for under a second)
	$(CARGO) build --release -p utopia-app
	$(CARGO) run -p xtask --release -- bench --ui

app: ## Release build of target/Utopia.app, ad-hoc signed
	$(CARGO) run -q -p xtask -- bundle
	codesign --force --deep --sign - "$(APP)"

zip: app ## target/Utopia-<version>-<arch>.zip, to copy to another Mac
	rm -f "$(ZIP)"
	ditto -c -k --keepParent "$(APP)" "$(ZIP)"
	@echo "$(ZIP)"

install-ut: ## Build the standalone `ut` CLI and copy it to PREFIX/bin (default ~/.local)
	$(CARGO) build --release -p utopia-cli
	install -d "$(PREFIX)/bin"
	install -m 0755 target/release/ut "$(PREFIX)/bin/ut"
	@echo "Installed $(PREFIX)/bin/ut"

install: app ## Copy Utopia.app into INSTALL_DIR (default /Applications)
	rm -rf "$(INSTALL_DIR)/Utopia.app"
	ditto "$(APP)" "$(INSTALL_DIR)/Utopia.app"
	@echo "Installed $(INSTALL_DIR)/Utopia.app (opening it registers utopia:// links)"

doctor: ## Check a graph's invariants: make doctor GRAPH=path/to/graph.utopia
	@test -n "$(GRAPH)" || { echo "usage: make doctor GRAPH=path/to/graph.utopia"; exit 2; }
	$(CARGO) run -q -p utopia-app -- doctor "$(GRAPH)"

clean-debug: ## Delete target/debug when it grows too big (the next build starts over)
	rm -rf target/debug
