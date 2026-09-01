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

mod shortfall_receiver {
    use super::DataKey;
    use amm::{AmmPoolClient, FlashLoanReceiver};
    use soroban_sdk::{
        contract, contractimpl, contracttype, token::Client as TokenClient, Address, Bytes, Env,
    };

    /// A receiver that deliberately repays less than `amount + fee`.
    ///
    /// It returns `true` from `on_flash_loan` — so the failure is caught by the
    /// pool's post-callback balance check, not by the receiver's own return value.
    /// `shortfall` is the number of units withheld from the token A repayment.
    #[contract]
    pub struct ShortfallReceiver;

    #[contractimpl]
    impl ShortfallReceiver {
        pub fn initialize(env: Env, pool: Address, shortfall: i128) {
            let info = AmmPoolClient::new(&env, &pool).get_info();
            env.storage().instance().set(&DataKey::Pool, &pool);
            env.storage()
                .instance()
                .set(&DataKey::TokenA, &info.token_a);
            env.storage()
                .instance()
                .set(&DataKey::TokenB, &info.token_b);
            env.storage()
                .instance()
                .set(&ShortfallKey::Shortfall, &shortfall);
        }
    }

    #[contracttype]
    enum ShortfallKey {
        Shortfall,
    }

    #[contractimpl]
    impl FlashLoanReceiver for ShortfallReceiver {
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
            let shortfall: i128 = env
                .storage()
                .instance()
                .get(&ShortfallKey::Shortfall)
                .unwrap();
            let receiver = env.current_contract_address();

            if token_a_amount > 0 || fee_a > 0 {
                let owed = token_a_amount + fee_a - shortfall;
                if owed > 0 {
                    TokenClient::new(&env, &token_a).transfer(&receiver, &pool, &owed);
                }
            }
            if token_b_amount > 0 || fee_b > 0 {
                TokenClient::new(&env, &token_b).transfer(
                    &receiver,
                    &pool,
                    &(token_b_amount + fee_b),
                );
            }

            true
        }
    }
}

mod add_liquidity_receiver {
    use super::DataKey;
    use amm::{AmmPoolClient, FlashLoanReceiver};
    use soroban_sdk::{
        contract, contractimpl, contracttype, token::Client as TokenClient, Address, Bytes, Env,
    };

    /// A receiver that uses the flash-borrowed tokens to add liquidity to the pool,
    /// then repays the loan out of a separately-funded reserve.
    ///
    /// `add_liquidity` is called on a *different* pool than the lender, because the
    /// lending pool holds the reentrancy lock for the duration of the callback.
    #[contract]
    pub struct AddLiquidityReceiver;

    #[contracttype]
    enum AddLiqKey {
        /// The pool liquidity is added to (distinct from the lending pool).
        TargetPool,
    }

    #[contractimpl]
    impl AddLiquidityReceiver {
        pub fn initialize(env: Env, pool: Address, target_pool: Address) {
            let info = AmmPoolClient::new(&env, &pool).get_info();
            env.storage().instance().set(&DataKey::Pool, &pool);
            env.storage()
                .instance()
                .set(&DataKey::TokenA, &info.token_a);
            env.storage()
                .instance()
                .set(&DataKey::TokenB, &info.token_b);
            env.storage()
                .instance()
                .set(&AddLiqKey::TargetPool, &target_pool);
        }
    }

    #[contractimpl]
    impl FlashLoanReceiver for AddLiquidityReceiver {
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
            let target: Address = env
                .storage()
                .instance()
                .get(&AddLiqKey::TargetPool)
                .unwrap();
            let receiver = env.current_contract_address();

            // Put the borrowed token A to work: deposit it (paired with token B the
            // receiver already holds) into the second pool for LP shares.
            AmmPoolClient::new(&env, &target).add_liquidity(
                &receiver,
                &token_a_amount,
                &token_a_amount,
                &0_i128,
                &u64::MAX,
            );

