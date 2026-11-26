//! Cross-contract factory treasury & protocol-fee sweep integration tests.
//!
//! Verifies the end-to-end protocol fee flow across the factory, V2 AMM pool,
//! and SEP-41 token contracts:
//! `set_treasury` → swap accrues protocol fees → `sweep_fees` forwards them
//! to the treasury and zeroes the pool's accrued fee balance.

use crate::fixture::Protocol;
use amm::AmmPoolClient;
use factory::{FactoryClient, FactoryError};
use soroban_sdk::testutils::Address as _;
use soroban_sdk::token::{Client as TokenClient, StellarAssetClient};
use soroban_sdk::Address;

const DEADLINE: u64 = u64::MAX;

#[test]
fn treasury_sweep_forwards_accrued_swap_fees_to_treasury() {
    let protocol = Protocol::deploy();
    let factory_client = FactoryClient::new(&protocol.env, &protocol.factory);
    let amm_client = AmmPoolClient::new(&protocol.env, &protocol.v2_pool);

    // Configure treasury with a 10 bps global protocol fee (synced to existing pools).
    let treasury = Address::generate(&protocol.env);
    factory_client.set_treasury(&protocol.admin, &treasury, &10_i128);
    assert_eq!(
        factory_client.get_treasury(),
        Some((treasury.clone(), 10_i128)),
        "treasury and global protocol fee should be configured"
    );

    // Seed the V2 pool with liquidity.
    let provider = Address::generate(&protocol.env);
    StellarAssetClient::new(&protocol.env, &protocol.token_a).mint(&provider, &1_000_000_i128);
    StellarAssetClient::new(&protocol.env, &protocol.token_b).mint(&provider, &1_000_000_i128);
    amm_client.add_liquidity(&provider, &500_000_i128, &500_000_i128, &1_i128, &DEADLINE);

    // Execute a swap so protocol fees accrue in token_a.
    let trader = Address::generate(&protocol.env);
    StellarAssetClient::new(&protocol.env, &protocol.token_a).mint(&trader, &100_000_i128);
    let amount_out = amm_client.swap(
        &trader,
        &protocol.token_a,
        &100_000_i128,
        &1_i128,
        &DEADLINE,
    );
    assert!(amount_out > 0, "swap should return positive output");

    let (accrued_a, accrued_b) = amm_client.get_accrued_fees();
    assert!(
        accrued_a > 0,
        "protocol fees should accrue in token_a after swap"
    );
    assert_eq!(accrued_b, 0, "no protocol fees should accrue in token_b");

    // Sweep accrued fees to the treasury and verify exact balance delta.
    let token_a_client = TokenClient::new(&protocol.env, &protocol.token_a);
    let treasury_before = token_a_client.balance(&treasury);

    let swept = factory_client.sweep_fees(&protocol.token_a);
    assert_eq!(
        swept, accrued_a,
        "swept amount should equal the pool's accrued fees"
    );

    let treasury_after = token_a_client.balance(&treasury);
    assert_eq!(
        treasury_after - treasury_before,
        accrued_a,
        "treasury token_a balance delta should equal accrued fees before sweep"
    );

    // Pool accrued fees must be zero after the sweep.
    assert_eq!(
        amm_client.get_accrued_fees(),
        (0_i128, 0_i128),
        "pool accrued fees should be zero after sweep"
    );
}

#[test]
fn sweep_fees_before_set_treasury_returns_fee_not_configured() {
    let protocol = Protocol::deploy();
    let factory_client = FactoryClient::new(&protocol.env, &protocol.factory);

    let result = factory_client.try_sweep_fees(&protocol.token_a);
    assert_eq!(
        result,
        Err(Ok(FactoryError::FeeNotConfigured)),
        "sweep_fees before set_treasury must return FeeNotConfigured"
    );
}

#[test]
fn treasury_sweep_collects_fees_for_both_tokens_after_bidirectional_swaps() {
    let protocol = Protocol::deploy();
    let factory_client = FactoryClient::new(&protocol.env, &protocol.factory);
    let amm_client = AmmPoolClient::new(&protocol.env, &protocol.v2_pool);

    let treasury = Address::generate(&protocol.env);
    factory_client.set_treasury(&protocol.admin, &treasury, &10_i128);

    // Seed the V2 pool.
    let provider = Address::generate(&protocol.env);
    StellarAssetClient::new(&protocol.env, &protocol.token_a).mint(&provider, &1_000_000_i128);
    StellarAssetClient::new(&protocol.env, &protocol.token_b).mint(&provider, &1_000_000_i128);
    amm_client.add_liquidity(&provider, &500_000_i128, &500_000_i128, &1_i128, &DEADLINE);

    // Swap in both directions so fees accrue in both tokens.
    let trader = Address::generate(&protocol.env);
    StellarAssetClient::new(&protocol.env, &protocol.token_a).mint(&trader, &50_000_i128);
    StellarAssetClient::new(&protocol.env, &protocol.token_b).mint(&trader, &50_000_i128);
    amm_client.swap(&trader, &protocol.token_a, &50_000_i128, &1_i128, &DEADLINE);
    amm_client.swap(&trader, &protocol.token_b, &50_000_i128, &1_i128, &DEADLINE);

    let (accrued_a, accrued_b) = amm_client.get_accrued_fees();
    assert!(accrued_a > 0, "token_a protocol fees should be positive");
    assert!(accrued_b > 0, "token_b protocol fees should be positive");

    let token_a_client = TokenClient::new(&protocol.env, &protocol.token_a);
    let token_b_client = TokenClient::new(&protocol.env, &protocol.token_b);
    let treasury_a_before = token_a_client.balance(&treasury);
    let treasury_b_before = token_b_client.balance(&treasury);

    // One sweep forwards both tokens. The return value totals only the
    // requested token, and a second sweep finds nothing left to withdraw.
    let swept_a = factory_client.sweep_fees(&protocol.token_a);
    assert_eq!(swept_a, accrued_a);
    assert_eq!(
        token_a_client.balance(&treasury) - treasury_a_before,
        accrued_a
    );
    assert_eq!(
        token_b_client.balance(&treasury) - treasury_b_before,
        accrued_b
    );
    assert_eq!(amm_client.get_accrued_fees(), (0_i128, 0_i128));
    assert_eq!(factory_client.sweep_fees(&protocol.token_b), 0);
}

#[test]
fn set_global_fee_without_treasury_returns_fee_not_configured() {
    let protocol = Protocol::deploy();
    let factory_client = FactoryClient::new(&protocol.env, &protocol.factory);

    let result = factory_client.try_set_global_fee(&protocol.admin, &10_i128);
    assert_eq!(
        result,
        Err(Ok(FactoryError::FeeNotConfigured)),
        "set_global_fee before set_treasury must return FeeNotConfigured"
    );
}
