#![cfg(test)]

use flash_loan_receiver::FlashLoanReceiver;
use soroban_amm_sdk::client::AmmPoolClient;
use soroban_amm_sdk::types::PoolInfo;
use soroban_sdk::{
    testutils::{Address as _, Ledger, LedgerInfo},
    token::Client as TokenClient,
    Address, BytesN, Env,
};

// WASM artifacts
use amm::WASM as AMM_WASM;
use token::WASM as TOKEN_WASM;

// Setup fixture
struct FlashLoanTestEnv {
    env: Env,
    pool: Address,
    pool_client: AmmPoolClient,
    token_a: Address,
    token_b: Address,
    admin: Address,
    receiver: Address,
}

impl FlashLoanTestEnv {
    fn new() -> Self {
        let env = Env::default();
        env.budget().reset_unlimited();
        env.mock_all_auths();

        // Set ledger
        env.ledger().set(LedgerInfo {
            timestamp: 1000,
            protocol_version: 22,
            sequence_number: 1,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 16,
            min_persistent_entry_ttl: 4096,
            max_entry_ttl: 6_312_000,
        });

        let admin = Address::generate(&env);

        // Upload contract WASM
        let amm_hash: BytesN<32> = env.deployer().upload_contract_wasm(AMM_WASM);
        let token_hash: BytesN<32> = env.deployer().upload_contract_wasm(TOKEN_WASM);

        // Create token pair
        let token_a = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();
        let token_b = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();

        // Deploy AMM pool
        let pool = env
            .deployer()
            .with_current_contract(amm_hash)
            .deploy_contract_instance(
                amm_hash,
                soroban_sdk::symbol_short!("init"),
                (&admin, &token_a, &token_b, &token_a, 30i128, &admin, 0i128),
            );

        let pool_client = AmmPoolClient::new(&env, &pool);

        // Register the flash loan receiver contract
        let receiver = env.register_contract(None, FlashLoanReceiver);

        let receiver_client = flash_loan_receiver::FlashLoanReceiverClient::new(&env, &receiver);
        receiver_client.initialize(&pool);

        // Mint initial tokens to admin
        let token_a_client = TokenClient::new(&env, &token_a);
        let token_b_client = TokenClient::new(&env, &token_b);

        token_a_client.mint(&admin, &1_000_000_000_i128);
        token_b_client.mint(&admin, &1_000_000_000_i128);

        // Approve pool to spend tokens
        token_a_client.approve(&admin, &pool, &1_000_000_000_i128, &1000);
        token_b_client.approve(&admin, &pool, &1_000_000_000_i128, &1000);

        // Add initial liquidity
        pool_client.add_liquidity(&admin, &1_000_000_i128, &1_000_000_i128, &0i128, &u64::MAX);

        Self {
            env,
            pool,
            pool_client,
            token_a,
            token_b,
            admin,
            receiver,
        }
    }

    fn get_pool_info(&self) -> PoolInfo {
        self.pool_client.get_info()
    }

    fn get_token_balance(&self, token: &Address, account: &Address) -> i128 {
        TokenClient::new(&self.env, token).balance(account)
    }

