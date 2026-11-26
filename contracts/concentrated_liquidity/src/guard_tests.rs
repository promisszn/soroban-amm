//! Slippage guard and quote-vs-mint parity tests for `ConcentratedLiquidity`.
//!
//! Covers the previously unexercised guards:
//! 1. `swap`: `amount_out_total < min_amount_out` -> `ClError::SlippageExceeded`
//!    (both directions, exact boundary, +1 failure with full state preservation,
//!    sqrt-price-limited early stop, and zero-liquidity-in-range).
//! 2. `modify_position`: `amount_a < min_a || amount_b < min_b` ->
//!    `ClError::SlippageExceeded` (min_a alone, min_b alone, both, exact
//!    equality, state preservation on failure, plus negative delta rejection).
//! 3. `quote_single_token_deposit` vs `mint_position_single_token`: full 3x2
//!    parity table over `(tick below / inside / above range) x (token A / B)`,
//!    explicitly exercising both wrong-side `SlippageExceeded` guards.

use soroban_sdk::testutils::Address as _;
use soroban_sdk::token::{Client as TokenClient, StellarAssetClient};
use soroban_sdk::{Address, Env};

use crate::{ClError, ConcentratedLiquidity, ConcentratedLiquidityClient, DataKey, PoolState};

const FEE_BPS: i128 = 30;
const TICK_SPACING: i32 = 10;
const RANGE_LOWER: i32 = -1_000;
const RANGE_UPPER: i32 = 1_000;
const INITIAL_MINT_AMOUNT: i128 = 10_000_000_000;
const INITIAL_LIQ_DEPOSIT: i128 = 1_000_000;
const SWAP_AMOUNT_IN: i128 = 100_000;
const MODIFY_LIQUIDITY_DELTA: i128 = 50_000;
const SINGLE_TOKEN_AMOUNT: i128 = 50_000;

#[derive(Clone, Debug, PartialEq)]
struct PoolSnapshot {
    reserve_a: i128,
    reserve_b: i128,
    user_a: i128,
    user_b: i128,
    pool_state: PoolState,
    fee_growth_global_a: i128,
    fee_growth_global_b: i128,
    accrued_protocol_fee_a: i128,
    accrued_protocol_fee_b: i128,
}

fn snapshot_pool(
    env: &Env,
    contract_id: &Address,
    client: &ConcentratedLiquidityClient<'_>,
    token_a: &Address,
    token_b: &Address,
    user: &Address,
) -> PoolSnapshot {
    let token_a_client = TokenClient::new(env, token_a);
    let token_b_client = TokenClient::new(env, token_b);
    let (fg_a, fg_b, proto_a, proto_b) = env.as_contract(contract_id, || {
        let inst = env.storage().instance();
        (
            inst.get::<_, i128>(&DataKey::FeeGrowthGlobalA).unwrap_or(0),
            inst.get::<_, i128>(&DataKey::FeeGrowthGlobalB).unwrap_or(0),
            inst.get::<_, i128>(&DataKey::AccruedProtocolFeeA)
                .unwrap_or(0),
            inst.get::<_, i128>(&DataKey::AccruedProtocolFeeB)
                .unwrap_or(0),
        )
    });

    PoolSnapshot {
        reserve_a: token_a_client.balance(contract_id),
        reserve_b: token_b_client.balance(contract_id),
        user_a: token_a_client.balance(user),
        user_b: token_b_client.balance(user),
        pool_state: client.get_pool_state(),
        fee_growth_global_a: fg_a,
        fee_growth_global_b: fg_b,
        accrued_protocol_fee_a: proto_a,
        accrued_protocol_fee_b: proto_b,
    }
}

fn assert_snapshot_unchanged(before: &PoolSnapshot, after: &PoolSnapshot) {
    assert_eq!(before.reserve_a, after.reserve_a, "reserve_a mutated");
    assert_eq!(before.reserve_b, after.reserve_b, "reserve_b mutated");
    assert_eq!(before.user_a, after.user_a, "user token_a balance mutated");
    assert_eq!(before.user_b, after.user_b, "user token_b balance mutated");
    assert_eq!(
        before.pool_state.current_tick, after.pool_state.current_tick,
        "current_tick mutated"
    );
    assert_eq!(
        before.pool_state.sqrt_price, after.pool_state.sqrt_price,
        "sqrt_price mutated"
    );
    assert_eq!(
        before.pool_state.active_liquidity, after.pool_state.active_liquidity,
        "active_liquidity mutated"
    );
    assert_eq!(
        before.pool_state.tick_spacing, after.pool_state.tick_spacing,
        "tick_spacing mutated"
    );
    assert_eq!(
        before.fee_growth_global_a, after.fee_growth_global_a,
        "fee_growth_global_a mutated"
    );
    assert_eq!(
        before.fee_growth_global_b, after.fee_growth_global_b,
        "fee_growth_global_b mutated"
    );
    assert_eq!(
        before.accrued_protocol_fee_a, after.accrued_protocol_fee_a,
        "accrued_protocol_fee_a mutated"
    );
    assert_eq!(
        before.accrued_protocol_fee_b, after.accrued_protocol_fee_b,
        "accrued_protocol_fee_b mutated"
    );
}

