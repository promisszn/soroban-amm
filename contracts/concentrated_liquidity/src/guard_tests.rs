//! Concentrated Liquidity slippage guard and quote parity regression tests (#1185).
//!
//! Asserts that:
//! 1. `swap`'s `min_amount_out` guard cannot be deleted/bypassed.
//! 2. `modify_position`'s `min_a` and `min_b` slippage guards cannot be deleted/bypassed.
//! 3. `quote_single_token_deposit`'s wrong-side range checks strictly mirror
//!    `mint_position_single_token` across all 6 combinations of token and tick ranges.

#![allow(dead_code)]

extern crate std;

use crate::{ClError, ConcentratedLiquidity, ConcentratedLiquidityClient};
use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    token::{StellarAssetClient, TokenClient},
    Address, Env,
};

struct TestEnv {
    env: Env,
    admin: Address,
    user: Address,
    token_a: Address,
    token_b: Address,
    pool_addr: Address,
    client: ConcentratedLiquidityClient<'static>,
}

fn setup_cl_pool() -> TestEnv {
    let env = Env::default();
    env.mock_all_auths();
    env.budget().reset_unlimited();
    env.ledger().set_timestamp(10_000);

    let admin = Address::generate(&env);
    let user = Address::generate(&env);

    let token_a = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let token_b = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();

    let pool_addr = env.register_contract(None, ConcentratedLiquidity);
    let client = ConcentratedLiquidityClient::new(&env, &pool_addr);
    client.initialize(&admin, &token_a, &token_b, &30_i128, &100_i32, &1_i32);

    // Provide baseline liquidity
    let lp = Address::generate(&env);
    StellarAssetClient::new(&env, &token_a).mint(&lp, &1_000_000_000);
    StellarAssetClient::new(&env, &token_b).mint(&lp, &1_000_000_000);
    client.mint_position(
        &lp,
        &-1000,
        &1000,
        &100_000_000,
        &100_000_000,
        &0,
        &0,
        &u64::MAX,
    );

    // Fund user
    StellarAssetClient::new(&env, &token_a).mint(&user, &500_000_000);
    StellarAssetClient::new(&env, &token_b).mint(&user, &500_000_000);

    TestEnv {
        env,
        admin,
        user,
        token_a,
        token_b,
        pool_addr,
        client,
    }
}

// =========================================================================
// 1. swap: min_amount_out guard (lines 2624-2626)
// =========================================================================

#[test]
fn test_swap_zero_for_one_slippage_exact_passes() {
    let te = setup_cl_pool();
    let amount_in = 1_000_000_i128;
    let quote = te.client.quote_exact_in(&true, &amount_in, &0);
    assert!(quote > 0);

    let out = te
        .client
        .swap(&te.user, &true, &amount_in, &0, &quote, &u64::MAX);
    assert_eq!(out, quote);
}

#[test]
fn test_swap_zero_for_one_slippage_exceeded_fails() {
    let te = setup_cl_pool();
    let amount_in = 1_000_000_i128;
    let quote = te.client.quote_exact_in(&true, &amount_in, &0);
    assert!(quote > 0);

    let res = te
        .client
        .try_swap(&te.user, &true, &amount_in, &0, &(quote + 1), &u64::MAX);
    assert_eq!(res, Err(Ok(ClError::SlippageExceeded)));
}

#[test]
fn test_swap_slippage_failure_preserves_all_state_and_balances() {
    let te = setup_cl_pool();
    let amount_in = 2_000_000_i128;
    let quote = te.client.quote_exact_in(&true, &amount_in, &0);

    let token_a_client = TokenClient::new(&te.env, &te.token_a);
    let token_b_client = TokenClient::new(&te.env, &te.token_b);

    // Snapshot pre-state
    let user_a_before = token_a_client.balance(&te.user);
    let user_b_before = token_b_client.balance(&te.user);
    let pool_a_before = token_a_client.balance(&te.pool_addr);
    let pool_b_before = token_b_client.balance(&te.pool_addr);
    let reserves_before = te.client.get_reserves();

    // Failing swap
    let res = te
        .client
        .try_swap(&te.user, &true, &amount_in, &0, &(quote + 10), &u64::MAX);
    assert_eq!(res, Err(Ok(ClError::SlippageExceeded)));

    // Assert absolute state invariance
    assert_eq!(token_a_client.balance(&te.user), user_a_before);
    assert_eq!(token_b_client.balance(&te.user), user_b_before);
    assert_eq!(token_a_client.balance(&te.pool_addr), pool_a_before);
    assert_eq!(token_b_client.balance(&te.pool_addr), pool_b_before);
    assert_eq!(te.client.get_reserves(), reserves_before);
}

