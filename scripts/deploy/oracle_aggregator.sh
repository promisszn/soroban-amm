#!/usr/bin/env bash
# oracle_aggregator.sh — deploy Oracle Aggregator
# Sourceable module for deploy.sh

deploy_oracle_aggregator() {
  CURRENT_CONTRACT="oracle_aggregator"
  log "== oracle_aggregator =="

  local wasm
  wasm=$(wasm_path oracle_aggregator)

  if should_skip_persisted "ORACLE_AGGREGATOR_CONTRACT_ID"; then
    ORACLE_AGGREGATOR_CONTRACT_ID=$(get_persisted ORACLE_AGGREGATOR_CONTRACT_ID)
    log "skipping oracle_aggregator deploy (already at $ORACLE_AGGREGATOR_CONTRACT_ID)"
  else
    if ! should_deploy "oracle_aggregator"; then
      log "skipping oracle_aggregator deploy (--only/--skip filter)"
      ORACLE_AGGREGATOR_CONTRACT_ID=$(get_persisted ORACLE_AGGREGATOR_CONTRACT_ID || echo "")
      if [[ -z "$ORACLE_AGGREGATOR_CONTRACT_ID" ]]; then return 0; fi
    else
      CURRENT_STEP="deploy oracle_aggregator"
      log "deploying oracle_aggregator: $wasm"
      if [[ ! -f "$wasm" ]]; then
        warn "WASM not found: $wasm — skipping"
        return 0
      fi
      ORACLE_AGGREGATOR_CONTRACT_ID=$(deploy_contract "$wasm")
      persist_var "ORACLE_AGGREGATOR_CONTRACT_ID" "$ORACLE_AGGREGATOR_CONTRACT_ID"
      log "oracle_aggregator: $ORACLE_AGGREGATOR_CONTRACT_ID"
    fi
  fi

  export ORACLE_AGGREGATOR_CONTRACT_ID
  if [[ -z "${ORACLE_AGGREGATOR_CONTRACT_ID:-}" ]]; then return 0; fi

  if should_skip_persisted "ORACLE_AGGREGATOR_INITIALIZED"; then
    log "skipping oracle_aggregator initialize (already done)"
  else
    if ! should_deploy "oracle_aggregator"; then
      log "skipping oracle_aggregator initialize (--only/--skip filter)"
    else
      CURRENT_STEP="initialize oracle_aggregator"
      log "initializing oracle_aggregator admin=$ADMIN_ADDRESS staleness=$DEFAULT_ORACLE_MAX_STALENESS_SECS"
      if ! invoke "$ORACLE_AGGREGATOR_CONTRACT_ID" initialize --admin "$ADMIN_ADDRESS" --max_staleness_seconds "$DEFAULT_ORACLE_MAX_STALENESS_SECS" >/dev/null 2>&1; then
        if invoke_read "$ORACLE_AGGREGATOR_CONTRACT_ID" -- get_admin 2>&1 | grep -q "$ADMIN_ADDRESS" || invoke_read "$ORACLE_AGGREGATOR_CONTRACT_ID" -- get_sources >/dev/null 2>&1; then
          log "oracle_aggregator already initialized"
        else
          warn "failed to initialize oracle_aggregator"
        fi
      else
        log "oracle_aggregator initialized"
      fi
      persist_var "ORACLE_AGGREGATOR_INITIALIZED" "1"
    fi
  fi

  CURRENT_STEP="verify oracle_aggregator"
  if invoke_read "$ORACLE_AGGREGATOR_CONTRACT_ID" -- get_sources >/dev/null 2>&1 || invoke_read "$ORACLE_AGGREGATOR_CONTRACT_ID" -- get_admin >/dev/null 2>&1; then
    log "verified oracle_aggregator liveness"
  else
    warn "oracle_aggregator verification warning"
  fi
}
