#!/usr/bin/env bash
# amm.sh — AMM WASM artifact helpers (upload handled in factory.sh for dependency order)
# Sourceable module for deploy.sh
# AMM pools are deployed via the factory (factory.create_pool), not directly.
# This module exists so --only amm / --skip amm filtering works and so
# operators can reason about the AMM as a standalone deployable unit.

deploy_amm() {
  CURRENT_CONTRACT="amm"
  log "== amm (WASM artifact) =="
  local wasm
  wasm=$(wasm_path amm)
  if [[ -f "$wasm" ]]; then
    log "AMM WASM artifact present: $wasm (upload happens in factory step)"
  else
    warn "AMM WASM not found at $wasm — build with: cargo build --release --target ${WASM_TARGET}"
  fi
  # No direct deploy — factory handles it. Persist marker for filtering completeness.
  if should_deploy "amm"; then
    log "amm pools are created via factory.create_pool — no direct deploy"
  else
    log "skipping amm (--only/--skip filter)"
  fi
}
