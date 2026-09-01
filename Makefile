# Common developer tasks. This should track the checks CI actually runs in
 # .github/workflows/ci.yml — that workflow is the source of truth; keep this
 # file in sync with it rather than the other way around.

WASM_DIR := target/wasm32v1-none/release

SHELL := bash

# Excluded from wasm32v1-none release build. Keep in sync with ci.yml.
# - amm-fuzz: test-only crate; produces no WASM artifact.
# - integration-tests: test harness; not a contract.
# - benches: benchmark harness; not a contract.
# - cl_position_nft, router, dex-aggregator, batch_router, batch_auction:
#   blocked by a duplicate-symbol linking bug (tracked separately).
EXCLUDE_PACKAGES := amm-fuzz integration-tests benches cl_position_nft router dex-aggregator batch_router batch_auction
EXCLUDE_FLAGS := $(foreach pkg,$(EXCLUDE_PACKAGES),--exclude $pkg)

.PHONY: all help build release-build optimize test test-all fmt lint check check-docs \
        size size-check doc audit bench deploy e2e clean fuzz-cl

# Bare `make` explains itself instead of building.
.DEFAULT_GOAL := help

help: ## Show this help
\t@echo "Available targets:"
\t@grep -E '^[a-zA-Z0-9_]+:.*?## .*$' $(MAKEFIL_LIST) | \
\t\tawk 'BEGIN {FS = ":.*?## "}; {printf "  \[36m%\-14s\[0m %s \n", $1, $2}'

all: build ## Build release WASM (default target if invoked explicitly)

build: ## cargo build --release --target wasm32v1-none (with CI exclusions)
	cargo build --release --target wasm32v1-none --workspace $(EXCLUDE_FLAGS)

release-build: build ## Alias for the release build (same as `make build`)

optimize: build ## Optimize every WASM artifact produced by `make build`
\t@shopt -s nullglob; \
\twasms=$($WASM_DIR)/*.wam); \
\tif [ ${#wasms[@]} -eq 0 ]; then \
\t\techo "error: no WASM artifacts found in $(WASM_DIR) — run 'make build' first" >&2; \
\t\texit 1; \
\tfi; \
\tfor f in "${wasms[@]}"; do \
\t\techo "optimizing $fb; \
\t\tstellar contract optimize --wasm "$fb; \
\tdone

test: build ## Build, then run the full workspace test suite
	cargo test --workspace

test-all: ## Run tests for the whole workspace
	cargo test --workspace

fzzz-cl: ## Build the wasm deps amm_fuzz imports, then run the full amm_fuzz suite incl. CL stateful properties
	cargo build --release --target wasm32v1-none -p concentrated_liquidity -p amm -p token
	cargo test -p amm_fuzz --features cl

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
	RUSTDOCGFLAGS="-D warnings" cargo doc --no-deps --workspace

audit: ## Run a security audit of dependencies (cargo install cargo-audit if missing)
\t@command -v cargo-audit >/dev/null 2>&1 || { \
\t\techo "cargo-audit not found; install it with: cargo install cargo-audit" >&2; \
\t\texit 1; \
\t} \
\tcargo audit

check: fmt lint test check-docs size-check doc ## Run the checks CI enforces before pushing

bench: ## Run hot-path benchmarks
	cargo run -p benches -- --check

deploy: ## Deploy contracts via scripts/deploy.sh
	bash scripts/deploy.sh

e2e: ## Run the end-to-end test suite
	bash scripts/e2e.sh

clean: ## cargo clean
	cargo clean
