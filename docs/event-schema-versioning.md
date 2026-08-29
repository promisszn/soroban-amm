# Event schema versioning (#302)

Every event emitted by the AMM, CL, governance, factory (and any future
contract that adopts the pattern) carries a `schema_version: u32`
field. Off-chain consumers (GraphQL indexer, health dashboard, any
WebSocket subscriber) read this field before decoding the rest of
the payload so an unexpected version can be quarantined rather than
silently misinterpreted.

## How it's emitted

Every emit site uses the `soroban_amm_sdk::emit_versioned_event!`
macro instead of `env.events().publish(...)`:

```rust
// Before:
env.events().publish(
    (Symbol::new(&env, "swap"), trader.clone()),
    (token_in, amount_in, token_out, amount_out),
);

// After (#302):
soroban_amm_sdk::emit_versioned_event!(
    env,
    (Symbol::new(&env, "swap"), trader.clone()),
    (token_in, amount_in, token_out, amount_out),
);
```

The macro expands to:

```rust
env.events().publish(
    /* topic */ (Symbol::new( &env, "swap"), trader.clone()),
    /* data  */ (soroban_amm_sdk::EVENT_SCHEMA_VERSION, (token_in, amount_in, token_out, amount_out)),);

```

So the on-wire shape is `(version: u32, original_payload)`. **Topic is
unchanged** -- consumers can keep filtering by event name + author the
way they always have.

## How consumers decode

```rust
let (version, payload): (u32, (Address, i128, Address, i128)) =
    event.data.try_into_val(env)?;

match version {
    1 => decode_v1(payload),
    2 => decode_v2(payload),  // future
    other => {
        log::warn!("unknown event schema version {other}; skipping");
        return Ok(());
    }
}
```

## When to bump `EVENT_SCHEMA_VERSION`

`pub const EVENT_SCHEMA_VERSION: u32 = 1;` lives in
`contracts/amm-sdk/src/lib.rs`.

Bump it (single integer, +1 per release) when ANY event's payload
shape changes:

- Field added
- Field removed
- Field type changed
- Field order changed (Soroban encodes tuples positionally)
- Field renamed (no runtime effect, but indexers that key on names
  will break)

Don't bump for:

- Adding a new event type (consumers can ignore unknown topics)
- Changing event topic content (topic is separate from payload a
  topic change is independently observable)

## Versioning is global, not per-event

One `EVENT_SCHEMA_VERSION` covers every contract event in the
workspace. The alternative -- per-event version -- was rejected because:

1. Consumer state machines would have to track N independent version
   sequences, one per event type.
2. The contracts ship together as a single workspace release; if any
   event payload changes, the deployment cycle re-bumps every consumer
   anyway.
3. A global version is one branch in the consumer's decoder, not N.

The cost is that bumping any event's payload bumps the "version" for
every event, even unchanged ones. That's fine: consumers see the
unchanged payload as the same bytes, just with a different `version`
prefix -- easy to validate during the upgrade window.

## Affected contracts

The catalogue below is the current source-level event inventory for the contracts
that emit versioned events. Keep it synchronized with every
`emit_versioned_event!` call when adding or changing events; do not rely on a
hard-coded site count because tests and helper examples may also contain macro
calls.

`contracts/staking/src/lib.rs` already emits several plain
(unversioned) events via `env.events().publish(...)` directly, predating
this scheme -- it does not depend on `soroban_amm_sdk`, so it was left on
its existing style rather than migrated to `emit_versioned_event!` as
part of an unrelated change (see the new `boost_exp` event below, added
for issue #699, which follows the same existing plain-event convention
for consistency with the rest of the contract). A full migration of
`staking` onto the versioned scheme is tracked separately and out of
scope here.

`contracts/twal_consumer/src/lib.rs` is intentionally out of scope. It
already emitted an unversioned `snapshot_deleted` event before this
migration, and the tracked-pool lifecycle events added in #695
(`pool_add`, `pool_remove`) follow that same existing, unversioned
convention rather than mixing versioning schemes within one contract.
Migrating `twal_consumer` to `emit_versioned_event!` — all three event
sites at once — is left as a follow-up.

Test files in each of those crates were updated to decode the
versioned payload shape; see `__ver_N_locals+ assert_eq(!version,
EVENT_SCHEMA_VERSION)` assertions added by `migrate_tests.py`.

## New events in this change

The factory now emits two additional events:

- `pool_meta`: emitted when pool metadata is created or updated. Payload:
  `(schema_version: u32, (label: String, category: PoolCategory, created_at: u64, created_by: Address, verified: bool))`.
- `pool_ver`: emitted when the verified flag changes. Payload:
  `(schema_version: u32, verified: bool)`.

Consumers should ignore unknown topics; existing versioning rules apply.

## Event catalogue

Every row below has the on-wire data shape `(schema_version, payload)`.
The topic is the event name, optionally followed by the address shown in the
`topics` column. Field types use the Rust/Soroban names from the emitting
contract.

### Constant-product AMM — `contracts/amm/src/lib.rs`