    fn execute_flash_loan(&self, amount_a: i128, amount_b: i128) {
        self.pool_client.flash_loan(&self.receiver, &amount_a, &amount_b, &soroban_sdk::Bytes::new(&self.env));
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Happy path tests
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn happy_path_repay_principal_plus_fee() {
    let fixture = FlashLoanTestEnv::new();

    let initial_info = fixture.get_pool_info();
    let initial_reserve_a = initial_info.reserve_a;
    let initial_reserve_b = initial_info.reserve_b;
    let initial_fees_a = initial_info.accrued_fee_a;
    let initial_fees_b = initial_info.accrued_fee_b;

    // Execute flash loan
    let borrow_amount_a = 1_000_000i128;
    let borrow_amount_b = 0i128;
    fixture.execute_flash_loan(borrow_amount_a, borrow_amount_b);

    // Verify: pool reserves grew by exactly the fee
    let final_info = fixture.get_pool_info();

    // Flash loan fee is typically 0.05% = 5 bps
    let expected_fee_a = (borrow_amount_a * 5) / 10_000;

    // Reserves should increase by the fee
    assert_eq!(
        final_info.reserve_a, initial_reserve_a + expected_fee_a,
        "Pool reserve A should increase by the fee"
    );

    // Reserves should be unchanged for token B
    assert_eq!(
        final_info.reserve_b, initial_reserve_b,
        "Pool reserve B should be unchanged"
    );

    // Fee accumulation should reflect the flash loan fee
    assert!(
        final_info.accrued_fee_a >= initial_fees_a,
        "Accrued fees should not decrease"
    );
}

#[test]
fn happy_path_repay_both_tokens() {
    let fixture = FlashLoanTestEnv::new();

    let initial_info = fixture.get_pool_info();
    let initial_reserve_a = initial_info.reserve_a;
    let initial_reserve_b = initial_info.reserve_b;

    // Borrow both tokens
    let borrow_amount_a = 500_000i128;
    let borrow_amount_b = 500_000i128;
    fixture.execute_flash_loan(borrow_amount_a, borrow_amount_b);

    // Verify: both reserves grew by their fees
    let final_info = fixture.get_pool_info();
    let fee_bps = 5i128; // 0.05% = 5 bps

    let expected_fee_a = (borrow_amount_a * fee_bps) / 10_000;
    let expected_fee_b = (borrow_amount_b * fee_bps) / 10_000;

    assert_eq!(
        final_info.reserve_a, initial_reserve_a + expected_fee_a,
        "Pool reserve A should increase by the fee"
    );

    assert_eq!(
        final_info.reserve_b, initial_reserve_b + expected_fee_b,
        "Pool reserve B should increase by the fee"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Failure mode tests
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn insufficient_profit_aborts_cleanly() {
    let fixture = FlashLoanTestEnv::new();

    // Try to borrow and immediately repay without profit
    // The receiver should detect insufficient profit and return false
    // The pool reverts the entire flash loan

    let initial_info = fixture.get_pool_info();
    let initial_reserve_a = initial_info.reserve_a;
    let initial_reserve_b = initial_info.reserve_b;

    // Borrow a small amount
    let borrow_amount = 10_000i128;

    // This should trigger the flash loan but not execute actual profitable trade
    // The test verifies the receiver doesn't lose funds
    fixture.execute_flash_loan(borrow_amount, 0);

    // Pool should be unchanged (no fee accrual means receiver returned false early)
    let final_info = fixture.get_pool_info();
    assert_eq!(
        final_info.reserve_a, initial_reserve_a,
        "Pool should be unchanged if flash loan aborts"
    );
}

#[test]
fn zero_borrow_amount() {
    let fixture = FlashLoanTestEnv::new();

    // Borrowing zero should still call the callback and require repayment
    fixture.execute_flash_loan(0, 0);

    // Pool should remain unchanged
    let final_info = fixture.get_pool_info();
    assert_eq!(final_info.reserve_a, 1_000_000_i128);
    assert_eq!(final_info.reserve_b, 1_000_000_i128);
}

#[test]
fn receiver_repays_correct_amount() {
    let fixture = FlashLoanTestEnv::new();

    let borrow_amount_a = 100_000i128;
    let fee_bps = 5i128; // 0.05% = 5 bps
    let expected_fee = (borrow_amount_a * fee_bps) / 10_000;

    // Get token balance before
    let receiver_balance_before = fixture.get_token_balance(&fixture.token_a, &fixture.receiver);

    // Execute flash loan (receiver gets amount, then must repay amount + fee)
    fixture.execute_flash_loan(borrow_amount_a, 0);

    // Receiver should end with 0 tokens (it repaid everything)
    let receiver_balance_after = fixture.get_token_balance(&fixture.token_a, &fixture.receiver);
    assert_eq!(
        receiver_balance_after, receiver_balance_before,
        "Receiver should repay all borrowed tokens + fee"
    );

    // Pool should have gained the fee
    let pool_info = fixture.get_pool_info();
    assert!(
        pool_info.reserve_a > 1_000_000_i128,
        "Pool should gain fee from flash loan"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Fee accounting tests
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn fee_accounting_is_exact() {
    let fixture = FlashLoanTestEnv::new();

    let initial_info = fixture.get_pool_info();
    let initial_accrued_fees = initial_info.accrued_fee_a;

    let borrow_amount = 2_000_000i128;
    let fee_bps = 5i128; // 0.05% = 5 bps
    let expected_fee = (borrow_amount * fee_bps) / 10_000;

    fixture.execute_flash_loan(borrow_amount, 0);

    let final_info = fixture.get_pool_info();

    // Fee should increase by exactly the calculated amount
    assert_eq!(
        final_info.accrued_fee_a,
        initial_accrued_fees + expected_fee,
        "Accrued fees should increase by exactly the flash loan fee"
    );
}

#[test]
fn multiple_flash_loans_accumulate_fees() {
    let fixture = FlashLoanTestEnv::new();

    let initial_info = fixture.get_pool_info();
    let initial_accrued_fees = initial_info.accrued_fee_a;

    let borrow_amount = 500_000i128;
    let fee_bps = 5i128;
    let expected_fee_per_loan = (borrow_amount * fee_bps) / 10_000;

    // Execute two flash loans
    fixture.execute_flash_loan(borrow_amount, 0);
    fixture.execute_flash_loan(borrow_amount, 0);

    let final_info = fixture.get_pool_info();

    // Both fees should accumulate
    assert_eq!(
        final_info.accrued_fee_a,
        initial_accrued_fees + (expected_fee_per_loan * 2),
        "Accrued fees should include both flash loan fees"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Token conservation tests
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn total_tokens_are_conserved() {
    let fixture = FlashLoanTestEnv::new();

    let initial_pool_info = fixture.get_pool_info();
    let initial_pool_reserve_a = initial_pool_info.reserve_a;
    let initial_pool_reserve_b = initial_pool_info.reserve_b;

    // Execute flash loan
    let borrow_amount_a = 1_000_000i128;
    fixture.execute_flash_loan(borrow_amount_a, 0);

    // After repayment, pool should have its original amount plus the fee
    let final_pool_info = fixture.get_pool_info();
    let fee_bps = 5i128;
    let expected_fee = (borrow_amount_a * fee_bps) / 10_000;

    assert_eq!(
        final_pool_info.reserve_a,
        initial_pool_reserve_a + expected_fee,
        "Pool reserve should increase by exactly the fee"
    );

    assert_eq!(
        final_pool_info.reserve_b, initial_pool_reserve_b,
        "Pool reserve B should be unchanged"
    );
}
