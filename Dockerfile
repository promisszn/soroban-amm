FROM rust:1.93.0-slim-bookworm

# Install system dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    build-essential \
    ca-certificates \
    curl \
    git \
    make \
    && rm -rf /var/lib/apt/lists/*

# Install wasm32 target
RUN rustup target add wasm32-unknown-unknown

# Install the Stellar CLI at the repo-wide pinned version. `.stellar-version`
# is the single source of truth, read here and by .github/workflows/release.yml
# and .github/workflows/smoke-test.yml, so the optimizer that produces release
# artifacts, the container contributors build in, and the smoke test that
# verifies a deploy cannot drift to different versions.
COPY .stellar-version /tmp/.stellar-version
RUN cargo install stellar-cli --version "$(cat /tmp/.stellar-version)" --locked

# Set working directory
WORKDIR /app

# Copy only Cargo.toml/lock first to leverage Docker cache
# (This assumes a flat workspace structure, adjust if needed)
# COPY Cargo.toml Cargo.lock ./
# RUN cargo fetch

# Copy the rest of the application
COPY . .

# Default command
CMD ["make", "build"]

