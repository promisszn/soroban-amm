#!/usr/bin/env bash
# pol_vesting.sh — deploy POL Vesting
# Sourceable module for deploy.sh

deploy_pol_vesting() {
  CURRENT_CONTRACT="pol_vesting"
  log "== pol_vesting =="

  local wasm
  wasm=$(wasm_path pol_vesting)

  if [[ -z "${GOVERNANCE_CONTRACT_ID:-}" ]]; then GOVERNANCE_CONTRACT_ID=$(get_persisted GOVERNANCE_CONTRACT_ID || echo ""); fi
  if [[ -z "${GOVERNANCE_CONTRACT_ID:-}" ]]; then
    warn "governance not deployed — skipping pol_vesting"
    return 0
  fi

  if should_skip_persisted "POL_VESTING_CONTRACT_ID"; then
    POL_VESTING_CONTRACT_ID=$(get_persisted POL_VESTING_CONTRACT_ID)
    log "skipping pol_vesting deploy (already at $POL_VESTING_CONTRACT_ID)"
  else
    if ! should_deploy "pol_vesting"; then
      log "skipping pol_vesting deploy (--only/--skip filter)"
      POL_VESTING_CONTRACT_ID=$(get_persisted POL_VESTING_CONTRACT_ID || echo "")
      if [[ -z "$POL_VESTING_CONTRACT_ID" ]]; then return 0; fi
    else
      CURRENT_STEP="deploy pol_vesting"
      log "deploying pol_vesting: $wasm"
      if [[ ! -f "$wasm" ]]; then
        warn "WASM not found: $wasm — skipping"
        return 0
      fi
      POL_VESTING_CONTRACT_ID=$(deploy_contract "$wasm")
      persist_var "POL_VESTING_CONTRACT_ID" "$POL_VESTING_CONTRACT_ID"
      log "pol_vesting: $POL_VESTING_CONTRACT_ID"
    fi
  fi

  export POL_VESTING_CONTRACT_ID
  if [[ -z "${POL_VESTING_CONTRACT_ID:-}" ]]; then return 0; fi

  if should_skip_persisted "POL_VESTING_INITIALIZED"; then
    log "skipping pol_vesting initialize (already done)"
  else
    if ! should_deploy "pol_vesting"; then
      log "skipping pol_vesting initialize (--only/--skip filter)"
    else
      CURRENT_STEP="initialize pol_vesting"
      log "initializing pol_vesting governance=$GOVERNANCE_CONTRACT_ID treasury=$ADMIN_ADDRESS"
      if ! invoke "$POL_VESTING_CONTRACT_ID" initialize --governance "$GOVERNANCE_CONTRACT_ID" --treasury "$ADMIN_ADDRESS" >/dev/null 2>&1; then
        if invoke_read "$POL_VESTING_CONTRACT_ID" -- get_governance 2>&1 | grep -q "$GOVERNANCE_CONTRACT_ID"; then
          log "pol_vesting already initialized"
        else
          warn "failed to initialize pol_vesting"
        fi
      else
        log "pol_vesting initialized"
      fi
      persist_var "POL_VESTING_INITIALIZED" "1"
    fi
  fi

  CURRENT_STEP="verify pol_vesting"
  if invoke_read "$POL_VESTING_CONTRACT_ID" -- get_governance >/dev/null 2>&1; then
    log "verified pol_vesting liveness"
  else
    warn "pol_vesting verification warning"
  fi
}