struct TestPool<'a> {
    contract_id: Address,
    provider: Address,
    trader: Address,
    token_a: Address,
    token_b: Address,
    client: ConcentratedLiquidityClient<'a>,
}

fn setup_pool(env: &Env, initial_tick: i32, with_liquidity: bool) -> TestPool<'_> {
    env.mock_all_auths();
    env.budget().reset_unlimited();

    let admin = Address::generate(env);
    let provider = Address::generate(env);
    let trader = Address::generate(env);

    let token_a = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let token_b = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();

    let asset_a = StellarAssetClient::new(env, &token_a);
    let asset_b = StellarAssetClient::new(env, &token_b);
    asset_a.mint(&provider, &INITIAL_MINT_AMOUNT);
    asset_b.mint(&provider, &INITIAL_MINT_AMOUNT);
    asset_a.mint(&trader, &INITIAL_MINT_AMOUNT);
    asset_b.mint(&trader, &INITIAL_MINT_AMOUNT);

    let contract_id = env.register_contract(None, ConcentratedLiquidity);
    let client = ConcentratedLiquidityClient::new(env, &contract_id);
    client.initialize(
        &admin,
        &token_a,
        &token_b,
        &FEE_BPS,
        &initial_tick,
        &TICK_SPACING,
    );

    if with_liquidity {
        client.mint_position(
            &provider,
            &RANGE_LOWER,
            &RANGE_UPPER,
            &INITIAL_LIQ_DEPOSIT,
            &INITIAL_LIQ_DEPOSIT,
            &0_i128,
            &0_i128,
            &u64::MAX,
        );
    }

    TestPool {
        contract_id,
        provider,
        trader,
        token_a,
        token_b,
        client,
    }
}

/// Runs an unconstrained swap on a fresh, identically configured pool to derive
/// the exact achievable output without hard-coding swap math constants.
fn reference_swap_output(zero_for_one: bool, amount_in: i128, sqrt_price_limit_x96: u128) -> i128 {
    let env = Env::default();
    let pool = setup_pool(&env, 0, true);
    pool.client.swap(
        &pool.trader,
        &zero_for_one,
        &amount_in,
        &sqrt_price_limit_x96,
        &0_i128,
        &u64::MAX,
    )
}

/// Runs an unconstrained `modify_position` on a fresh, identically configured
/// pool to derive the exact `(amount_a, amount_b)` required for `liquidity_delta`.
fn reference_modify_position_amounts(liquidity_delta: i128) -> (i128, i128) {
    let env = Env::default();
    let pool = setup_pool(&env, 0, true);
    pool.client.modify_position(
        &pool.provider,
        &RANGE_LOWER,
        &RANGE_UPPER,
        &liquidity_delta,
        &0_i128,
        &0_i128,
        &u64::MAX,
    )
}

// ============================================================================
// 1. swap: min_amount_out slippage guard tests
// ============================================================================

#[test]
fn swap_zero_for_one_exact_min_amount_out_succeeds() {
    let expected_out = reference_swap_output(true, SWAP_AMOUNT_IN, 0);
    assert!(expected_out > 0);

    let env = Env::default();
    let pool = setup_pool(&env, 0, true);

    let actual_out = pool.client.swap(
        &pool.trader,
        &true,
        &SWAP_AMOUNT_IN,
        &0_u128,
        &expected_out,
        &u64::MAX,
    );
    assert_eq!(actual_out, expected_out);
}

