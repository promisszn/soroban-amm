#![no_std]

//! # Flash Loan Receiver Reference Implementation
//!
//! This crate demonstrates safe and profitable usage of the Soroban AMM's flash
//! loan mechanism. It is the canonical reference for any integrator building a
//! flash loan receiver.
//!
//! ## What is a flash loan?
//!
//! A flash loan is a loan of arbitrary size that is borrowed and repaid within
//! a single transaction:
//!
//! 1. A contract calls `pool.flash_loan(receiver, amount_a, amount_b, data)`
//! 2. The pool transfers `amount_a` of token A and `amount_b` of token B to the
//!    receiver contract
//! 3. The pool calls `receiver.on_flash_loan(amount_a, amount_b, fee_a, fee_b, data)`
//! 4. The receiver must repay `amount_a + fee_a` of token A and `amount_b + fee_b`
//!    of token B to the pool address before the call returns
//! 5. If repayment is incomplete, the entire transaction reverts
//!
//! Flash loans are dangerous: the receiver must handle multiple failure modes
//! correctly or lose all borrowed funds. This implementation demonstrates
//! every pitfall and how to avoid it.
//!
//! ## Callback contract guarantees
//!
//! **What the pool guarantees to the receiver:**
//! - The receiver's `on_flash_loan` is called exactly once per flash loan
//! - Balances of both tokens are increased by the borrowed amounts before the callback
//! - The pool will not check repayment until the callback returns
//!
//! **What the receiver must guarantee to the pool:**
//! - Before returning from `on_flash_loan`, the receiver must transfer back:
//!   - `amount_a + fee_a` of token A to the pool address
//!   - `amount_b + fee_b` of token B to the pool address
//! - No transfer to the pool address = entire transaction reverts
//! - Partial repayment = entire transaction reverts
//! - The receiver cannot call any pool functions during the callback (reentrancy guard)
//!
//! ## Strategies included
//!
//! This implementation includes two realistic use cases:
//!
//! ### Arbitrage
//!
//! Borrow from one pool, execute a profitable trade at a better price in another
//! market, and repay the loan plus fee. This is the most common flash loan use case.
//! See [`arbitrage`] for the implementation and economics.
//!
//! ### Collateral swap / Liquidity migration
//!
//! Borrow liquidity in one form (e.g., LP tokens), use it to restructure a position
//! (e.g., migrate between pool versions), and repay from the proceeds. See
//! [`collateral_swap`] for details.
//!
//! ## Failure modes demonstrated
//!
//! This crate also includes explicit demonstrations of how to **not** use flash loans:
//!
//! - **Insufficient profit**: A receiver that borrows and attempts to repay without
//!   a profitable trade. It must abort cleanly (return `false`) when the opportunity
//!   closes, and the pool must revert the entire flash loan atomically.
//!
//! - **Reentrancy attempt**: A receiver that tries to call the pool during the
//!   callback. The pool's reentrancy guard rejects this with `Reentrant`.
//!
//! - **Incomplete repayment**: A receiver that repays principal but not the fee.
//!   The pool's check fails and the transaction reverts.
//!
//! - **Wrong recipient**: A receiver that repays to the wrong address.
//!   The pool does not receive the funds and reverts.
//!
//! Each failure mode is tested to verify the pool ends in a correct state and
//! the receiver gained nothing from the attempt.
//!
//! ## Deployment
//!
//! The `initialize` entry point must be called before any flash loan:
//!
//! ```ignore
//! let env = Env::default();
//! let receiver = env.register_contract(None, FlashLoanReceiver);
//! let receiver_client = FlashLoanReceiverClient::new(&env, &receiver);
//! receiver_client.initialize(&pool_address);
//! ```
//!
//! Then call `on_flash_loan` from the pool, or trigger it via `flash_loan_execute`
//! in a test.

pub mod arbitrage;
pub mod collateral_swap;
pub mod failure_modes;

use soroban_amm_sdk::types::PoolInfo;
use soroban_sdk::{contract, contractimpl, contracttype, token::Client as TokenClient, Address, Bytes, Env};

/// Storage key for the pool address.
#[contracttype]
enum DataKey {
    Pool,
}

/// The main flash loan receiver contract.
///
/// This contract is called by the pool with borrowed funds and must repay them
/// within the callback, with fees.
#[contract]
pub struct FlashLoanReceiver;

#[contractimpl]
impl FlashLoanReceiver {
    /// Initialize the receiver with the pool address.
    ///
    /// This must be called before any flash loan callback. It stores the pool
    /// address in contract storage.
    ///
    /// # Arguments
    /// - `pool`: The address of the AMM pool contract
    ///
    /// # Panics
    /// None; initialization is idempotent.
    pub fn initialize(env: Env, pool: Address) {
        env.storage().instance().set(&DataKey::Pool, &pool);
    }

    /// Handle an incoming flash loan.
    ///
    /// This is the entry point called by the pool after transferring borrowed
    /// tokens to the receiver. The receiver has the entire callback to execute
    /// its strategy and repay.
    ///
    /// # Arguments
    /// - `token_a_amount`: Amount of token A borrowed (0 if not borrowed)
    /// - `token_b_amount`: Amount of token B borrowed (0 if not borrowed)
    /// - `fee_a`: Fee on token A (in token A units)
    /// - `fee_b`: Fee on token B (in token B units)
    /// - `_data`: Optional opaque data passed by the caller (unused here)
    ///
    /// # Returns
    /// - `true` if the loan was repaid successfully
    /// - `false` if the strategy aborted (e.g., opportunity closed)
    ///
    /// # Panics
    /// If repayment is incomplete or to the wrong address, the pool reverts
    /// the entire transaction before this function returns.
    pub fn on_flash_loan(
        env: Env,
        token_a_amount: i128,
        token_b_amount: i128,
        fee_a: i128,
        fee_b: i128,
        _data: Bytes,
    ) -> bool {
        // Retrieve pool address from storage
        let pool: Address = match env.storage().instance().get(&DataKey::Pool) {
            Ok(addr) => addr,
            Err(_) => return false, // Pool not initialized
        };

        // Get pool info to discover token addresses
        let pool_client = soroban_amm_sdk::client::AmmPoolClient::new(&env, &pool);
        let info: PoolInfo = pool_client.get_info();
        let receiver = env.current_contract_address();

        // ═══════════════════════════════════════════════════════════════════════
        // Core happy path: transfer principal + fees back to the pool
        // ═══════════════════════════════════════════════════════════════════════

        // Repay token A if borrowed
        if token_a_amount > 0 || fee_a > 0 {
            TokenClient::new(&env, &info.token_a).transfer(
                &receiver,
                &pool,
                &(token_a_amount + fee_a),
            );
        }

        // Repay token B if borrowed
        if token_b_amount > 0 || fee_b > 0 {
            TokenClient::new(&env, &info.token_b).transfer(
                &receiver,
                &pool,
                &(token_b_amount + fee_b),
            );
        }

        true
    }
}
