# Roadmap

This roadmap describes where Soroban AMM is today and where it's headed. It is a
living document — priorities may shift as the protocol and the Stellar ecosystem
evolve. For how to get involved, see [CONTRIBUTING.md](CONTRIBUTING.md); issues
labeled [`good first issue`](https://github.com/promisszn/soroban-amm/labels/good%20first%20issue)
and [`help wanted`](https://github.com/promisszn/soroban-amm/labels/help%20wanted)
are the best entry points.

Status legend: ✅ shipped · 🚧 in active development · 🗓️ planned

---

## Phase 1 — V2 Core (shipped)

The production-ready constant-product AMM and its supporting contracts. These
form the stable foundation the rest of the protocol builds on.

- ✅ **Constant-product AMM pool** (`contracts/amm`) — add/remove liquidity,
  slippage-protected swaps, configurable fees, flash loans.
- ✅ **SEP-41 LP token** (`contracts/token`) — standards-compliant LP share
  accounting.
- ✅ **Factory & registry** (`contracts/factory`) — deploy and initialize an
  AMM + LP token pair in a single transaction.
- ✅ **On-chain governance** (`contracts/governance`) — LP-token-holder
  proposals and voting on fee changes, with quorum, voting windows, minimum
  stake, vote locking, and proposal cancellation.
- ✅ **TWAP oracle** (`contracts/twap_consumer`) — manipulation-resistant
  time-weighted average price feed other protocols can consume.
- ✅ **Test & fuzz coverage** — 460+ tests plus a property-based fuzz suite
  (`contracts/amm-fuzz`) verifying swap invariants, fee accounting, and
  liquidity math.
- ✅ **CI** — build, WASM size limits, `clippy`, `rustfmt`, and full test suite
  enforced on every pull request.

---

## Phase 2 — V3 Concentrated Liquidity (in progress)

A Uniswap-v3-style tick-based engine — the only open-source concentrated
liquidity implementation targeting Stellar. This is the current primary focus.

- 🚧 **Concentrated liquidity core** (`contracts/concentrated_liquidity`) —
  tick-based range positions, sqrt-price math, and per-range fee accrual for
  far greater capital efficiency than full-range V2 pools.
- 🚧 **Position NFTs** (`contracts/cl_position_nft`) — represent individual
  concentrated-liquidity positions as transferable tokens.
- 🚧 **Oracle aggregator** (`contracts/oracle_aggregator`) — combine and
  sanity-check price sources for the concentrated pools.
- 🚧 **V2 → V3 migration** (`contracts/v2_to_v3_migration`) — a path for LPs to
  move liquidity from V2 pools into concentrated positions.
- 🗓️ **Complete V3 test & fuzz coverage** — extend the property-based suite to
  the tick math and range-fee accounting before mainnet consideration.

---

## Phase 3 — Routing & Ecosystem Contracts (in progress)

Periphery contracts that make the protocol composable and capital-efficient for
integrators.

- 🚧 **Swap router** (`contracts/router`) — multi-hop swaps across pools.
- 🚧 **DEX aggregator** (`contracts/dex_aggregator`) — best-execution routing
  across available liquidity.
- 🚧 **Batch router & batch auctions** (`contracts/batch_router`,
  `contracts/batch_auction`) — batched execution to reduce MEV and improve
  pricing.
- 🚧 **Staking & incentives** (`contracts/staking`,
  `contracts/incentive_campaigns`) — liquidity-mining and reward campaigns.
- 🚧 **Protocol-owned liquidity** (`contracts/pol_vesting`,
  `contracts/reserve_manager`) — vesting and reserve management for
  protocol-controlled liquidity.

---

## Phase 4 — Hardening & Launch (planned)

- 🗓️ **Formal third-party security audit** — an external review to complement
  the existing internal property-based audit ([AUDIT.md](AUDIT.md)). See
  [SECURITY.md](SECURITY.md) for the disclosure process.
- 🗓️ **Resolve tracked audit findings** — including the open items documented in
  [AUDIT.md](AUDIT.md).
- 🗓️ **Testnet deployment & public smoke tests** — reproducible end-to-end
  deployment against Stellar testnet.
- 🗓️ **Mainnet deployment** — production release of the audited contract set.

---

## Phase 5 — Developer Experience & Frontend (planned)

- 🚧 **Rust SDK** (`contracts/amm-sdk`) — typed helpers for interacting with the
  contracts.
- ✅ **Client examples** — TypeScript and Python examples plus an off-chain
  simulator under `examples/`.
- 🗓️ **Web frontend** — a user-facing app for swaps, liquidity management, and
  governance voting (swap UI, LP dashboards, and proposal/voting flows).
- 🗓️ **Expanded documentation** — integration guides so other Stellar protocols
  (lending markets, derivatives, liquidation bots) can build on the pools,
  governance, and TWAP oracle.

---

## How priorities are set

Work is prioritized by (1) security and correctness of shipped contracts, (2)
completing the V3 concentrated liquidity engine, and (3) composability for
ecosystem integrators. Community input via issues and discussions directly
informs ordering — if something here matters to you, open an issue or comment on
an existing one.
