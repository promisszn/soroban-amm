#![allow(dead_code)]

use crate::{ClError, ConcentratedLiquidity, ConcentratedLiquidityClient, MIN_TICK};
use soroban_sdk::token::{Client as TokenClient, StellarAssetClient};
use soroban_sdk::{testutils::{Address as _, Ledger}, Address, Env};

struct BoundaryPool {
    env: Env,
    token_a: Address,
    token_b: Address,
    pool: Address,
}

fn setup_min_tick_pool() -> BoundaryPool {
    let env = Env::default();
    env.mock_all_auths();
    env.budget().reset_unlimited();
    env.ledger().set_timestamp(1_000);

    let admin = Address::generate(&env);
    let provider = Address::generate(&env);
    let token_a = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let token_b = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let pool = env.register_contract(None, ConcentratedLiquidity);
    let client = ConcentratedLiquidityClient::new(&env, &pool);

    client.initialize(&admin, &token_a, &token_b, &0, &0, &1);

    let provider_a = 100_000_000_000_000_i128;
    let provider_b = 500_000_000_000_000_i128;
    StellarAssetClient::new(&env, &token_a).mint(&provider, &provider_a);
    StellarAssetClient::new(&env, &token_b).mint(&provider, &provider_b);
    let (deposited_a, deposited_b) = client.mint_position(
        &provider,
        &MIN_TICK,
        &100,
        &provider_a,
        &provider_b,
        &1,
        &1,
        &u64::MAX,
    );
    assert!(deposited_a > 0 && deposited_b > 0);
    assert!(client.get_pool_state().active_liquidity > 0);

    BoundaryPool {
        env,
        token_a,
        token_b,
        pool,
    }
}

fn setup_saturated_tick_pool() -> BoundaryPool {
    const FIRST_SATURATED_TICK: i32 = 443_637;

    let env = Env::default();
    env.mock_all_auths();
    env.budget().reset_unlimited();
    env.ledger().set_timestamp(1_000);

    let admin = Address::generate(&env);
    let provider = Address::generate(&env);
    let token_a = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let token_b = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let pool = env.register_contract(None, ConcentratedLiquidity);
    let client = ConcentratedLiquidityClient::new(&env, &pool);

    client.initialize(
        &admin,
        &token_a,
        &token_b,
        &0,
        &(FIRST_SATURATED_TICK + 1),
        &1,
    );

    // The first position supplies active liquidity above the saturated
    // boundary. The second initializes a tick inside the flat-price region.
    let provider_b = 200_000_000_000_000_i128;
    StellarAssetClient::new(&env, &token_b).mint(&provider, &provider_b);
    client.mint_position(
        &provider,
        &443_636,
        &443_700,
        &0,
        &(provider_b / 2),
        &0,
        &0,
        &u64::MAX,
    );
    client.mint_position(
        &provider,
        &443_636,
        &FIRST_SATURATED_TICK,
        &0,
        &(provider_b / 2),
        &0,
        &0,
        &u64::MAX,
    );
    assert!(client.get_pool_state().active_liquidity > 0);

    BoundaryPool {
        env,
        token_a,
        token_b,
        pool,
    }
}

#[test]
fn zero_for_one_swap_reaches_min_tick_and_refunds_unspent_input() {
    let pool = setup_min_tick_pool();
    let trader = Address::generate(&pool.env);
    let client = ConcentratedLiquidityClient::new(&pool.env, &pool.pool);
    // Keep the contract's amount_remaining * 10_000 fee calculation in range.
    let amount_in = i128::MAX / 10_000;
    StellarAssetClient::new(&pool.env, &pool.token_a).mint(&trader, &amount_in);

    let token_a = TokenClient::new(&pool.env, &pool.token_a);
    let balance_before = token_a.balance(&trader);
    let amount_out = client.swap(&trader, &true, &amount_in, &0, &1, &u64::MAX);

    let spent = balance_before - token_a.balance(&trader);
    assert!(
        amount_out > 0,
        "the walk should trade before reaching MIN_TICK"
    );
    assert!(
        spent > 0 && spent < amount_in,
        "unspent input must remain with the trader"
    );
    assert!(
        client.get_pool_state().current_tick <= MIN_TICK,
        "the walk should stop at the minimum tick boundary"
    );

    // A subsequent read/simulation must remain usable after the boundary stop.
    let quote = client.estimate_price_impact(&true, &1_000, &0);
    assert_eq!(quote.amount_in, 0);
    assert_eq!(quote.amount_out, 0);
}

#[test]
fn zero_for_one_swap_stops_inside_saturated_high_tick_region() {
    let pool = setup_saturated_tick_pool();
    let trader = Address::generate(&pool.env);
    StellarAssetClient::new(&pool.env, &pool.token_a).mint(&trader, &1_000_000);

    let client = ConcentratedLiquidityClient::new(&pool.env, &pool.pool);
    let token_a = TokenClient::new(&pool.env, &pool.token_a);
    let before = token_a.balance(&trader);
    let amount_out = client.swap(&trader, &true, &1_000_000, &0, &0, &u64::MAX);

    assert_eq!(amount_out, 0);
    assert_eq!(
        token_a.balance(&trader),
        before,
        "zero-progress input must not be charged"
    );
}

#[test]
fn exact_out_swap_reports_unfilled_at_saturated_tick_without_looping() {
    let pool = setup_saturated_tick_pool();
    let trader = Address::generate(&pool.env);
    StellarAssetClient::new(&pool.env, &pool.token_a).mint(&trader, &1_000_000);

    let client = ConcentratedLiquidityClient::new(&pool.env, &pool.pool);
    let token_a = TokenClient::new(&pool.env, &pool.token_a);
    let balance_before = token_a.balance(&trader);
    let result = client.try_swap_exact_out(
        &trader,
        &true,
        &1,
        &0,
        &1_000_000,
        &u64::MAX,
    );
    assert_eq!(result, Err(Ok(ClError::ExactOutNotFullyFilled)));
    assert_eq!(token_a.balance(&trader), balance_before);
}

#[test]
fn estimate_price_impact_returns_quote_at_saturated_tick_without_looping() {
    let pool = setup_saturated_tick_pool();

    let client = ConcentratedLiquidityClient::new(&pool.env, &pool.pool);
    let quote = client.estimate_price_impact(&true, &1_000_000, &0);
    assert_eq!(quote.amount_in, 0);
    assert_eq!(quote.amount_out, 0);
    assert_eq!(quote.sqrt_price_after, quote.sqrt_price_before);
}
