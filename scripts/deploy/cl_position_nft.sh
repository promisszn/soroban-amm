#!/usr/bin/env bash
# cl_position_nft.sh — deploy CL Position NFT, wired to CL pool
# Sourceable module for deploy.sh

deploy_cl_position_nft() {
  CURRENT_CONTRACT="cl_position_nft"
  log "== cl_position_nft =="

  local wasm
  wasm=$(wasm_path cl_position_nft)

  if [[ -z "${CL_POOL_CONTRACT_ID:-}" ]]; then CL_POOL_CONTRACT_ID=$(get_persisted CL_POOL_CONTRACT_ID || echo ""); fi
  if [[ -z "${CL_POOL_CONTRACT_ID:-}" ]]; then
    warn "CL pool not deployed — skipping cl_position_nft (requires CL pool)"
    return 0
  fi

  if should_skip_persisted "CL_POSITION_NFT_CONTRACT_ID"; then
    CL_POSITION_NFT_CONTRACT_ID=$(get_persisted CL_POSITION_NFT_CONTRACT_ID)
    log "skipping cl_position_nft deploy (already at $CL_POSITION_NFT_CONTRACT_ID)"
  else
    if ! should_deploy "cl_position_nft"; then
      log "skipping cl_position_nft deploy (--only/--skip filter)"
      CL_POSITION_NFT_CONTRACT_ID=$(get_persisted CL_POSITION_NFT_CONTRACT_ID || echo "")
      if [[ -z "$CL_POSITION_NFT_CONTRACT_ID" ]]; then return 0; fi
    else
      CURRENT_STEP="deploy cl_position_nft"
      log "deploying cl_position_nft: $wasm"
      if [[ ! -f "$wasm" ]]; then
        warn "WASM not found: $wasm — skipping"
        return 0
      fi
      CL_POSITION_NFT_CONTRACT_ID=$(deploy_contract "$wasm")
      persist_var "CL_POSITION_NFT_CONTRACT_ID" "$CL_POSITION_NFT_CONTRACT_ID"
      log "cl_position_nft: $CL_POSITION_NFT_CONTRACT_ID"
    fi
  fi

  export CL_POSITION_NFT_CONTRACT_ID
  if [[ -z "${CL_POSITION_NFT_CONTRACT_ID:-}" ]]; then return 0; fi

  if should_skip_persisted "CL_POSITION_NFT_INITIALIZED"; then
    log "skipping cl_position_nft initialize (already done)"
  else
    if ! should_deploy "cl_position_nft"; then
      log "skipping cl_position_nft initialize (--only/--skip filter)"
    else
      CURRENT_STEP="initialize cl_position_nft"
      log "initializing cl_position_nft admin=$ADMIN_ADDRESS cl_pool=$CL_POOL_CONTRACT_ID"
      if ! invoke "$CL_POSITION_NFT_CONTRACT_ID" initialize --admin "$ADMIN_ADDRESS" --cl_pool "$CL_POOL_CONTRACT_ID" >/dev/null 2>&1; then
        if invoke_read "$CL_POSITION_NFT_CONTRACT_ID" -- get_admin 2>&1 | grep -q "$ADMIN_ADDRESS"; then
          log "cl_position_nft already initialized"
        else
          warn "failed to initialize cl_position_nft"
        fi
      else
        log "cl_position_nft initialized"
      fi
      persist_var "CL_POSITION_NFT_INITIALIZED" "1"

      # Wire NFT into CL pool (set_position_nft) if pool supports it
      CURRENT_STEP="wire NFT into CL pool"
      log "wiring NFT contract into CL pool"
      if ! invoke "$CL_POOL_CONTRACT_ID" set_position_nft --position_nft "$CL_POSITION_NFT_CONTRACT_ID" >/dev/null 2>&1; then
        warn "failed to wire NFT into CL pool — pool may not support set_position_nft or already wired"
      else
        log "NFT wired into CL pool"
      fi
    fi
  fi

  CURRENT_STEP="verify cl_position_nft"
  if invoke_read "$CL_POSITION_NFT_CONTRACT_ID" -- get_admin 2>&1 | grep -q "$ADMIN_ADDRESS" || invoke_read "$CL_POSITION_NFT_CONTRACT_ID" -- next_token_id >/dev/null 2>&1; then
    log "verified cl_position_nft liveness"
  else
    warn "cl_position_nft verification warning"
  fi
}
