#![allow(dead_code)]

use crate::{ClError, ConcentratedLiquidity, ConcentratedLiquidityClient};
use soroban_sdk::token::{StellarAssetClient, TokenClient};
use soroban_sdk::{testutils::Address as _, testutils::Ledger as _, Address, Env};

struct ParityEnv {
    env: Env,
    provider: Address,
    token_a: Address,
    token_b: Address,
    pool_addr: Address,
    client: ConcentratedLiquidityClient<'static>,
}

fn setup_parity_pool(current_tick: i32, tick_spacing: i32) -> ParityEnv {
    let env = Env::default();
    env.mock_all_auths();
    env.budget().reset_unlimited();
    env.ledger().set_timestamp(1_000);

    let admin = Address::generate(&env);
    let token_a = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let token_b = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();

    let pool_addr = env.register_contract(None, ConcentratedLiquidity);
    let client = ConcentratedLiquidityClient::new(&env, &pool_addr);
    client.initialize(
        &admin,
        &token_a,
        &token_b,
        &30_i128,
        &current_tick,
        &tick_spacing,
    );

    let provider = Address::generate(&env);
    StellarAssetClient::new(&env, &token_a).mint(&provider, &1_000_000_000_000_i128);
    StellarAssetClient::new(&env, &token_b).mint(&provider, &1_000_000_000_000_i128);

    ParityEnv {
        env,
        provider,
        token_a,
        token_b,
        pool_addr,
        client,
    }
}

/// Issue #1306 (1): quote_single_token_deposit must reject misaligned ticks with
/// TickNotAligned, matching mint_position_single_token.
#[test]
fn quote_single_token_deposit_rejects_misaligned_ticks() {
    let pe = setup_parity_pool(0, 10);

    let quote_lower_misaligned =
        pe.client
            .try_quote_single_token_deposit(&105_i32, &200_i32, &pe.token_a, &10_000_i128);
    let mint_lower_misaligned = pe.client.try_mint_position_single_token(
        &pe.provider,
        &105_i32,
        &200_i32,
        &pe.token_a,
        &10_000_i128,
        &1_i128,
        &u64::MAX,
    );
    assert_eq!(quote_lower_misaligned, Err(Ok(ClError::TickNotAligned)));
    assert_eq!(mint_lower_misaligned, Err(Ok(ClError::TickNotAligned)));

    let quote_upper_misaligned =
        pe.client
            .try_quote_single_token_deposit(&100_i32, &205_i32, &pe.token_a, &10_000_i128);
    assert_eq!(quote_upper_misaligned, Err(Ok(ClError::TickNotAligned)));
}

/// Issue #1306 (2): When current_tick and upper_tick saturate at MAX_SQRT_PRICE
/// while lower_tick is below saturation, token B covers [sqrt_lower, sqrt_current].
/// Quote and mint must return identical (amount_used, dust, liquidity).
#[test]
fn quote_matches_mint_for_token_b_when_upper_and_current_ticks_saturate() {
    // Ticks > 443_636 clamp to MAX_SQRT_PRICE; 440_000 is strictly below saturation.
    let pe = setup_parity_pool(444_000, 10);
    let amount_in = 10_000_000_000_i128;

    let quote =
        pe.client
            .quote_single_token_deposit(&440_000_i32, &445_000_i32, &pe.token_b, &amount_in);
    let minted = pe.client.mint_position_single_token(
        &pe.provider,
        &440_000_i32,
        &445_000_i32,
        &pe.token_b,
        &amount_in,
        &1_i128,
        &u64::MAX,
    );

    assert!(minted.liquidity > 1);
    assert!(minted.amount_used > 0);
    assert_eq!(quote.amount_used, minted.amount_used);
    assert_eq!(quote.dust, minted.dust);
    assert_eq!(quote.liquidity, minted.liquidity);
}