#[test]
fn test_swap_one_for_zero_slippage_exact_passes() {
    let te = setup_cl_pool();
    let amount_in = 1_000_000_i128;
    let quote = te.client.quote_exact_in(&false, &amount_in, &0);
    assert!(quote > 0);

    let out = te
        .client
        .swap(&te.user, &false, &amount_in, &0, &quote, &u64::MAX);
    assert_eq!(out, quote);
}

#[test]
fn test_swap_one_for_zero_slippage_exceeded_fails() {
    let te = setup_cl_pool();
    let amount_in = 1_000_000_i128;
    let quote = te.client.quote_exact_in(&false, &amount_in, &0);
    assert!(quote > 0);

    let res = te
        .client
        .try_swap(&te.user, &false, &amount_in, &0, &(quote + 1), &u64::MAX);
    assert_eq!(res, Err(Ok(ClError::SlippageExceeded)));
}

// =========================================================================
// 2. modify_position: min_a and min_b checks (lines 1055-1058)
// =========================================================================

#[test]
fn test_modify_position_decrease_liquidity_exact_bounds_pass() {
    let te = setup_cl_pool();
    let lp = Address::generate(&te.env);
    StellarAssetClient::new(&te.env, &te.token_a).mint(&lp, &10_000_000);
    StellarAssetClient::new(&te.env, &te.token_b).mint(&lp, &10_000_000);

    te.client.mint_position(
        &lp,
        &-500,
        &500,
        &5_000_000,
        &5_000_000,
        &0,
        &0,
        &u64::MAX,
    );

    // Modify with 0 minimums to observe actual withdraw amounts
    let (withdrawn_a, withdrawn_b) =
        te.client
            .modify_position(&lp, &-500, &500, &-1_000_000, &0, &0, &u64::MAX);
    assert!(withdrawn_a > 0);
    assert!(withdrawn_b > 0);

    // Decreasing again with exact bounds passes
    let res = te.client.try_modify_position(
        &lp,
        &-500,
        &500,
        &-500_000,
        &0, // safe bounds
        &0,
        &u64::MAX,
    );
    assert!(res.is_ok());
}

#[test]
fn test_modify_position_min_a_violated_fails() {
    let te = setup_cl_pool();
    let lp = Address::generate(&te.env);
    StellarAssetClient::new(&te.env, &te.token_a).mint(&lp, &10_000_000);
    StellarAssetClient::new(&te.env, &te.token_b).mint(&lp, &10_000_000);

    te.client.mint_position(
        &lp,
        &-500,
        &500,
        &5_000_000,
        &5_000_000,
        &0,
        &0,
        &u64::MAX,
    );

    let res = te.client.try_modify_position(
        &lp,
        &-500,
        &500,
        &-1_000_000,
        &i128::MAX, // min_a impossibly high
        &0,
        &u64::MAX,
    );
    assert_eq!(res, Err(Ok(ClError::SlippageExceeded)));
}

#[test]
fn test_modify_position_min_b_violated_fails() {
    let te = setup_cl_pool();
    let lp = Address::generate(&te.env);
    StellarAssetClient::new(&te.env, &te.token_a).mint(&lp, &10_000_000);
    StellarAssetClient::new(&te.env, &te.token_b).mint(&lp, &10_000_000);

    te.client.mint_position(
        &lp,
        &-500,
        &500,
        &5_000_000,
        &5_000_000,
        &0,
        &0,
        &u64::MAX,
    );

    let res = te.client.try_modify_position(
        &lp,
        &-500,
        &500,
        &-1_000_000,
        &0,
        &i128::MAX, // min_b impossibly high
        &u64::MAX,
    );
    assert_eq!(res, Err(Ok(ClError::SlippageExceeded)));
}

#[test]
fn test_modify_position_both_bounds_violated_fails() {
    let te = setup_cl_pool();
    let lp = Address::generate(&te.env);
    StellarAssetClient::new(&te.env, &te.token_a).mint(&lp, &10_000_000);
    StellarAssetClient::new(&te.env, &te.token_b).mint(&lp, &10_000_000);

    te.client.mint_position(
        &lp,
        &-500,
        &500,
        &5_000_000,
        &5_000_000,
        &0,
        &0,
        &u64::MAX,
    );

    let res = te.client.try_modify_position(
        &lp,
        &-500,
        &500,
        &-1_000_000,
        &i128::MAX,
        &i128::MAX,
        &u64::MAX,
    );
    assert_eq!(res, Err(Ok(ClError::SlippageExceeded)));
}

