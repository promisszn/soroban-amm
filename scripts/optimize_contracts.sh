#!/usr/bin/env bash
set -euo pipefail

# Build and optimize all contracts producing WASM artifacts.
# Requires: cargo and the wasm32v1-none Rust target. The Stellar CLI is used
# when available; wasm-opt is supported as a fallback.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET="wasm32v1-none"
WASM_DIR="$ROOT_DIR/target/$TARGET/release"
OUTPUT_DIR="${OUTPUT_DIR:-$ROOT_DIR/optimized-artifacts}"

cd "$ROOT_DIR"
printf '[optimize] Building workspace for %s\n' "$TARGET"
cargo build --workspace --release --target "$TARGET"

shopt -s nullglob
artifacts=("$WASM_DIR"/*.wasm)
if [[ ${#artifacts[@]} -eq 0 ]]; then
  echo "[optimize] no WASM artifacts found in $WASM_DIR" >&2
  exit 1
fi

mkdir -p "$OUTPUT_DIR"
for wasm in "${artifacts[@]}"; do
  name=$(basename "$wasm")
  output="$OUTPUT_DIR/$name"
  printf '[optimize] %s -> %s\n' "${wasm#$ROOT_DIR/}" "${output#$ROOT_DIR/}"

  if command -v stellar >/dev/null 2>&1; then
    stellar contract optimize --wasm "$wasm" --output "$output"
  elif command -v wasm-opt >/dev/null 2>&1; then
    wasm-opt -O3 --strip-dwarf -o "$output" "$wasm"
  else
    echo '[optimize] neither stellar CLI nor wasm-opt is installed' >&2
    exit 1
  fi
done

printf '[optimize] Optimized artifacts are in %s\n' "${OUTPUT_DIR#$ROOT_DIR/}"