| Event | Topics | Payload |
|---|---|---|
| `swap` | `trader` | `(token_in: Address, amount_in: i128, token_out: Address, amount_out: i128)` on standard paths; the fee-on-transfer path uses `(token_in, actual_received, token_out, amount_out, referrer: Option<Address>)` |
| `flash_loan` | `receiver` | `(amount_a: i128, amount_b: i128, fee_a: i128, fee_b: i128)` |
| `fot_detected` | `token_in` | `(amount_in: i128, actual_received: i128)` |
| `add_liquidity` | `provider` | `(amount_a: i128, amount_b: i128, shares: i128)` |
| `rm_liq` | — | `(provider: Address, shares: i128, amount_a: i128, amount_b: i128)` |
| `rm_liq_1s` | — | `(provider: Address, shares: i128, token_out: Address, amount_out: i128)` |
| `admin_nominated` | — | `(current_admin: Address, new_admin: Address)` |
| `admin_changed` | — | `(new_admin: Address)` |

| `cb_config` | — | `(threshold_bps: i128, cooldown_secs: u64)` |
| `circuit_break` | — | `(baseline_price: i128, current_price: i128, deviation_bps: i128, threshold_bps: i128)` |
| `emergency_withdraw` | `admin` | `(to: Address, reserve_a: i128, reserve_b: i128)` |
| `flash_fee_upd` | `admin` | `(new_fee_bps: i128)` |
| `lp_rebate_set` | `admin` | `(lp_rebate_bps: i128)` |
| `multisig_set` | `admin` | `(quorum: u32)` |
| `ms_proposed` | `signer` | `(recipient: Address, approvals: u32)` |
| `ms_ew` | `signer` | `(to: Address, reserve_a: i128, reserve_b: i128)` |
| `force_unlock` | — | `(timestamp: u64)` |

The two swap payload variants and all configuration payloads above are emitted
by the current source. Consumers should use the event topic and the global
schema version together when selecting a decoder.

### Concentrated-liquidity AMM — `contracts/concentrated_liquidity/src/lib.rs`

| Event | Topics | Payload |
|---|---|---|
| `mint_pos` | `provider` | `(lower_tick: i32, upper_tick: i32, liquidity: i128, amount_a: i128, amount_b: i128)` |
| `mod_pos` | `provider` | `(lower_tick: i32, upper_tick: i32, liquidity_delta: i128, amount_a: i128, amount_b: i128)` |
| `mint_1t` | `provider` | `(lower_tick: i32, upper_tick: i32, liquidity: i128, amount_used: i128, dust: i128)` |
| `rng_ord` | `provider` | `(lower_tick: i32, upper_tick: i32, liquidity: i128, is_above: bool)` |
| `burn_pos` | `recipient` | `(lower_tick: i32, upper_tick: i32, liquidity: i128, amount_a: i128, amount_b: i128)` |
| `coll_fees` | `recipient` | `(lower_tick: i32, upper_tick: i32, total_a: i128, total_b: i128)` |
| `nft_link` | `provider` | `(token_id: i128, lower_tick: i32, upper_tick: i32)` |
| `swap` | `sender` | `(zero_for_one: bool, amount_in: i128, amount_out: i128, sqrt_price_x96: i128, current_tick: i32, liquidity: i128)` |
| `swap_out` | `sender` | `(zero_for_one: bool, amount_in: i128, amount_out: i128, sqrt_price_x96: i128, current_tick: i32)` |
| `price_upd` | `token_in`, `token_out` | `(amount_in: i128, amount_out: i128, sqrt_price_x96: i128, current_tick: i32)` |

### Factory — `contracts/factory/src/lib.rs`

| Event | Payload |
|---|---|
| `pool_created` | `(token_a: Address, token_b: Address, pool: Address, fee_bps: u32, lp_token: Address, ...)` |
| `cl_pool_created` | `(token_a: Address, token_b: Address, fee_bps: u32, pool: Address)` |
| `wasm_updated` | `(amm_wasm_hash: BytesN<32>, token_wasm_hash: BytesN<32>)` |
| `creation_paused` / `creation_unpaused` | `(admin: Address)` |
| `mode_changed` | `(enabled: bool)` |
| `creation_fee_set` | `(fee_token: Address, fee_amount: i128)` |
| `treasury_set` | `(treasury: Address, global_protocol_fee_bps: u32, updated: u32)` |
| `global_fee_set` | `(protocol_fee_bps: u32, offset: u32, updated: u32)` |
| `fees_swept` | `(treasury: Address, token: Address, offset: u32, pools_swept: u32, total_collected: i128)` |
| `creation_fee_paid` | `(caller: Address, fee_amount: i128)` |
| `pool_meta` | `(label: String, category: PoolCategory, created_at: u64, created_by: Address, verified: bool)` |
| `pool_ver` | `(verified: bool)` |

### Governance — `contracts/governance/src/lib.rs`

