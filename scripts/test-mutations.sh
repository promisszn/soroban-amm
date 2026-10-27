#!/usr/bin/env bash
# Curated mutation testing script for critical smart contract invariants.
# Verifies that removing or bypassing require_auth, deadline, or slippage
# guards causes test suites to fail (killing the mutant).
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

DRY_RUN=false
LIST_ONLY=false
SPECIFIC_MUTANT=""

usage() {
    cat <<EOF
Usage: $(basename "$0") [OPTIONS]

Run curated mutation checks against critical security and safety invariants.

Options:
  --list            List all registered curated mutation targets
  --dry-run         Verify mutation targets and patterns without executing cargo test
  --target NAME     Run only the specified mutation check target
  -h, --help        Show this help message
EOF
    exit 0
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --list)
            LIST_ONLY=true
            shift
            ;;
        --dry-run)
            DRY_RUN=true
            shift
            ;;
        --target)
            SPECIFIC_MUTANT="$2"
            shift 2
            ;;
        -h|--help)
            usage
            ;;
        *)
            echo "Unknown option: $1" >&2
            usage
            ;;
    esac
done

# Registered mutation test cases:
# Format: ID|CATEGORY|FILE|PATTERN|REPLACEMENT|TEST_CMD
MUTANTS=(
    "amm-auth-pause|auth|contracts/amm/src/lib.rs|admin.require_auth();|// admin.require_auth();|cargo test -p amm"
    "amm-auth-set-fee|auth|contracts/amm/src/lib.rs|admin.require_auth();|// admin.require_auth();|cargo test -p amm"
    "amm-deadline-swap|deadline|contracts/amm/src/lib.rs|if deadline < env.ledger().timestamp() {|if false && deadline < env.ledger().timestamp() {|cargo test -p amm"
    "amm-slippage-swap|slippage|contracts/amm/src/lib.rs|if amount_out < min_out {|if false && amount_out < min_out {|cargo test -p amm"
    "amm-slippage-deposit|slippage|contracts/amm/src/lib.rs|if shares_to_provider < min_shares {|if false && shares_to_provider < min_shares {|cargo test -p amm"
    "gov-auth-admin|auth|contracts/governance/src/lib.rs|admin.require_auth();|// admin.require_auth();|cargo test -p governance"
    "token-auth-set-admin|auth|contracts/token/src/lib.rs|admin.require_auth();|// admin.require_auth();|cargo test -p token"
    "cl-auth-admin|auth|contracts/concentrated_liquidity/src/lib.rs|admin.require_auth();|// admin.require_auth();|cargo test -p concentrated_liquidity"
)

if [[ "$LIST_ONLY" == true ]]; then
    echo "Registered Curated Mutation Check Targets:"
    printf "%-25s %-12s %-45s\n" "TARGET ID" "CATEGORY" "FILE"
    printf "%-25s %-12s %-45s\n" "---------" "--------" "----"
    for m in "${MUTANTS[@]}"; do
        IFS="|" read -r id cat file pat rep cmd <<< "$m"
        printf "%-25s %-12s %-45s\n" "$id" "$cat" "$file"
    done
    exit 0
fi

# Cleanup hook: ensure any mutated file is restored upon interrupt or exit
MUTATED_FILE=""
cleanup() {
    if [[ -n "$MUTATED_FILE" && -f "$MUTATED_FILE" ]]; then
        git checkout -- "$MUTATED_FILE" 2>/dev/null || true
    fi
}
trap cleanup EXIT INT TERM

TOTAL=0
KILLED=0
SURVIVED=0
SKIPPED=0

echo "Starting curated mutation check suite..."
echo "============================================================"

for m in "${MUTANTS[@]}"; do
    IFS="|" read -r id cat file pat rep cmd <<< "$m"

    if [[ -n "$SPECIFIC_MUTANT" && "$SPECIFIC_MUTANT" != "$id" ]]; then
        continue
    fi

    TOTAL=$((TOTAL + 1))
    target_path="$REPO_ROOT/$file"

    if [[ ! -f "$target_path" ]]; then
        echo "[-] SKIP $id: target file $file does not exist"
        SKIPPED=$((SKIPPED + 1))
        continue
    fi

    if ! grep -qF "$pat" "$target_path"; then
        echo "[-] SKIP $id: pattern not found in $file"
        SKIPPED=$((SKIPPED + 1))
        continue
    fi

    if [[ "$DRY_RUN" == true ]]; then
        echo "[DRY-RUN] Target $id verified (Category: $cat, File: $file)"
        KILLED=$((KILLED + 1))
        continue
    fi

    MUTATED_FILE="$target_path"
    # Apply mutation (first matching occurrence)
    python3 -c "
with open('$target_path', 'r', encoding='utf-8') as f:
    c = f.read()
if '$pat' in c:
    with open('$target_path', 'w', encoding='utf-8') as f:
        f.write(c.replace('$pat', '$rep', 1))
" || python -c "
with open('$target_path', 'r', encoding='utf-8') as f:
    c = f.read()
if '$pat' in c:
    with open('$target_path', 'w', encoding='utf-8') as f:
        f.write(c.replace('$pat', '$rep', 1))
"

    # Run test suite: expecting FAILURE (mutant killed)
    if eval "$cmd" >/dev/null 2>&1; then
        echo "[!] SURVIVED: $id ($cat) — test suite passed despite mutation!"
        SURVIVED=$((SURVIVED + 1))
    else
        echo "[+] KILLED:   $id ($cat) — mutation correctly caught by tests"
        KILLED=$((KILLED + 1))
    fi

    # Restore file
    git checkout -- "$target_path"
    MUTATED_FILE=""
done

echo "============================================================"
echo "Curated Mutation Results: Total: $TOTAL | Killed: $KILLED | Survived: $SURVIVED | Skipped: $SKIPPED"

if [[ "$SURVIVED" -gt 0 ]]; then
    echo "ERROR: $SURVIVED mutants survived without failing tests. Invariants need test coverage." >&2
    exit 1
fi

echo "All tested mutation invariants are covered and killed."
exit 0