#[test]
fn swap_zero_for_one_min_amount_out_plus_one_fails_and_preserves_state() {
    let expected_out = reference_swap_output(true, SWAP_AMOUNT_IN, 0);
    assert!(expected_out > 0);

    let env = Env::default();
    let pool = setup_pool(&env, 0, true);
    let before = snapshot_pool(
        &env,
        &pool.contract_id,
        &pool.client,
        &pool.token_a,
        &pool.token_b,
        &pool.trader,
    );

    let res = pool.client.try_swap(
        &pool.trader,
        &true,
        &SWAP_AMOUNT_IN,
        &0_u128,
        &(expected_out + 1),
        &u64::MAX,
    );
    assert_eq!(res, Err(Ok(ClError::SlippageExceeded)));

    let after = snapshot_pool(
        &env,
        &pool.contract_id,
        &pool.client,
        &pool.token_a,
        &pool.token_b,
        &pool.trader,
    );
    assert_snapshot_unchanged(&before, &after);
}

#[test]
fn swap_one_for_zero_exact_min_amount_out_succeeds() {
    let expected_out = reference_swap_output(false, SWAP_AMOUNT_IN, 0);
    assert!(expected_out > 0);

    let env = Env::default();
    let pool = setup_pool(&env, 0, true);

    let actual_out = pool.client.swap(
        &pool.trader,
        &false,
        &SWAP_AMOUNT_IN,
        &0_u128,
        &expected_out,
        &u64::MAX,
    );
    assert_eq!(actual_out, expected_out);
}

#[test]
fn swap_one_for_zero_min_amount_out_plus_one_fails_and_preserves_state() {
    let expected_out = reference_swap_output(false, SWAP_AMOUNT_IN, 0);
    assert!(expected_out > 0);

    let env = Env::default();
    let pool = setup_pool(&env, 0, true);
    let before = snapshot_pool(
        &env,
        &pool.contract_id,
        &pool.client,
        &pool.token_a,
        &pool.token_b,
        &pool.trader,
    );

    let res = pool.client.try_swap(
        &pool.trader,
        &false,
        &SWAP_AMOUNT_IN,
        &0_u128,
        &(expected_out + 1),
        &u64::MAX,
    );
    assert_eq!(res, Err(Ok(ClError::SlippageExceeded)));

    let after = snapshot_pool(
        &env,
        &pool.contract_id,
        &pool.client,
        &pool.token_a,
        &pool.token_b,
        &pool.trader,
    );
    assert_snapshot_unchanged(&before, &after);
}

#[test]
fn swap_zero_for_one_sqrt_price_limit_stops_early_and_enforces_min_out() {
    let limit_x96 = ConcentratedLiquidity::tick_to_sqrt_price_x96(-50);
    let nominal_out = reference_swap_output(true, SWAP_AMOUNT_IN, 0);
    let limited_out = reference_swap_output(true, SWAP_AMOUNT_IN, limit_x96);
    assert!(
        limited_out > 0 && limited_out < nominal_out,
        "expected sqrt-price limit to stop swap early: limited={limited_out}, nominal={nominal_out}"
    );

    let env = Env::default();
    let pool = setup_pool(&env, 0, true);
    let before = snapshot_pool(
        &env,
        &pool.contract_id,
        &pool.client,
        &pool.token_a,
        &pool.token_b,
        &pool.trader,
    );

    // Asking for one unit more than the limited output fails and leaves state untouched.
    let err = pool.client.try_swap(
        &pool.trader,
        &true,
        &SWAP_AMOUNT_IN,
        &limit_x96,
        &(limited_out + 1),
        &u64::MAX,
    );
    assert_eq!(err, Err(Ok(ClError::SlippageExceeded)));

    let after_fail = snapshot_pool(
        &env,
        &pool.contract_id,
        &pool.client,
        &pool.token_a,
        &pool.token_b,
        &pool.trader,
    );
    assert_snapshot_unchanged(&before, &after_fail);

    // Exact limited output succeeds.
    let actual_out = pool.client.swap(
        &pool.trader,
        &true,
        &SWAP_AMOUNT_IN,
        &limit_x96,
        &limited_out,
        &u64::MAX,
    );
    assert_eq!(actual_out, limited_out);
}

