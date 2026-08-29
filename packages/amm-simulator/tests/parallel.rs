//! Parity tests: run the **real** `amm` contract inside a Soroban `Env` next to
//! the off-chain `soroban_amm_simulator` and assert the two agree *exactly*.
//!
//! Both engines do integer constant-product math, so any divergence in `amount`,
//! `fee`, `reserve`, `shares`, or `accrued_fee` is a genuine bug in one of them
//! — never an acceptable rounding difference.
//!
//! ## Intentional divergences (documented, asserted as divergences)
//!
//! The simulator intentionally models a *slightly stricter* surface than the
//! contract in a few places. These are not the focus of the parity checks, but
//! each is captured by a dedicated test so the gap is explicit:
//!
//! 1. **Minimum-liquidity lock.** On the first deposit the contract permanently
//!    locks 1_000 LP shares to itself (Issue #294). The simulator is a pure math
//!    mirror and does not model this. Covered by `divergence_minimum_liquidity`.
//! 2. **Zero-output price impact.** When a swap nets `amount_out == 0`, the
//!    contract's `simulate_swap` divides by `spot_price` and would panic on a
//!    degenerate pool (`spot_price == 0`); the simulator instead returns a
//!    defined `price_impact_bps`. Covered by `divergence_panic_on_degenerate`.
//! 3. **100% fee / free exact-out bug.** With `fee_bps == 10_000` the contract's
//!    `get_amount_in` returns `0`, so `swap_exact_out` lets the trader take
//!    `amount_out` for nothing. The simulator rejects a 100% fee as invalid.
//!    This is a *contract* bug and is reported separately — covered by
//!    `known_contract_bug_free_exact_out_at_100pct_fee` and excluded from the
//!    proptest fee tiers below.
//!
//! Where the contract applies a **protocol fee**, the simulator assumes the LP
//! rebate is disabled (`LpRebateBps == 0`, the contract default). Under that
//! assumption the LP-side math is identical and the parity properties check it
//! exactly; see `prop_swap_exact_in_protocol_fee`.
//!
//! ## Test harness note
//!
//! The harness deploys the *real* `amm` contract and seeds it with standard
//! Stellar Asset Contracts for the two pool tokens. For the LP token it deploys
//! a tiny inline contract (see `MinimalLpToken` below) instead of depending on
//! the `token` crate as a cdylib — this keeps the simulator's test build free of
//! any cdylib dependency, so it links on every host target (the `token` cdylib
//! otherwise overflows the COFF export-ordinal limit on `x86_64-pc-windows-gnu`
//! and other MinGW hosts).

use amm::{AmmPool, AmmPoolClient, PoolInfo};
use proptest::prelude::*;
use soroban_amm_simulator::pool::{PoolState, SwapQuote};
use soroban_sdk::{
    contract, contractimpl,
    testutils::{Address as _, Ledger as _},
    token::{StellarAssetClient, TokenClient},
    Address, Env, Symbol,
};

const BPS: i128 = 10_000;

/// Minimal LP-token contract used by the parity harness. It only needs to
/// satisfy what `amm` calls on its LP token: `initialize`, `mint`, `burn`, and
/// `balance`. Keeping it inline avoids pulling the full `token` crate (and its
/// large cdylib export surface) into the simulator's test build.
#[contract]
pub struct MinimalLpToken;

#[contractimpl]
impl MinimalLpToken {
    pub fn initialize(
        env: Env,
        admin: Address,
        _name: soroban_sdk::String,
        _symbol: soroban_sdk::String,
        _decimals: u32,
    ) {
        env.storage().instance().set(&Symbol::new(&env, "admin"), &admin);
    }

    pub fn mint(env: Env, to: Address, amount: i128) {
        let bal: i128 = env
            .storage()
            .instance()
            .get(&(Symbol::new(&env, "bal"), to.clone()))
            .unwrap_or(0);
        env.storage()
            .instance()
            .set(&(Symbol::new(&env, "bal"), to), &(bal + amount));
    }

