//! Protocol-Owned Liquidity (POL) Vesting Contract
//!
//! Governance can create time-based vesting schedules for LP tokens so that
//! protocol-owned liquidity cannot be withdrawn in a single governance vote.
//! Tokens vest linearly between `cliff_ledger` and `end_ledger`.

#![no_std]

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, token, Address, Env, Symbol,
};

// ── Errors ────────────────────────────────────────────────────────────────────

#[contracterror]
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum VestingError {
    AlreadyInitialized = 1,
    NotGovernance = 2,
    VestingNotFound = 3,
    VestingAlreadyExists = 4,
    NothingToRelease = 5,
    InvalidSchedule = 6,
    NotBeneficiary = 7,
    NoPendingGovernance = 8,
}

// ── Constants ─────────────────────────────────────────────────────────────────

const MIN_TTL: u32 = 241_920; // ~14 days (at 5s per ledger)
const BUMP_TO: u32 = 3_110_400; // ~180 days (at 5s per ledger)

// ── Storage keys ──────────────────────────────────────────────────────────────

#[contracttype]
pub enum DataKey {
    /// Governance contract address (the only caller allowed to create/revoke).
    Governance,
    /// Pending governance nominee for two-step governance rotation.
    PendingGovernance,
    /// Treasury address — receives tokens on revocation.
    Treasury,
    /// Per-beneficiary vesting schedule.
    Vesting(Address),
}

// ── Types ─────────────────────────────────────────────────────────────────────

#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct PolVesting {
    pub lp_token: Address,
    pub pool: Address,
    pub total: i128,
    pub released: i128,
    pub start_ledger: u32,
    pub cliff_ledger: u32,
    pub end_ledger: u32,
    pub beneficiary: Address,
}

// ── Contract ──────────────────────────────────────────────────────────────────

#[contract]
pub struct PolVestingContract;

#[contractimpl]
impl PolVestingContract {
    /// One-time setup. `governance` is the only address allowed to create or
    /// revoke schedules. `treasury` receives tokens when a schedule is revoked.
    pub fn initialize(
        env: Env,
        governance: Address,
        treasury: Address,
    ) -> Result<(), VestingError> {
        if env.storage().instance().has(&DataKey::Governance) {
            return Err(VestingError::AlreadyInitialized);
        }
        env.storage()
            .instance()
            .set(&DataKey::Governance, &governance);
        env.storage().instance().set(&DataKey::Treasury, &treasury);
        Ok(())
    }

    /// Nominate a new governance address.
    ///
    /// Requires current governance authorization. The nominee must call
    /// `accept_governance` to complete the handover.
    pub fn propose_governance(
        env: Env,
        current_governance: Address,
        new_governance: Address,
    ) -> Result<(), VestingError> {
        current_governance.require_auth();
        Self::require_governance(&env, &current_governance)?;

        env.storage()
            .instance()
            .set(&DataKey::PendingGovernance, &Some(new_governance.clone()));

        env.events().publish(
            (Symbol::new(&env, "governance_proposed"),),
            (current_governance, new_governance),
        );
        Ok(())
    }

    /// Accept a pending governance nomination.
    ///
    /// Only the nominated address can accept. On success, governance is updated,
    /// the pending nominee is cleared, and a transfer event is emitted.
    pub fn accept_governance(env: Env, new_governance: Address) -> Result<(), VestingError> {
        let pending: Option<Address> = env
            .storage()
            .instance()
            .get(&DataKey::PendingGovernance)
            .unwrap_or(None);
        let nominee = pending.ok_or(VestingError::NoPendingGovernance)?;

        if new_governance != nominee {
            return Err(VestingError::NotGovernance);
        }
        new_governance.require_auth();

        let old_governance: Address = env.storage().instance().get(&DataKey::Governance).unwrap();
        env.storage()
            .instance()
            .set(&DataKey::Governance, &new_governance);
        env.storage()
            .instance()
            .set(&DataKey::PendingGovernance, &Option::<Address>::None);

        env.events().publish(
            (Symbol::new(&env, "governance_transferred"),),
            (old_governance, new_governance),
        );
        Ok(())
    }