#[test]
fn swap_one_for_zero_sqrt_price_limit_stops_early_and_enforces_min_out() {
    let limit_x96 = ConcentratedLiquidity::tick_to_sqrt_price_x96(50);
    let nominal_out = reference_swap_output(false, SWAP_AMOUNT_IN, 0);
    let limited_out = reference_swap_output(false, SWAP_AMOUNT_IN, limit_x96);
    assert!(
        limited_out > 0 && limited_out < nominal_out,
        "expected sqrt-price limit to stop swap early: limited={limited_out}, nominal={nominal_out}"
    );

    let env = Env::default();
    let pool = setup_pool(&env, 0, true);
    let before = snapshot_pool(
        &env,
        &pool.contract_id,
        &pool.client,
        &pool.token_a,
        &pool.token_b,
        &pool.trader,
    );

    let err = pool.client.try_swap(
        &pool.trader,
        &false,
        &SWAP_AMOUNT_IN,
        &limit_x96,
        &(limited_out + 1),
        &u64::MAX,
    );
    assert_eq!(err, Err(Ok(ClError::SlippageExceeded)));

    let after_fail = snapshot_pool(
        &env,
        &pool.contract_id,
        &pool.client,
        &pool.token_a,
        &pool.token_b,
        &pool.trader,
    );
    assert_snapshot_unchanged(&before, &after_fail);

    let actual_out = pool.client.swap(
        &pool.trader,
        &false,
        &SWAP_AMOUNT_IN,
        &limit_x96,
        &limited_out,
        &u64::MAX,
    );
    assert_eq!(actual_out, limited_out);
}

#[test]
fn swap_zero_liquidity_in_range_min_amount_out_one_fails_zero_for_one() {
    let env = Env::default();
    let pool = setup_pool(&env, 0, false);
    assert_eq!(pool.client.get_pool_state().active_liquidity, 0);

    let before = snapshot_pool(
        &env,
        &pool.contract_id,
        &pool.client,
        &pool.token_a,
        &pool.token_b,
        &pool.trader,
    );

    let res = pool.client.try_swap(
        &pool.trader,
        &true,
        &SWAP_AMOUNT_IN,
        &0_u128,
        &1_i128,
        &u64::MAX,
    );
    assert_eq!(res, Err(Ok(ClError::SlippageExceeded)));

    let after = snapshot_pool(
        &env,
        &pool.contract_id,
        &pool.client,
        &pool.token_a,
        &pool.token_b,
        &pool.trader,
    );
    assert_snapshot_unchanged(&before, &after);
}

#[test]
fn swap_zero_liquidity_in_range_min_amount_out_one_fails_one_for_zero() {
    let env = Env::default();
    let pool = setup_pool(&env, 0, false);
    assert_eq!(pool.client.get_pool_state().active_liquidity, 0);

    let before = snapshot_pool(
        &env,
        &pool.contract_id,
        &pool.client,
        &pool.token_a,
        &pool.token_b,
        &pool.trader,
    );

    let res = pool.client.try_swap(
        &pool.trader,
        &false,
        &SWAP_AMOUNT_IN,
        &0_u128,
        &1_i128,
        &u64::MAX,
    );
    assert_eq!(res, Err(Ok(ClError::SlippageExceeded)));

    let after = snapshot_pool(
        &env,
        &pool.contract_id,
        &pool.client,
        &pool.token_a,
        &pool.token_b,
        &pool.trader,
    );
    assert_snapshot_unchanged(&before, &after);
}

// ============================================================================
// 2. modify_position: min_a / min_b slippage guard tests
// ============================================================================

#[test]
fn modify_position_exact_min_a_and_min_b_succeeds() {
    let (expected_a, expected_b) = reference_modify_position_amounts(MODIFY_LIQUIDITY_DELTA);
    assert!(expected_a > 0 && expected_b > 0);

    let env = Env::default();
    let pool = setup_pool(&env, 0, true);

    let (actual_a, actual_b) = pool.client.modify_position(
        &pool.provider,
        &RANGE_LOWER,
        &RANGE_UPPER,
        &MODIFY_LIQUIDITY_DELTA,
        &expected_a,
        &expected_b,
        &u64::MAX,
    );
    assert_eq!((actual_a, actual_b), (expected_a, expected_b));
}

#[test]
fn modify_position_min_a_violated_by_one_fails_and_preserves_state() {
    let (expected_a, expected_b) = reference_modify_position_amounts(MODIFY_LIQUIDITY_DELTA);
    assert!(expected_a > 0 && expected_b > 0);

    let env = Env::default();
    let pool = setup_pool(&env, 0, true);
    let before = snapshot_pool(
        &env,
        &pool.contract_id,
        &pool.client,
        &pool.token_a,
        &pool.token_b,
        &pool.provider,
    );
    let pos_before = pool
        .client
        .get_position(&pool.provider, &RANGE_LOWER, &RANGE_UPPER);

    let res = pool.client.try_modify_position(
        &pool.provider,
        &RANGE_LOWER,
        &RANGE_UPPER,
        &MODIFY_LIQUIDITY_DELTA,
        &(expected_a + 1),
        &expected_b,
        &u64::MAX,
    );
    assert_eq!(res, Err(Ok(ClError::SlippageExceeded)));

    let after = snapshot_pool(
        &env,
        &pool.contract_id,
        &pool.client,
        &pool.token_a,
        &pool.token_b,
        &pool.provider,
    );
    assert_snapshot_unchanged(&before, &after);
    assert_eq!(
        pos_before,
        pool.client
            .get_position(&pool.provider, &RANGE_LOWER, &RANGE_UPPER)
    );
}