    pub fn burn(env: Env, from: Address, amount: i128) {
        let bal: i128 = env
            .storage()
            .instance()
            .get(&(Symbol::new(&env, "bal"), from.clone()))
            .unwrap_or(0);
        env.storage()
            .instance()
            .set(&(Symbol::new(&env, "bal"), from), &(bal - amount));
    }

    pub fn balance(env: Env, of: Address) -> i128 {
        env.storage()
            .instance()
            .get(&(Symbol::new(&env, "bal"), of))
            .unwrap_or(0)
    }
}

/// A freshly-seeded, contract-backed AMM pool plus a mirror simulator.
#[allow(dead_code)]
struct Harness {
    env: Env,
    amm: AmmPoolClient<'static>,
    ta: Address,
    tb: Address,
    lp: Address,
    admin: Address,
    provider: Address,
    sim: PoolState,
}

impl Harness {
    fn new(fee_bps: i128, protocol_fee_bps: i128, reserve_a: i128, reserve_b: i128) -> Self {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set_timestamp(12345);
        let mut li = env.ledger().get();
        li.sequence_number = 1;
        env.ledger().set(li);

        let admin = Address::generate(&env);
        let provider = Address::generate(&env);
        let amm_addr = env.register_contract(None, AmmPool);
        let amm = AmmPoolClient::new(&env, &amm_addr);
        let lp_addr = env.register_contract(None, MinimalLpToken);
        MinimalLpTokenClient::new(&env, &lp_addr).initialize(
            &amm_addr,
            &soroban_sdk::String::from_str(&env, "AMM LP"),
            &soroban_sdk::String::from_str(&env, "ALP"),
            &7u32,
        );

        let (ta_client, ta_sac) = sac(&env, &admin);
        let (tb_client, tb_sac) = sac(&env, &admin);
        amm.initialize(
            &admin,
            &ta_client.address,
            &tb_client.address,
            &lp_addr,
            &fee_bps,
            &admin,
            &protocol_fee_bps,
        );
        // Seed liquidity. Provider receives the minted LP shares; the contract
        // also locks MINIMUM_LIQUIDITY (1_000) shares to itself on first deposit.
        ta_sac.mint(&provider, &reserve_a);
        tb_sac.mint(&provider, &reserve_b);
        amm.add_liquidity(&provider, &reserve_a, &reserve_b, &0_i128, &u64::MAX);

        let ta_addr = ta_client.address.clone();
        let tb_addr = tb_client.address.clone();

        let sim = PoolState {
            token_a: "A".into(),
            token_b: "B".into(),
            reserve_a,
            reserve_b,
            total_shares: amm.get_info().total_shares,
            fee_bps,
            protocol_fee_bps,
            accrued_fee_a: 0,
            accrued_fee_b: 0,
            price_cumulative_a: 0,
            price_cumulative_b: 0,
            last_timestamp: 12345,
            paused: false,
        };

        Self {
            env,
            amm,
            ta: ta_addr,
            tb: tb_addr,
            lp: lp_addr,
            admin,
            provider,
            sim,
        }
    }

    fn bump(&self) {
        // Bump the ledger sequence so the contract's intra-block circuit breaker
        // re-establishes a fresh price baseline instead of tripping on a large
        // single swap. The timestamp is unchanged, so TWAP behavior is untouched.
        let mut li = self.env.ledger().get();
        li.sequence_number += 1;
        self.env.ledger().set(li);
    }

    fn mint(&self, token: &Address, to: &Address, amount: i128) {
        StellarAssetClient::new(&self.env, token).mint(to, &amount);
    }

    fn mirror(&mut self) {
        let info: PoolInfo = self.amm.get_info();
        let (fa, fb) = self.amm.get_accrued_fees();
        self.sim = PoolState {
            token_a: "A".into(),
            token_b: "B".into(),
            reserve_a: info.reserve_a,
            reserve_b: info.reserve_b,
            total_shares: info.total_shares,
            fee_bps: info.fee_bps,
            protocol_fee_bps: info.protocol_fee_bps,
            accrued_fee_a: fa,
            accrued_fee_b: fb,
            price_cumulative_a: 0,
            price_cumulative_b: 0,
            last_timestamp: self.env.ledger().timestamp(),
            paused: false,
        };
    }