    /// Return the active governance address.
    pub fn get_governance(env: Env) -> Address {
        env.storage().instance().get(&DataKey::Governance).unwrap()
    }

    /// Return the pending governance nominee, if any.
    pub fn get_pending_governance(env: Env) -> Option<Address> {
        env.storage()
            .instance()
            .get(&DataKey::PendingGovernance)
            .unwrap_or(None)
    }

    /// Governance creates a vesting schedule for `beneficiary`.
    ///
    /// The caller must be the governance contract and must have already
    /// transferred `total` LP tokens to this contract before calling.
    ///
    /// - `cliff_ledger` must be >= `start_ledger`
    /// - `end_ledger` must be > `cliff_ledger`
    #[allow(clippy::too_many_arguments)]
    pub fn create_vesting(
        env: Env,
        governance: Address,
        beneficiary: Address,
        lp_token: Address,
        pool: Address,
        total: i128,
        start_ledger: u32,
        cliff_ledger: u32,
        end_ledger: u32,
    ) -> Result<(), VestingError> {
        governance.require_auth();
        Self::require_governance(&env, &governance)?;

        if cliff_ledger < start_ledger || end_ledger <= cliff_ledger || total <= 0 {
            return Err(VestingError::InvalidSchedule);
        }
        if env
            .storage()
            .persistent()
            .has(&DataKey::Vesting(beneficiary.clone()))
        {
            return Err(VestingError::VestingAlreadyExists);
        }

        let schedule = PolVesting {
            lp_token,
            pool,
            total,
            released: 0,
            start_ledger,
            cliff_ledger,
            end_ledger,
            beneficiary: beneficiary.clone(),
        };
        let key = DataKey::Vesting(beneficiary.clone());
        env.storage().persistent().set(&key, &schedule);
        env.storage()
            .persistent()
            .extend_ttl(&key, MIN_TTL, BUMP_TO);

        env.events().publish(
            (Symbol::new(&env, "vesting_created"),),
            (beneficiary, total, start_ledger, cliff_ledger, end_ledger),
        );
        Ok(())
    }

    /// Release all currently vested (but unreleased) LP tokens to the beneficiary.
    pub fn release(env: Env, beneficiary: Address) -> Result<i128, VestingError> {
        beneficiary.require_auth();

        let key = DataKey::Vesting(beneficiary.clone());
        let mut schedule: PolVesting = env
            .storage()
            .persistent()
            .get(&key)
            .ok_or(VestingError::VestingNotFound)?;

        env.storage()
            .persistent()
            .extend_ttl(&key, MIN_TTL, BUMP_TO);

        if schedule.beneficiary != beneficiary {
            return Err(VestingError::NotBeneficiary);
        }

        let current_ledger = env.ledger().sequence();
        let releasable = Self::vested_amount(&schedule, current_ledger) - schedule.released;
        if releasable <= 0 {
            return Err(VestingError::NothingToRelease);
        }

        schedule.released += releasable;
        env.storage().persistent().set(&key, &schedule);

        token::Client::new(&env, &schedule.lp_token).transfer(
            &env.current_contract_address(),
            &beneficiary,
            &releasable,
        );

        env.events()
            .publish((Symbol::new(&env, "released"),), (beneficiary, releasable));
        Ok(releasable)
    }

    /// Read the vesting schedule for a beneficiary.
    pub fn get_vesting(env: Env, beneficiary: Address) -> Result<PolVesting, VestingError> {
        env.storage()
            .persistent()
            .get(&DataKey::Vesting(beneficiary))
            .ok_or(VestingError::VestingNotFound)
    }