#[test]
fn modify_position_min_b_violated_by_one_fails_and_preserves_state() {
    let (expected_a, expected_b) = reference_modify_position_amounts(MODIFY_LIQUIDITY_DELTA);
    assert!(expected_a > 0 && expected_b > 0);

    let env = Env::default();
    let pool = setup_pool(&env, 0, true);
    let before = snapshot_pool(
        &env,
        &pool.contract_id,
        &pool.client,
        &pool.token_a,
        &pool.token_b,
        &pool.provider,
    );
    let pos_before = pool
        .client
        .get_position(&pool.provider, &RANGE_LOWER, &RANGE_UPPER);

    let res = pool.client.try_modify_position(
        &pool.provider,
        &RANGE_LOWER,
        &RANGE_UPPER,
        &MODIFY_LIQUIDITY_DELTA,
        &expected_a,
        &(expected_b + 1),
        &u64::MAX,
    );
    assert_eq!(res, Err(Ok(ClError::SlippageExceeded)));

    let after = snapshot_pool(
        &env,
        &pool.contract_id,
        &pool.client,
        &pool.token_a,
        &pool.token_b,
        &pool.provider,
    );
    assert_snapshot_unchanged(&before, &after);
    assert_eq!(
        pos_before,
        pool.client
            .get_position(&pool.provider, &RANGE_LOWER, &RANGE_UPPER)
    );
}

#[test]
fn modify_position_both_min_a_and_min_b_violated_by_one_fails_and_preserves_state() {
    let (expected_a, expected_b) = reference_modify_position_amounts(MODIFY_LIQUIDITY_DELTA);
    assert!(expected_a > 0 && expected_b > 0);

    let env = Env::default();
    let pool = setup_pool(&env, 0, true);
    let before = snapshot_pool(
        &env,
        &pool.contract_id,
        &pool.client,
        &pool.token_a,
        &pool.token_b,
        &pool.provider,
    );
    let pos_before = pool
        .client
        .get_position(&pool.provider, &RANGE_LOWER, &RANGE_UPPER);

    let res = pool.client.try_modify_position(
        &pool.provider,
        &RANGE_LOWER,
        &RANGE_UPPER,
        &MODIFY_LIQUIDITY_DELTA,
        &(expected_a + 1),
        &(expected_b + 1),
        &u64::MAX,
    );
    assert_eq!(res, Err(Ok(ClError::SlippageExceeded)));

    let after = snapshot_pool(
        &env,
        &pool.contract_id,
        &pool.client,
        &pool.token_a,
        &pool.token_b,
        &pool.provider,
    );
    assert_snapshot_unchanged(&before, &after);
    assert_eq!(
        pos_before,
        pool.client
            .get_position(&pool.provider, &RANGE_LOWER, &RANGE_UPPER)
    );
}

#[test]
fn modify_position_negative_liquidity_delta_rejected_and_preserves_state() {
    let env = Env::default();
    let pool = setup_pool(&env, 0, true);
    let before = snapshot_pool(
        &env,
        &pool.contract_id,
        &pool.client,
        &pool.token_a,
        &pool.token_b,
        &pool.provider,
    );
    let pos_before = pool
        .client
        .get_position(&pool.provider, &RANGE_LOWER, &RANGE_UPPER);

    let res = pool.client.try_modify_position(
        &pool.provider,
        &RANGE_LOWER,
        &RANGE_UPPER,
        &-MODIFY_LIQUIDITY_DELTA,
        &0_i128,
        &0_i128,
        &u64::MAX,
    );
    assert_eq!(res, Err(Ok(ClError::ZeroLiquidity)));

    let after = snapshot_pool(
        &env,
        &pool.contract_id,
        &pool.client,
        &pool.token_a,
        &pool.token_b,
        &pool.provider,
    );
    assert_snapshot_unchanged(&before, &after);
    assert_eq!(
        pos_before,
        pool.client
            .get_position(&pool.provider, &RANGE_LOWER, &RANGE_UPPER)
    );
}