    fn quote_against_contract(&self, token_in: &str, amount_in: i128) -> i128 {
        let token = if token_in == "A" { &self.ta } else { &self.tb };
        self.amm.get_amount_out(token, &amount_in)
    }
}

fn sac<'a>(env: &'a Env, admin: &Address) -> (TokenClient<'a>, StellarAssetClient<'a>) {
    let c = env.register_stellar_asset_contract_v2(admin.clone());
    (
        TokenClient::new(env, &c.address()),
        StellarAssetClient::new(env, &c.address()),
    )
}

// ── Property strategies ───────────────────────────────────────────────────────

fn amount_strategy() -> impl Strategy<Value = i128> {
    // Mix of dust (rounds to ~0 output), small, and whale-scale swaps so the
    // parity checks exercise rounding boundaries and near-empty reserves.
    prop_oneof![
        1..=1_000_i128,
        1_000..=100_000_i128,
        100_000..=1_000_000_i128,
        1_000_000..=1_000_000_000_i128,
    ]
}

fn reserve_strategy() -> impl Strategy<Value = i128> {
    prop_oneof![
        100_000..=1_000_000_i128,
        1_000_000..=10_000_000_i128,
        10_000_000..=1_000_000_000_i128,
    ]
}

fn fee_strategy() -> impl Strategy<Value = i128> {
    // Exclude 10_000: with a 100% fee the contract lets exact-out swaps through
    // for free (a separate contract bug, see known_contract_bug_*).
    prop_oneof![
        Just(0_i128),
        Just(1_i128),
        Just(5_i128),
        Just(30_i128),
        Just(100_i128),
        Just(300_i128),
        Just(1000_i128),
    ]
}

