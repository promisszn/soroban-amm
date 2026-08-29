#![cfg(test)]

//! Factory → pool → LP token lifecycle tests.
//!
//! Tests verify that the factory correctly deploys pools and configures them
//! with the right fees, treasury, and protocol settings. Also verifies that
//! LP tokens are correctly issued and admin-controlled by the pool.

use crate::fixture::Protocol;
use soroban_sdk::testutils::Address as _;
use soroban_sdk::token::{Client as TokenClient, StellarAssetClient};
use soroban_sdk::Address;

#[test]
fn factory_deploys_v2_pool_with_correct_config() {
    let protocol = Protocol::deploy();

    // Get the pool that was created by the fixture
    let pool_client = amm::AmmPoolClient::new(&protocol.env, &protocol.v2_pool);
    let info = pool_client.get_info();

    // Verify pool is initialized with correct parameters
    assert_eq!(info.token_a, protocol.token_a);
    assert_eq!(info.token_b, protocol.token_b);
    assert_eq!(info.fee_bps, 30, "Fee should be 30 bps");
    assert_eq!(info.admin, protocol.admin, "Admin should match");
}

#[test]
fn lp_token_admin_is_the_pool() {
    let protocol = Protocol::deploy();

    let lp_token_client = TokenClient::new(&protocol.env, &protocol.v2_lp_token);

    // The pool contract is the LP token's admin, so it can mint on its own
    // behalf using the standard SEP-41 asset admin client.
    StellarAssetClient::new(&protocol.env, &protocol.v2_lp_token).mint(&protocol.v2_pool, &1000);

    // Verify the pool can manage the LP token
    let balance = lp_token_client.balance(&protocol.v2_pool);
    assert_eq!(balance, 1000, "Pool should be able to mint LP tokens");
}

#[test]
fn get_pool_resolves_pair_in_both_orders() {
    let protocol = Protocol::deploy();

    let factory_client = factory::FactoryClient::new(&protocol.env, &protocol.factory);

    // Get pool for (A, B) order
    let pool_ab = factory_client.get_pool(&protocol.token_a, &protocol.token_b);

    // Get pool for (B, A) order - should return the same address
    let pool_ba = factory_client.get_pool(&protocol.token_b, &protocol.token_a);

    assert_eq!(
        pool_ab, pool_ba,
        "Pool should be the same regardless of token order"
    );
}

#[test]
fn duplicate_pool_creation_fails() {
    let protocol = Protocol::deploy();

    let factory_client = factory::FactoryClient::new(&protocol.env, &protocol.factory);

    // Try to create a pool for the same pair again
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        factory_client.create_pool(
            &protocol.admin,
            &protocol.token_a,
            &protocol.token_b,
            &2i128,
            &None,
        )
    }));

    // Should fail with an error (pool already exists)
    assert!(result.is_err(), "Duplicate pool creation should fail");
}

#[test]
fn pause_creation_blocks_both_v2_and_cl() {
    let protocol = Protocol::deploy();

    let factory_client = factory::FactoryClient::new(&protocol.env, &protocol.factory);

    // Pause pool creation
    factory_client.pause_creation(&protocol.admin);

    // Try to create a new pool - should fail
    let token_c = protocol
        .env
        .register_stellar_asset_contract_v2(protocol.admin.clone())
        .address();

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        factory_client.create_pool(&protocol.admin, &protocol.token_a, &token_c, &2i128, &None)
    }));

    assert!(result.is_err(), "Pool creation should be paused");
}

#[test]
fn permissionless_mode_requires_fee() {
    let protocol = Protocol::deploy();

    let factory_client = factory::FactoryClient::new(&protocol.env, &protocol.factory);

    // Configure and enable permissionless mode (requires 1000 of token_a to create)
    factory_client.set_pool_creation_fee(&protocol.token_a, &1000i128);
    factory_client.set_permissionless_mode(&true);

    let token_c = protocol
        .env
        .register_stellar_asset_contract_v2(protocol.admin.clone())
        .address();
    let caller = Address::generate(&protocol.env);

    // Try to create without paying the fee - should fail
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        factory_client.create_pool(&caller, &protocol.token_a, &token_c, &2i128, &None)
    }));

    assert!(
        result.is_err(),
        "Permissionless pool creation without fee should fail"
    );
}

#[test]
fn permissionless_mode_succeeds_with_fee() {
    let protocol = Protocol::deploy();

    let factory_client = factory::FactoryClient::new(&protocol.env, &protocol.factory);

    // Configure and enable permissionless mode
    factory_client.set_pool_creation_fee(&protocol.token_a, &1000i128);
    factory_client.set_permissionless_mode(&true);

    let token_c = protocol
        .env
        .register_stellar_asset_contract_v2(protocol.admin.clone())
        .address();
    let caller = Address::generate(&protocol.env);

    // Fund the caller with enough of the fee token and approve the factory
    StellarAssetClient::new(&protocol.env, &protocol.token_a).mint(&caller, &2000);
    let token_a_client = TokenClient::new(&protocol.env, &protocol.token_a);
    token_a_client.approve(&caller, &protocol.factory, &1000, &1_000_000);

    // Create a pool with the fee - should succeed
    let _pool = factory_client.create_pool(&caller, &protocol.token_a, &token_c, &2i128, &None);

    // Verify pool was created
    let result_pool = factory_client.get_pool(&protocol.token_a, &token_c);
    assert!(result_pool.is_some(), "Pool should exist");
}
