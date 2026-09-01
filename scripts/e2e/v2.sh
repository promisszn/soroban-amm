#!/usr/bin/env bash
# v2.sh — end-to-end flow for the constant-product (V2) AMM path:
# mint -> add_liquidity -> swap -> remove_liquidity, asserting real numeric
# outcomes at each step (not just exit codes).
set -Eeuo pipefail

run_v2_flow() {
  CURRENT_FLOW="v2"

  local amount_a="${AMOUNT_A:-1000000}"
  local amount_b="${AMOUNT_B:-2000000}"
  local swap_amount_in="${SWAP_AMOUNT_IN:-100000}"
  local min_swap_out="${MIN_SWAP_OUT:-150000}"
  local max_swap_out="${MAX_SWAP_OUT:-200000}"
  local dust_limit="${DUST_LIMIT:-10}"

  invoke "$TOKEN_A_CONTRACT_ID" mint \
    --to "$SOURCE_PUBLIC_KEY" \
    --amount "$amount_a" >/dev/null
  pass "v2: minted token A to test account"

  invoke "$TOKEN_B_CONTRACT_ID" mint \
    --to "$SOURCE_PUBLIC_KEY" \
    --amount "$amount_b" >/dev/null
  pass "v2: minted token B to test account"

  local deadline
  deadline=$(( $(date +%s) + 300 ))

  local add_output lp_shares
  add_output="$(invoke "$AMM_CONTRACT_ID" add_liquidity \
    --provider "$SOURCE_PUBLIC_KEY" \
    --amount_a "$amount_a" \
    --amount_b "$amount_b" \
    --min_shares 0 \
    --deadline "$deadline")"
  lp_shares="$(printf '%s\n' "$add_output" | parse_i128)"
  if [[ -z "$lp_shares" || "$lp_shares" -le 0 ]]; then
    die "v2: add_liquidity did not return positive LP shares: $add_output"
  fi
  pass "v2: added liquidity and received LP shares: $lp_shares"

  local info_output reserve_a reserve_b
  info_output="$(invoke "$AMM_CONTRACT_ID" get_info)"
  reserve_a="$(printf '%s\n' "$info_output" | field_value reserve_a)"
  reserve_b="$(printf '%s\n' "$info_output" | field_value reserve_b)"
  assert_eq "v2: reserve A after add_liquidity" "$reserve_a" "$amount_a"
  assert_eq "v2: reserve B after add_liquidity" "$reserve_b" "$amount_b"

  deadline=$(( $(date +%s) + 300 ))

  local swap_output swap_out
  swap_output="$(invoke "$AMM_CONTRACT_ID" swap \
    --trader "$SOURCE_PUBLIC_KEY" \
    --token_in "$TOKEN_A_CONTRACT_ID" \
    --amount_in "$swap_amount_in" \
    --min_out 0 \
    --deadline "$deadline")"
  swap_out="$(printf '%s\n' "$swap_output" | parse_i128)"
  if [[ -z "$swap_out" ]]; then
    die "v2: swap did not return an amount: $swap_output"
  fi
  assert_between "v2: swap output" "$swap_out" "$min_swap_out" "$max_swap_out"

  deadline=$(( $(date +%s) + 300 ))

  invoke "$AMM_CONTRACT_ID" remove_liquidity \
    --provider "$SOURCE_PUBLIC_KEY" \
    --shares "$lp_shares" \
    --min_a 0 \
    --min_b 0 \
    --deadline "$deadline" >/dev/null
  pass "v2: removed all LP shares"

  local final_info final_reserve_a final_reserve_b
  final_info="$(invoke "$AMM_CONTRACT_ID" get_info)"
  final_reserve_a="$(printf '%s\n' "$final_info" | field_value reserve_a)"
  final_reserve_b="$(printf '%s\n' "$final_info" | field_value reserve_b)"
  assert_lte_abs "v2: final reserve A" "$final_reserve_a" "$dust_limit"
  assert_lte_abs "v2: final reserve B" "$final_reserve_b" "$dust_limit"
}
