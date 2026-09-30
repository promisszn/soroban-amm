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
      if invoke "$CL_POSITION_NFT_CONTRACT_ID" initialize --admin "$ADMIN_ADDRESS" --cl_pool "$CL_POOL_CONTRACT_ID" >/dev/null 2>&1; then
        log "cl_position_nft initialized"
      elif cl_position_nft_pool_is "$CL_POSITION_NFT_CONTRACT_ID" "$CL_POOL_CONTRACT_ID"; then
        # A previous run initialized it but died before persisting the marker.
        log "cl_position_nft already initialized"
      else
        die "failed to initialize cl_position_nft at $CL_POSITION_NFT_CONTRACT_ID"
      fi
      persist_var "CL_POSITION_NFT_INITIALIZED" "1"

      # Wire the NFT into the CL pool. The pool admin is the factory admin
      # (create_cl_pool), which this deploy signs as. `nft` is an
      # Option<Address>, which the CLI parses as JSON, hence the quotes.
      CURRENT_STEP="wire NFT into CL pool"
      log "wiring NFT contract into CL pool"
      if invoke "$CL_POOL_CONTRACT_ID" set_position_nft --admin "$ADMIN_ADDRESS" --nft "\"$CL_POSITION_NFT_CONTRACT_ID\"" >/dev/null 2>&1; then
        log "NFT wired into CL pool"
      elif cl_pool_nft_is "$CL_POOL_CONTRACT_ID" "$CL_POSITION_NFT_CONTRACT_ID"; then
        log "NFT already wired into CL pool"
      else
        die "failed to wire cl_position_nft $CL_POSITION_NFT_CONTRACT_ID into CL pool $CL_POOL_CONTRACT_ID"
      fi
    fi
  fi

  CURRENT_STEP="verify cl_position_nft"
  if ! cl_position_nft_pool_is "$CL_POSITION_NFT_CONTRACT_ID" "$CL_POOL_CONTRACT_ID"; then
    die "cl_position_nft $CL_POSITION_NFT_CONTRACT_ID verification failed: cl_pool is not $CL_POOL_CONTRACT_ID"
  fi
  if ! cl_pool_nft_is "$CL_POOL_CONTRACT_ID" "$CL_POSITION_NFT_CONTRACT_ID"; then
    die "CL pool $CL_POOL_CONTRACT_ID verification failed: position_nft is not $CL_POSITION_NFT_CONTRACT_ID"
  fi
  log "verified cl_position_nft $CL_POSITION_NFT_CONTRACT_ID and CL pool $CL_POOL_CONTRACT_ID point at each other"
}

# cl_position_nft_pool_is NFT POOL — true when the NFT's cl_pool is POOL.
cl_position_nft_pool_is() {
  local out
  out=$(invoke_read "$1" cl_pool 2>/dev/null || true)
  [[ "$(printf '%s\n' "$out" | extract_contract_id)" == "$2" ]]
}

# cl_pool_nft_is POOL NFT — true when the CL pool's position_nft is NFT.
cl_pool_nft_is() {
  local out
  out=$(invoke_read "$1" position_nft 2>/dev/null || true)
  [[ "$(printf '%s\n' "$out" | extract_contract_id)" == "$2" ]]
}