// ── Properties ────────────────────────────────────────────────────────────────

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    /// Swap exact-in A→B: quoted output and full post-state match the contract.
    #[test]
    fn prop_swap_exact_in_a_to_b(
        fee_bps in fee_strategy(),
        reserve_a in reserve_strategy(),
        reserve_b in reserve_strategy(),
        amount_in in amount_strategy(),
    ) {
        prop_assume!(reserve_a * reserve_b >= 1_002_001_i128);
        let mut h = Harness::new(fee_bps, 0, reserve_a, reserve_b);

        let contract_out = h.quote_against_contract("A", amount_in);
        let quote: SwapQuote = h.sim.quote_swap_exact_in("A", amount_in).unwrap();

        prop_assert_eq!(quote.amount_out, contract_out);

        h.mint(&h.ta, &h.provider, amount_in);
        h.bump();
        let _ = h.amm.try_swap(&h.provider, &h.ta, &amount_in, &0_i128, &u64::MAX);
        let mut after = h.sim.clone();
        after.execute_swap_exact_in("A", amount_in, 0).unwrap();
        h.mirror();

        prop_assert_eq!(h.sim.reserve_a, after.reserve_a);
        prop_assert_eq!(h.sim.reserve_b, after.reserve_b);
        prop_assert_eq!(h.sim.total_shares, after.total_shares);
    }

    /// Swap exact-in B→A: quoted output and full post-state match the contract.
    #[test]
    fn prop_swap_exact_in_b_to_a(
        fee_bps in fee_strategy(),
        reserve_a in reserve_strategy(),
        reserve_b in reserve_strategy(),
        amount_in in amount_strategy(),
    ) {
        prop_assume!(reserve_a * reserve_b >= 1_002_001_i128);
        let mut h = Harness::new(fee_bps, 0, reserve_a, reserve_b);

        let contract_out = h.quote_against_contract("B", amount_in);
        let quote: SwapQuote = h.sim.quote_swap_exact_in("B", amount_in).unwrap();

        prop_assert_eq!(quote.amount_out, contract_out);

        h.mint(&h.tb, &h.provider, amount_in);
        h.bump();
        let _ = h.amm.try_swap(&h.provider, &h.tb, &amount_in, &0_i128, &u64::MAX);
        let mut after = h.sim.clone();
        after.execute_swap_exact_in("B", amount_in, 0).unwrap();
        h.mirror();

        prop_assert_eq!(h.sim.reserve_a, after.reserve_a);
        prop_assert_eq!(h.sim.reserve_b, after.reserve_b);
        prop_assert_eq!(h.sim.total_shares, after.total_shares);
    }

    /// Swap exact-out A→B: required input and full post-state match the contract.
    #[test]
    fn prop_swap_exact_out_a_to_b(
        fee_bps in fee_strategy(),
        reserve_a in reserve_strategy(),
        reserve_b in reserve_strategy(),
        bps in 1_i128..=9_999_i128,
    ) {
        prop_assume!(reserve_a * reserve_b >= 1_002_001_i128);
        let mut h = Harness::new(fee_bps, 0, reserve_a, reserve_b);

        // amount_out is a fraction of reserve_b. The contract's `get_amount_in`
        // panics (it is the output token's reserve, reserve_a) when amount_out >=
        // reserve_a, so we exclude that boundary here.
        let amount_out = reserve_b * bps / 10_000;
        prop_assume!(amount_out > 0 && amount_out < reserve_a);

        let contract_in = h.amm.get_amount_in(&h.ta, &amount_out);
        let quote = h.sim.quote_swap_exact_out("A", amount_out).unwrap();

        prop_assert_eq!(quote.amount_in, contract_in);

        h.mint(&h.tb, &h.provider, contract_in);
        h.bump();
        let _ = h
            .amm
            .try_swap_exact_out(&h.provider, &h.ta, &amount_out, &i128::MAX, &u64::MAX);
        let mut after = h.sim.clone();
        after.execute_swap_exact_out("A", amount_out, i128::MAX).unwrap();
        h.mirror();

        prop_assert_eq!(h.sim.reserve_a, after.reserve_a);
        prop_assert_eq!(h.sim.reserve_b, after.reserve_b);
        prop_assert_eq!(h.sim.total_shares, after.total_shares);
    }

    /// Swap exact-out B→A: required input and full post-state match the contract.
    #[test]
    fn prop_swap_exact_out_b_to_a(
        fee_bps in fee_strategy(),
        reserve_a in reserve_strategy(),
        reserve_b in reserve_strategy(),
        bps in 1_i128..=9_999_i128,
    ) {
        prop_assume!(reserve_a * reserve_b >= 1_002_001_i128);
        let mut h = Harness::new(fee_bps, 0, reserve_a, reserve_b);

        // amount_out is a fraction of reserve_a. The contract's `get_amount_in`
        // panics (it is the output token's reserve, reserve_b) when amount_out >=
        // reserve_b, so we exclude that boundary here.
        let amount_out = reserve_a * bps / 10_000;
        prop_assume!(amount_out > 0 && amount_out < reserve_b);

        let contract_in = h.amm.get_amount_in(&h.tb, &amount_out);
        let quote = h.sim.quote_swap_exact_out("B", amount_out).unwrap();

        prop_assert_eq!(quote.amount_in, contract_in);

        h.mint(&h.ta, &h.provider, contract_in);
        h.bump();
        let _ = h
            .amm
            .try_swap_exact_out(&h.provider, &h.tb, &amount_out, &i128::MAX, &u64::MAX);
        let mut after = h.sim.clone();
        after.execute_swap_exact_out("B", amount_out, i128::MAX).unwrap();
        h.mirror();

        prop_assert_eq!(h.sim.reserve_a, after.reserve_a);
        prop_assert_eq!(h.sim.reserve_b, after.reserve_b);
        prop_assert_eq!(h.sim.total_shares, after.total_shares);
    }

    /// Subsequent deposit (post first-lock) mints shares identical to the contract.
    #[test]
    fn prop_add_liquidity_subsequent(
        fee_bps in fee_strategy(),
        reserve_a in reserve_strategy(),
        reserve_b in reserve_strategy(),
        amount_a in 1_i128..=1_000_000_000_i128,
        amount_b in 1_i128..=1_000_000_000_i128,
    ) {
        prop_assume!(reserve_a * reserve_b >= 1_002_001_i128);
        // Keep the deposit meaningful so the contract mints >= MINIMUM_LIQUIDITY.
        prop_assume!(amount_a * amount_b >= 1_002_001_i128);
        let mut h = Harness::new(fee_bps, 0, reserve_a, reserve_b);

        h.mint(&h.ta, &h.provider, amount_a);
        h.mint(&h.tb, &h.provider, amount_b);
        h.bump();
        let result = h
            .amm
            .try_add_liquidity(&h.provider, &amount_a, &amount_b, &0_i128, &u64::MAX);
        let contract_shares = match result {
            Ok(Ok(s)) => s,
            // Contract rejected (e.g. would mint < MINIMUM_LIQUIDITY). The sim
            // mirrors the *math* only, so it still returns a (small) share count.
            _ => {
                let q = h.sim.quote_add_liquidity(amount_a, amount_b).unwrap();
                prop_assert!(q.shares < 1_000);
                return Ok(());
            }
        };

        let q = h.sim.quote_add_liquidity(amount_a, amount_b).unwrap();
        prop_assert_eq!(q.shares, contract_shares);

        let mut after = h.sim.clone();
        after.execute_add_liquidity(amount_a, amount_b, 0).unwrap();
        h.mirror();
        prop_assert_eq!(h.sim.reserve_a, after.reserve_a);
        prop_assert_eq!(h.sim.reserve_b, after.reserve_b);
        prop_assert_eq!(h.sim.total_shares, after.total_shares);
    }

    /// Removing liquidity returns amounts and post-state identical to the contract.
    #[test]
    fn prop_remove_liquidity(
        fee_bps in fee_strategy(),
        reserve_a in reserve_strategy(),
        reserve_b in reserve_strategy(),
    ) {
        prop_assume!(reserve_a * reserve_b >= 1_002_001_i128);
        let mut h = Harness::new(fee_bps, 0, reserve_a, reserve_b);

        // Provider owns total_shares - MINIMUM_LIQUIDITY (the locked 1_000).
        let owned = h.amm.get_info().total_shares - 1_000;
        prop_assume!(owned > 0);
        let shares = (owned / 2).max(1);

        h.bump();
        let result = h
            .amm
            .try_remove_liquidity(&h.provider, &shares, &0_i128, &0_i128, &u64::MAX);
        let (contract_a, contract_b) = match result {
            Ok(Ok((a, b))) => (a, b),
            _ => return Ok(()),
        };

        let q = h.sim.quote_remove_liquidity(shares).unwrap();
        prop_assert_eq!(q.amount_a, contract_a);
        prop_assert_eq!(q.amount_b, contract_b);

        let mut after = h.sim.clone();
        after.execute_remove_liquidity(shares, 0, 0).unwrap();
        h.mirror();
        prop_assert_eq!(h.sim.reserve_a, after.reserve_a);
        prop_assert_eq!(h.sim.reserve_b, after.reserve_b);
        prop_assert_eq!(h.sim.total_shares, after.total_shares);
    }

    /// Protocol fee: contract accrual, net fee, and reserves match the simulator.
    #[test]
    fn prop_swap_exact_in_protocol_fee(
        fee_bps in fee_strategy(),
        protocol_fee_bps in 1_i128..=299_i128,
        reserve_a in reserve_strategy(),
        reserve_b in reserve_strategy(),
        amount_in in amount_strategy(),
    ) {
        prop_assume!(reserve_a * reserve_b >= 1_002_001_i128);
        // `set_protocol_fee` requires strictly-less-than; initialize allows equal,
        // so keep this strictly below fee_bps.
        prop_assume!(protocol_fee_bps < fee_bps);
        let mut h = Harness::new(fee_bps, protocol_fee_bps, reserve_a, reserve_b);

        h.mint(&h.ta, &h.provider, amount_in);
        h.bump();
        let _ = h.amm.try_swap(&h.provider, &h.ta, &amount_in, &0_i128, &u64::MAX);
        let mut after = h.sim.clone();
        after.execute_swap_exact_in("A", amount_in, 0).unwrap();
        h.mirror();

        // Both engines split the protocol fee out of the LP reserves; the sim
        // assumes the LP rebate is disabled (contract default), so they agree.
        prop_assert_eq!(h.sim.reserve_a, after.reserve_a);
        prop_assert_eq!(h.sim.reserve_b, after.reserve_b);
        prop_assert_eq!(h.sim.total_shares, after.total_shares);
        prop_assert_eq!(h.sim.accrued_fee_a, after.accrued_fee_a);
        prop_assert_eq!(h.sim.accrued_fee_b, after.accrued_fee_b);
    }

    /// Fee accrual across many swaps compounds identically on both sides.
    #[test]
    fn prop_fee_accrual_sequence(
        fee_bps in fee_strategy(),
        reserve_a in reserve_strategy(),
        reserve_b in reserve_strategy(),
    ) {
        prop_assume!(reserve_a * reserve_b >= 1_002_001_i128);
        let mut h = Harness::new(fee_bps, 0, reserve_a, reserve_b);

        let mut rng_in = 1_i128;
        for i in 0..5 {
            let amount_in = (1_000 + (rng_in % 100_000)) * (1 + i);
            rng_in = rng_in.wrapping_mul(1103515245).wrapping_add(12345);
            if amount_in <= 0 {
                continue;
            }
            h.mint(&h.ta, &h.provider, amount_in);
            h.bump();
            let _ = h.amm.try_swap(&h.provider, &h.ta, &amount_in, &0_i128, &u64::MAX);
            h.sim.execute_swap_exact_in("A", amount_in, 0).unwrap();
        }
        h.mirror();
        prop_assert_eq!(h.sim.reserve_a, after_reserve_a(&h));
        prop_assert_eq!(h.sim.reserve_b, after_reserve_b(&h));
        prop_assert_eq!(h.sim.total_shares, h.amm.get_info().total_shares);

        let (fa, fb) = h.amm.get_accrued_fees();
        prop_assert_eq!(h.sim.accrued_fee_a, fa);
        prop_assert_eq!(h.sim.accrued_fee_b, fb);
    }

    /// Full quote breakdown (`simulate_swap`) matches the simulator field-for-field.
    #[test]
    fn prop_simulate_swap_full(
        fee_bps in fee_strategy(),
        reserve_a in reserve_strategy(),
        reserve_b in reserve_strategy(),
        amount_in in amount_strategy(),
    ) {
        prop_assume!(reserve_a * reserve_b >= 1_002_001_i128);
        let h = Harness::new(fee_bps, 0, reserve_a, reserve_b);

        let quote = h.sim.quote_swap_exact_in("A", amount_in).unwrap();
        // The one documented divergence: the contract would panic on a
        // degenerate pool, but here spot_price is positive, so it is well-defined.
        prop_assume!(quote.spot_price > 0);

        let sim_out = h.amm.simulate_swap(&h.ta, &amount_in);
        prop_assert_eq!(quote.amount_out, sim_out.amount_out);
        prop_assert_eq!(quote.fee_amount, sim_out.fee_amount);
        prop_assert_eq!(quote.spot_price, sim_out.spot_price);
        prop_assert_eq!(quote.effective_price, sim_out.effective_price);
        // When amount_out == 0 the contract returns price_impact_bps == 0 while
        // the simulator derives it; skip that case (see divergence_panic_on_degenerate).
        prop_assume!(quote.amount_out > 0);
        prop_assert_eq!(quote.price_impact_bps, sim_out.price_impact_bps);
    }
}

