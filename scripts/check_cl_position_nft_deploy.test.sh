#!/usr/bin/env bash
# Exercise cl_position_nft deployment read-back checks with a stub stellar CLI.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
mkdir -p "$scratch/bin"

cat > "$scratch/bin/stellar" <<'STUB'
#!/usr/bin/env bash
fn=""
seen=0
for arg in "$@"; do
  if [[ $seen == 1 ]]; then fn="$arg"; break; fi
  if [[ "$arg" == "--" ]]; then seen=1; fi
done
case "$fn" in
  admin) printf '%s\n' "$NFT_ADMIN" ;;
  cl_pool) printf '%s\n' "$NFT_POOL" ;;
  position_nft) printf '%s\n' "\"$NFT_ID\"" ;;
  *) echo "unexpected read: $fn" >&2; exit 1 ;;
esac
STUB
chmod +x "$scratch/bin/stellar"

rep() { printf "$1%.0s" $(seq 1 55); }
ADMIN="G$(rep A)"
OTHER_ADMIN="G$(rep C)"
POOL="C$(rep B)"
NFT="C$(rep D)"

run_deploy() {
  local contract_admin="$1" contract_pool="$2"
  local deploy_env="$scratch/deploy.env"
  printf "export CL_POSITION_NFT_CONTRACT_ID='%s'\n" "$NFT" > "$deploy_env"
  printf "export CL_POSITION_NFT_INITIALIZED='1'\n" >> "$deploy_env"
  (
    export PATH="$scratch/bin:$PATH"
    export DEPLOY_ENV="$deploy_env" NETWORK=testnet SOURCE_ACCOUNT=stub
    export ADMIN_ADDRESS="$ADMIN" CL_POOL_CONTRACT_ID="$POOL"
    export NFT_ID="$NFT" NFT_ADMIN="$contract_admin" NFT_POOL="$contract_pool"
    # shellcheck source=/dev/null
    source "$here/deploy/common.sh"
    # shellcheck source=/dev/null
    source "$here/deploy/cl_position_nft.sh"
    load_env
    deploy_cl_position_nft
  ) > "$scratch/output" 2>&1
}

if ! run_deploy "$ADMIN" "$POOL"; then
  echo "FAIL: matching admin and pool were rejected" >&2
  cat "$scratch/output" >&2
  exit 1
fi
grep -q "verified cl_position_nft" "$scratch/output"
echo "PASS: matching admin and pool"

if run_deploy "$OTHER_ADMIN" "$POOL"; then
  echo "FAIL: mismatched admin was accepted" >&2
  exit 1
fi
grep -q "admin is not $ADMIN" "$scratch/output"
echo "PASS: mismatched admin is rejected"

if run_deploy "$ADMIN" "$OTHER_ADMIN"; then
  echo "FAIL: mismatched CL pool was accepted" >&2
  exit 1
fi
grep -q "cl_pool is not $POOL" "$scratch/output"
echo "PASS: mismatched CL pool is rejected"