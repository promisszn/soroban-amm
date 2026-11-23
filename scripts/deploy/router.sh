#!/usr/bin/env bash
# router.sh — deploy the Router (batch_router has its own module)
# Sourceable module for deploy.sh

deploy_router() {
  CURRENT_CONTRACT="router"
  log "== router =="

  local wasm
  wasm=$(wasm_path router)

  if [[ -z "${FACTORY_CONTRACT_ID:-}" ]]; then FACTORY_CONTRACT_ID=$(get_persisted FACTORY_CONTRACT_ID || echo ""); fi
  if [[ -z "${FACTORY_CONTRACT_ID:-}" ]]; then
    warn "factory not deployed — skipping router"
    return 0
  fi

  if should_skip_persisted "ROUTER_CONTRACT_ID"; then
    ROUTER_CONTRACT_ID=$(get_persisted ROUTER_CONTRACT_ID)
    log "skipping router deploy (already at $ROUTER_CONTRACT_ID)"
  else
    if ! should_deploy "router"; then
      log "skipping router deploy (--only/--skip filter)"
      ROUTER_CONTRACT_ID=$(get_persisted ROUTER_CONTRACT_ID || echo "")
      if [[ -z "$ROUTER_CONTRACT_ID" ]]; then return 0; fi
    else
      CURRENT_STEP="deploy router"
      log "deploying router: $wasm"
      if [[ ! -f "$wasm" ]]; then
        warn "WASM not found: $wasm — skipping"
        return 0
      fi
      ROUTER_CONTRACT_ID=$(deploy_contract "$wasm")
      persist_var "ROUTER_CONTRACT_ID" "$ROUTER_CONTRACT_ID"
      log "router: $ROUTER_CONTRACT_ID"
    fi
  fi

  export ROUTER_CONTRACT_ID
  if [[ -z "${ROUTER_CONTRACT_ID:-}" ]]; then return 0; fi

  if should_skip_persisted "ROUTER_INITIALIZED"; then
    log "skipping router initialize (already done)"
  else
    if ! should_deploy "router"; then
      log "skipping router initialize (--only/--skip filter)"
    else
      CURRENT_STEP="initialize router"
      log "initializing router admin=$ADMIN_ADDRESS factory=$FACTORY_CONTRACT_ID"
      if invoke "$ROUTER_CONTRACT_ID" initialize --admin "$ADMIN_ADDRESS" --factory "$FACTORY_CONTRACT_ID" >/dev/null 2>&1; then
        log "router initialized"
      elif router_factory_is "$ROUTER_CONTRACT_ID" "$FACTORY_CONTRACT_ID"; then
        # A previous run initialized it but died before persisting the marker.
        log "router already initialized"
      else
        die "failed to initialize router at $ROUTER_CONTRACT_ID"
      fi
      persist_var "ROUTER_INITIALIZED" "1"
    fi
  fi

  CURRENT_STEP="verify router"
  if router_factory_is "$ROUTER_CONTRACT_ID" "$FACTORY_CONTRACT_ID"; then
    log "verified router $ROUTER_CONTRACT_ID points at factory $FACTORY_CONTRACT_ID"
  else
    die "router $ROUTER_CONTRACT_ID verification failed: get_factory does not return $FACTORY_CONTRACT_ID"
  fi
}

# router_factory_is ID FACTORY — true when the router's get_factory is FACTORY.
# get_factory errors until initialize has run, so this also probes init state.
router_factory_is() {
  local out
  out=$(invoke_read "$1" get_factory 2>/dev/null || true)
  [[ "$(printf '%s\n' "$out" | extract_contract_id)" == "$2" ]]
}
