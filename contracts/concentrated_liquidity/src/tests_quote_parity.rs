//! Parity and edge-case tests for `quote_single_token_deposit` vs
//! `mint_position_single_token` (tick alignment, saturated-tick degenerate
//! branches, and zero-token deposit rejection).

use soroban_sdk::testutils::Address as _;
use soroban_sdk::token::StellarAssetClient;
use soroban_sdk::{Address, Env};

use crate::{ClError, ConcentratedLiquidity, ConcentratedLiquidityClient};

fn setup_pool(
    env: &Env,
    initial_tick: i32,
    tick_spacing: i32,
) -> (Address, Address, Address, ConcentratedLiquidityClient<'_>) {
    env.mock_all_auths();
    env.budget().reset_unlimited();

    let admin = Address::generate(env);
    let provider = Address::generate(env);

    let token_a = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let token_b = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();

    StellarAssetClient::new(env, &token_a).mint(&provider, &10_000_000_000_i128);
    StellarAssetClient::new(env, &token_b).mint(&provider, &10_000_000_000_i128);

    let contract_id = env.register_contract(None, ConcentratedLiquidity);
    let client = ConcentratedLiquidityClient::new(env, &contract_id);
    client.initialize(
        &admin,
        &token_a,
        &token_b,
        &30,
        &initial_tick,
        &tick_spacing,
    );

    (provider, token_a, token_b, client)
}

#[test]
fn quote_single_token_deposit_rejects_misaligned_ticks() {
    let env = Env::default();
    let (provider, token_a, _token_b, client) = setup_pool(&env, 0, 10);

    // Lower tick misaligned
    assert_eq!(
        client.try_quote_single_token_deposit(&-15_i32, &10_i32, &token_a, &10_000_i128),
        Err(Ok(ClError::TickNotAligned))
    );
    assert_eq!(
        client.try_mint_position_single_token(
            &provider,
            &-15_i32,
            &10_i32,
            &token_a,
            &10_000_i128,
            &1_i128,
            &u64::MAX,
        ),
        Err(Ok(ClError::TickNotAligned))
    );

    // Upper tick misaligned
    assert_eq!(
        client.try_quote_single_token_deposit(&-10_i32, &15_i32, &token_a, &10_000_i128),
        Err(Ok(ClError::TickNotAligned))
    );
    assert_eq!(
        client.try_mint_position_single_token(
            &provider,
            &-10_i32,
            &15_i32,
            &token_a,
            &10_000_i128,
            &1_i128,
            &u64::MAX,
        ),
        Err(Ok(ClError::TickNotAligned))
    );
}

#[test]
fn quote_matches_mint_in_saturated_degenerate_range() {
    let env = Env::default();
    // Both 887_200 and 887_260 clamp to MAX_SQRT_PRICE, so sqrt_current == sqrt_upper
    // while lower_tick = 0 has sqrt_lower < sqrt_current.
    let (provider, _token_a, token_b, client) = setup_pool(&env, 887_200, 10);

    let lower_tick = 0_i32;
    let upper_tick = 887_260_i32;
    let amount_in = 10_000_i128;

    let quote = client.quote_single_token_deposit(&lower_tick, &upper_tick, &token_b, &amount_in);
    let mint = client.mint_position_single_token(
        &provider,
        &lower_tick,
        &upper_tick,
        &token_b,
        &amount_in,
        &1_i128,
        &u64::MAX,
    );

    assert_eq!(quote.amount_used, mint.amount_used);
    assert_eq!(quote.dust, mint.dust);
    assert_eq!(quote.liquidity, mint.liquidity);
    assert!(mint.amount_used > 0);
}

#[test]
fn saturated_zero_token_deposit_reverts_with_zero_liquidity() {
    let env = Env::default();
    // All three ticks clamp to MAX_SQRT_PRICE (sqrt_lower == sqrt_current == sqrt_upper),
    // so both token A and token B deltas evaluate to 0.
    let (provider, token_a, token_b, client) = setup_pool(&env, 887_200, 10);

    let lower_tick = 887_190_i32;
    let upper_tick = 887_260_i32;
    let amount_in = 10_000_i128;

    for token_in in [&token_a, &token_b] {
        assert_eq!(
            client.try_quote_single_token_deposit(&lower_tick, &upper_tick, token_in, &amount_in),
            Err(Ok(ClError::ZeroLiquidity))
        );
        assert_eq!(
            client.try_mint_position_single_token(
                &provider,
                &lower_tick,
                &upper_tick,
                token_in,
                &amount_in,
                &1_i128,
                &u64::MAX,
            ),
            Err(Ok(ClError::ZeroLiquidity))
        );
    }
}
