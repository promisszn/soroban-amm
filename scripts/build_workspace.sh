#!/usr/bin/env bash
#
# Build every deployable contract in the workspace for wasm32v1-none.
#
# This is the single source of truth for which crates are excluded from the
# wasm build. It is called by .github/workflows/ci.yml, .github/workflows/
# release.yml, and the Makefile's `build` target, so the three can no longer
# drift apart.
#
# Any extra arguments are forwarded to cargo, e.g.:
#   bash scripts/build_workspace.sh --locked
set -euo pipefail

# Crates that produce no deployable wasm artifact:
# - amm_fuzz, integration_tests, benches: test/bench harnesses, not contracts.
# - soroban_amm_simulator: off-chain CLI. Its host-only dependencies (clap,
#   csv, rand/getrandom) have no wasm32v1-none support and fail to compile,
#   so building it for wasm breaks the whole workspace build.
EXCLUDE_PACKAGES=(
  amm_fuzz
  integration_tests
  benches
  soroban_amm_simulator
)

exclude_flags=()
for pkg in "${EXCLUDE_PACKAGES[@]}"; do
  exclude_flags+=(--exclude "$pkg")
done

set -x
cargo build --release --target wasm32v1-none --workspace "${exclude_flags[@]}" "$@"