            // Repay the lending pool out of the receiver's own pre-funded reserve.
            if token_a_amount > 0 || fee_a > 0 {
                TokenClient::new(&env, &token_a).transfer(
                    &receiver,
                    &pool,
                    &(token_a_amount + fee_a),
                );
            }
            if token_b_amount > 0 || fee_b > 0 {
                TokenClient::new(&env, &token_b).transfer(
                    &receiver,
                    &pool,
                    &(token_b_amount + fee_b),
                );
            }

            true
        }
    }
}

use add_liquidity_receiver::{AddLiquidityReceiver, AddLiquidityReceiverClient};
use shortfall_receiver::{ShortfallReceiver, ShortfallReceiverClient};

/// Shared scaffolding: deploy a pool with `flash_loan_fee_bps`, seed it with
/// `liquidity` of each token, and return `(env, admin, pool, token_a, token_b)`.
fn setup_pool(
    flash_loan_fee_bps: i128,
    liquidity: i128,
) -> (Env, Address, Address, Address, Address) {
    let env = Env::default();
    env.budget().reset_unlimited();
    // Non-root auth is required because `flash_loan_then_add_liquidity_in_same_flow`
    // has its receiver call `add_liquidity` from inside the `on_flash_loan`
    // callback, i.e. below the root invocation.
    env.mock_all_auths_allowing_non_root_auth();

    let admin = Address::generate(&env);
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

    AmmPoolClient::new(&env, &amm_addr).initialize_with_flash_loan_fee(
        &admin,
        &token_a,
        &token_b,
        &lp_token,
        &30_i128,
        &admin,
        &0_i128,
        &flash_loan_fee_bps,
    );

    StellarAssetClient::new(&env, &token_a).mint(&admin, &(liquidity * 10));
    StellarAssetClient::new(&env, &token_b).mint(&admin, &(liquidity * 10));
    AmmPoolClient::new(&env, &amm_addr).add_liquidity(
        &admin,
        &liquidity,
        &liquidity,
        &0_i128,
        &u64::MAX,
    );

    (env, admin, amm_addr, token_a, token_b)
}

/// Issue #827: a receiver that repays less than `amount + fee` must roll the
/// whole flash loan back, leaving pool reserves byte-for-byte unchanged.
#[test]
fn flash_loan_repayment_shortfall_reverts_entire_transaction() {
    let (env, _admin, amm_addr, token_a, _token_b) = setup_pool(10, 1_000_000);
    let amm = AmmPoolClient::new(&env, &amm_addr);

    let before = amm.get_info();

    // The receiver is funded well beyond what it owes, so the revert is caused
    // by it *choosing* to underpay, not by it running out of tokens.
    let receiver = env.register_contract(None, ShortfallReceiver);
    ShortfallReceiverClient::new(&env, &receiver).initialize(&amm_addr, &1_i128);
    StellarAssetClient::new(&env, &token_a).mint(&receiver, &1_000_000_i128);

    let res = amm.try_flash_loan(
        &receiver,
        &500_000_i128,
        &0_i128,
        &Bytes::from_array(&env, &[0; 0]),
    );
    assert!(res.is_err(), "a one-unit shortfall must revert the loan");

    let after = amm.get_info();
    assert_eq!(
        after.reserve_a, before.reserve_a,
        "reserve A must be unchanged after the revert"
    );
    assert_eq!(
        after.reserve_b, before.reserve_b,
        "reserve B must be unchanged after the revert"
    );
    assert_eq!(after.total_shares, before.total_shares);
}

