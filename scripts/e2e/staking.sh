#!/usr/bin/env bash
# staking.sh — end-to-end flow for the LP staking path:
# stake -> add_rewards -> update_rewards -> claim -> unstake, asserting
# reward accrual is nonzero and the LP round-trips back to the staker.
#
# Adds its own fresh liquidity to the shared AMM_POOL_CONTRACT_ID to obtain
# LP tokens to stake, rather than depending on v2.sh having run first (v2.sh
# removes all its liquidity by the end of its own flow).
set -Eeuo pipefail

run_staking_flow() {
  CURRENT_FLOW="staking"

  if [[ -z "${STAKING_CONTRACT_ID:-}" ]]; then
    die "staking: STAKING_CONTRACT_ID not set — did deploy.sh initialize staking?"
  fi
  if [[ -z "${LP_TOKEN_CONTRACT_ID:-}" || -z "${REWARD_TOKEN_CONTRACT_ID:-}" ]]; then
    die "staking: LP_TOKEN_CONTRACT_ID / REWARD_TOKEN_CONTRACT_ID not set"
  fi
  if [[ -z "${AMM_POOL_CONTRACT_ID:-}" ]]; then
    die "staking: AMM_POOL_CONTRACT_ID not set — cannot mint LP tokens to stake"
  fi

  local staker="$SOURCE_PUBLIC_KEY"
  local amount_a="${STAKING_AMOUNT_A:-500000}"
  local amount_b="${STAKING_AMOUNT_B:-1000000}"
  local reward_amount="${STAKING_REWARD_AMOUNT:-100000}"

  # ── obtain LP tokens by adding liquidity to the shared AMM pool ─────────
  invoke "$TOKEN_A_CONTRACT_ID" mint --to "$staker" --amount "$amount_a" >/dev/null
  invoke "$TOKEN_B_CONTRACT_ID" mint --to "$staker" --amount "$amount_b" >/dev/null

  local deadline add_output lp_shares
  deadline=$(( $(date +%s) + 300 ))
  add_output=$(invoke "$AMM_POOL_CONTRACT_ID" add_liquidity \
    --provider "$staker" \
    --amount_a "$amount_a" \
    --amount_b "$amount_b" \
    --min_shares 0 \
    --deadline "$deadline")
  lp_shares=$(printf '%s\n' "$add_output" | parse_i128)
  assert_gt "staking: obtained LP shares to stake" "$lp_shares" 0

  # ── stake (no lock, 1x boost) ────────────────────────────────────────────
  local lp_balance_before
  lp_balance_before=$(invoke "$LP_TOKEN_CONTRACT_ID" balance --id "$staker" | parse_i128)

  invoke "$STAKING_CONTRACT_ID" stake --staker "$staker" --amount "$lp_shares" >/dev/null
  pass "staking: staked $lp_shares LP tokens"

  local lp_balance_after_stake
  lp_balance_after_stake=$(invoke "$LP_TOKEN_CONTRACT_ID" balance --id "$staker" | parse_i128)
  assert_eq "staking: LP balance decreased by staked amount" \
    "$lp_balance_after_stake" "$(( lp_balance_before - lp_shares ))"

  # ── add_rewards + update_rewards ─────────────────────────────────────────
  # add_rewards only deposits reward tokens into the pool's balance;
  # update_rewards is the separate call that advances the per-share
  # accumulator so stakers actually start accruing (see
  # contracts/staking/src/lib.rs).
  invoke "$REWARD_TOKEN_CONTRACT_ID" mint --to "$SOURCE_PUBLIC_KEY" --amount "$reward_amount" >/dev/null
  invoke "$STAKING_CONTRACT_ID" add_rewards --admin "$SOURCE_PUBLIC_KEY" --amount "$reward_amount" >/dev/null
  pass "staking: added $reward_amount reward tokens to the pool"

  invoke "$STAKING_CONTRACT_ID" update_rewards --admin "$SOURCE_PUBLIC_KEY" --new_rewards "$reward_amount" >/dev/null
  pass "staking: distributed rewards across the pool"

  local pending
  pending=$(invoke "$STAKING_CONTRACT_ID" pending_rewards --staker "$staker" | parse_i128)
  assert_gt "staking: pending_rewards accrued" "$pending" 0

  # ── claim ────────────────────────────────────────────────────────────────
  local reward_balance_before
  reward_balance_before=$(invoke "$REWARD_TOKEN_CONTRACT_ID" balance --id "$staker" | parse_i128)

  local claimed
  claimed=$(invoke "$STAKING_CONTRACT_ID" claim --staker "$staker" | parse_i128)
  assert_gt "staking: claim returned nonzero rewards" "$claimed" 0

  local reward_balance_after
  reward_balance_after=$(invoke "$REWARD_TOKEN_CONTRACT_ID" balance --id "$staker" | parse_i128)
  assert_eq "staking: reward token balance increased by claimed amount" \
    "$reward_balance_after" "$(( reward_balance_before + claimed ))"

  # ── unstake ──────────────────────────────────────────────────────────────
  # No lock was used (stake, not stake_locked), so unstake is available
  # immediately.
  invoke "$STAKING_CONTRACT_ID" unstake --staker "$staker" --amount "$lp_shares" >/dev/null
  pass "staking: unstaked $lp_shares LP tokens"

  local lp_balance_final
  lp_balance_final=$(invoke "$LP_TOKEN_CONTRACT_ID" balance --id "$staker" | parse_i128)
  assert_eq "staking: LP balance round-trips back to pre-stake balance" \
    "$lp_balance_final" "$lp_balance_before"
}
