#!/usr/bin/env bash
# common.sh — shared helpers for the e2e test flows in scripts/e2e/*.sh
# Sourceable only; no side effects beyond defining functions/vars.

ROOT_DIR="${ROOT_DIR:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
NETWORK="${STELLAR_NETWORK:-${NETWORK:-testnet}}"
SOURCE_ACCOUNT="${SOURCE_ACCOUNT:-soroban-amm-e2e-$(date +%s)}"
DEPLOY_ENV="${DEPLOY_ENV:-"$ROOT_DIR/.soroban-amm.e2e.env"}"

PASS_COUNT=0
FAIL_COUNT=0
CURRENT_FLOW=""

pass() {
  PASS_COUNT=$((PASS_COUNT + 1))
  printf '[PASS] %s\n' "$*"
}

fail() {
  FAIL_COUNT=$((FAIL_COUNT + 1))
  printf '[FAIL] %s\n' "$*" >&2
}

die() {
  fail "$*"
  exit 1
}

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

extract_contract_id() {
  grep -Eo 'C[A-Z0-9]{55}' | tail -n 1
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

assert_gt() {
  local label="$1"
  local actual="$2"
  local floor="$3"

  if [[ ! "$actual" =~ ^-?[0-9]+$ ]]; then
    die "$label: expected numeric value, got '$actual'"
  fi
  if (( actual > floor )); then
    pass "$label: $actual > $floor"
  else
    die "$label: expected > $floor, got $actual"
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
