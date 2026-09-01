#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WASM_DIR="${WASM_DIR:-$ROOT_DIR/target/wasm32v1-none/release}"
MAX_BYTES="${WASM_MAX_BYTES:-204800}"
FAIL_ON_LIMIT=false

if [[ "${1:-}" == "--fail-on-limit" ]]; then
  FAIL_ON_LIMIT=true
elif [[ $# -gt 0 ]]; then
  echo "Usage: $0 [--fail-on-limit]" >&2
  exit 2
fi

shopt -s nullglob
artifacts=("$WASM_DIR"/*.wasm)

printf 'Contract WASM size report\n'
printf '%s\n' '-------------------------'

if [[ ${#artifacts[@]} -eq 0 ]]; then
  echo "No WASM artifacts found in $WASM_DIR" >&2
  exit 1
fi

status=0
for wasm in "${artifacts[@]}"; do
  size=$(wc -c < "$wasm")
  human=$(numfmt --to=iec-i --suffix=B --format='%.1f' "$size" 2>/dev/null || printf '%sB' "$size")
  relative=$(realpath --relative-to="$ROOT_DIR" "$wasm")
  if (( size > MAX_BYTES )); then
    printf '%s: %s (%s bytes) EXCEEDS LIMIT (%s bytes)\n' "$relative" "$human" "$size" "$MAX_BYTES"
    status=1
  else
    printf '%s: %s (%s bytes)\n' "$relative" "$human" "$size"
  fi
done

printf '%s\n' '-------------------------'
if $FAIL_ON_LIMIT && (( status != 0 )); then
  exit "$status"
fi
exit 0