| Event | Payload |
|---|---|
| `admin_nominated` | `(current_admin: Address, new_admin: Address)` |
| `admin_changed` | `(new_admin: Address)` |
| `proposed` | `(id: u64, proposer: Address, kind: ProposalKind, vote_end: u64, snapshot_ledger: u32)` |
| `voted` | `(proposal_id: u64, voter: Address, choice: VoteChoice, voting_power: i128)` |
| `executed` | `(proposal_id: u64, kind: ProposalKind)` |
| `vote_unlocked` | `(proposal_id: u64, locked: i128)` |
| `vetoed` | `(proposal_id: u64, multisig: Address, now: u64, discussion_end: u64)` |

### BatchRouter — `contracts/batch_router/src/lib.rs`

| Event | Payload |
|---|---|
| `batch_op` | `(op_index: u32, kind: Symbol, result: BatchOpResult)` — emitted once per operation, in order, before the batch-level event. |
| `batch_executed` | `(ops_len: u32)` |

### DEX aggregator — `contracts/dex_aggregator/src/lib.rs`

`venue` is the pool a route is *entered* through: the venue that won the
first-hop decision. `venue_kind` is the `PoolKind` enum (`Amm` for the
constant-product V2 pools, `Cl` for concentrated liquidity).

| Event | Topics | Payload |
|---|---|---|
| `cl_reg` | — | `(token_a: Address, token_b: Address, fee_bps: i128, pool: Address)` |
| `route_sel` | — | `(venue: Address, venue_kind: PoolKind, amount_in: i128, amount_out: i128)` |
| `route_alt` | — | `(venue: Address, amount_out: i128, alt_venue: Address, alt_venue_kind: PoolKind, alt_amount_out: i128)` |
| `route_exe` | — | `(trader: Address, token_in: Address, token_out: Address, amount_in: i128, amount_out: i128, pool: Address)` |
| `tol_fail` | — | `(pool: Address, observed_bps: i128, tolerance_bps: i128)` |

`route_sel` is emitted by `find_best_route` (and therefore by `get_quote`,
`swap_best` and `is_price_within_tolerance`, which all route through it).
`route_alt` accompanies it only when a second venue actually completed a
quote to the destination token, so the improvement the aggregator delivered
is measurable; a single-venue pair emits `route_sel` alone. `route_exe` is
emitted by `execute_route` (and so by `swap_best`) after the swap settles and
carries the output the pools actually returned, not the quoted amount.
`tol_fail` is emitted only when `is_price_within_tolerance` rejects a quote.

### Staking — `contracts/staking/src/lib.rs` (unversioned, plain events)

Staking predates this scheme and is not yet migrated (see note above), so
its events carry no `schema_version` prefix -- the payload below is the
on-wire shape as-is, not `(schema_version, payload)`.

| Event | Topics | Payload |
|---|---|---|
| `boost_exp` | — | `(staker: Address, previous_boost: i128, settled_boost: i128)` |

`boost_exp` (issue #699) is emitted by `settle_boost`/`settle_boost_batch`
only when a lock's stored boost is actually reduced from a stale
post-expiry value down to `min_boost` -- it does not fire on a no-op call
(active lock, already-settled staker, or unknown address).

### Consumer rules

A consumer must inspect the leading `schema_version` before decoding any row in
this catalogue. It may decode a version it explicitly supports, quarantine a
newer version, and ignore an unknown topic. A payload shape change requires a
single global version bump even if the change affects only one event; adding a
new topic does not require a bump by itself.

## Update (#685)

`contracts/dex_aggregator/src/lib.rs` previously emitted nothing at all, so a
swap routed through the aggregator was invisible: the underlying pool emitted
its own `swap` event, but nothing recorded that the aggregator chose that
venue, what the alternatives quoted, or who requested the route. It now emits
five events -- `cl_reg`, `route_sel`, `route_alt`, `route_exe` and `tol_fail`
-- all through `emit_versioned_event!`, listed in the new DEX aggregator table
above and decodable through `soroban_amm_sdk::events`.

The execution event is named `route_exe` rather than `route_exec` because
`symbol_short!` accepts at most nine characters.

## Update (#711)

`contracts/batch_router/src/lib.rs` moved its `batch_executed` event off a
raw `env.events().publish` call and onto `emit_versioned_event!`, and gained
a new per-operation `batch_op` event carrying the op's index, kind, and
result so consumers can attribute effects within a batch without
re-deriving them from `batch_executed` alone. Both rows are listed in the
new BatchRouter event table above.

## Update (#698)

`contracts/amm/src/lib.rs` gained one more versioned emit site: the new
admin/multisig-gated `force_unlock` recovery function emits a `force_unlock`
event through `emit_versioned_event!`, consistent with every other
state-mutating call in this contract. The row for it is listed in the
Constant-product AMM event table above.

## Update (#696)

`contracts/concentrated_liquidity/src/lib.rs` gained one more versioned
emit site: `swap_exact_out` emits a `swap_out` event through
`emit_versioned_event!` (payload shape mirrors the existing `swap` event),
plus reuses the existing `price_upd` event on a price-moving trade,
consistent with every other state-mutating call in this contract. The row
for it is listed in the Concentrated-liquidity AMM event table above.