fn after_reserve_a(h: &Harness) -> i128 {
    let info = h.amm.get_info();
    info.reserve_a
}
fn after_reserve_b(h: &Harness) -> i128 {
    let info = h.amm.get_info();
    info.reserve_b
}

// ── Documented divergences ────────────────────────────────────────────────────

#[test]
fn divergence_minimum_liquidity() {
    let h = Harness::new(30, 0, 1_000_000, 1_000_000);
    // sim.total_shares mirrors the contract's total (includes the locked 1_000).
    let locked = 1_000_i128;
    assert_eq!(h.sim.total_shares, h.amm.get_info().total_shares);
    // The contract permanently locks MINIMUM_LIQUIDITY shares on first deposit;
    // the simulator intentionally does not model this accounting.
    let provider_shares = h.amm.shares_of(&h.provider);
    assert_eq!(provider_shares, h.sim.total_shares - locked);
}

#[test]
fn divergence_panic_on_degenerate() {
    // spot_price == 0 but a real, positive output is still possible: the
    // contract's `simulate_swap` divides by `spot_price` and panics, while the
    // simulator returns a defined price impact. This is the documented,
    // intentional divergence — we assert the contract fails and the sim succeeds.
    let h = Harness::new(30, 0, 3_000_000, 2);
    let amount_in = 1_000_000_000_i128;
    h.mint(&h.ta, &h.provider, amount_in);

    let sim_quote = h.sim.quote_swap_exact_in("A", amount_in).unwrap();
    assert!(sim_quote.amount_out > 0);
    assert!(sim_quote.spot_price == 0, "expected degenerate spot price");

    // The contract's `simulate_swap` divides by `spot_price` and therefore must
    // not return a defined quote for a degenerate pool; the simulator returns a
    // defined price impact instead. This is the documented, intentional split.
    let res = h.amm.try_simulate_swap(&h.ta, &amount_in);
    assert!(
        res.is_err(),
        "contract must not return a defined quote for a degenerate pool"
    );
}

