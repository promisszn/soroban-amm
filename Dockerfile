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

# Install Stellar CLI pinned to version 25.1.0
RUN cargo install stellar-cli --version 25.1.0 --locked

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
