#!/usr/bin/env bash
# token.sh — deploy underlying asset tokens (Token A, Token B, Reward Token)
# Sourceable module for deploy.sh

deploy_tokens() {
  CURRENT_CONTRACT="token"
  log "== tokens =="

  local token_wasm
  token_wasm=$(wasm_path token)
  if [[ ! -f "$token_wasm" ]]; then
    # try hyphen variant
    token_wasm="${token_wasm//_/-}"
  fi
  if [[ ! -f "$token_wasm" ]]; then
    die "token WASM not found at $token_wasm — run cargo build --release --target ${WASM_TARGET} first"
  fi

  # Token A
  if should_skip_persisted "TOKEN_A_CONTRACT_ID"; then
    TOKEN_A_CONTRACT_ID=$(get_persisted TOKEN_A_CONTRACT_ID)
    log "skipping Token A deploy (already at $TOKEN_A_CONTRACT_ID, use --force to redeploy)"
  else
    if ! should_deploy "token"; then
      log "skipping Token A (--only/--skip filter)"
      TOKEN_A_CONTRACT_ID=$(get_persisted TOKEN_A_CONTRACT_ID || echo "")
    else
      CURRENT_STEP="deploy Token A"
      log "deploying Token A"
      TOKEN_A_CONTRACT_ID=$(deploy_contract "$token_wasm")
      persist_var "TOKEN_A_CONTRACT_ID" "$TOKEN_A_CONTRACT_ID"
      log "Token A: $TOKEN_A_CONTRACT_ID"
    fi
  fi

  # Token B
  if should_skip_persisted "TOKEN_B_CONTRACT_ID"; then
    TOKEN_B_CONTRACT_ID=$(get_persisted TOKEN_B_CONTRACT_ID)
    log "skipping Token B deploy (already at $TOKEN_B_CONTRACT_ID)"
  else
    if ! should_deploy "token"; then
      log "skipping Token B (--only/--skip filter)"
      TOKEN_B_CONTRACT_ID=$(get_persisted TOKEN_B_CONTRACT_ID || echo "")
    else
      CURRENT_STEP="deploy Token B"
      log "deploying Token B"
      TOKEN_B_CONTRACT_ID=$(deploy_contract "$token_wasm")
      persist_var "TOKEN_B_CONTRACT_ID" "$TOKEN_B_CONTRACT_ID"
      log "Token B: $TOKEN_B_CONTRACT_ID"
    fi
  fi

  # Reward Token (for staking)
  if should_skip_persisted "REWARD_TOKEN_CONTRACT_ID"; then
    REWARD_TOKEN_CONTRACT_ID=$(get_persisted REWARD_TOKEN_CONTRACT_ID)
    log "skipping Reward Token deploy (already at $REWARD_TOKEN_CONTRACT_ID)"
  else
    if ! should_deploy "token"; then
      log "skipping Reward Token (--only/--skip filter)"
      REWARD_TOKEN_CONTRACT_ID=$(get_persisted REWARD_TOKEN_CONTRACT_ID || echo "")
    else
      CURRENT_STEP="deploy Reward Token"
      log "deploying Reward Token"
      REWARD_TOKEN_CONTRACT_ID=$(deploy_contract "$token_wasm")
      persist_var "REWARD_TOKEN_CONTRACT_ID" "$REWARD_TOKEN_CONTRACT_ID"
      log "Reward Token: $REWARD_TOKEN_CONTRACT_ID"
    fi
  fi

  # Initialize tokens (idempotent — check marker)
  if [[ -n "${TOKEN_A_CONTRACT_ID:-}" ]]; then
    if should_skip_persisted "TOKEN_A_INITIALIZED"; then
      log "skipping Token A initialize (already done)"
    else
      if ! should_deploy "token"; then
        log "skipping Token A initialize (--only/--skip filter)"
      else
        CURRENT_STEP="initialize Token A"
        log "initializing Token A"
        invoke "$TOKEN_A_CONTRACT_ID" initialize \
          --admin "$SOURCE_PUBLIC_KEY" \
          --name "Soroban AMM Token A" \
          --symbol "SAMA" \
          --decimals 7 >/dev/null 2>&1 || {
            # Already initialized is ok for idempotency
            if invoke_read "$TOKEN_A_CONTRACT_ID" -- name >/dev/null 2>&1; then
              log "Token A already initialized (verified via name read)"
            else
              die "failed to initialize Token A"
            fi
          }
        persist_var "TOKEN_A_INITIALIZED" "1"
        if ! verify_token "$TOKEN_A_CONTRACT_ID" "$SOURCE_PUBLIC_KEY"; then
          warn "Token A verification warning (non-fatal)"
        fi
      fi
    fi
  fi

  if [[ -n "${TOKEN_B_CONTRACT_ID:-}" ]]; then
    if should_skip_persisted "TOKEN_B_INITIALIZED"; then
      log "skipping Token B initialize (already done)"
    else
      if ! should_deploy "token"; then
        log "skipping Token B initialize (--only/--skip filter)"
      else
        CURRENT_STEP="initialize Token B"
        log "initializing Token B"
        invoke "$TOKEN_B_CONTRACT_ID" initialize \
          --admin "$SOURCE_PUBLIC_KEY" \
          --name "Soroban AMM Token B" \
          --symbol "SAMB" \
          --decimals 7 >/dev/null 2>&1 || {
            if invoke_read "$TOKEN_B_CONTRACT_ID" -- name >/dev/null 2>&1; then
              log "Token B already initialized"
            else
              die "failed to initialize Token B"
            fi
          }
        persist_var "TOKEN_B_INITIALIZED" "1"
        if ! verify_token "$TOKEN_B_CONTRACT_ID" "$SOURCE_PUBLIC_KEY"; then
          warn "Token B verification warning"
        fi
      fi
    fi
  fi

  if [[ -n "${REWARD_TOKEN_CONTRACT_ID:-}" ]]; then
    if should_skip_persisted "REWARD_TOKEN_INITIALIZED"; then
      log "skipping Reward Token initialize (already done)"
    else
      if ! should_deploy "token"; then
        log "skipping Reward Token initialize (--only/--skip filter)"
      else
        CURRENT_STEP="initialize Reward Token"
        log "initializing Reward Token"
        invoke "$REWARD_TOKEN_CONTRACT_ID" initialize \
          --admin "$SOURCE_PUBLIC_KEY" \
          --name "Soroban AMM Reward" \
          --symbol "SAMR" \
          --decimals 7 >/dev/null 2>&1 || {
            if invoke_read "$REWARD_TOKEN_CONTRACT_ID" -- name >/dev/null 2>&1; then
              log "Reward Token already initialized"
            else
              die "failed to initialize Reward Token"
            fi
          }
        persist_var "REWARD_TOKEN_INITIALIZED" "1"
      fi
    fi
  fi

  # Export for downstream modules
  export TOKEN_A_CONTRACT_ID TOKEN_B_CONTRACT_ID REWARD_TOKEN_CONTRACT_ID
}
