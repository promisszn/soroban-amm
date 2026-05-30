//! Batch Vesting Contract
//!
//! Supports the industry-standard "Cliff then Linear" vesting pattern:
//!   1. No tokens are claimable before `cliff_time`.
//!   2. After `cliff_time`, linear vesting is calculated from `start_time`
//!      through `end_time`, so any time already elapsed before the cliff
//!      passes contributes to the claimable amount immediately.
//!
//! Multiple beneficiaries can be deposited in a single call, reducing the
//! number of transactions required for bulk grant programs.

#![no_std]

use soroban_sdk::{
    contract, contractimpl, contracterror, contracttype, symbol_short, vec, Address, Env, Symbol,
    Vec,
};
use soroban_sdk::token::Client as TokenClient;

// ── Errors ────────────────────────────────────────────────────────────────────

#[contracterror]
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum VestingError {
    AlreadyInitialized    = 1,
    Unauthorized          = 2,
    ZeroAmount            = 3,
    InvalidSchedule       = 4,
    /// cliff_time must be ≥ start_time
    CliffBeforeStart      = 5,
    /// end_time must be > cliff_time
    EndBeforeCliff        = 6,
    NoVestingFound        = 7,
    NothingToClaim        = 8,
}

// ── Storage keys ──────────────────────────────────────────────────────────────

#[contracttype]
pub enum DataKey {
    Admin,
    Token,
    /// VestingData keyed by beneficiary address.
    Vesting(Address),
}

// ── Core types ────────────────────────────────────────────────────────────────

/// Vesting schedule for a single beneficiary.
///
/// # Cliff + Linear vesting
///
/// ```text
/// start_time      cliff_time                         end_time
///     |               |                                 |
///     |←── cliff ────→|←────── linear vesting ─────────→|
///     0               0  (nothing claimable until here)
/// ```
///
/// After `cliff_time` the entire elapsed portion since `start_time` is
/// immediately claimable, so beneficiaries are not penalised for the cliff.
#[contracttype]
#[derive(Clone, Debug)]
pub struct VestingData {
    /// Total token amount locked in this schedule.
    pub total_amount: i128,
    /// Timestamp (Unix seconds) when the linear vesting clock starts.
    pub start_time: u64,
    /// Timestamp at or after which tokens begin to be claimable.
    /// Must be ≥ `start_time`. Set equal to `start_time` to disable the cliff.
    pub cliff_time: u64,
    /// Timestamp when 100 % of `total_amount` is vested.
    /// Must be > `cliff_time`.
    pub end_time: u64,
    /// Tokens already claimed by the beneficiary.
    pub claimed_amount: i128,
}

/// Input record for a single grant in a batch deposit.
#[contracttype]
#[derive(Clone, Debug)]
pub struct VestingGrant {
    pub beneficiary: Address,
    pub total_amount: i128,
    pub start_time: u64,
    pub cliff_time: u64,
    pub end_time: u64,
}

// ── Contract ──────────────────────────────────────────────────────────────────

#[contract]
pub struct BatchVesting;

#[contractimpl]
impl BatchVesting {
    // ── Setup ─────────────────────────────────────────────────────────────────