    /// Governance cancels a vesting schedule. Any unreleased tokens are
    /// returned to the treasury; already-released tokens are unaffected.
    pub fn revoke_vesting(
        env: Env,
        governance: Address,
        beneficiary: Address,
    ) -> Result<(), VestingError> {
        governance.require_auth();
        Self::require_governance(&env, &governance)?;

        let key = DataKey::Vesting(beneficiary.clone());
        let schedule: PolVesting = env
            .storage()
            .persistent()
            .get(&key)
            .ok_or(VestingError::VestingNotFound)?;

        env.storage()
            .persistent()
            .extend_ttl(&key, MIN_TTL, BUMP_TO);

        let current_ledger = env.ledger().sequence();
        let vested = Self::vested_amount(&schedule, current_ledger);
        // Tokens already vested but not yet released go to beneficiary first.
        let to_beneficiary = vested - schedule.released;
        let to_treasury = schedule.total - vested;

        let lp = token::Client::new(&env, &schedule.lp_token);
        let contract_addr = env.current_contract_address();

        if to_beneficiary > 0 {
            lp.transfer(&contract_addr, &beneficiary, &to_beneficiary);
        }
        if to_treasury > 0 {
            let treasury: Address = env.storage().instance().get(&DataKey::Treasury).unwrap();
            lp.transfer(&contract_addr, &treasury, &to_treasury);
        }

        env.storage().persistent().remove(&key);

        env.events().publish(
            (Symbol::new(&env, "vesting_revoked"),),
            (beneficiary, to_beneficiary, to_treasury),
        );
        Ok(())
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn require_governance(env: &Env, caller: &Address) -> Result<(), VestingError> {
        let gov: Address = env.storage().instance().get(&DataKey::Governance).unwrap();
        if gov != *caller {
            return Err(VestingError::NotGovernance);
        }
        Ok(())
    }

    /// Linear vesting after cliff; 0 before cliff; `total` after end.
    fn vested_amount(schedule: &PolVesting, current_ledger: u32) -> i128 {
        if current_ledger < schedule.cliff_ledger {
            return 0;
        }
        if current_ledger >= schedule.end_ledger {
            return schedule.total;
        }
        let elapsed = (current_ledger - schedule.start_ledger) as i128;
        let duration = (schedule.end_ledger - schedule.start_ledger) as i128;
        schedule.total * elapsed / duration
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{
        testutils::{Address as _, Ledger},
        token::{StellarAssetClient, TokenClient},
        Env,
    };

    struct Setup {
        env: Env,
        contract_id: Address,
        governance: Address,
        treasury: Address,
        beneficiary: Address,
        lp_token: Address,
        pool: Address,
    }

    fn setup() -> Setup {
        let env = Env::default();
        env.mock_all_auths();

        let governance = Address::generate(&env);
        let treasury = Address::generate(&env);
        let beneficiary = Address::generate(&env);
        let pool = Address::generate(&env);

        // Use the built-in Stellar asset contract as a SEP-41 token.
        let lp_token = env
            .register_stellar_asset_contract_v2(governance.clone())
            .address();

        let contract_id = env.register_contract(None, PolVestingContract);
        let client = PolVestingContractClient::new(&env, &contract_id);
        client.initialize(&governance, &treasury);

        // Mint 1_000_000 LP tokens to the vesting contract.
        StellarAssetClient::new(&env, &lp_token).mint(&contract_id, &1_000_000);

        Setup {
            env,
            contract_id,
            governance,
            treasury,
            beneficiary,
            lp_token,
            pool,
        }
    }

    fn create_schedule(s: &Setup, start: u32, cliff: u32, end: u32) {
        let client = PolVestingContractClient::new(&s.env, &s.contract_id);
        client.create_vesting(
            &s.governance,
            &s.beneficiary,
            &s.lp_token,
            &s.pool,
            &1_000_000,
            &start,
            &cliff,
            &end,
        );
    }

    #[test]
    fn test_cliff_enforcement() {
        let s = setup();
        s.env.ledger().set_sequence_number(100);
        create_schedule(&s, 100, 200, 400);

        // Before cliff — nothing to release.
        s.env.ledger().set_sequence_number(150);
        let client = PolVestingContractClient::new(&s.env, &s.contract_id);
        let err = client.try_release(&s.beneficiary).unwrap_err().unwrap();
        assert_eq!(err, VestingError::NothingToRelease);
    }

    #[test]
    fn test_linear_vesting() {
        let s = setup();
        // start=0, cliff=0, end=1000 → fully linear from ledger 0
        create_schedule(&s, 0, 0, 1000);

        s.env.ledger().set_sequence_number(500);
        let client = PolVestingContractClient::new(&s.env, &s.contract_id);
        let released = client.release(&s.beneficiary);
        assert_eq!(released, 500_000); // 50% of 1_000_000

        let schedule = client.get_vesting(&s.beneficiary);
        assert_eq!(schedule.released, 500_000);
    }

    #[test]
    fn test_full_release() {
        let s = setup();
        create_schedule(&s, 0, 0, 1000);

        s.env.ledger().set_sequence_number(1000);
        let client = PolVestingContractClient::new(&s.env, &s.contract_id);
        let released = client.release(&s.beneficiary);
        assert_eq!(released, 1_000_000);

        // Nothing left to release.
        let err = client.try_release(&s.beneficiary).unwrap_err().unwrap();
        assert_eq!(err, VestingError::NothingToRelease);
    }

    #[test]
    fn test_revoke() {
        let s = setup();
        // start=0, cliff=0, end=1000; revoke at ledger 250 → 25% vested
        create_schedule(&s, 0, 0, 1000);

        s.env.ledger().set_sequence_number(250);
        let client = PolVestingContractClient::new(&s.env, &s.contract_id);
        client.revoke_vesting(&s.governance, &s.beneficiary);

        let lp = TokenClient::new(&s.env, &s.lp_token);
        // 25% went to beneficiary, 75% to treasury
        assert_eq!(lp.balance(&s.beneficiary), 250_000);
        assert_eq!(lp.balance(&s.treasury), 750_000);

        // Schedule is gone.
        let err = client.try_get_vesting(&s.beneficiary).unwrap_err().unwrap();
        assert_eq!(err, VestingError::VestingNotFound);
    }

    #[test]
    fn test_propose_and_accept_governance() {
        let s = setup();
        let client = PolVestingContractClient::new(&s.env, &s.contract_id);
        let new_governance = Address::generate(&s.env);

        client.propose_governance(&s.governance, &new_governance);
        assert_eq!(
            client.get_pending_governance(),
            Some(new_governance.clone())
        );

        client.accept_governance(&new_governance);
        assert_eq!(client.get_governance(), new_governance);
        assert_eq!(client.get_pending_governance(), None);
    }

    #[test]
    fn test_propose_governance_requires_current_governance() {
        let s = setup();
        let client = PolVestingContractClient::new(&s.env, &s.contract_id);
        let rando = Address::generate(&s.env);
        let new_governance = Address::generate(&s.env);

        let err = client
            .try_propose_governance(&rando, &new_governance)
            .unwrap_err()
            .unwrap();
        assert_eq!(err, VestingError::NotGovernance);
    }

    #[test]
    fn test_accept_governance_requires_pending_nominee() {
        let s = setup();
        let client = PolVestingContractClient::new(&s.env, &s.contract_id);
        let new_governance = Address::generate(&s.env);
        let other = Address::generate(&s.env);

        let err = client
            .try_accept_governance(&new_governance)
            .unwrap_err()
            .unwrap();
        assert_eq!(err, VestingError::NoPendingGovernance);

        client.propose_governance(&s.governance, &new_governance);
        let err = client.try_accept_governance(&other).unwrap_err().unwrap();
        assert_eq!(err, VestingError::NotGovernance);
    }

    #[test]
    fn test_new_governance_controls_vesting_after_acceptance() {
        let s = setup();
        let client = PolVestingContractClient::new(&s.env, &s.contract_id);
        let new_governance = Address::generate(&s.env);

        client.propose_governance(&s.governance, &new_governance);
        client.accept_governance(&new_governance);

        client.create_vesting(
            &new_governance,
            &s.beneficiary,
            &s.lp_token,
            &s.pool,
            &1_000_000,
            &0,
            &0,
            &1000,
        );

        let old_governance_err = client
            .try_revoke_vesting(&s.governance, &s.beneficiary)
            .unwrap_err()
            .unwrap();
        assert_eq!(old_governance_err, VestingError::NotGovernance);

        client.revoke_vesting(&new_governance, &s.beneficiary);
    }
}
