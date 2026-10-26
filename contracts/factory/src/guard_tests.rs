//! Factory Guard Regression Tests (#1190).
//!
//! Asserts that:
//! 1. `initialize` cannot be called a second time (`AlreadyInitialized`).
//! 2. `create_pool_with_fee_bps` fee validation runs BEFORE charging creation fees (`InvalidFeeBps`).
//! 3. `create_cl_pool` fee validation runs BEFORE charging creation fees (`InvalidFeeBps`).
//! 4. `set_pool_creation_fee` rejects zero or negative amounts (`FeeNotConfigured`).
//! 5. `set_global_fee` rejects protocol fee out of bounds `0..=10_000` (`InvalidFeeBps`).
//! 6. `set_global_fee_paginated` rejects invocation when treasury is not set (`FeeNotConfigured`).

extern crate std;

use super::*;
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token::{StellarAssetClient, TokenClient},
    Address, BytesN, Env,
};

struct FactoryFixture {
    env: Env,
    admin: Address,
    creator: Address,
    factory_addr: Address,
    fee_token: Address,
    amm_hash: BytesN<32>,
    token_hash: BytesN<32>,
}

impl FactoryFixture {
    fn new() -> Self {
        let env = Env::default();
        env.budget().reset_unlimited();
        env.mock_all_auths();
        env.ledger().set_timestamp(1_000);

        let admin = Address::generate(&env);
        let creator = Address::generate(&env);

        let amm_hash = env.deployer().upload_contract_wasm(amm::WASM);
        let token_hash = env.deployer().upload_contract_wasm(token::WASM);

        let factory_addr = env.register_contract(None, Factory);
        let factory = FactoryClient::new(&env, &factory_addr);
        factory.initialize(&admin, &amm_hash, &token_hash);

        let fee_token = env.register_stellar_asset_contract_v2(admin.clone()).address();

        // Mint fee tokens to creator
        StellarAssetClient::new(&env, &fee_token).mint(&creator, &1_000_000);

        Self {
            env,
            admin,
            creator,
            factory_addr,
            fee_token,
            amm_hash,
            token_hash,
        }
    }

    fn client(&self) -> FactoryClient<'_> {
        FactoryClient::new(&self.env, &self.factory_addr)
    }
}

// =========================================================================
// 1. initialize: AlreadyInitialized Guard (line 278)
// =========================================================================

#[test]
fn test_reinitialize_fails_and_preserves_admin_and_hashes() {
    let fix = FactoryFixture::new();
    let factory = fix.client();

    let new_admin = Address::generate(&fix.env);
    let dummy_hash: BytesN<32> = BytesN::from_array(&fix.env, &[7u8; 32]);

    let res = factory.try_initialize(&new_admin, &dummy_hash, &dummy_hash);
    assert_eq!(res, Err(Ok(FactoryError::AlreadyInitialized)));

    // State invariant: original admin and wasm hashes preserved
    assert_eq!(factory.get_admin(), fix.admin);
    assert_eq!(factory.get_amm_wasm_hash(), fix.amm_hash);
    assert_eq!(factory.get_token_wasm_hash(), fix.token_hash);
}

// =========================================================================
// 2. create_pool_with_fee_bps: fee_bps bounds & pre-charge validation (#520)
// =========================================================================

#[test]
fn test_create_pool_negative_fee_rejected_without_charging_or_registering() {
    let fix = FactoryFixture::new();
    let factory = fix.client();

    // Configure a creation fee and treasury
    let treasury = Address::generate(&fix.env);
    factory.set_treasury(&treasury, &50);
    factory.set_pool_creation_fee(&fix.fee_token, &5_000);

    let token_client = TokenClient::new(&fix.env, &fix.fee_token);
    let creator_balance_before = token_client.balance(&fix.creator);
    let treasury_balance_before = token_client.balance(&treasury);
    let count_before = factory.get_pool_count();

    let ta = Address::generate(&fix.env);
    let tb = Address::generate(&fix.env);

    let res = factory.try_create_pool_with_fee_bps(&fix.creator, &ta, &tb, &-1, &None);
    assert_eq!(res, Err(Ok(FactoryError::InvalidFeeBps)));

    // Invariants: zero side effects, no fees deducted, no pool registered
    assert_eq!(token_client.balance(&fix.creator), creator_balance_before);
    assert_eq!(token_client.balance(&treasury), treasury_balance_before);
    assert_eq!(factory.get_pool_count(), count_before);
}

#[test]
fn test_create_pool_excess_fee_rejected_without_charging() {
    let fix = FactoryFixture::new();
    let factory = fix.client();

    let treasury = Address::generate(&fix.env);
    factory.set_treasury(&treasury, &50);
    factory.set_pool_creation_fee(&fix.fee_token, &5_000);

    let token_client = TokenClient::new(&fix.env, &fix.fee_token);
    let creator_balance_before = token_client.balance(&fix.creator);

    let ta = Address::generate(&fix.env);
    let tb = Address::generate(&fix.env);

    let res = factory.try_create_pool_with_fee_bps(&fix.creator, &ta, &tb, &10_000, &None);
    assert_eq!(res, Err(Ok(FactoryError::InvalidFeeBps)));
    assert_eq!(token_client.balance(&fix.creator), creator_balance_before);

    let res_overflow = factory.try_create_pool_with_fee_bps(&fix.creator, &ta, &tb, &10_001, &None);
    assert_eq!(res_overflow, Err(Ok(FactoryError::InvalidFeeBps)));
    assert_eq!(token_client.balance(&fix.creator), creator_balance_before);
}

