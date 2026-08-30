# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]
### Fixed
- `LpToken::unlock` previously authorised against the currently configured `DataKey::Locker`, so any `set_locker` rotation orphaned LP tokens whose locker had locked them via `LockedVote`. The unlock function now requires auth from the locker that originally locked the tokens, recorded per-locker in a new `LockEntry(Address, Address)` storage entry. Each locker retains authority over its own contribution; a freshly-set locker can only unlock tokens it itself locked. (closes #556)
- `contracts/router/Cargo.toml` and the workspace `Cargo.toml` both contained duplicate table entries (`[dependencies]`, `[dev-dependencies]`, and member list) that caused `cargo` to refuse to load the workspace entirely. Merged into single tables and removed duplicate members.
- `concentrated_liquidity`: two compounding bugs let `current_tick` desync from the discrete `active_liquidity`/`fee_growth_outside` bookkeeping after a swap. First, the tick-crossing target price was derived via a crude reimplementation (`sqrt(tick_to_price(tick))` rescaled) instead of the existing high-precision `tick_to_sqrt_price_x96`, losing enough precision to round-trip to the wrong tick; fixed by reusing `tick_to_sqrt_price_x96` at all 3 call sites. Second, a swap step that stays *within* the current range (not crossing) computes its landing price in the engine's deliberately low-precision (~3 significant digit), pool-favorable-rounded scale, which right at a boundary can be indistinguishable from (or past) the next tick once reconstructed at full precision — resolving `current_tick` past a boundary the discrete bookkeeping never actually crossed. Fixed by clamping that branch's `current_tick` to the known-correct side of the boundary it was told not to reach. Together these caused `active_liquidity()` to diverge from the true sum of in-range positions after a multi-tick swap, and caused positions to lose previously-accrued fees once price crossed out of their range. The originally-suspected nearest-vs-floor rounding difference between `price_to_tick` and `sqrt_price_x96_to_tick` was real but not itself sufficient to reproduce the bug; `price_to_tick` is still removed as a genuine (if secondary) inconsistency. (closes #786, closes #785)
- `concentrated_liquidity`: `burn_position`/`burn_position_by_token_id`/`collect_fees` recomputed principal and fee payouts independently of the swap engine's own (lower-precision) price-stepping math, so a payout could exceed the contract's actual token balance and hard-trap instead of failing gracefully. Burn and fee-collection payouts are now clamped to the contract's real on-hand balance at transfer time, with any shortfall preserved in the position's `tokens_owed` so it remains claimable once the contract's balance recovers, instead of being lost. (closes #787)

### Added
- `LpToken::migrate_legacy_lock(holder, locker, amount)` admin-only helper to migrate a holder's pre-fix `Locked(holder) > 0` balance into per-locker `LockEntry` entries after upgrading from a contract version that tracked only the total `Locked` counter.
- `LpTokenInterface::unlock` now takes an explicit `locker: Address` parameter; `governance::unlock_vote` calls it with `env.current_contract_address()` as the locker.
- `batch_auction`: settlement venues are now validated against a registry instead of trusting a pool's self-reported token pair, closing a fund-drain vector where an order could name an arbitrary attacker-controlled contract as its venue. A venue is accepted only if the admin has explicitly allow-listed it (`add_venue`/`remove_venue`/`is_venue_allowed`/`list_venues`) or the configured factory (`set_factory`) attests to it having deployed that pool for that token pair (using a new `concentrated_liquidity::fee_bps()` getter for CL venues). Venues are re-validated at settlement, not just submission, so one removed in between causes that order to be refunded instead of executed. Also adds a `MAX_ORDER_LIFETIME_SECS` ceiling on trader-supplied deadlines, a permissionless `expire_order`/`get_expired_orders` path so a trader isn't dependent on the full batch window elapsing to reclaim an expired order, and `claim_refund` so a refund/payout whose transfer fails at settlement becomes claimable later instead of being permanently stranded. (closes #700)

### Breaking
- `LpToken::unlock(holder, amount)` is replaced by `LpToken::unlock(holder, locker, amount)`. The previous locker parameter read from `DataKey::Locker` storage is now an explicit argument. External SDK clients bound to the old public ABI must switch to the new signature.

### Legacy
- `contracts/amm/src/lib.rs` references several `DataKey` enum variants that are not declared in the enum on `main` (`FeeBps`, `AccruedFeeA`, `AccruedFeeB`, `FeeRecipient`, `ProtocolFeeBps`, `FlashLoanFeeBps`, `Paused`, `Admin`, `PendingAdmin`). They are unrelated to this fix and are tracked as a separate AMM-compile-blocker issue.
- Governance contract with multi-type parameter voting (`ProposalKind` enum covering Fee, Protocol Fee, Flash Loan Fee, Transfer Admin, Pause, and Unpause), timelocks, quorum requirements, and voting power locks (#137)
- Factory contract for deploying and registering AMM pools, featuring pool count (`get_pool_count`) and paginated pool queries (`get_pools`) (#139)
- Flash loan support with a dedicated update interface (`update_flash_loan_fee`) and configurable fees
- TWAP price accumulators via `get_price_cumulative` and a sample `TwapConsumer` contract
- Protocol fee collection (`set_protocol_fee`, `get_protocol_fee`, `withdraw_protocol_fees`)
- Emergency pause/unpause circuit breakers (`pause`, `unpause`, `is_paused`)
- Post-deployment swap fee adjustment (`update_fee`)
- Two-step administrator transfer (`propose_admin`, `accept_admin`)
- Ledger timestamp `deadline` parameter on `swap`, `swap_exact_out`, `add_liquidity`, and `remove_liquidity` for execution safety
- Detailed swap quotes (`simulate_swap`) including price impact and fee breakdown
- Reverse query quote (`get_amount_in`)
- Python client example (`examples/python/`)
- TS client example (`examples/client/`)
- Reproducible contract build environment with Docker
- Makefile with shortcuts for building, testing, linting, formatting, and end-to-end testing
- Complete machine-readable ABI schema JSON (`docs/abi.json`) (#143)
### Changed
- `reserve_manager` docs clarify the contract is off-chain-only; on-chain AMM hookup is deferred (#518).