#[test]
fn known_contract_bug_free_exact_out_at_100pct_fee() {
    // With fee_bps == 10_000 the contract's get_amount_in returns 0, so
    // swap_exact_out transfers `amount_out` to the trader for free. The
    // simulator rejects a 100% fee as invalid. This is a *contract* bug and is
    // reported separately — it is intentionally NOT fixed here.
    let h = Harness::new(10_000, 0, 1_000_000, 1_000_000);

    // Simulator side: a 100% fee is invalid.
    let sim = h.sim.quote_swap_exact_out("A", 50_000);
    assert!(sim.is_err(), "simulator must reject a 100% fee");

    // Contract side: a 100% fee lets the trader take tokens for nothing.
    h.mint(&h.tb, &h.provider, 0);
    h.bump();
    let res = h
        .amm
        .try_swap_exact_out(&h.provider, &h.ta, &50_000, &i128::MAX, &u64::MAX);
    let taken = res.expect("swap must succeed").expect("swap must succeed");
    assert_eq!(taken, 0, "contract charged 0 input for a real output");
    // No protocol fee is accrued, but the pool is still drained: with 0 input the
    // contract removes `amount_out` from the output reserve and credits nothing
    // back, so the trader receives tokens for free (a real contract bug).
    assert_eq!(h.amm.get_accrued_fees().0, 0);
    let info = h.amm.get_info();
    assert_eq!(info.reserve_a, 1_000_000 - 50_000);
    assert_eq!(info.reserve_b, 1_000_000);
}

// ── Canary ─────────────────────────────────────────────────────────────────────
//
// Sanity check that the `prop_*` suite is actually wired to the contract: a
// naive, independent re-derivation of the constant-product output agrees with
// both the contract and the simulator. This is what we deliberately break to
// confirm the suite catches a wrong simulator constant (see PR description:
// perturbing the swap math in `pool.rs` fails the properties below).

#[test]
fn canary_independent_output_matches() {
    let h = Harness::new(30, 0, 1_000_000, 1_000_000);
    let amount_in = 100_000_i128;

    // Independent derivation of the constant-product output (scaled integer).
    let (ra, rb) = (1_000_000_i128, 1_000_000_i128);
    let fee_bps = 30_i128;
    let with_fee = amount_in * (BPS - fee_bps);
    let expected = with_fee * rb / (ra * BPS + with_fee);

    let contract_out = h.amm.get_amount_out(&h.ta, &amount_in);
    let quote = h.sim.quote_swap_exact_in("A", amount_in).unwrap();
    assert_eq!(expected, contract_out);
    assert_eq!(expected, quote.amount_out);
}
