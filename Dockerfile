# Keep this in sync with rust-toolchain.toml.  The toolchain file overrides
# the compiler version at runtime, but aligning the base image means the
# pre-installed toolchain is reused rather than replaced on every build.
FROM rust:1.98.1-slim-bookworm

# Install system dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    build-essential \
    ca-certificates \
    curl \
    git \
    make \
    libdbus-1-3 \
    libudev1 \
    && rm -rf /var/lib/apt/lists/*

# rust-toolchain.toml already declares wasm32v1-none as a required target,
# so rustup will install it automatically when cargo first runs.  There is
# no need (and it would be wrong) to add wasm32-unknown-unknown here.

# Install the Stellar CLI from its prebuilt release binary rather than
# `cargo install`: compiling it ties the CLI to this image's compiler, and
# stellar-cli 25.1.0's locked `ethnum` no longer builds on Rust 1.98. The
# version stays the one documented in README.md and the deployment runbook
# (unifying the pins is #955). The binary links libdbus and libudev
# (installed above) and needs glibc >= 2.34.
ARG STELLAR_CLI_VERSION=25.1.0
RUN arch="$(uname -m)" \
    && curl -fsSL "https://github.com/stellar/stellar-cli/releases/download/v${STELLAR_CLI_VERSION}/stellar-cli-${STELLAR_CLI_VERSION}-${arch}-unknown-linux-gnu.tar.gz" \
    | tar -xz -C /usr/local/bin stellar \
    && stellar --version

WORKDIR /app

# ---------------------------------------------------------------------------
# Dependency-caching layer
#
# Copy every Cargo.toml (workspace root + all members) and Cargo.lock first,
# then run `cargo fetch`.  Docker caches this layer as long as no manifest
# changes, so a source-only edit skips the network round-trip entirely.
#
# The workspace is *not* flat, so we copy each member manifest explicitly.
# If a new member is added to Cargo.toml it must also appear here.
# ---------------------------------------------------------------------------
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY contracts/amm/Cargo.toml                   contracts/amm/Cargo.toml
COPY contracts/amm-sdk/Cargo.toml               contracts/amm-sdk/Cargo.toml
COPY contracts/pool_interfaces/Cargo.toml       contracts/pool_interfaces/Cargo.toml
COPY contracts/concentrated_liquidity/Cargo.toml contracts/concentrated_liquidity/Cargo.toml
COPY contracts/factory/Cargo.toml               contracts/factory/Cargo.toml
COPY contracts/router/Cargo.toml                contracts/router/Cargo.toml
COPY contracts/batch_router/Cargo.toml          contracts/batch_router/Cargo.toml
COPY contracts/batch_auction/Cargo.toml         contracts/batch_auction/Cargo.toml
COPY contracts/token/Cargo.toml                 contracts/token/Cargo.toml
COPY contracts/reserve_manager/Cargo.toml       contracts/reserve_manager/Cargo.toml
COPY contracts/amm-fuzz/Cargo.toml              contracts/amm-fuzz/Cargo.toml
COPY contracts/twap_consumer/Cargo.toml         contracts/twap_consumer/Cargo.toml
COPY contracts/twal_consumer/Cargo.toml         contracts/twal_consumer/Cargo.toml
COPY contracts/incentive_campaigns/Cargo.toml   contracts/incentive_campaigns/Cargo.toml
COPY contracts/dex_aggregator/Cargo.toml        contracts/dex_aggregator/Cargo.toml
COPY contracts/governance/Cargo.toml            contracts/governance/Cargo.toml
COPY contracts/staking/Cargo.toml               contracts/staking/Cargo.toml
COPY contracts/integration-tests/Cargo.toml     contracts/integration-tests/Cargo.toml
COPY contracts/oracle_aggregator/Cargo.toml     contracts/oracle_aggregator/Cargo.toml
COPY contracts/cl_position_nft/Cargo.toml       contracts/cl_position_nft/Cargo.toml
COPY contracts/v2_to_v3_migration/Cargo.toml    contracts/v2_to_v3_migration/Cargo.toml
COPY contracts/pol_vesting/Cargo.toml           contracts/pol_vesting/Cargo.toml
COPY benches/Cargo.toml                         benches/Cargo.toml
COPY packages/amm-simulator/Cargo.toml          packages/amm-simulator/Cargo.toml
COPY examples/flash_loan_receiver/Cargo.toml    examples/flash_loan_receiver/Cargo.toml

# Fetch all registry dependencies so the network is not needed during the
# actual build.  rustup installs the pinned toolchain (including
# wasm32v1-none) on first invocation via rust-toolchain.toml.  Only
# manifests are present here, so the script stubs each target's source file
# for the duration of the fetch.
COPY scripts/docker_fetch_deps.sh scripts/docker_fetch_deps.sh
RUN sh scripts/docker_fetch_deps.sh

# Copy the full source tree and build.
COPY . .

# Default command — build every deployable contract.
CMD ["make", "build"]