#[test]
fn test_create_pool_boundary_fee_bps_succeeds() {
    let fix = FactoryFixture::new();
    let factory = fix.client();

    let ta1 = Address::generate(&fix.env);
    let tb1 = Address::generate(&fix.env);
    let res0 = factory.try_create_pool_with_fee_bps(&fix.admin, &ta1, &tb1, &0, &None);
    assert!(res0.is_ok());

    let ta2 = Address::generate(&fix.env);
    let tb2 = Address::generate(&fix.env);
    let res9999 = factory.try_create_pool_with_fee_bps(&fix.admin, &ta2, &tb2, &9_999, &None);
    assert!(res9999.is_ok());
}

// =========================================================================
// 3. create_cl_pool: fee_bps bounds & pre-charge validation
// =========================================================================

#[test]
fn test_create_cl_pool_negative_fee_rejected() {
    let fix = FactoryFixture::new();
    let factory = fix.client();

    let ta = Address::generate(&fix.env);
    let tb = Address::generate(&fix.env);

    let res = factory.try_create_cl_pool(&fix.creator, &ta, &tb, &-1, &0);
    assert_eq!(res, Err(Ok(FactoryError::InvalidFeeBps)));
}

#[test]
fn test_create_cl_pool_excess_fee_rejected() {
    let fix = FactoryFixture::new();
    let factory = fix.client();

    let ta = Address::generate(&fix.env);
    let tb = Address::generate(&fix.env);

    let res10000 = factory.try_create_cl_pool(&fix.creator, &ta, &tb, &10_000, &0);
    assert_eq!(res10000, Err(Ok(FactoryError::InvalidFeeBps)));

    let res10001 = factory.try_create_cl_pool(&fix.creator, &ta, &tb, &10_001, &0);
    assert_eq!(res10001, Err(Ok(FactoryError::InvalidFeeBps)));
}

// =========================================================================
// 4. set_pool_creation_fee: zero or negative amount rejected (line 909)
// =========================================================================

#[test]
fn test_set_pool_creation_fee_zero_or_negative_rejected() {
    let fix = FactoryFixture::new();
    let factory = fix.client();

    let res0 = factory.try_set_pool_creation_fee(&fix.fee_token, &0);
    assert_eq!(res0, Err(Ok(FactoryError::FeeNotConfigured)));

    let res_neg = factory.try_set_pool_creation_fee(&fix.fee_token, &-500);
    assert_eq!(res_neg, Err(Ok(FactoryError::FeeNotConfigured)));

    // Positive succeeds
    let res_pos = factory.try_set_pool_creation_fee(&fix.fee_token, &1_000);
    assert!(res_pos.is_ok());
    assert_eq!(factory.get_pool_creation_fee(), 1_000);
}

// =========================================================================
// 5. set_global_fee: protocol_fee_bps bounds 0..=10_000 (line 1547)
// =========================================================================

#[test]
fn test_set_global_fee_out_of_bounds_rejected() {
    let fix = FactoryFixture::new();
    let factory = fix.client();

    let treasury = Address::generate(&fix.env);
    factory.set_treasury(&treasury, &50);

    let res_neg = factory.try_set_global_fee(&fix.admin, &-1);
    assert_eq!(res_neg, Err(Ok(FactoryError::InvalidFeeBps)));

    let res_over = factory.try_set_global_fee(&fix.admin, &10_001);
    assert_eq!(res_over, Err(Ok(FactoryError::InvalidFeeBps)));

    // Boundaries 0 and 10_000 succeed
    let res0 = factory.try_set_global_fee(&fix.admin, &0);
    assert!(res0.is_ok());

    let res10000 = factory.try_set_global_fee(&fix.admin, &10_000);
    assert!(res10000.is_ok());
}

// =========================================================================
// 6. set_global_fee_paginated: Treasury unconfigured check (line 1573)
// =========================================================================

#[test]
fn test_set_global_fee_paginated_rejected_without_treasury() {
    let fix = FactoryFixture::new();
    let factory = fix.client();

    // Call before set_treasury
    let res = factory.try_set_global_fee_paginated(&fix.admin, &0, &10);
    assert_eq!(res, Err(Ok(FactoryError::FeeNotConfigured)));

    // After setting treasury, succeeds
    let treasury = Address::generate(&fix.env);
    factory.set_treasury(&treasury, &50);

    let res_ok = factory.try_set_global_fee_paginated(&fix.admin, &0, &10);
    assert!(res_ok.is_ok());
}