/// Issue #827: with `flash_loan_fee_bps = 0` the repayment is the principal
/// exactly — the pool charges nothing and its reserves come back level.
#[test]
fn flash_loan_with_zero_fee_configuration_succeeds() {
    let (env, _admin, amm_addr, token_a, _token_b) = setup_pool(0, 1_000_000);
    let amm = AmmPoolClient::new(&env, &amm_addr);

    assert_eq!(amm.get_info().flash_loan_fee_bps, 0);
    let before = amm.get_info();

    let receiver = env.register_contract(None, GoodReceiver);
    GoodReceiverClient::new(&env, &receiver).initialize(&amm_addr);
    // Fund the receiver so a nonzero fee *could* have been paid; the assertion
    // below is meaningful only because the balance was available and untouched.
    let seed = 100_000_i128;
    StellarAssetClient::new(&env, &token_a).mint(&receiver, &seed);

    let principal = 500_000_i128;
    let (fee_a, fee_b) = amm.flash_loan(
        &receiver,
        &principal,
        &0_i128,
        &Bytes::from_array(&env, &[0; 0]),
    );

    assert_eq!(fee_a, 0, "zero-bps pool must charge no fee on token A");
    assert_eq!(fee_b, 0);

    // Repayment == principal exactly: the receiver's own funds are untouched.
    assert_eq!(
        TokenClient::new(&env, &token_a).balance(&receiver),
        seed,
        "receiver repaid exactly the principal, so its seed balance is intact"
    );
    let after = amm.get_info();
    assert_eq!(after.reserve_a, before.reserve_a);
    assert_eq!(after.reserve_b, before.reserve_b);
}

/// Issue #827: flash-borrowed tokens are used to add liquidity to a second pool
/// inside the callback, and the loan is repaid from another source. The
/// receiver's LP share balance must grow by exactly what the deposit minted.
#[test]
fn flash_loan_then_add_liquidity_in_same_flow() {
    let (env, admin, lender_addr, token_a, token_b) = setup_pool(10, 2_000_000);
    let lender = AmmPoolClient::new(&env, &lender_addr);

    // A second pool over the same token pair, which the receiver deposits into.
    let target_addr = env.register_contract(None, AmmPool);
    let target_lp = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let target = AmmPoolClient::new(&env, &target_addr);
    target.initialize(
        &admin, &token_a, &token_b, &target_lp, &30_i128, &admin, &0_i128,
    );
    // Seed the target pool so the receiver's deposit is priced against a real
    // 1:1 reserve ratio rather than bootstrapping it.
    StellarAssetClient::new(&env, &token_a).mint(&admin, &1_000_000_i128);
    StellarAssetClient::new(&env, &token_b).mint(&admin, &1_000_000_i128);
    target.add_liquidity(&admin, &1_000_000, &1_000_000, &0_i128, &u64::MAX);

    let receiver = env.register_contract(None, AddLiquidityReceiver);
    AddLiquidityReceiverClient::new(&env, &receiver).initialize(&lender_addr, &target_addr);

    let borrow = 200_000_i128;
    // Token B for the paired deposit, plus token A to repay principal + fee out
    // of a source other than the borrowed funds (which go into the deposit).
    StellarAssetClient::new(&env, &token_b).mint(&receiver, &borrow);
    StellarAssetClient::new(&env, &token_a).mint(&receiver, &(borrow * 2));

    let lp_client = TokenClient::new(&env, &target_lp);
    let lp_before = lp_client.balance(&receiver);
    assert_eq!(lp_before, 0);
    let target_shares_before = target.get_info().total_shares;
    let lender_reserve_a_before = lender.get_info().reserve_a;

    let (fee_a, fee_b) = lender.flash_loan(
        &receiver,
        &borrow,
        &0_i128,
        &Bytes::from_array(&env, &[0; 0]),
    );
    assert!(fee_a > 0, "10 bps pool charges a fee on the principal");
    assert_eq!(fee_b, 0);

    // The deposit inside the callback minted shares to the receiver, and every
    // share minted by that deposit went to it.
    let lp_after = lp_client.balance(&receiver);
    let minted = target.get_info().total_shares - target_shares_before;
    assert!(minted > 0, "the in-callback deposit must mint shares");
    assert_eq!(
        lp_after - lp_before,
        minted,
        "receiver's LP balance grew by exactly the shares its deposit minted"
    );

    // The lender was made whole and kept the fee.
    assert_eq!(
        lender.get_info().reserve_a,
        lender_reserve_a_before + fee_a,
        "lender reserve grows by exactly the flash-loan fee"
    );
}