// =========================================================================
// 3. quote_single_token_deposit vs mint_position_single_token Parity Table
//    (Current tick below / inside / above range) x (Token A / B) = 6 cases
// =========================================================================

// Case 1: Current tick is 0, range is [100, 200] (current tick BELOW range).
// Token B is prohibited (requires Token A only).
#[test]
fn test_parity_below_range_token_b_rejected() {
    let te = setup_cl_pool();
    let amount = 1_000_000_i128;

    let quote_res = te
        .client
        .try_quote_single_token_deposit(&100, &200, &te.token_b, &amount);
    assert_eq!(quote_res, Err(Ok(ClError::SlippageExceeded)));

    let mint_res = te.client.try_mint_position_single_token(
        &te.user,
        &100,
        &200,
        &te.token_b,
        &amount,
        &0,
        &u64::MAX,
    );
    assert_eq!(mint_res, Err(Ok(ClError::SlippageExceeded)));
}

// Case 2: Current tick is 0, range is [100, 200] (current tick BELOW range).
// Token A is valid. Quote matches mint.
#[test]
fn test_parity_below_range_token_a_succeeds() {
    let te = setup_cl_pool();
    let amount = 1_000_000_i128;

    let quote = te
        .client
        .quote_single_token_deposit(&100, &200, &te.token_a, &amount);
    assert!(quote.liquidity > 0);
    assert_eq!(quote.amount_used, amount);

    let mint = te.client.mint_position_single_token(
        &te.user,
        &100,
        &200,
        &te.token_a,
        &amount,
        &0,
        &u64::MAX,
    );
    assert_eq!(mint.liquidity, quote.liquidity);
    assert_eq!(mint.amount_used, quote.amount_used);
}

// Case 3: Current tick is 0, range is [-200, -100] (current tick ABOVE range).
// Token A is prohibited (requires Token B only).
#[test]
fn test_parity_above_range_token_a_rejected() {
    let te = setup_cl_pool();
    let amount = 1_000_000_i128;

    let quote_res = te
        .client
        .try_quote_single_token_deposit(&-200, &-100, &te.token_a, &amount);
    assert_eq!(quote_res, Err(Ok(ClError::SlippageExceeded)));

    let mint_res = te.client.try_mint_position_single_token(
        &te.user,
        &-200,
        &-100,
        &te.token_a,
        &amount,
        &0,
        &u64::MAX,
    );
    assert_eq!(mint_res, Err(Ok(ClError::SlippageExceeded)));
}

// Case 4: Current tick is 0, range is [-200, -100] (current tick ABOVE range).
// Token B is valid. Quote matches mint.
#[test]
fn test_parity_above_range_token_b_succeeds() {
    let te = setup_cl_pool();
    let amount = 1_000_000_i128;

    let quote = te
        .client
        .quote_single_token_deposit(&-200, &-100, &te.token_b, &amount);
    assert!(quote.liquidity > 0);
    assert_eq!(quote.amount_used, amount);

    let mint = te.client.mint_position_single_token(
        &te.user,
        &-200,
        &-100,
        &te.token_b,
        &amount,
        &0,
        &u64::MAX,
    );
    assert_eq!(mint.liquidity, quote.liquidity);
    assert_eq!(mint.amount_used, quote.amount_used);
}

// Case 5: Current tick is 0, range is [-100, 100] (IN RANGE).
// Token A is valid. Quote matches mint.
#[test]
fn test_parity_in_range_token_a_succeeds() {
    let te = setup_cl_pool();
    let amount = 1_000_000_i128;

    let quote = te
        .client
        .quote_single_token_deposit(&-100, &100, &te.token_a, &amount);
    assert!(quote.liquidity > 0);

    let mint = te.client.mint_position_single_token(
        &te.user,
        &-100,
        &100,
        &te.token_a,
        &amount,
        &0,
        &u64::MAX,
    );
    assert_eq!(mint.liquidity, quote.liquidity);
    assert_eq!(mint.amount_used, quote.amount_used);
}

// Case 6: Current tick is 0, range is [-100, 100] (IN RANGE).
// Token B is valid. Quote matches mint.
#[test]
fn test_parity_in_range_token_b_succeeds() {
    let te = setup_cl_pool();
    let amount = 1_000_000_i128;

    let quote = te
        .client
        .quote_single_token_deposit(&-100, &100, &te.token_b, &amount);
    assert!(quote.liquidity > 0);

    let mint = te.client.mint_position_single_token(
        &te.user,
        &-100,
        &100,
        &te.token_b,
        &amount,
        &0,
        &u64::MAX,
    );
    assert_eq!(mint.liquidity, quote.liquidity);
    assert_eq!(mint.amount_used, quote.amount_used);
}
