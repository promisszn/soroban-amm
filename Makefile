# Common developer tasks. This should track the checks CI actually runs in
# .github/workflows/ci.yml — that workflow is the source of truth; keep this
# file in sync with it rather than the other way around.

WASM_DIR := target/wasm32v1-none/release

SHELL := bash

.PHONY: all help build optimize test test-all sim-test fmt lint check check-docs \
        size size-check doc audit bench deploy e2e clean

# Bare `make` explains itself instead of building.
.DEFAULT_GOAL := help

help: ## Show this help
	@echo "Available targets:"
	@grep -E '^[a-zA-Z0-9_-]+:.*?## .*$$' $(MAKEFILE_LIST) | \
		awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-14s\033[0m %s\n", $$1, $$2}'

all: build ## Build release WASM (default target if invoked explicitly)

build: ## cargo build --release --target wasm32v1-none
	cargo build --release --target wasm32v1-none

optimize: build ## Optimize every WASM artifact produced by `make build`
	@shopt -s nullglob; \
	wasms=($(WASM_DIR)/*.wasm); \
	if [ $${#wasms[@]} -eq 0 ]; then \
		echo "error: no WASM artifacts found in $(WASM_DIR) — run 'make build' first" >&2; \
		exit 1; \
	fi; \
	for f in "$${wasms[@]}"; do \
		echo "optimizing $$f"; \
		stellar contract optimize --wasm "$$f"; \
	done

test: build ## Build, then run the default-members test suite
	cargo test

test-all: ## Run tests for the whole workspace, bypassing default-members
	cargo test --workspace

sim-test: ## Build and run the off-chain amm-simulator parity suite
	cargo test -p soroban-amm-simulator

fmt: ## cargo fmt --all
	cargo fmt --all

lint: ## cargo clippy --all -- -D warnings
	cargo clippy --all -- -D warnings

check-docs: ## Verify docs/error-codes.md matches #[contracterror] enums
	bash scripts/check_error_docs.sh

size: ## Print a WASM size report for all built contracts
	bash scripts/size_report.sh

size-check: ## Fail if any contract WASM exceeds the size limit
	bash scripts/size_report.sh --fail-on-limit

doc: ## Build workspace docs with warnings denied
	RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace

audit: ## Run a security audit of dependencies (cargo install cargo-audit if missing)
	@command -v cargo-audit >/dev/null 2>&1 || { \
		echo "cargo-audit not found; install it with: cargo install cargo-audit" >&2; \
		exit 1; \
	}
	cargo audit

check: fmt lint test check-docs size-check doc ## Run the checks CI enforces before pushing

bench: ## Run hot-path benchmarks
	cargo run -p benches -- --check

deploy: ## Deploy contracts via scripts/deploy.sh
	bash scripts/deploy.sh

e2e: ## Run the end-to-end test suite
	bash scripts/e2e.sh

clean: ## cargo clean
	cargo clean