    pub fn initialize(env: Env, admin: Address, token: Address) -> Result<(), VestingError> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(VestingError::AlreadyInitialized);
        }
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Token, &token);
        Ok(())
    }

    // ── Deposit ───────────────────────────────────────────────────────────────

    /// Deposit tokens for multiple beneficiaries in one transaction.
    ///
    /// The caller must have pre-approved this contract to transfer the sum of
    /// all grant amounts from their account.
    ///
    /// Emits a `VestingDeposited` event for each grant, including `cliff_time`
    /// so off-chain indexers can reconstruct the full schedule.
    pub fn batch_deposit(
        env: Env,
        depositor: Address,
        grants: Vec<VestingGrant>,
    ) -> Result<(), VestingError> {
        depositor.require_auth();

        let token: Address = env.storage().instance().get(&DataKey::Token).unwrap();
        let token_client = TokenClient::new(&env, &token);
        let pool = env.current_contract_address();

        let mut total: i128 = 0;

        for grant in grants.iter() {
            Self::validate_schedule(&grant)?;
            total += grant.total_amount;
        }

        if total <= 0 {
            return Err(VestingError::ZeroAmount);
        }

        // Pull the aggregate amount in a single transfer to save fees.
        token_client.transfer(&depositor, &pool, &total);

        for grant in grants.iter() {
            let data = VestingData {
                total_amount: grant.total_amount,
                start_time: grant.start_time,
                cliff_time: grant.cliff_time,
                end_time: grant.end_time,
                claimed_amount: 0,
            };
            env.storage()
                .instance()
                .set(&DataKey::Vesting(grant.beneficiary.clone()), &data);

            env.events().publish(
                (Symbol::new(&env, "VestingDeposited"), grant.beneficiary.clone()),
                (
                    grant.total_amount,
                    grant.start_time,
                    grant.cliff_time,
                    grant.end_time,
                ),
            );
        }

        Ok(())
    }

    // ── Claim ─────────────────────────────────────────────────────────────────

    /// Claim all currently vested (and unclaimed) tokens for the caller.
    pub fn claim(env: Env, beneficiary: Address) -> Result<i128, VestingError> {
        beneficiary.require_auth();

        let key = DataKey::Vesting(beneficiary.clone());
        let mut data: VestingData = env
            .storage()
            .instance()
            .get(&key)
            .ok_or(VestingError::NoVestingFound)?;

        let now = env.ledger().timestamp();
        let vested = Self::calculate_vested_amount(&data, now);
        let claimable = vested - data.claimed_amount;

        if claimable <= 0 {
            return Err(VestingError::NothingToClaim);
        }

        data.claimed_amount += claimable;
        env.storage().instance().set(&key, &data);

        let token: Address = env.storage().instance().get(&DataKey::Token).unwrap();
        TokenClient::new(&env, &token).transfer(
            &env.current_contract_address(),
            &beneficiary,
            &claimable,
        );

        env.events().publish(
            (symbol_short!("claimed"), beneficiary),
            (claimable, now),
        );

        Ok(claimable)
    }

    // ── View ──────────────────────────────────────────────────────────────────

    /// Return how many tokens are currently claimable by `beneficiary`.
    pub fn claimable(env: Env, beneficiary: Address) -> i128 {
        let key = DataKey::Vesting(beneficiary);
        let data: Option<VestingData> = env.storage().instance().get(&key);
        match data {
            None => 0,
            Some(d) => {
                let now = env.ledger().timestamp();
                let vested = Self::calculate_vested_amount(&d, now);
                (vested - d.claimed_amount).max(0)
            }
        }
    }

    /// Return the full vesting schedule for `beneficiary`.
    pub fn get_vesting(env: Env, beneficiary: Address) -> Option<VestingData> {
        env.storage()
            .instance()
            .get(&DataKey::Vesting(beneficiary))
    }

    // ── Internal helpers ──────────────────────────────────────────────────────

    /// Calculate the total vested amount for `data` at timestamp `now`.
    ///
    /// # Rules
    /// - Returns 0 if `now < cliff_time` (cliff not yet reached).
    /// - Returns `total_amount` if `now >= end_time` (fully vested).
    /// - Otherwise applies linear interpolation from `start_time` to `end_time`.
    ///   Because vesting starts accumulating from `start_time` (not `cliff_time`),
    ///   the moment the cliff passes the beneficiary can claim everything that
    ///   linearly accrued between `start_time` and `now`.
    pub(crate) fn calculate_vested_amount(data: &VestingData, now: u64) -> i128 {
        // Nothing claimable before the cliff.
        if now < data.cliff_time {
            return 0;
        }

        // Fully vested once end_time is reached.
        if now >= data.end_time {
            return data.total_amount;
        }

        // Linear vesting from start_time through end_time.
        // elapsed / duration gives the fraction vested.
        let elapsed = (now - data.start_time) as i128;
        let duration = (data.end_time - data.start_time) as i128;

        data.total_amount * elapsed / duration
    }

    fn validate_schedule(grant: &VestingGrant) -> Result<(), VestingError> {
        if grant.total_amount <= 0 {
            return Err(VestingError::ZeroAmount);
        }
        if grant.cliff_time < grant.start_time {
            return Err(VestingError::CliffBeforeStart);
        }
        if grant.end_time <= grant.cliff_time {
            return Err(VestingError::EndBeforeCliff);
        }
        Ok(())
    }
}
