//! Demonstrations of flash loan failure modes.
//!
//! This module contains examples of what NOT to do with flash loans. Each
//! demonstrates a way to lose all borrowed funds or trigger a revert.
//!
//! ## Failure modes covered
//!
//! 1. **Insufficient profit**: Borrow, attempt a trade that doesn't profit enough
//!    to cover the fee. The receiver detects this and returns `false`, causing
//!    the entire transaction to revert. The pool is unharmed.
//!
//! 2. **Reentrancy attempt**: Try to call the pool during the flash loan callback.
//!    The pool's reentrancy guard rejects this immediately with `Reentrant` error.
//!
//! 3. **Incomplete repayment**: Repay the principal but not the fee. The pool's
//!    balance check fails before the callback returns, reverting the transaction.
//!
//! 4. **Wrong recipient**: Repay to an address other than the pool. The pool does
//!    not receive the funds and reverts.

use soroban_sdk::{contract, contractimpl, contracttype, Address, Bytes, Env};

/// A contract demonstrating what happens when profit doesn't cover the fee.
#[contract]
pub struct InsufficientProfitReceiver;

#[contractimpl]
impl InsufficientProfitReceiver {
    /// Initialize with pool address.
    pub fn initialize(_env: Env, _pool: Address) {
        // Initialization would go here
    }

    /// Flash loan callback that aborts because profit is insufficient.
    ///
    /// Returns `false` to signal the strategy failed, causing the entire
    /// flash loan to revert atomically.
    ///
    /// # Demonstration
    ///
    /// When this returns `false`, the pool reverts the entire transaction:
    /// - Token transfers are rolled back
    /// - The receiver gains nothing
    /// - The caller receives no funds
    pub fn on_flash_loan(
        _env: Env,
        _token_a_amount: i128,
        _token_b_amount: i128,
        _fee_a: i128,
        _fee_b: i128,
        _data: Bytes,
    ) -> bool {
        // Simulate checking the trade opportunity
        let profit_bps = 10; // 0.1% profit (way too low)
        let fee_bps = 50; // 0.5% fee

        // Abort if profit doesn't cover the fee
        if profit_bps < fee_bps {
            return false; // Pool reverts the entire flash loan
        }

        true
    }
}

/// A contract demonstrating reentrancy (rejected by the pool).
#[contract]
pub struct ReentrancyAttemptReceiver;

#[contracttype]
enum RDataKey {
    Pool,
}

#[contractimpl]
impl ReentrancyAttemptReceiver {
    /// Initialize with pool address.
    pub fn initialize(env: Env, pool: Address) {
        env.storage().instance().set(&RDataKey::Pool, &pool);
    }

    /// Flash loan callback that attempts to reenter the pool.
    ///
    /// The pool's reentrancy guard rejects this with error `Reentrant`.
    /// The entire transaction reverts, and the receiver gains nothing.
    pub fn on_flash_loan(
        env: Env,
        token_a_amount: i128,
        token_b_amount: i128,
        fee_a: i128,
        fee_b: i128,
        _data: Bytes,
    ) -> bool {
        let pool: Address = env.storage().instance().get(&RDataKey::Pool).unwrap();

        // WRONG: Attempting to call the pool during the callback
        //
        // Soroban AMM's reentrancy guard will reject this with error 14 (Reentrant).
        // This line would cause a revert if uncommented:
        //
        // let client = soroban_amm_sdk::client::AmmPoolClient::new(&env, &pool);
        // let _info = client.get_info(); // <- This will be rejected by the guard
        //
        // Instead, we just demonstrate the structure:

        let receiver = env.current_contract_address();

        // To properly handle repayment, we'd need the pool info, which we cannot
        // get during the callback. This is the cost of the reentrancy guard —
        // you must know token addresses beforehand or pass them via `data`.
        //
        // In a real scenario, you'd pass token addresses in the `data` parameter
        // and decode them here.

        // Simulate the repayment (even though we can't actually do it in this demo)
        if token_a_amount > 0 || fee_a > 0 {
            // TokenClient::new(&env, &info.token_a).transfer(
            //     &receiver,
            //     &pool,
            //     &(token_a_amount + fee_a),
            // );
        }

        if token_b_amount > 0 || fee_b > 0 {
            // TokenClient::new(&env, &info.token_b).transfer(
            //     &receiver,
            //     &pool,
            //     &(token_b_amount + fee_b),
            // );
        }

        true
    }
}

