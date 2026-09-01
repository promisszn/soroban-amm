#!/usr/bin/env bash
# batch_auction.sh — deploy Batch Auction
# Sourceable module for deploy.sh

deploy_batch_auction() {
  CURRENT_CONTRACT="batch_auction"
  log "== batch_auction =="

  local wasm
  wasm=$(wasm_path batch_auction)

  if should_skip_persisted "BATCH_AUCTION_CONTRACT_ID"; then
    BATCH_AUCTION_CONTRACT_ID=$(get_persisted BATCH_AUCTION_CONTRACT_ID)
    log "skipping batch_auction deploy (already at $BATCH_AUCTION_CONTRACT_ID)"
  else
    if ! should_deploy "batch_auction"; then
      log "skipping batch_auction deploy (--only/--skip filter)"
      BATCH_AUCTION_CONTRACT_ID=$(get_persisted BATCH_AUCTION_CONTRACT_ID || echo "")
      if [[ -z "$BATCH_AUCTION_CONTRACT_ID" ]]; then return 0; fi
    else
      CURRENT_STEP="deploy batch_auction"
      log "deploying batch_auction: $wasm"
      if [[ ! -f "$wasm" ]]; then
        warn "WASM not found: $wasm — skipping"
        return 0
      fi
      BATCH_AUCTION_CONTRACT_ID=$(deploy_contract "$wasm")
      persist_var "BATCH_AUCTION_CONTRACT_ID" "$BATCH_AUCTION_CONTRACT_ID"
      log "batch_auction: $BATCH_AUCTION_CONTRACT_ID"
    fi
  fi

  export BATCH_AUCTION_CONTRACT_ID
  if [[ -z "${BATCH_AUCTION_CONTRACT_ID:-}" ]]; then return 0; fi

  if should_skip_persisted "BATCH_AUCTION_INITIALIZED"; then
    log "skipping batch_auction initialize (already done)"
  else
    if ! should_deploy "batch_auction"; then
      log "skipping batch_auction initialize (--only/--skip filter)"
    else
      CURRENT_STEP="initialize batch_auction"
      log "initializing batch_auction admin=$ADMIN_ADDRESS window=$DEFAULT_BATCH_WINDOW_SECS"
      if ! invoke "$BATCH_AUCTION_CONTRACT_ID" initialize --admin "$ADMIN_ADDRESS" --batch_window_secs "$DEFAULT_BATCH_WINDOW_SECS" >/dev/null 2>&1; then
        if invoke_read "$BATCH_AUCTION_CONTRACT_ID" -- get_admin 2>&1 | grep -q "$ADMIN_ADDRESS" || invoke_read "$BATCH_AUCTION_CONTRACT_ID" -- get_batch_window >/dev/null 2>&1; then
          log "batch_auction already initialized"
        else
          warn "failed to initialize batch_auction"
        fi
      else
        log "batch_auction initialized"
      fi
      persist_var "BATCH_AUCTION_INITIALIZED" "1"
    fi
  fi

  CURRENT_STEP="verify batch_auction"
  if invoke_read "$BATCH_AUCTION_CONTRACT_ID" -- get_admin 2>&1 | grep -q "$ADMIN_ADDRESS" || invoke_read "$BATCH_AUCTION_CONTRACT_ID" -- get_batch_window >/dev/null 2>&1; then
    log "verified batch_auction liveness"
  else
    warn "batch_auction verification warning"
  fi
}
