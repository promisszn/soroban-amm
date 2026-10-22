# Soroban AMM

[![CI](https://github.com/promisszn/soroban-amm/workflows/CI/badge.svg)](https://github.com/promisszn/soroban-amm/actions)

A full-stack AMM protocol built on Stellar's Soroban smart contract platform. It ships a battle-tested V2 constant-product pool today and is actively building a V3-style concentrated liquidity engine — the only open-source implementation of its kind on Stellar.

---

## Table of Contents

- [Why This Project](section)
- [Overview](section)
- [Architecture](section)
- [Contracts](section)
  - [AMM Pool Contract](section)
  - [LP Token Contract](section)
  - [Factory Contract](section)
  - [Governance Contract](section)
  - [TWAP Consumer Contract](section)
  - [Concentrated Liquidity Contract](section)
  - [Staking Contract](section)
- [Storage Layout and Upgrade Considerations](section)
- [Error Codes](section)
- [Math and Formulas](section)
- [Getting Started](section)
  - [Prerequisites](section)
  - [Build](section)
  - [Test](section)
- [Usage](section)
  - [Deploy via Factory](section)
  - [Deploy Manually](section)
  - [Add Liquidity](section)
  - [Swap Tokens](section)
  - [Remove Liquidity](section)
  - [Query the Pool](section)
  - [Use the TWAP Oracle](section)
  - [TypeScript Client Example](section)
  - [Python Client Example](section)
- [Off-chain Simulator](section)
- [Deployment Runbook](docs/deployment-runbook.md)
- [Roadmap](section)
- [Contributing](section)
- [Changelog](section)
- [Security](section)
- [License](section)

---

## Why This Project

Stellar has fast finality (~5 seconds), sub-cent fees, and a large existing user base around stablecoins and remittances. Its native DEX is order-book based — a constant-product AMM is a fundamentally different and more composable liquidity model that fills a real gap in the ecosystem.

Several AMM protocols already exist on Stellar, but each has meaningful limitations:

| Protocol | Pool model | Governance | Open source |
| |---|---|---|
| **Soroswap** | V2 constant-product only | None | Yes |
| **Phoenix** | V2 + stable pools | None | Yes |
| **Sushi** | Concentrated liquidity | None | No (Sushi mainline) |
| **Aquarius** | Governance layer only | AQUA token | Partial |
| **This project** | V2 now + V3 CL in progress | On-chain, LP-token-governed | Yes |

What makes this project different:

- **Concentrated liquidity (V3) in development** — the only open-source Soroban implementation targeting tick-based range positions, the same capital-efficiency model pioneered by Uniswap v3. LPs can earn more fees by concentrating capital in active price ranges instead of spreading it across an infinite curve.
- **On-chain governance** — LP token holders can propose and vote on fee changes directly through a governance contract, with configurable quorum, voting windows, minimum stake, vote locking, and proposal cancellation. No protocol is governed off-chain or by a single admin key.
- **TWAP oracle** — a manipulation-resistant time-weighted average price feed that other protocols (lending markets, derivatives) can build on top of, without needing a separate oracle network.
- **Flash loans** — single-transaction borrowing from pool reserves with a configurable fee, enabling arbitrage, collateral swaps, and liquidation bots.
- **Full composability** — every contract in the protocol is independently deployable and interoperable. The factory, governance, and TWAP contracts can be used with any pool, not just the ones deployed here.

---

## Overview

The protocol lets users:

- **Provide liquidity** — deposit two tokens into a pool and receive LP tokens representing their share of the reserves (V2), or mint a tick-range position for concentrated capital efficiency (V3).
- **Swap tokens** — exchange one pool token for the other at a price determined by the pool's invariant, with slippage protection.
- **Redeem liquidity** — burn LP tokens (V2) or close a position (V3) to withdraw a proportional share of reserves plus accrued fees.
- **Govern the protocol** — stake LP tokens to propose fee changes; vote on active proposals; execute passing proposals on-chain.
- **Access price data** — query a TWAP oracle for a manipulation-resistant average price over any window.

---

## Architecture

The project is a Cargo workspace with five contracts:

```
soroban-amm/
├── Cargo.toml                        # Workspace root
└── contracts/
    ├── amm/                          # V2 constant-product AMM pool
    │   └── src/lib.rs
    ├── token/                        # SEP-41 LP token contract
    │   └── src/lib.rs
    ├── factory/                      # Pool factory and registry
    │   └── src/lib.rs
    ├── governance/                   # On-chain LP governance
    ├── twap_consumer/                # TWAP oracle consumer
    │   └── src/lib.rs
    └── concentrated_liquidity/       # V3-style tick-based AMM (in progress)
        └── src/lib.rs
```

The V2 AMM contract depends on the token contract — adding or removing liquidity mints or burns LP shares via the token contract. The factory deploys and initialises an AMM + LP token pair in a single transaction. The governance contract holds a reference to a pool and allows LP token holders to vote on parameter changes. The TWAP consumer reads cumulative price state from any AMM pool. The concentrated liquidity contract is standalone — it does not use the LP token and manages positions internally.

---

## Contracts

The protocol consists of modular, composable smart contracts. **[`docs/abi.json`](docs/abi.json)** contains Stellar `SCSpecEntry` JSON interfaces generated from every deployable contract WASM. The complete contract-to-key index is:

| Contract | ABI key |
| |---|
| AMM Pool | `amm` |
| Batch Auction | `batch_auction` |
| Batch Router | `batch_router` |
| CL Position NFT | `cl_position_nft` |
| Concentrated Liquidity | `concentrated_liquidity` |
| DEX Aggregator | `dex_aggregator` |
| Factory | `factory` |
| Governance | `governance` |
| Incentive Campaigns | `incentive_campaigns` |
| Oracle Aggregator | oracle_aggregator |
| POL Vesting | `pol_vesting` |
| Reserve Manager | `reserve_manager` |
| Router | `router` |
| Staking | `staking` |
| LP Token | `token` |
| TWAL Consumer | `twal_consumer` |
| TWAP Consumer | `twap_consumer` |
| V2 to V3 Migration | `v2_to_v3_migration` |

### AMM Pool Contract

Located in [`contracts/amm/src/lib.rs`](contracts/amm/src/lib.rs).

The core V2 constant-product pool (`x * y = k`). It manages reserves for a token pair, executes swaps with configurable fees and slippage protection, mints and burns LP shares, accumulates TSAP price ratios, provides flash loans, and enforces emergency circuit breakers and pause controls.

- **Flash loans**: Single-transaction borrowing from pool reserves repayable within the receiver callback with configurable fees (`flash_loan`). Borrowers must implement the [`FlashLoanReceiver`](examples/flash_loan_receiver/README.md) callback interface.
- **Protocol fees**: Configurable protocol fee share routed to a dedicated recipient address.
- **Admin & Safety**: Two-step admin transfer (`propose_admin` / `accept_admin`), emergency pause/unpause, and single-block price deviation circuit breaker.
- **LP locker delegation**: `set_lp_locker(locker)` lets the pool admin authorize a contract (normally governance) as the LP token's locker, so `governance::vote` can lock LP tokens. Requires admin auth.
- **Interface & Schema**: See [`docs/abi.json`](docs/abi.json) under `"amm"` for complete function definitions (`initialize`, `swap`, `add_liquidity`, `remove_liquidity`, `flash_loan`, `get_amount_out`, `get_info`, etc.).

#### Flash Loan Receiver Interface

Borrowers must implement a callback contract with this interface:

```rust
pub trait FlashLoanReceiver {
    fn on_flash_loan(env: Env, token: Address, amount: i128, fee: i128, data: Bytes) -> bool;
}
```

During `flash_loan`, the AMM transfers `amount` of `token` to `receiver`, invokes `on_flash_loan`, and verifies that the pool's token balance increased by at least `fee`. If the receiver does not return `amount + fee` before the callback finishes, the transaction reverts. See [examples/flash_loan_receiver/README.md](examples/flash_loan_receiver/READMG.md) for a reference implementation covering arbitrage, collateral swaps, and failure modes.

### LP Token Contract

Located in [`contracts/token/src/lib.rs`](contracts/token/src/lib.rs).

A SEP-41 compliant token representing proportional liquidity shares in a pool. The corresponding AMM pool contract is the administrator with sole authority to `mint` and `burn` shares.

- **Capabilities**: Standard SEP-41 balance queries, transfers, and allowances, plus balance locking for governance voting.
- **Interface & Schema**: See [`docs/abi.json`](docs/abi.json) under `"token"`.

### Factory Contract

Located in [`contracts/factory/src/lib.rs`](contracts/factory/src/lib.rs).

A single-entry-point registry for deploying and discovering pools.

- Deploys an AMM pool and its paired LP token in a single atomic transaction.
- Normalizes token pair order (smaller address first) to enforce uniqueness per pair.
- Maintains a registry of all deployed pools with total count, paginated listings (`get_pools`), and reverse LP token lookups.
- **Interface & Schema**: See [`docs/abi.json`](docs/abi.json) under `"factory"`.

### Governance Contract

Located in [`contracts/governance/src/lib.rs`](contracts/governance/src/lib.rs).

Enables on-chain parameter management governed by LP token holders.

- **Proposals**: LP token holders meeting the minimum stake threshold can propose fee and parameter changes.
- **Voting**: Votes are weighted by the voter's LP token balance at the time of voting; tokens are locked until proposal resolution. Locking requires governance to be the LP token's locker — `claim_lp_locker()` makes this contract the locker while it is the pool admin, and `factory::create_pool` calls it automatically when deploying governance.
- **Execution & Safety**: Passed proposals require a timelock delay before execution, and support emergency cancellation and veto mechanisms.
- **Interface & Schema**: See [`docs/abi.json`](docs/abi.json) under `"governance".

### TWAP Consumer Contract

Located in [`contracts/twap_consumer/src/lib.rs`](contracts/twap_consumer/src/lib.rs).

An integration contract that reads the AMM's cumulative price oracle and computes a fixed-window TSAP for external consumers.

- Stores periodic snapshots (`save_snapshot`, `save_cl_snapshot`).
- **Tracked pool set**: add and remove pools with `keeper`-only `add_tracked_pool(pool, pool_type)` and `remove_tracked_pool(pool)` (idempotent add, `NotTracked` on remove). The set is capped at `MAX_TRACKED_POOLS = 100`; `save_*snapshot` uses the same capped registration and returns `TooManyTrackedPools` past the cap. Add/remove emit versioned `pool_add` / `pool_remove` events.
- Computes windowed average prices (`get_twap_price`, `get_cl_twap`, `get_twap_all`). `get_twap_all` returns the first error encountered for ABI compatibility; use `get_twap_all_safe` for fault-isolated reads.
- **Fault-isolated batch reads**: `get_twap_all_safe(window)` returns one entry per tracked pool with `ok', `error_code`, `twap` and `pool_type`, and `get_twap_batch(pools, window)` reads a caller-chosen subset (capped at `MAX_TRACKED_POOLS`). Both isolate failures, including cross-contract panics, via `try_` clients and never abort the whole call.
- Validates spot prices against TWAP with configurable maximum deviation (`validate_price_against_twap`, `assert_lending_price_safe`).
- **Keeper lifecycle**: two-step rotation via `propose_keeper` / `accept_keeper` with `pending_keeper` getter, plus keeper-gated `upgrade(new_wasm_hash)`. `initialize` emits an `initialized` event and the deploy script reads back `get_keeper` and aborts on a mismatch.
- **Interface & Schema**: See [`docs/abi.json`](docs/abi.json) under `"twap_consumer"`.

### Concentrated Liquidity Contract

Located in [`contracts/concentrated_liquidity/src/lib.rs`](contracts/concentrated_liquidity/src/lib.rs).

A V3-style tick-based AMM where liquidity providers specify a price range `[lower_tick, upper_tick]` for their capital. Only liquidity within the active price range earns fees, enabling significantly higher capital efficiency than full-range V2 pools.

- **Status: In active development.** The position model, fee accounting, and in-place position modification flows are implemented. The tick registry, tick bitmap, math library, and swap engine are tracked in issues [#177](https://github.com/promisszn/soroban-amm/issues/177)–[#180](https://github.com/promisszn/soroban-amm/issues/180).
- **Key differences from V2**: Range-bound capital concentration, non-fungible position records `(owner, lower_tick, upper_tick)`, and tick accumla
