//! Tests for flash-loan interactions across contracts.
//! Covers a flash loan that repays principal + fee via a simple receiver.

#![cfg(test)]

use soroban_sdk::{
    contract, contractimpl, contracttype,
    testutils::Address as _,
    token::{Client as TokenClient, StellarAssetClient},
    Address, Bytes, Env,
};

use amm::{AmmPool, AmmPoolClient, FlashLoanReceiver};

/// Storage keys for the pool address and its token pair, cached at
/// `initialize` time — mirroring `examples/flash_loan_receiver`'s reference
/// implementation.
#[contracttype]
enum DataKey {
    Pool,
    TokenA,
    TokenB,
}

/// A minimal flash loan receiver that repays exactly what it owes.
///
/// The pool has already credited the borrowed amounts to this contract's
/// token balances before calling `on_flash_loan`. The token pair is cached
/// at `initialize` time: the pool is still on the call stack during
/// `on_flash_loan`, and Soroban's host rejects any call back into a
/// contract that is already executing, so `get_info` cannot be called on
/// the pool from inside the callback.
#[contract]
pub struct GoodReceiver;

#[contractimpl]
impl GoodReceiver {
    pub fn initialize(env: Env, pool: Address) {
        let info = AmmPoolClient::new(&env, &pool).get_info();
        env.storage().instance().set(&DataKey::Pool, &pool);
        env.storage()
            .instance()
            .set(&DataKey::TokenA, &info.token_a);
        env.storage()
            .instance()
            .set(&DataKey::TokenB, &info.token_b);
    }
}

#[contractimpl]
impl FlashLoanReceiver for GoodReceiver {
    fn on_flash_loan(
        env: Env,
        token_a_amount: i128,
        token_b_amount: i128,
        fee_a: i128,
        fee_b: i128,
        _data: Bytes,
    ) -> bool {
        let pool: Address = env.storage().instance().get(&DataKey::Pool).unwrap();
        let token_a: Address = env.storage().instance().get(&DataKey::TokenA).unwrap();
        let token_b: Address = env.storage().instance().get(&DataKey::TokenB).unwrap();
        let receiver = env.current_contract_address();

        if token_a_amount > 0 || fee_a > 0 {
            TokenClient::new(&env, &token_a).transfer(&receiver, &pool, &(token_a_amount + fee_a));
        }
        if token_b_amount > 0 || fee_b > 0 {
            TokenClient::new(&env, &token_b).transfer(&receiver, &pool, &(token_b_amount + fee_b));
        }

        true
    }
}

#[test]
fn flash_loan_then_swap() {
    let env = Env::default();
    env.budget().reset_unlimited();
    env.mock_all_auths();

    let admin = Address::generate(&env);

    // Deploy AMM and token contracts
    let amm_addr = env.register_contract(None, AmmPool);
    let token_a = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let token_b = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let lp_token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();

    // Initialize pool with a flash-loan fee (10 bps)
    let amm = AmmPoolClient::new(&env, &amm_addr);
    amm.initialize_with_flash_loan_fee(
        &admin, &token_a, &token_b, &lp_token, &30i128, &admin, &0i128, &10i128,
    );

    // Seed the pool with liquidity so it has funds to lend.
    StellarAssetClient::new(&env, &token_a).mint(&admin, &10_000_000_i128);
    StellarAssetClient::new(&env, &token_b).mint(&admin, &10_000_000_i128);
    TokenClient::new(&env, &token_a).approve(&admin, &amm_addr, &10_000_000_i128, &1_000_000);
    TokenClient::new(&env, &token_b).approve(&admin, &amm_addr, &10_000_000_i128, &1_000_000);
    amm.add_liquidity(&admin, &1_000_000_i128, &1_000_000_i128, &0i128, &u64::MAX);

    // Prepare receiver for flash loan, pre-funded to cover the fee on repayment.
    let receiver = env.register_contract(None, GoodReceiver);
    let receiver_client = GoodReceiverClient::new(&env, &receiver);
    receiver_client.initialize(&amm_addr);
    StellarAssetClient::new(&env, &token_a).mint(&receiver, &1_000_000_i128);

    // Execute flash loan
    let (fee_a, fee_b) = amm.flash_loan(
        &receiver,
        &500_000_i128,
        &0_i128,
        &Bytes::from_array(&env, &[0; 0]),
    );
    assert!(fee_a > 0);
    assert_eq!(fee_b, 0);
}
