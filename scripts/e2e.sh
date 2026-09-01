#!/usr/bin/env bash
# e2e.sh — thin wrapper kept for backward compatibility (smoke-test.yml and
# `make e2e` both call this path). The real flows live under scripts/e2e/,
# split by contract so the suite can grow without becoming one flat script.
#
# See `bash scripts/e2e/run.sh --help` for flags (--only, --skip).
set -Eeuo pipefail
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
exec bash "$ROOT_DIR/scripts/e2e/run.sh" "$@"
