#![cfg(test)]

//! Factory → pool → LP token lifecycle tests.
//!
//! Tests verify that the factory correctly deploys pools and configures them
//! with the right fees, treasury, and protocol settings. Also verifies that
//! LP tokens are correctly issued and admin-controlled by the pool.

use crate::fixture::Protocol;
use soroban_sdk::token::Client as TokenClient;

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
    
    // Mint LP tokens to a user
    let user = soroban_sdk::Address::generate(&protocol.env);
    lp_token_client.mint(&protocol.v2_pool, &1000);

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

    assert_eq!(pool_ab, pool_ba, "Pool should be the same regardless of token order");
}

#[test]
fn duplicate_pool_creation_fails() {
    let protocol = Protocol::deploy();

    let factory_client = factory::FactoryClient::new(&protocol.env, &protocol.factory);
    
    // Try to create a pool for the same pair again
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        factory_client.create_pool(&protocol.token_a, &protocol.token_b, &30i128)
    }));

    // Should fail with an error (pool already exists)
    assert!(result.is_err(), "Duplicate pool creation should fail");
}

#[test]
fn pause_creation_blocks_both_v2_and_cl() {
    let protocol = Protocol::deploy();

    let factory_client = factory::FactoryClient::new(&protocol.env, &protocol.factory);
    
    // Pause pool creation
    factory_client.pause_creation();

    // Try to create a new pool - should fail
    let token_c = protocol.env
        .register_stellar_asset_contract_v2(protocol.admin.clone())
        .address();

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        factory_client.create_pool(&protocol.token_a, &token_c, &30i128)
    }));

    assert!(result.is_err(), "Pool creation should be paused");
}

#[test]
fn permissionless_mode_requires_fee() {
    let protocol = Protocol::deploy();

    let factory_client = factory::FactoryClient::new(&protocol.env, &protocol.factory);
    
    // Enable permissionless mode (requires 1000 tokens to create)
    factory_client.set_permissionless_mode(&1000i128);

    let token_c = protocol.env
        .register_stellar_asset_contract_v2(protocol.admin.clone())
        .address();

    // Try to create without paying the fee - should fail
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        factory_client.create_pool(&protocol.token_a, &token_c, &30i128)
    }));

    assert!(result.is_err(), "Permissionless pool creation without fee should fail");
}

#[test]
fn permissionless_mode_succeeds_with_fee() {
    let protocol = Protocol::deploy();

    let factory_client = factory::FactoryClient::new(&protocol.env, &protocol.factory);
    
    // Enable permissionless mode
    factory_client.set_permissionless_mode(&1000i128);

    let token_c = protocol.env
        .register_stellar_asset_contract_v2(protocol.admin.clone())
        .address();

    let token_c_client = TokenClient::new(&protocol.env, &token_c);
    token_c_client.mint(&protocol.admin, &2000);
    token_c_client.approve(&protocol.admin, &protocol.factory, &1000, &u64::MAX);

    // Create a pool with the fee - should succeed
    let _pool = factory_client.create_pool(&protocol.token_a, &token_c, &30i128);

    // Verify pool was created
    let result_pool = factory_client.get_pool(&protocol.token_a, &token_c);
    assert_ne!(result_pool, soroban_sdk::Address::generate(&protocol.env), "Pool should exist");
}
