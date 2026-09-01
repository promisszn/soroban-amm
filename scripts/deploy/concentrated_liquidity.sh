#!/usr/bin/env bash
# concentrated_liquidity.sh — CL WASM artifact helpers
# Sourceable module for deploy.sh
# CL pools are deployed via factory.create_cl_pool.

deploy_concentrated_liquidity() {
  CURRENT_CONTRACT="concentrated_liquidity"
  log "== concentrated_liquidity (WASM artifact) =="
  local wasm
  wasm=$(wasm_path concentrated_liquidity)
  if [[ -f "$wasm" ]]; then
    log "CL WASM artifact present: $wasm (upload happens in factory step)"
  else
    warn "CL WASM not found at $wasm — build with: cargo build --release --target ${WASM_TARGET}"
  fi
  if should_deploy "concentrated_liquidity"; then
    log "CL pools are created via factory.create_cl_pool — no direct deploy"
  else
    log "skipping concentrated_liquidity (--only/--skip filter)"
  fi
}
