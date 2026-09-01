#!/usr/bin/env bash
# pools.sh — create AMM and CL pools via the factory
# Sourceable module for deploy.sh

deploy_pools() {
  CURRENT_CONTRACT="pools"
  log "== pools =="

  if [[ -z "${FACTORY_CONTRACT_ID:-}" ]]; then
    FACTORY_CONTRACT_ID=$(get_persisted FACTORY_CONTRACT_ID || echo "")
  fi
  if [[ -z "${TOKEN_A_CONTRACT_ID:-}" ]]; then TOKEN_A_CONTRACT_ID=$(get_persisted TOKEN_A_CONTRACT_ID || echo ""); fi
  if [[ -z "${TOKEN_B_CONTRACT_ID:-}" ]]; then TOKEN_B_CONTRACT_ID=$(get_persisted TOKEN_B_CONTRACT_ID || echo ""); fi

  if [[ -z "${FACTORY_CONTRACT_ID:-}" ]]; then
    warn "factory contract ID not set — cannot create pools, skipping"
    return 0
  fi
  if [[ -z "${TOKEN_A_CONTRACT_ID:-}" || -z "${TOKEN_B_CONTRACT_ID:-}" ]]; then
    warn "token addresses not set — cannot create pools, skipping"
    return 0
  fi

  # ── AMM pool via factory create_pool ───────────────────────────────────
  if should_skip_persisted "AMM_POOL_CONTRACT_ID"; then
    AMM_POOL_CONTRACT_ID=$(get_persisted AMM_POOL_CONTRACT_ID)
    LP_TOKEN_CONTRACT_ID=$(get_persisted LP_TOKEN_CONTRACT_ID || echo "")
    log "skipping AMM pool creation (already at $AMM_POOL_CONTRACT_ID)"
  else
    if ! should_deploy "pools" && ! should_deploy "factory"; then
      log "skipping AMM pool creation (--only/--skip filter)"
      AMM_POOL_CONTRACT_ID=$(get_persisted AMM_POOL_CONTRACT_ID || echo "")
      LP_TOKEN_CONTRACT_ID=$(get_persisted LP_TOKEN_CONTRACT_ID || echo "")
    else
      CURRENT_STEP="factory.create_pool (AMM)"
      log "creating AMM pool via factory: token_a=$TOKEN_A_CONTRACT_ID token_b=$TOKEN_B_CONTRACT_ID fee_tier=$DEFAULT_FEE_TIER"
      local out pool_addr lp_addr governance_opt
      # create_pool returns (Address, Option<Address>); we capture stdout and parse
      out=$(invoke "$FACTORY_CONTRACT_ID" create_pool \
        --caller "$ADMIN_ADDRESS" \
        --token_a "$TOKEN_A_CONTRACT_ID" \
        --token_b "$TOKEN_B_CONTRACT_ID" \
        --fee_tier "$DEFAULT_FEE_TIER" 2>&1) || {
        # Check if pool already exists (idempotent retry: lookup existing)
        local existing
        existing=$(invoke_read "$FACTORY_CONTRACT_ID" -- get_pool --token_a "$TOKEN_A_CONTRACT_ID" --token_b "$TOKEN_B_CONTRACT_ID" 2>&1 | grep -Eo 'C[A-Z0-9]{55}' | tail -n 1 || true)
        if [[ -n "$existing" ]]; then
          log "AMM pool already exists: $existing (idempotent)"
          pool_addr="$existing"
          # Lookup LP token
          lp_addr=$(invoke_read "$FACTORY_CONTRACT_ID" -- get_lp_token --pool "$pool_addr" 2>&1 | grep -Eo 'C[A-Z0-9]{55}' | tail -n 1 || echo "")
        else
          printf '[deploy] create_pool failed:\n%s\n' "$out" >&2
          die "failed to create AMM pool"
        fi
        AMM_POOL_CONTRACT_ID="$pool_addr"
        LP_TOKEN_CONTRACT_ID="$lp_addr"
        persist_var "AMM_POOL_CONTRACT_ID" "$AMM_POOL_CONTRACT_ID"
        if [[ -n "$LP_TOKEN_CONTRACT_ID" ]]; then persist_var "LP_TOKEN_CONTRACT_ID" "$LP_TOKEN_CONTRACT_ID"; fi
        export AMM_POOL_CONTRACT_ID LP_TOKEN_CONTRACT_ID
        # Continue to verification
        out=""
      }
      if [[ -n "$out" ]]; then
        # Parse both addresses from output — first is pool, second may be LP or governance
        pool_addr=$(printf '%s\n' "$out" | grep -Eo 'C[A-Z0-9]{55}' | head -n 1)
        # LP token is deterministic second contract; also query factory registry
        if [[ -z "$pool_addr" ]]; then
          die "could not parse AMM pool address from create_pool output: $out"
        fi
        AMM_POOL_CONTRACT_ID="$pool_addr"
        persist_var "AMM_POOL_CONTRACT_ID" "$AMM_POOL_CONTRACT_ID"
        # LP_TOKEN is created by factory; query it
        lp_addr=$(invoke_read "$FACTORY_CONTRACT_ID" -- get_lp_token --pool "$AMM_POOL_CONTRACT_ID" 2>&1 | grep -Eo 'C[A-Z0-9]{55}' | tail -n 1 || echo "")
        if [[ -n "$lp_addr" ]]; then
          LP_TOKEN_CONTRACT_ID="$lp_addr"
          persist_var "LP_TOKEN_CONTRACT_ID" "$LP_TOKEN_CONTRACT_ID"
          log "LP token (factory-deployed): $LP_TOKEN_CONTRACT_ID"
        fi
        log "AMM pool: $AMM_POOL_CONTRACT_ID"
      fi
    fi
  fi

  export AMM_POOL_CONTRACT_ID LP_TOKEN_CONTRACT_ID

  # Verify pool
  if [[ -n "${AMM_POOL_CONTRACT_ID:-}" ]]; then
    CURRENT_STEP="verify AMM pool"
    if ! verify_amm_pool "$AMM_POOL_CONTRACT_ID" "$TOKEN_A_CONTRACT_ID" "$TOKEN_B_CONTRACT_ID" "$DEFAULT_FEE_BPS"; then
      warn "AMM pool verification warning"
    fi
    # Verify LP token admin is pool
    if [[ -n "${LP_TOKEN_CONTRACT_ID:-}" ]]; then
      local lp_admin_out lp_admin
      lp_admin_out=$(invoke_read "$LP_TOKEN_CONTRACT_ID" -- admin 2>&1 || true)
      lp_admin=$(printf '%s\n' "$lp_admin_out" | grep -Eo 'C[A-Z0-9]{55}' | tail -n 1 || true)
      if [[ "$lp_admin" == "$AMM_POOL_CONTRACT_ID" ]]; then
        log "verified LP token admin is AMM pool"
      else
        warn "LP token admin mismatch: expected $AMM_POOL_CONTRACT_ID got ${lp_admin:-<empty>} output: $lp_admin_out"
      fi
    fi
    # Persist alias for backward compat with e2e.sh (AMM_CONTRACT_ID)
    if [[ -n "${AMM_POOL_CONTRACT_ID:-}" ]]; then
      persist_var "AMM_CONTRACT_ID" "$AMM_POOL_CONTRACT_ID"
    fi
  fi

  # ── CL pool via factory create_cl_pool ─────────────────────────────────
  if should_skip_persisted "CL_POOL_CONTRACT_ID"; then
    CL_POOL_CONTRACT_ID=$(get_persisted CL_POOL_CONTRACT_ID)
    log "skipping CL pool creation (already at $CL_POOL_CONTRACT_ID)"
  else
    if ! should_deploy "pools" && ! should_deploy "concentrated_liquidity"; then
      log "skipping CL pool creation (--only/--skip filter)"
      CL_POOL_CONTRACT_ID=$(get_persisted CL_POOL_CONTRACT_ID || echo "")
    else
      CURRENT_STEP="factory.create_cl_pool"
      log "creating CL pool via factory: fee_bps=30 initial_tick=0"
      local cl_out cl_addr
      cl_out=$(invoke "$FACTORY_CONTRACT_ID" create_cl_pool \
        --caller "$ADMIN_ADDRESS" \
        --token_a "$TOKEN_A_CONTRACT_ID" \
        --token_b "$TOKEN_B_CONTRACT_ID" \
        --fee_bps 30 \
        --initial_tick 0 2>&1) || {
        # Check if CL pool already exists
        local existing_cl
        existing_cl=$(invoke_read "$FACTORY_CONTRACT_ID" -- get_cl_pool --token_a "$TOKEN_A_CONTRACT_ID" --token_b "$TOKEN_B_CONTRACT_ID" --fee_bps 30 2>&1 | grep -Eo 'C[A-Z0-9]{55}' | tail -n 1 || true)
        if [[ -n "$existing_cl" ]]; then
          log "CL pool already exists: $existing_cl"
          CL_POOL_CONTRACT_ID="$existing_cl"
          persist_var "CL_POOL_CONTRACT_ID" "$CL_POOL_CONTRACT_ID"
          export CL_POOL_CONTRACT_ID
          cl_out=""
        else
          printf '[deploy] create_cl_pool failed:\n%s\n' "$cl_out" >&2
          warn "failed to create CL pool — may need CL WASM hash registered first"
          CL_POOL_CONTRACT_ID=$(get_persisted CL_POOL_CONTRACT_ID || echo "")
          cl_out=""
        fi
      }
      if [[ -n "$cl_out" ]]; then
        cl_addr=$(printf '%s\n' "$cl_out" | grep -Eo 'C[A-Z0-9]{55}' | tail -n 1 || echo "")
        if [[ -n "$cl_addr" ]]; then
          CL_POOL_CONTRACT_ID="$cl_addr"
          persist_var "CL_POOL_CONTRACT_ID" "$CL_POOL_CONTRACT_ID"
          log "CL pool: $CL_POOL_CONTRACT_ID"
        else
          warn "could not parse CL pool address from output: $cl_out"
        fi
      fi
    fi
  fi

  export CL_POOL_CONTRACT_ID
  if [[ -n "${CL_POOL_CONTRACT_ID:-}" ]]; then
    CURRENT_STEP="verify CL pool"
    local cl_info
    cl_info=$(invoke_read "$CL_POOL_CONTRACT_ID" -- get_pool_state 2>&1 || invoke_read "$CL_POOL_CONTRACT_ID" -- current_tick 2>&1 || true)
    if echo "$cl_info" | grep -qE '[0-9]+'; then
      log "verified CL pool liveness: $cl_info"
    else
      warn "could not verify CL pool: $cl_info"
    fi
  fi
}
