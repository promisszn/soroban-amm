#!/usr/bin/env bash
# Test harness for test-mutations.sh in isolated mock environment.
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
guard="$script_dir/test-mutations.sh"

echo "Testing test-mutations.sh CLI options..."

# 1. Test --help
bash "$guard" --help >/dev/null
echo "PASS: --help works"

# 2. Test --list
list_out=$(bash "$guard" --list)
if ! echo "$list_out" | grep -q "amm-auth-pause"; then
    echo "FAIL: --list missing amm-auth-pause" >&2
    exit 1
fi
echo "PASS: --list outputs registered targets"

# 3. Test --dry-run
dry_out=$(bash "$guard" --dry-run)
if ! echo "$dry_out" | grep -q "DRY-RUN"; then
    echo "FAIL: --dry-run output missing DRY-RUN prefix" >&2
    exit 1
fi
echo "PASS: --dry-run verifies targets"

echo "All test-mutations.sh self-tests passed."
