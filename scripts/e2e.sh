#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
NETWORK="${STELLAR_NETWORK:-${NETWORK:-testnet}}"
SOURCE_ACCOUNT="${SOURCE_ACCOUNT:-soroban-amm-e2e-$(date +%s)}"
DEPLOY_ENV="${DEPLOY_ENV:-"$ROOT_DIR/.soroban-amm.e2e.env"}"

AMOUNT_A="${AMOUNT_A:-1000000}"
AMOUNT_B="${AMOUNT_B:-2000000}"
SWAP_AMOUNT_IN="${SWAP_AMOUNT_IN:-100000}"
MIN_SWAP_OUT="${MIN_SWAP_OUT:-150000}"
MAX_SWAP_OUT="${MAX_SWAP_OUT:-200000}"
DUST_LIMIT="${DUST_LIMIT:-10}"

PASS_COUNT=0
FAIL_COUNT=0

pass() {
  PASS_COUNT=$((PASS_COUNT + 1))
  printf '[PASS] %s\n' "$*"
}

fail() {
  FAIL_COUNT=$((FAIL_COUNT + 1))
  printf '[FAIL] %s\n' "$*" >&2
}

summary() {
  printf '\nSummary: %s passed, %s failed\n' "$PASS_COUNT" "$FAIL_COUNT"
}

on_error() {
  local exit_code=$?
  local line="$1"
  fail "unexpected error on line $line (exit $exit_code)"
  summary
  exit "$exit_code"
}

die() {
  fail "$*"
  summary
  exit 1
}

trap 'on_error $LINENO' ERR

require_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    die "missing required command: $1"
  fi
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

parse_i128() {
  grep -Eo -- '-?[0-9]+' | tail -n 1
}

field_value() {
  local field="$1"
  grep -Eo "\"?${field}\"?[[:space:]]*[:=][[:space:]]*-?[0-9]+" | grep -Eo -- '-?[0-9]+' | tail -n 1
}

assert_eq() {
  local label="$1"
  local actual="$2"
  local expected="$3"

  if [[ "$actual" == "$expected" ]]; then
    pass "$label: $actual"
  else
    die "$label: expected $expected, got $actual"
  fi
}

assert_between() {
  local label="$1"
  local actual="$2"
  local min="$3"
  local max="$4"

  if [[ ! "$actual" =~ ^-?[0-9]+$ ]]; then
    die "$label: expected numeric value, got '$actual'"
  fi

  if (( actual >= min && actual <= max )); then
    pass "$label: $actual is within [$min, $max]"
  else
    die "$label: expected value within [$min, $max], got $actual"
  fi
}

assert_lte_abs() {
  local label="$1"
  local actual="$2"
  local max_abs="$3"
  local abs="$actual"

  if [[ ! "$actual" =~ ^-?[0-9]+$ ]]; then
    die "$label: expected numeric value, got '$actual'"
  fi

  if (( abs < 0 )); then
    abs=$(( -abs ))
  fi

  if (( abs <= max_abs )); then
    pass "$label: $actual <= dust limit $max_abs"
  else
    die "$label: expected <= $max_abs dust, got $actual"
  fi
}

generate_and_fund_source() {
  if stellar keys address "$SOURCE_ACCOUNT" >/dev/null 2>&1; then
    pass "source account exists: $SOURCE_ACCOUNT"
    return
  fi

  if stellar keys generate "$SOURCE_ACCOUNT" --network "$NETWORK" --fund >/dev/null 2>&1; then
    pass "generated and funded source account: $SOURCE_ACCOUNT"
    return
  fi

  stellar keys generate --default-seed "$SOURCE_ACCOUNT" >/dev/null
  stellar keys fund "$SOURCE_ACCOUNT" --network "$NETWORK" >/dev/null
  pass "generated and funded source account: $SOURCE_ACCOUNT"
}

require_cmd stellar

generate_and_fund_source
SOURCE_PUBLIC_KEY="$(stellar keys address "$SOURCE_ACCOUNT")"
export NETWORK SOURCE_ACCOUNT SOURCE_PUBLIC_KEY DEPLOY_ENV

if "$ROOT_DIR/scripts/deploy.sh" >/dev/null; then
  pass "deployed and initialized fresh contracts"
else
  die "deploy script failed"
fi

# shellcheck disable=SC1090
source "$DEPLOY_ENV"

invoke "$TOKEN_A_CONTRACT_ID" mint \
  --to "$SOURCE_PUBLIC_KEY" \
  --amount "$AMOUNT_A" >/dev/null
pass "minted token A to test account"

invoke "$TOKEN_B_CONTRACT_ID" mint \
  --to "$SOURCE_PUBLIC_KEY" \
  --amount "$AMOUNT_B" >/dev/null
pass "minted token B to test account"

ADD_OUTPUT="$(invoke "$AMM_CONTRACT_ID" add_liquidity \
  --provider "$SOURCE_PUBLIC_KEY" \
  --amount_a "$AMOUNT_A" \
  --amount_b "$AMOUNT_B" \
  --min_shares 0)"
LP_SHARES="$(printf '%s\n' "$ADD_OUTPUT" | parse_i128)"
if [[ -z "$LP_SHARES" || "$LP_SHARES" -le 0 ]]; then
  die "add_liquidity did not return positive LP shares: $ADD_OUTPUT"
fi
pass "added liquidity and received LP shares: $LP_SHARES"

INFO_OUTPUT="$(invoke "$AMM_CONTRACT_ID" get_info)"
RESERVE_A="$(printf '%s\n' "$INFO_OUTPUT" | field_value reserve_a)"
RESERVE_B="$(printf '%s\n' "$INFO_OUTPUT" | field_value reserve_b)"
assert_eq "reserve A after add_liquidity" "$RESERVE_A" "$AMOUNT_A"
assert_eq "reserve B after add_liquidity" "$RESERVE_B" "$AMOUNT_B"

SWAP_OUTPUT="$(invoke "$AMM_CONTRACT_ID" swap \
  --trader "$SOURCE_PUBLIC_KEY" \
  --token_in "$TOKEN_A_CONTRACT_ID" \
  --amount_in "$SWAP_AMOUNT_IN" \
  --min_out 0)"
SWAP_OUT="$(printf '%s\n' "$SWAP_OUTPUT" | parse_i128)"
if [[ -z "$SWAP_OUT" ]]; then
  die "swap did not return an amount: $SWAP_OUTPUT"
fi
assert_between "swap output" "$SWAP_OUT" "$MIN_SWAP_OUT" "$MAX_SWAP_OUT"

invoke "$AMM_CONTRACT_ID" remove_liquidity \
  --provider "$SOURCE_PUBLIC_KEY" \
  --shares "$LP_SHARES" \
  --min_a 0 \
  --min_b 0 >/dev/null
pass "removed all LP shares"

FINAL_INFO_OUTPUT="$(invoke "$AMM_CONTRACT_ID" get_info)"
FINAL_RESERVE_A="$(printf '%s\n' "$FINAL_INFO_OUTPUT" | field_value reserve_a)"
FINAL_RESERVE_B="$(printf '%s\n' "$FINAL_INFO_OUTPUT" | field_value reserve_b)"
assert_lte_abs "final reserve A" "$FINAL_RESERVE_A" "$DUST_LIMIT"
assert_lte_abs "final reserve B" "$FINAL_RESERVE_B" "$DUST_LIMIT"

summary
printf 'E2E PASS\n'