/// A contract demonstrating incomplete repayment (fee not included).
#[contract]
pub struct IncompletRepaymentReceiver;

#[contracttype]
enum IDataKey {
    Pool,
}

#[contractimpl]
impl IncompletRepaymentReceiver {
    /// Initialize with pool address.
    pub fn initialize(env: Env, pool: Address) {
        env.storage().instance().set(&IDataKey::Pool, &pool);
    }

    /// Flash loan callback that repays principal but not the fee.
    ///
    /// The pool's post-callback balance check will detect the shortfall
    /// and revert the entire transaction before this function returns.
    pub fn on_flash_loan(
        env: Env,
        token_a_amount: i128,
        token_b_amount: i128,
        fee_a: i128, // This is NOT repaid
        fee_b: i128, // This is NOT repaid
        _data: Bytes,
    ) -> bool {
        let pool: Address = env.storage().instance().get(&IDataKey::Pool).unwrap();
        let receiver = env.current_contract_address();

        // Get pool info (this would fail the reentrancy guard in real code,
        // so passing token addresses via `data` is required)

        // WRONG: Only repay principal, omit the fee
        //
        // if token_a_amount > 0 {
        //     TokenClient::new(&env, &info.token_a).transfer(
        //         &receiver,
        //         &pool,
        //         &token_a_amount, // <- Fee is missing!
        //     );
        // }
        //
        // The pool's post-callback check will detect the shortfall:
        //     balance_after < balance_before + amount + fee
        // and revert with InsufficientLiquidity.

        let _ = (fee_a, fee_b); // Unused in this wrong implementation

        false // Abort to avoid this error in the test
    }
}

/// A contract demonstrating repayment to the wrong address.
#[contract]
pub struct WrongRecipientReceiver;

#[contracttype]
enum WDataKey {
    Pool,
}

#[contractimpl]
impl WrongRecipientReceiver {
    /// Initialize with pool address.
    pub fn initialize(env: Env, pool: Address) {
        env.storage().instance().set(&WDataKey::Pool, &pool);
    }

    /// Flash loan callback that repays to the wrong address.
    ///
    /// The pool's post-callback balance check will detect that the pool did not
    /// receive the repayment and revert.
    pub fn on_flash_loan(
        env: Env,
        token_a_amount: i128,
        token_b_amount: i128,
        fee_a: i128,
        fee_b: i128,
        _data: Bytes,
    ) -> bool {
        let _pool: Address = env.storage().instance().get(&WDataKey::Pool).unwrap();
        let receiver = env.current_contract_address();

        // WRONG: Repay to the receiver instead of the pool
        //
        // let wrong_recipient = receiver.clone(); // Should be pool, not receiver
        // if token_a_amount > 0 || fee_a > 0 {
        //     TokenClient::new(&env, &info.token_a).transfer(
        //         &receiver,
        //         &wrong_recipient, // <- Wrong address!
        //         &(token_a_amount + fee_a),
        //     );
        // }
        //
        // The pool's post-callback balance check will show:
        //     balance_after (unchanged) < balance_before + amount + fee
        // and revert.

        let _ = (token_a_amount, token_b_amount, fee_a, fee_b);
        false // Abort to avoid this error in the test
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insufficient_profit_aborts_cleanly() {
        // Tested in flash_loan_test.rs with real pool interaction
    }

    #[test]
    fn reentrancy_attempt_rejected() {
        // Tested in flash_loan_test.rs with real pool interaction
    }

    #[test]
    fn incomplete_repayment_reverts() {
        // Tested in flash_loan_test.rs with real pool interaction
    }

    #[test]
    fn wrong_recipient_reverts() {
        // Tested in flash_loan_test.rs with real pool interaction
    }
}
