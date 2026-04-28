# Pull Request: Fixes for Issues #117, #118, #119, and #120

## Overview
This pull request resolves four issues across the Soroban AMM repository. The changes eliminate build ambiguities, harden numeric constraints against overflow, ensure consistency in multi-step trading routes, and prevent repository tracking of backup files.

## Resolved Issues

### 1. Unified `update_fee` method (#117)
- Unified two overloaded variants of `update_fee` in `contracts/amm/src/lib.rs` into a single canonical `pub fn update_fee(env: Env, new_fee_bps: i128)`.
- Updated governance timelock integrations to use non-root authorization configurations for proper event propagation during test passes.

### 2. Transfer-before-Mint ordering enforcement (#119)
- Validated and preserved liquidity transfer constraints in `add_liquidity` enforcing reserve transfer completions prior to share allocation.

### 3. Tracking prevention of development backups (#118)
- Appended ignoring rules to the project `.gitignore` file to reject `.backup` file commits.

### 4. Large reserve overflow constraints (#120)
- Implemented multiplication guards for `get_amount_in` inside `contracts/amm/src/lib.rs` protecting operations on extremely large trade sizes.
