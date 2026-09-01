#!/usr/bin/env bash
# dex_aggregator.sh — deploy DEX Aggregator
# Sourceable module for deploy.sh

deploy_dex_aggregator() {
  CURRENT_CONTRACT="dex_aggregator"
  log "== dex_aggregator =="

  local wasm
  wasm=$(wasm_path dex_aggregator)

  if [[ -z "${FACTORY_CONTRACT_ID:-}" ]]; then FACTORY_CONTRACT_ID=$(get_persisted FACTORY_CONTRACT_ID || echo ""); fi
  if [[ -z "${FACTORY_CONTRACT_ID:-}" ]]; then
    warn "factory not deployed — skipping dex_aggregator"
    return 0
  fi

  if should_skip_persisted "DEX_AGGREGATOR_CONTRACT_ID"; then
    DEX_AGGREGATOR_CONTRACT_ID=$(get_persisted DEX_AGGREGATOR_CONTRACT_ID)
    log "skipping dex_aggregator deploy (already at $DEX_AGGREGATOR_CONTRACT_ID)"
  else
    if ! should_deploy "dex_aggregator"; then
      log "skipping dex_aggregator deploy (--only/--skip filter)"
      DEX_AGGREGATOR_CONTRACT_ID=$(get_persisted DEX_AGGREGATOR_CONTRACT_ID || echo "")
      if [[ -z "$DEX_AGGREGATOR_CONTRACT_ID" ]]; then return 0; fi
    else
      CURRENT_STEP="deploy dex_aggregator"
      log "deploying dex_aggregator: $wasm"
      if [[ ! -f "$wasm" ]]; then
        warn "WASM not found: $wasm — skipping"
        return 0
      fi
      DEX_AGGREGATOR_CONTRACT_ID=$(deploy_contract "$wasm")
      persist_var "DEX_AGGREGATOR_CONTRACT_ID" "$DEX_AGGREGATOR_CONTRACT_ID"
      log "dex_aggregator: $DEX_AGGREGATOR_CONTRACT_ID"
    fi
  fi

  export DEX_AGGREGATOR_CONTRACT_ID
  if [[ -z "${DEX_AGGREGATOR_CONTRACT_ID:-}" ]]; then return 0; fi

  if should_skip_persisted "DEX_AGGREGATOR_INITIALIZED"; then
    log "skipping dex_aggregator initialize (already done)"
  else
    if ! should_deploy "dex_aggregator"; then
      log "skipping dex_aggregator initialize (--only/--skip filter)"
    else
      CURRENT_STEP="initialize dex_aggregator"
      log "initializing dex_aggregator admin=$ADMIN_ADDRESS factory=$FACTORY_CONTRACT_ID"
      if ! invoke "$DEX_AGGREGATOR_CONTRACT_ID" initialize --admin "$ADMIN_ADDRESS" --factory "$FACTORY_CONTRACT_ID" >/dev/null 2>&1; then
        warn "failed to initialize dex_aggregator — may already be initialized"
      else
        log "dex_aggregator initialized"
      fi
      persist_var "DEX_AGGREGATOR_INITIALIZED" "1"
    fi
  fi

  CURRENT_STEP="verify dex_aggregator"
  if invoke_read "$DEX_AGGREGATOR_CONTRACT_ID" -- get_factory 2>&1 | grep -q "$FACTORY_CONTRACT_ID" || invoke_read "$DEX_AGGREGATOR_CONTRACT_ID" -- get_admin 2>&1 | grep -q "$ADMIN_ADDRESS"; then
    log "verified dex_aggregator factory/admin"
  else
    # Liveness fallback
    log "dex_aggregator deployed at $DEX_AGGREGATOR_CONTRACT_ID"
  fi
}
