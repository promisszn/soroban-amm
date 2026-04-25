#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
NETWORK="${1:-${STELLAR_NETWORK:-${NETWORK:-testnet}}}"
SOURCE_ACCOUNT="${SOURCE_ACCOUNT:-soroban-amm-deployer}"
DEPLOY_ENV="${DEPLOY_ENV:-"$ROOT_DIR/.soroban-amm.deploy.env"}"
ADMIN_ADDRESS="${ADMIN_ADDRESS:-}"
FEE_RECIPIENT="${FEE_RECIPIENT:-}"
PROTOCOL_FEE_BPS="${PROTOCOL_FEE_BPS:-0}"

TOKEN_WASM="${TOKEN_WASM:-"$ROOT_DIR/target/wasm32-unknown-unknown/release/token.wasm"}"
AMM_WASM="${AMM_WASM:-"$ROOT_DIR/target/wasm32-unknown-unknown/release/amm.wasm"}"

log() {
  printf '[deploy] %s\n' "$*" >&2
}

require_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    printf '[deploy] missing required command: %s\n' "$1" >&2
    exit 1
  fi
}

extract_contract_id() {
  grep -Eo 'C[A-Z0-9]{55}' | tail -n 1
}

deploy_contract() {
  local wasm="$1"
  local output contract_id

  output="$(stellar contract deploy \
    --wasm "$wasm" \
    --network "$NETWORK" \
    --source "$SOURCE_ACCOUNT")"

  contract_id="$(printf '%s\n' "$output" | extract_contract_id)"
  if [[ -z "$contract_id" ]]; then
    printf '[deploy] could not parse contract id from output:\n%s\n' "$output" >&2
    exit 1
  fi

  printf '%s\n' "$contract_id"
}

invoke() {
  local contract_id="$1"
  shift

  stellar contract invoke \
    --id "$contract_id" \
    --network "$NETWORK" \
    --source "$SOURCE_ACCOUNT" \
    -- "$@"
}

generate_and_fund_source() {
  if stellar keys address "$SOURCE_ACCOUNT" >/dev/null 2>&1; then
    log "source account exists: $SOURCE_ACCOUNT"
    return
  fi

  log "generating and funding source account: $SOURCE_ACCOUNT"
  if stellar keys generate "$SOURCE_ACCOUNT" --network "$NETWORK" --fund >/dev/null 2>&1; then
    return
  fi

  stellar keys generate --default-seed "$SOURCE_ACCOUNT" >/dev/null
  stellar keys fund "$SOURCE_ACCOUNT" --network "$NETWORK" >/dev/null
}

require_cmd stellar

generate_and_fund_source
SOURCE_PUBLIC_KEY="$(stellar keys address "$SOURCE_ACCOUNT")"

if [[ -z "$ADMIN_ADDRESS" ]]; then
  ADMIN_ADDRESS="$SOURCE_PUBLIC_KEY"
fi

if [[ -z "$FEE_RECIPIENT" ]]; then
  FEE_RECIPIENT="$SOURCE_PUBLIC_KEY"
fi

if [[ ! -f "$TOKEN_WASM" || ! -f "$AMM_WASM" ]]; then
  require_cmd cargo
  log "building release WASM artifacts"
  (cd "$ROOT_DIR" && cargo build --release --target wasm32-unknown-unknown)
fi

log "deploying Token A"
TOKEN_A_CONTRACT_ID="$(deploy_contract "$TOKEN_WASM")"

log "deploying Token B"
TOKEN_B_CONTRACT_ID="$(deploy_contract "$TOKEN_WASM")"

log "deploying LP token"
LP_TOKEN_CONTRACT_ID="$(deploy_contract "$TOKEN_WASM")"

log "deploying AMM"
AMM_CONTRACT_ID="$(deploy_contract "$AMM_WASM")"

log "initializing Token A"
invoke "$TOKEN_A_CONTRACT_ID" initialize \
  --admin "$SOURCE_PUBLIC_KEY" \
  --name "E2E Token A" \
  --symbol "E2EA" \
  --decimals 7 >/dev/null

log "initializing Token B"
invoke "$TOKEN_B_CONTRACT_ID" initialize \
  --admin "$SOURCE_PUBLIC_KEY" \
  --name "E2E Token B" \
  --symbol "E2EB" \
  --decimals 7 >/dev/null

log "initializing LP token"
invoke "$LP_TOKEN_CONTRACT_ID" initialize \
  --admin "$AMM_CONTRACT_ID" \
  --name "E2E AMM LP" \
  --symbol "E2ELP" \
  --decimals 7 >/dev/null

log "initializing AMM"
invoke "$AMM_CONTRACT_ID" initialize \
--admin "$ADMIN_ADDRESS" \
  --token_a "$TOKEN_A_CONTRACT_ID" \
  --token_b "$TOKEN_B_CONTRACT_ID" \
  --lp_token "$LP_TOKEN_CONTRACT_ID" \
  --fee_bps 30 \
  --fee_recipient "$FEE_RECIPIENT" \
  --protocol_fee_bps "$PROTOCOL_FEE_BPS" >/dev/null

cat >"$DEPLOY_ENV" <<EOF
export NETWORK="$NETWORK"
export SOURCE_ACCOUNT="$SOURCE_ACCOUNT"
export SOURCE_PUBLIC_KEY="$SOURCE_PUBLIC_KEY"
export TOKEN_A_CONTRACT_ID="$TOKEN_A_CONTRACT_ID"
export TOKEN_B_CONTRACT_ID="$TOKEN_B_CONTRACT_ID"
export LP_TOKEN_CONTRACT_ID="$LP_TOKEN_CONTRACT_ID"
export AMM_CONTRACT_ID="$AMM_CONTRACT_ID"
EOF

log "wrote deployment env to $DEPLOY_ENV"
log "deployment successful"
printf 'AMM_CONTRACT_ID=%s\n' "$AMM_CONTRACT_ID"
printf 'TOKEN_A_CONTRACT_ID=%s\n' "$TOKEN_A_CONTRACT_ID"
printf 'TOKEN_B_CONTRACT_ID=%s\n' "$TOKEN_B_CONTRACT_ID"
printf 'LP_TOKEN_CONTRACT_ID=%s\n' "$LP_TOKEN_CONTRACT_ID"
