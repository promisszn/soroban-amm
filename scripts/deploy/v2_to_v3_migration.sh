#!/usr/bin/env bash
# v2_to_v3_migration.sh — deploy V2->V3 Migration helper
# Sourceable module for deploy.sh

deploy_v2_to_v3_migration() {
  CURRENT_CONTRACT="v2_to_v3_migration"
  log "== v2_to_v3_migration =="

  local wasm
  wasm=$(wasm_path v2_to_v3_migration)

  if [[ -z "${AMM_POOL_CONTRACT_ID:-}" ]]; then AMM_POOL_CONTRACT_ID=$(get_persisted AMM_POOL_CONTRACT_ID || echo ""); fi
  if [[ -z "${CL_POOL_CONTRACT_ID:-}" ]]; then CL_POOL_CONTRACT_ID=$(get_persisted CL_POOL_CONTRACT_ID || echo ""); fi

  if [[ -z "${AMM_POOL_CONTRACT_ID:-}" || -z "${CL_POOL_CONTRACT_ID:-}" ]]; then
    warn "missing AMM or CL pool — skipping v2_to_v3_migration"
    return 0
  fi

  if should_skip_persisted "V2_TO_V3_MIGRATION_CONTRACT_ID"; then
    V2_TO_V3_MIGRATION_CONTRACT_ID=$(get_persisted V2_TO_V3_MIGRATION_CONTRACT_ID)
    log "skipping v2_to_v3_migration deploy (already at $V2_TO_V3_MIGRATION_CONTRACT_ID)"
  else
    if ! should_deploy "v2_to_v3_migration"; then
      log "skipping v2_to_v3_migration deploy (--only/--skip filter)"
      V2_TO_V3_MIGRATION_CONTRACT_ID=$(get_persisted V2_TO_V3_MIGRATION_CONTRACT_ID || echo "")
      if [[ -z "$V2_TO_V3_MIGRATION_CONTRACT_ID" ]]; then return 0; fi
    else
      CURRENT_STEP="deploy v2_to_v3_migration"
      log "deploying v2_to_v3_migration: $wasm"
      if [[ ! -f "$wasm" ]]; then
        warn "WASM not found: $wasm — skipping"
        return 0
      fi
      V2_TO_V3_MIGRATION_CONTRACT_ID=$(deploy_contract "$wasm")
      persist_var "V2_TO_V3_MIGRATION_CONTRACT_ID" "$V2_TO_V3_MIGRATION_CONTRACT_ID"
      log "v2_to_v3_migration: $V2_TO_V3_MIGRATION_CONTRACT_ID"
    fi
  fi

  export V2_TO_V3_MIGRATION_CONTRACT_ID
  if [[ -z "${V2_TO_V3_MIGRATION_CONTRACT_ID:-}" ]]; then return 0; fi

  if should_skip_persisted "V2_TO_V3_MIGRATION_INITIALIZED"; then
    log "skipping v2_to_v3_migration initialize (already done)"
  else
    if ! should_deploy "v2_to_v3_migration"; then
      log "skipping v2_to_v3_migration initialize (--only/--skip filter)"
    else
      CURRENT_STEP="initialize v2_to_v3_migration"
      log "initializing v2_to_v3_migration admin=$ADMIN_ADDRESS v2=$AMM_POOL_CONTRACT_ID v3=$CL_POOL_CONTRACT_ID"
      if ! invoke "$V2_TO_V3_MIGRATION_CONTRACT_ID" initialize --admin "$ADMIN_ADDRESS" --v2_pool "$AMM_POOL_CONTRACT_ID" --v3_pool "$CL_POOL_CONTRACT_ID" >/dev/null 2>&1; then
        if invoke_read "$V2_TO_V3_MIGRATION_CONTRACT_ID" -- get_admin 2>&1 | grep -q "$ADMIN_ADDRESS" || invoke_read "$V2_TO_V3_MIGRATION_CONTRACT_ID" -- get_v2_pool 2>&1 | grep -q "$AMM_POOL_CONTRACT_ID"; then
          log "v2_to_v3_migration already initialized"
        else
          warn "failed to initialize v2_to_v3_migration"
        fi
      else
        log "v2_to_v3_migration initialized"
      fi
      persist_var "V2_TO_V3_MIGRATION_INITIALIZED" "1"
    fi
  fi

  CURRENT_STEP="verify v2_to_v3_migration"
  if invoke_read "$V2_TO_V3_MIGRATION_CONTRACT_ID" -- get_admin >/dev/null 2>&1; then
    log "verified v2_to_v3_migration liveness"
  else
    warn "v2_to_v3_migration verification warning"
  fi
}