// ============================================================================
// 3. quote_single_token_deposit vs mint_position_single_token parity table
//    (current_tick below / inside / above range) x (token A / token B)
// ============================================================================

fn assert_quote_mint_parity(lower_tick: i32, upper_tick: i32, use_token_a: bool) {
    let env = Env::default();
    let pool = setup_pool(&env, 0, false);
    let token_in = if use_token_a {
        &pool.token_a
    } else {
        &pool.token_b
    };

    let quote_res = pool.client.try_quote_single_token_deposit(
        &lower_tick,
        &upper_tick,
        token_in,
        &SINGLE_TOKEN_AMOUNT,
    );
    let mint_res = pool.client.try_mint_position_single_token(
        &pool.provider,
        &lower_tick,
        &upper_tick,
        token_in,
        &SINGLE_TOKEN_AMOUNT,
        &1_i128,
        &u64::MAX,
    );

    match (quote_res, mint_res) {
        (Ok(Ok(q)), Ok(Ok(m))) => {
            assert_eq!(q.liquidity, m.liquidity, "liquidity mismatch");
            assert_eq!(q.amount_used, m.amount_used, "amount_used mismatch");
            assert_eq!(q.dust, m.dust, "dust mismatch");
            assert!(m.liquidity > 0);
            assert!(m.amount_used > 0);
        }
        (Err(Ok(q_err)), Err(Ok(m_err))) => {
            assert_eq!(q_err, m_err, "error mismatch between quote and mint");
        }
        (q, m) => panic!("quote and mint disagreed: quote={q:?}, mint={m:?}"),
    }
}

#[test]
fn quote_vs_mint_single_token_tick_below_range_token_a_matches() {
    // current_tick = 0 < lower_tick = 100 (position above current tick)
    assert_quote_mint_parity(100, 500, true);
}

#[test]
fn quote_vs_mint_single_token_tick_below_range_token_b_both_fail_slippage() {
    // current_tick = 0 < lower_tick = 100 (position above current tick, wrong-side token B)
    let env = Env::default();
    let pool = setup_pool(&env, 0, false);

    let quote_res = pool.client.try_quote_single_token_deposit(
        &100_i32,
        &500_i32,
        &pool.token_b,
        &SINGLE_TOKEN_AMOUNT,
    );
    let mint_res = pool.client.try_mint_position_single_token(
        &pool.provider,
        &100_i32,
        &500_i32,
        &pool.token_b,
        &SINGLE_TOKEN_AMOUNT,
        &1_i128,
        &u64::MAX,
    );

    assert_eq!(quote_res, Err(Ok(ClError::SlippageExceeded)));
    assert_eq!(mint_res, Err(Ok(ClError::SlippageExceeded)));
}

#[test]
fn quote_vs_mint_single_token_tick_inside_range_token_a_matches() {
    // lower_tick = -200 <= current_tick = 0 < upper_tick = 200
    assert_quote_mint_parity(-200, 200, true);
}

#[test]
fn quote_vs_mint_single_token_tick_inside_range_token_b_matches() {
    // lower_tick = -200 <= current_tick = 0 < upper_tick = 200
    assert_quote_mint_parity(-200, 200, false);
}

#[test]
fn quote_vs_mint_single_token_tick_above_range_token_a_both_fail_slippage() {
    // current_tick = 0 >= upper_tick = -100 (position below current tick, wrong-side token A)
    let env = Env::default();
    let pool = setup_pool(&env, 0, false);

    let quote_res = pool.client.try_quote_single_token_deposit(
        &-500_i32,
        &-100_i32,
        &pool.token_a,
        &SINGLE_TOKEN_AMOUNT,
    );
    let mint_res = pool.client.try_mint_position_single_token(
        &pool.provider,
        &-500_i32,
        &-100_i32,
        &pool.token_a,
        &SINGLE_TOKEN_AMOUNT,
        &1_i128,
        &u64::MAX,
    );

    assert_eq!(quote_res, Err(Ok(ClError::SlippageExceeded)));
    assert_eq!(mint_res, Err(Ok(ClError::SlippageExceeded)));
}

#[test]
fn quote_vs_mint_single_token_tick_above_range_token_b_matches() {
    // current_tick = 0 >= upper_tick = -100 (position below current tick)
    assert_quote_mint_parity(-500, -100, false);
}
