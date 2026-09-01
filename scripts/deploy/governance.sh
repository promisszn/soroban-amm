#!/usr/bin/env bash
# governance.sh — deploy and initialize Governance contract
# Sourceable module for deploy.sh

deploy_governance() {
  CURRENT_CONTRACT="governance"
  log "== governance =="

  local gov_wasm
  gov_wasm=$(wasm_path governance)

  if [[ -z "${AMM_POOL_CONTRACT_ID:-}" ]]; then AMM_POOL_CONTRACT_ID=$(get_persisted AMM_POOL_CONTRACT_ID || echo ""); fi
  if [[ -z "${LP_TOKEN_CONTRACT_ID:-}" ]]; then LP_TOKEN_CONTRACT_ID=$(get_persisted LP_TOKEN_CONTRACT_ID || echo ""); fi

  if [[ -z "${AMM_POOL_CONTRACT_ID:-}" || -z "${LP_TOKEN_CONTRACT_ID:-}" ]]; then
    warn "missing AMM pool or LP token — deferring governance deploy"
    # Try to discover from factory if available
    if [[ -n "${FACTORY_CONTRACT_ID:-}" ]]; then
      local disc_pool
      disc_pool=$(invoke_read "$FACTORY_CONTRACT_ID" -- get_pool --token_a "$TOKEN_A_CONTRACT_ID" --token_b "$TOKEN_B_CONTRACT_ID" 2>&1 | grep -Eo 'C[A-Z0-9]{55}' | tail -n 1 || echo "")
      if [[ -n "$disc_pool" ]]; then
        AMM_POOL_CONTRACT_ID="$disc_pool"
        LP_TOKEN_CONTRACT_ID=$(invoke_read "$FACTORY_CONTRACT_ID" -- get_lp_token --pool "$disc_pool" 2>&1 | grep -Eo 'C[A-Z0-9]{55}' | tail -n 1 || echo "")
        export AMM_POOL_CONTRACT_ID LP_TOKEN_CONTRACT_ID
      fi
    fi
    if [[ -z "${AMM_POOL_CONTRACT_ID:-}" ]]; then
      warn "still missing AMM pool — skipping governance"
      return 0
    fi
  fi

  if should_skip_persisted "GOVERNANCE_CONTRACT_ID"; then
    GOVERNANCE_CONTRACT_ID=$(get_persisted GOVERNANCE_CONTRACT_ID)
    log "skipping governance deploy (already at $GOVERNANCE_CONTRACT_ID)"
  else
    if ! should_deploy "governance"; then
      log "skipping governance deploy (--only/--skip filter)"
      GOVERNANCE_CONTRACT_ID=$(get_persisted GOVERNANCE_CONTRACT_ID || echo "")
      if [[ -z "$GOVERNANCE_CONTRACT_ID" ]]; then return 0; fi
    else
      CURRENT_STEP="deploy governance"
      log "deploying governance: $gov_wasm"
      if [[ ! -f "$gov_wasm" ]]; then
        warn "governance WASM not found: $gov_wasm — skipping"
        return 0
      fi
      GOVERNANCE_CONTRACT_ID=$(deploy_contract "$gov_wasm")
      persist_var "GOVERNANCE_CONTRACT_ID" "$GOVERNANCE_CONTRACT_ID"
      log "governance: $GOVERNANCE_CONTRACT_ID"
    fi
  fi

  export GOVERNANCE_CONTRACT_ID

  if [[ -z "${GOVERNANCE_CONTRACT_ID:-}" ]]; then return 0; fi

  if should_skip_persisted "GOVERNANCE_INITIALIZED"; then
    log "skipping governance initialize (already done)"
  else
    if ! should_deploy "governance"; then
      log "skipping governance initialize (--only/--skip filter)"
    else
      CURRENT_STEP="initialize governance"
      log "initializing governance: amm=$AMM_POOL_CONTRACT_ID lp=$LP_TOKEN_CONTRACT_ID voting=$DEFAULT_VOTING_PERIOD_SECS timelock=$DEFAULT_TIMELOCK_SECS quorum=$DEFAULT_QUORUM_BPS stake=$DEFAULT_MIN_PROPOSER_STAKE_BPS"
      if ! invoke "$GOVERNANCE_CONTRACT_ID" initialize \
          --admin "$ADMIN_ADDRESS" \
          --amm_pool "$AMM_POOL_CONTRACT_ID" \
          --lp_token "$LP_TOKEN_CONTRACT_ID" \
          --voting_period_secs "$DEFAULT_VOTING_PERIOD_SECS" \
          --timelock_secs "$DEFAULT_TIMELOCK_SECS" \
          --quorum_bps "$DEFAULT_QUORUM_BPS" \
          --min_proposer_stake_bps "$DEFAULT_MIN_PROPOSER_STAKE_BPS" >/dev/null 2>&1; then
        if invoke_read "$GOVERNANCE_CONTRACT_ID" -- get_params >/dev/null 2>&1; then
          log "governance already initialized"
        else
          die "failed to initialize governance"
        fi
      else
        log "governance initialized"
      fi
      persist_var "GOVERNANCE_INITIALIZED" "1"

      # Wire LP token locker to governance so voting can lock tokens
      CURRENT_STEP="set_locker (LP token -> governance)"
      log "setting LP token locker to governance"
      if ! invoke "$LP_TOKEN_CONTRACT_ID" set_locker --locker "$GOVERNANCE_CONTRACT_ID" >/dev/null 2>&1; then
        warn "failed to set LP token locker — may already be set"
      else
        log "LP token locker set to governance"
      fi
    fi
  fi

  CURRENT_STEP="verify governance"
  if ! verify_governance "$GOVERNANCE_CONTRACT_ID" "$AMM_POOL_CONTRACT_ID" "$LP_TOKEN_CONTRACT_ID"; then
    warn "governance verification warning"
  else
    log "verified governance points at correct pool/lp"
  fi
}
