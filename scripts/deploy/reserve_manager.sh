#!/usr/bin/env bash
# reserve_manager.sh — deploy Reserve Manager
# Sourceable module for deploy.sh

deploy_reserve_manager() {
  CURRENT_CONTRACT="reserve_manager"
  log "== reserve_manager =="

  local wasm
  wasm=$(wasm_path reserve_manager)

  if [[ -z "${GOVERNANCE_CONTRACT_ID:-}" ]]; then GOVERNANCE_CONTRACT_ID=$(get_persisted GOVERNANCE_CONTRACT_ID || echo ""); fi
  if [[ -z "${FACTORY_CONTRACT_ID:-}" ]]; then FACTORY_CONTRACT_ID=$(get_persisted FACTORY_CONTRACT_ID || echo ""); fi

  if [[ -z "${GOVERNANCE_CONTRACT_ID:-}" || -z "${FACTORY_CONTRACT_ID:-}" ]]; then
    warn "missing governance or factory — skipping reserve_manager"
    return 0
  fi

  if should_skip_persisted "RESERVE_MANAGER_CONTRACT_ID"; then
    RESERVE_MANAGER_CONTRACT_ID=$(get_persisted RESERVE_MANAGER_CONTRACT_ID)
    log "skipping reserve_manager deploy (already at $RESERVE_MANAGER_CONTRACT_ID)"
  else
    if ! should_deploy "reserve_manager"; then
      log "skipping reserve_manager deploy (--only/--skip filter)"
      RESERVE_MANAGER_CONTRACT_ID=$(get_persisted RESERVE_MANAGER_CONTRACT_ID || echo "")
      if [[ -z "$RESERVE_MANAGER_CONTRACT_ID" ]]; then return 0; fi
    else
      CURRENT_STEP="deploy reserve_manager"
      log "deploying reserve_manager: $wasm"
      if [[ ! -f "$wasm" ]]; then
        warn "WASM not found: $wasm — skipping"
        return 0
      fi
      RESERVE_MANAGER_CONTRACT_ID=$(deploy_contract "$wasm")
      persist_var "RESERVE_MANAGER_CONTRACT_ID" "$RESERVE_MANAGER_CONTRACT_ID"
      log "reserve_manager: $RESERVE_MANAGER_CONTRACT_ID"
    fi
  fi

  export RESERVE_MANAGER_CONTRACT_ID
  if [[ -z "${RESERVE_MANAGER_CONTRACT_ID:-}" ]]; then return 0; fi

  if should_skip_persisted "RESERVE_MANAGER_INITIALIZED"; then
    log "skipping reserve_manager initialize (already done)"
  else
    if ! should_deploy "reserve_manager"; then
      log "skipping reserve_manager initialize (--only/--skip filter)"
    else
      CURRENT_STEP="initialize reserve_manager"
      log "initializing reserve_manager governance=$GOVERNANCE_CONTRACT_ID factory=$FACTORY_CONTRACT_ID"
      if ! invoke "$RESERVE_MANAGER_CONTRACT_ID" initialize --governance "$GOVERNANCE_CONTRACT_ID" --factory "$FACTORY_CONTRACT_ID" >/dev/null 2>&1; then
        if invoke_read "$RESERVE_MANAGER_CONTRACT_ID" -- get_governance 2>&1 | grep -q "$GOVERNANCE_CONTRACT_ID"; then
          log "reserve_manager already initialized"
        else
          warn "failed to initialize reserve_manager"
        fi
      else
        log "reserve_manager initialized"
      fi
      persist_var "RESERVE_MANAGER_INITIALIZED" "1"
    fi
  fi

  CURRENT_STEP="verify reserve_manager"
  if invoke_read "$RESERVE_MANAGER_CONTRACT_ID" -- get_governance >/dev/null 2>&1; then
    log "verified reserve_manager liveness"
  else
    warn "reserve_manager verification warning"
  fi
}
