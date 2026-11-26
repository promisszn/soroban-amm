//! Cross-contract factory concentrated-liquidity pool deployment tests.
//!
//! Verifies that `factory::create_cl_pool` deploys and initializes a usable
//! concentrated-liquidity pool capable of minting positions and executing swaps,
//! and enforces preconditions such as `set_cl_wasm_hash` and duplicate checks.

use crate::fixture::Protocol;
use concentrated_liquidity::{ConcentratedLiquidityClient, WASM as CL_WASM};
use factory::{FactoryClient, FactoryError};
use soroban_sdk::testutils::Address as _;
use soroban_sdk::token::{Client as TokenClient, StellarAssetClient};
use soroban_sdk::{Address, BytesN};

const DEADLINE: u64 = u64::MAX;

#[test]
fn create_cl_pool_before_set_cl_wasm_hash_returns_cl_wasm_not_set() {
    let protocol = Protocol::deploy();
    let factory_client = FactoryClient::new(&protocol.env, &protocol.factory);

    let result = factory_client.try_create_cl_pool(
        &protocol.admin,
        &protocol.token_a,
        &protocol.token_b,
        &30_i128,
        &0_i32,
    );
    assert_eq!(
        result,
        Err(Ok(FactoryError::ClWasmNotSet)),
        "create_cl_pool before set_cl_wasm_hash must return ClWasmNotSet"
    );
}

#[test]
fn factory_deployed_cl_pool_supports_mint_and_swap() {
    let protocol = Protocol::deploy();
    let factory_client = FactoryClient::new(&protocol.env, &protocol.factory);

    // Upload CL WASM and configure it on the factory.
    let cl_hash: BytesN<32> = protocol.env.deployer().upload_contract_wasm(CL_WASM);
    factory_client.set_cl_wasm_hash(&cl_hash);

    // Deploy a 30 bps CL pool at initial tick 0 via the factory.
    let cl_pool_addr = factory_client.create_cl_pool(
        &protocol.admin,
        &protocol.token_a,
        &protocol.token_b,
        &30_i128,
        &0_i32,
    );

    // Lookup must resolve in both token orders.
    assert_eq!(
        factory_client.get_cl_pool(&protocol.token_a, &protocol.token_b, &30_i128),
        Some(cl_pool_addr.clone())
    );
    assert_eq!(
        factory_client.get_cl_pool(&protocol.token_b, &protocol.token_a, &30_i128),
        Some(cl_pool_addr.clone())
    );

    // Factory normalises token order (smaller address is token_0).
    let (token_0, token_1) = if protocol.token_a < protocol.token_b {
        (protocol.token_a.clone(), protocol.token_b.clone())
    } else {
        (protocol.token_b.clone(), protocol.token_a.clone())
    };

    let cl_client = ConcentratedLiquidityClient::new(&protocol.env, &cl_pool_addr);

    // Fund provider and mint a concentrated position straddling tick 0.
    let provider = Address::generate(&protocol.env);
    StellarAssetClient::new(&protocol.env, &token_0).mint(&provider, &100_000_i128);
    StellarAssetClient::new(&protocol.env, &token_1).mint(&provider, &100_000_i128);

    let (dep_0, dep_1) = cl_client.mint_position(
        &provider,
        &-100_i32,
        &100_i32,
        &10_000_i128,
        &10_000_i128,
        &0_i128,
        &0_i128,
        &DEADLINE,
    );
    assert!(
        dep_0 > 0 && dep_1 > 0,
        "minting a position straddling tick 0 should deposit positive amounts of both tokens"
    );
    assert!(
        cl_client.active_liquidity() > 0,
        "active liquidity should be positive after mint"
    );

    // Execute a zero_for_one swap (token_0 -> token_1) through the factory-deployed pool.
    let trader = Address::generate(&protocol.env);
    StellarAssetClient::new(&protocol.env, &token_0).mint(&trader, &1_000_i128);

    let amount_out = cl_client.swap(&trader, &true, &1_000_i128, &0_u128, &1_i128, &DEADLINE);
    assert!(
        amount_out > 0,
        "swap should return a positive output amount"
    );
    assert_eq!(
        TokenClient::new(&protocol.env, &token_1).balance(&trader),
        amount_out,
        "trader should receive the swapped token_1 output"
    );
}

#[test]
fn duplicate_cl_pool_same_fee_tier_fails_but_different_fee_tier_succeeds() {
    let protocol = Protocol::deploy();
    let factory_client = FactoryClient::new(&protocol.env, &protocol.factory);

    let cl_hash: BytesN<32> = protocol.env.deployer().upload_contract_wasm(CL_WASM);
    factory_client.set_cl_wasm_hash(&cl_hash);

    let pool_30 = factory_client.create_cl_pool(
        &protocol.admin,
        &protocol.token_a,
        &protocol.token_b,
        &30_i128,
        &0_i32,
    );

    // Same pair + same fee tier (even with reversed token order) must fail.
    let dup = factory_client.try_create_cl_pool(
        &protocol.admin,
        &protocol.token_b,
        &protocol.token_a,
        &30_i128,
        &0_i32,
    );
    assert_eq!(
        dup,
        Err(Ok(FactoryError::ClPoolAlreadyExists)),
        "duplicate CL pool for same pair and fee tier must fail"
    );

    // Same pair at a different fee tier (100 bps) must succeed.
    let pool_100 = factory_client.create_cl_pool(
        &protocol.admin,
        &protocol.token_a,
        &protocol.token_b,
        &100_i128,
        &0_i32,
    );
    assert_ne!(
        pool_30, pool_100,
        "distinct fee tiers should deploy distinct CL pools"
    );
}

#[test]
fn fixture_cl_pool_is_initialized_and_usable() {
    let protocol = Protocol::deploy();
    let cl_client = ConcentratedLiquidityClient::new(&protocol.env, &protocol.cl_pool);

    let provider = Address::generate(&protocol.env);
    StellarAssetClient::new(&protocol.env, &protocol.token_a).mint(&provider, &100_000_i128);
    StellarAssetClient::new(&protocol.env, &protocol.token_b).mint(&provider, &100_000_i128);

    let (dep_a, dep_b) = cl_client.mint_position(
        &provider,
        &-100_i32,
        &100_i32,
        &10_000_i128,
        &10_000_i128,
        &0_i128,
        &0_i128,
        &DEADLINE,
    );
    assert!(
        dep_a > 0 && dep_b > 0,
        "fixture CL pool should be initialized and accept position mints"
    );
    assert!(
        cl_client.active_liquidity() > 0,
        "fixture CL pool should have positive active liquidity after mint"
    );
}