/// Issue #1306 (3): At current_tick == lower_tick, sqrt_current == sqrt_lower.
/// Depositing token A covers [sqrt_current, sqrt_upper], so quote must match mint
/// instead of falling into a degenerate token-B zero-width branch.
#[test]
fn quote_matches_mint_for_token_a_at_lower_tick_boundary() {
    let pe = setup_parity_pool(100, 10);
    let amount_in = 50_000_i128;

    let quote = pe
        .client
        .quote_single_token_deposit(&100_i32, &200_i32, &pe.token_a, &amount_in);
    let minted = pe.client.mint_position_single_token(
        &pe.provider,
        &100_i32,
        &200_i32,
        &pe.token_a,
        &amount_in,
        &1_i128,
        &u64::MAX,
    );

    assert!(minted.liquidity > 1);
    assert!(minted.amount_used > 0);
    assert_eq!(quote.amount_used, minted.amount_used);
    assert_eq!(quote.dust, minted.dust);
    assert_eq!(quote.liquidity, minted.liquidity);
}

/// Issue #1306 (4): In a fully saturated range where sqrt_lower == sqrt_current == sqrt_upper
/// (and at current_tick == lower_tick when supplying token B), zero tokens would be consumed.
/// Both mint and quote must revert with ZeroLiquidity rather than minting 1 free liquidity unit.
#[test]
fn saturated_and_zero_used_deposits_revert_with_zero_liquidity_in_both_quote_and_mint() {
    let pe = setup_parity_pool(444_500, 10);
    let tok_a = TokenClient::new(&pe.env, &pe.token_a);
    let bal_before = tok_a.balance(&pe.provider);

    // Token A in [444_000, 445_000] at current=444_500: sqrt_current == sqrt_upper == MAX_SQRT_PRICE
    let quote_a = pe.client.try_quote_single_token_deposit(
        &444_000_i32,
        &445_000_i32,
        &pe.token_a,
        &10_000_i128,
    );
    let mint_a = pe.client.try_mint_position_single_token(
        &pe.provider,
        &444_000_i32,
        &445_000_i32,
        &pe.token_a,
        &10_000_i128,
        &1_i128,
        &u64::MAX,
    );
    assert_eq!(quote_a, Err(Ok(ClError::ZeroLiquidity)));
    assert_eq!(mint_a, Err(Ok(ClError::ZeroLiquidity)));
    assert_eq!(tok_a.balance(&pe.provider), bal_before);
    assert_eq!(pe.client.active_liquidity(), 0);

    // Token B in [444_000, 445_000] at current=444_500: sqrt_lower == sqrt_current == MAX_SQRT_PRICE
    let quote_b = pe.client.try_quote_single_token_deposit(
        &444_000_i32,
        &445_000_i32,
        &pe.token_b,
        &10_000_i128,
    );
    let mint_b = pe.client.try_mint_position_single_token(
        &pe.provider,
        &444_000_i32,
        &445_000_i32,
        &pe.token_b,
        &10_000_i128,
        &1_i128,
        &u64::MAX,
    );
    assert_eq!(quote_b, Err(Ok(ClError::ZeroLiquidity)));
    assert_eq!(mint_b, Err(Ok(ClError::ZeroLiquidity)));

    // Token B at current_tick == lower_tick (non-saturated) also has zero active width [lower, current]
    let pe_boundary = setup_parity_pool(100, 10);
    let quote_boundary_b = pe_boundary.client.try_quote_single_token_deposit(
        &100_i32,
        &200_i32,
        &pe_boundary.token_b,
        &10_000_i128,
    );
    let mint_boundary_b = pe_boundary.client.try_mint_position_single_token(
        &pe_boundary.provider,
        &100_i32,
        &200_i32,
        &pe_boundary.token_b,
        &10_000_i128,
        &1_i128,
        &u64::MAX,
    );
    assert_eq!(quote_boundary_b, Err(Ok(ClError::ZeroLiquidity)));
    assert_eq!(mint_boundary_b, Err(Ok(ClError::ZeroLiquidity)));
}
