//! Governance contract for LP-weighted fee parameter voting.
//!
//! LP token holders can propose changes to the pool's `fee_bps`.
//! Voting power is locked during the proposal lifecycle to prevent
//! flash-loan and vote-then-sell attacks. A proposal passes when:
//!   - `votes_for > votes_against`
//!   - total votes cast >= quorum (10 % of total LP supply at snapshot)
//!
//! After the voting period ends a timelock delay must elapse before anyone
//! can call `execute()`, which applies the change via `update_fee()` on the
//! AMM contract.

#![no_std]

use soroban_sdk::{contract, contractimpl, contracttype, Address, Env};

// ── Constants ─────────────────────────────────────────────────────────────────

/// Voting period: 7 days expressed in seconds.
const VOTING_PERIOD_SECS: u64 = 7 * 24 * 60 * 60;

/// Timelock delay after voting closes before execution is allowed: 2 days.
const TIMELOCK_SECS: u64 = 2 * 24 * 60 * 60;

/// Quorum: 10 % of total LP supply at snapshot must participate.
const QUORUM_BPS: i128 = 1_000; // 10 % in basis points
const MAX_BPS: i128 = 10_000;
const MIN_PERSISTENT_TTL: u32 = 172_800; // ~10 days at 5s/ledger
const PERSISTENT_TTL_BUMP_TO: u32 = 259_200; // ~15 days at 5s/ledger

// ── Storage keys ─────────────────────────────────────────────────────────────

#[contracttype]
pub enum DataKey {
    /// Address of the AMM pool contract.
    AmmPool,
    /// Address of the LP token contract.
    LpToken,
    /// Monotonically increasing proposal counter.
    ProposalCount,
    /// Governance admin.
    Admin,
    /// Minimum proposer stake in basis points of total LP supply.
    MinProposerStakeBps,
    /// Individual proposal storage.
    Proposal(u32),
    /// Whether a voter has already voted on a proposal: (proposal_id, voter).
    HasVoted(u32, Address),
    /// Locked voting amount for a voter on a proposal.
    LockedVote(u32, Address),
}

// ── Types ─────────────────────────────────────────────────────────────────────

#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub enum ProposalStatus {
    /// Voting is open.
    Active,
    /// Voting closed; waiting for timelock to expire.
    Pending,
    /// Timelock elapsed; ready to execute.
    Queued,
    /// Proposal was executed successfully.
    Executed,
    /// Proposal failed quorum or majority.
    Defeated,
    /// Proposal expired without execution after timelock window.
    Expired,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct Proposal {
    pub id: u32,
    pub proposer: Address,
    pub new_fee_bps: i128,
    /// LP total supply snapshot at proposal creation.
    pub snapshot_total_supply: i128,
    /// Timestamp when voting opens (== creation timestamp).
    pub vote_start: u64,
    /// Timestamp when voting closes.
    pub vote_end: u64,
    /// Timestamp after which execution is allowed (vote_end + TIMELOCK_SECS).
    pub execute_after: u64,
    /// Timestamp after which the proposal expires if not executed (execute_after + TIMELOCK_SECS).
    pub expires_at: u64,
    pub votes_for: i128,
    pub votes_against: i128,
    pub executed: bool,
}

// ── LP token client ───────────────────────────────────────────────────────────

#[soroban_sdk::contractclient(name = "LpTokenClient")]
pub trait LpTokenInterface {
    fn balance(env: Env, id: Address) -> i128;
    fn total_supply(env: Env) -> i128;
    fn lock(env: Env, holder: Address, amount: i128);
    fn unlock(env: Env, holder: Address, amount: i128);
}

// ── AMM client ────────────────────────────────────────────────────────────────

#[soroban_sdk::contractclient(name = "AmmPoolClient")]
pub trait AmmPoolInterface {
    fn update_fee(env: Env, new_fee_bps: i128);
}

// ── Contract ──────────────────────────────────────────────────────────────────

#[contract]
pub struct Governance;

#[contractimpl]
impl Governance {
    // ── Setup ─────────────────────────────────────────────────────────────────

    /// One-time initialisation. Must be called after deployment.
    pub fn initialize(
        env: Env,
        admin: Address,
        amm_pool: Address,
        lp_token: Address,
        min_proposer_stake_bps: i128,
    ) {
        assert!(
            !env.storage().instance().has(&DataKey::AmmPool),
            "already initialized"
        );
        assert!(
            (0..=MAX_BPS).contains(&min_proposer_stake_bps),
            "invalid min proposer stake bps"
        );
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::AmmPool, &amm_pool);
        env.storage().instance().set(&DataKey::LpToken, &lp_token);
        env.storage()
            .instance()
            .set(&DataKey::MinProposerStakeBps, &min_proposer_stake_bps);
        env.storage().instance().set(&DataKey::ProposalCount, &0u32);
    }

    /// Admin-only governance parameter update.
    pub fn set_min_proposer_stake_bps(env: Env, new_bps: i128) {
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();
        assert!(
            (0..=MAX_BPS).contains(&new_bps),
            "invalid min proposer stake bps"
        );
        env.storage()
            .instance()
            .set(&DataKey::MinProposerStakeBps, &new_bps);
    }

    // ── Core functions ────────────────────────────────────────────────────────

    /// Create a new proposal to change the pool fee.
    ///
    /// The proposer must hold at least the configured minimum LP stake.
    /// Returns the new `proposal_id`.
    pub fn propose(env: Env, proposer: Address, new_fee_bps: i128) -> u32 {
        proposer.require_auth();

        assert!((0..=MAX_BPS).contains(&new_fee_bps), "invalid fee");

        let lp_token: Address = env.storage().instance().get(&DataKey::LpToken).unwrap();
        let lp_client = LpTokenClient::new(&env, &lp_token);

        let total_supply = lp_client.total_supply();
        assert!(total_supply > 0, "LP total supply is zero");
        let proposer_balance = lp_client.balance(&proposer);
        let min_bps: i128 = env
            .storage()
            .instance()
            .get(&DataKey::MinProposerStakeBps)
            .unwrap_or(0);
        let min_stake = ((total_supply * min_bps) / MAX_BPS).max(1);
        assert!(
            proposer_balance >= min_stake,
            "insufficient stake to propose"
        );

        let now = env.ledger().timestamp();
        let vote_end = now + VOTING_PERIOD_SECS;
        let execute_after = vote_end + TIMELOCK_SECS;
        let expires_at = execute_after + TIMELOCK_SECS; // extra window to execute

        let id: u32 = env
            .storage()
            .instance()
            .get(&DataKey::ProposalCount)
            .unwrap();

        let proposal = Proposal {
            id,
            proposer: proposer.clone(),
            new_fee_bps,
            snapshot_total_supply: total_supply,
            vote_start: now,
            vote_end,
            execute_after,
            expires_at,
            votes_for: 0,
            votes_against: 0,
            executed: false,
        };

        let proposal_key = DataKey::Proposal(id);
        env.storage().persistent().set(&proposal_key, &proposal);
        Self::bump_key_ttl(&env, &proposal_key);
        env.storage()
            .instance()
            .set(&DataKey::ProposalCount, &(id + 1));

        id
    }

    /// Cast a vote on an active proposal.
    ///
    /// Voting power = voter's current LP balance, which is then locked until
    /// the proposal concludes.
    /// Each address may only vote once per proposal.
    pub fn vote(env: Env, voter: Address, proposal_id: u32, support: bool) {
        voter.require_auth();

        let proposal_key = DataKey::Proposal(proposal_id);
        let mut proposal: Proposal = env
            .storage()
            .persistent()
            .get(&proposal_key)
            .expect("proposal not found");
        Self::bump_key_ttl(&env, &proposal_key);

        let now = env.ledger().timestamp();
        assert!(now >= proposal.vote_start, "voting not started");
        assert!(now <= proposal.vote_end, "voting period has ended");
        assert!(!proposal.executed, "proposal already executed");

        let voted_key = DataKey::HasVoted(proposal_id, voter.clone());
        assert!(!env.storage().persistent().has(&voted_key), "already voted");

        let lp_token: Address = env.storage().instance().get(&DataKey::LpToken).unwrap();
        let lp_client = LpTokenClient::new(&env, &lp_token);
        let voting_power = lp_client.balance(&voter);
        assert!(voting_power > 0, "no LP tokens: voting power is zero");
        lp_client.lock(&voter, &voting_power);

        if support {
            proposal.votes_for += voting_power;
        } else {
            proposal.votes_against += voting_power;
        }

        env.storage().persistent().set(&proposal_key, &proposal);
        Self::bump_key_ttl(&env, &proposal_key);
        env.storage().persistent().set(&voted_key, &true);
        Self::bump_key_ttl(&env, &voted_key);
        let lock_key = DataKey::LockedVote(proposal_id, voter);
        env.storage().persistent().set(&lock_key, &voting_power);
        Self::bump_key_ttl(&env, &lock_key);
    }

    /// Execute a passed proposal after the timelock has elapsed.
    ///
    /// Anyone can call this once the conditions are met.
    pub fn execute(env: Env, proposal_id: u32) {
        let proposal_key = DataKey::Proposal(proposal_id);
        let mut proposal: Proposal = env
            .storage()
            .persistent()
            .get(&proposal_key)
            .expect("proposal not found");
        Self::bump_key_ttl(&env, &proposal_key);

        assert!(!proposal.executed, "already executed");

        let now = env.ledger().timestamp();

        // Must be past voting period.
        assert!(now > proposal.vote_end, "voting period not ended");

        // Must not have expired.
        assert!(now <= proposal.expires_at, "proposal expired");

        // Timelock must have elapsed.
        assert!(now >= proposal.execute_after, "timelock not elapsed");

        // Check quorum: total votes >= 10% of snapshot supply.
        let total_votes = proposal.votes_for + proposal.votes_against;
        let quorum_threshold = proposal.snapshot_total_supply * QUORUM_BPS / MAX_BPS;
        assert!(
            total_votes >= quorum_threshold,
            "quorum not met: votes={total_votes}, required={quorum_threshold}"
        );

        // Check majority.
        assert!(
            proposal.votes_for > proposal.votes_against,
            "proposal defeated: for={}, against={}",
            proposal.votes_for,
            proposal.votes_against
        );

        // Apply the fee change on the AMM.
        let amm_pool: Address = env.storage().instance().get(&DataKey::AmmPool).unwrap();
        AmmPoolClient::new(&env, &amm_pool).update_fee(&proposal.new_fee_bps);

        proposal.executed = true;
        env.storage().persistent().set(&proposal_key, &proposal);
        Self::bump_key_ttl(&env, &proposal_key);
    }

    /// Unlock voting power for a concluded proposal.
    pub fn unlock_vote(env: Env, voter: Address, proposal_id: u32) {
        voter.require_auth();
        let status = Self::proposal_status(env.clone(), proposal_id);
        assert!(
            status == ProposalStatus::Executed
                || status == ProposalStatus::Defeated
                || status == ProposalStatus::Expired,
            "proposal not concluded"
        );
        let lock_key = DataKey::LockedVote(proposal_id, voter.clone());
        let locked: i128 = env.storage().persistent().get(&lock_key).unwrap_or(0);
        assert!(locked > 0, "no locked vote");

        let lp_token: Address = env.storage().instance().get(&DataKey::LpToken).unwrap();
        LpTokenClient::new(&env, &lp_token).unlock(&voter, &locked);
        env.storage().persistent().remove(&lock_key);
    }

    /// Read a proposal by id.
    pub fn get_proposal(env: Env, proposal_id: u32) -> Proposal {
        let key = DataKey::Proposal(proposal_id);
        let proposal: Proposal = env
            .storage()
            .persistent()
            .get(&key)
            .expect("proposal not found");
        Self::bump_key_ttl(&env, &key);
        proposal
    }

    /// Derive the current status of a proposal.
    pub fn proposal_status(env: Env, proposal_id: u32) -> ProposalStatus {
        let proposal: Proposal = env
            .storage()
            .persistent()
            .get(&DataKey::Proposal(proposal_id))
            .expect("proposal not found");
        Self::bump_key_ttl(&env, &DataKey::Proposal(proposal_id));

        if proposal.executed {
            return ProposalStatus::Executed;
        }

        let now = env.ledger().timestamp();

        if now <= proposal.vote_end {
            return ProposalStatus::Active;
        }

        // Voting closed — check outcome.
        let total_votes = proposal.votes_for + proposal.votes_against;
        let quorum_threshold = proposal.snapshot_total_supply * QUORUM_BPS / MAX_BPS;
        let passed = total_votes >= quorum_threshold && proposal.votes_for > proposal.votes_against;

        if !passed {
            return ProposalStatus::Defeated;
        }

        if now > proposal.expires_at {
            return ProposalStatus::Expired;
        }

        if now >= proposal.execute_after {
            ProposalStatus::Queued
        } else {
            ProposalStatus::Pending
        }
    }

    fn bump_key_ttl(env: &Env, key: &DataKey) {
        env.storage()
            .persistent()
            .extend_ttl(key, MIN_PERSISTENT_TTL, PERSISTENT_TTL_BUMP_TO);
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use amm::AmmPool;
    use soroban_sdk::{
        testutils::{Address as _, Ledger},
        Env,
    };
    use token::LpToken;

    // ── Helpers ───────────────────────────────────────────────────────────────

    struct Suite {
        env: Env,
        gov_addr: Address,
        lp_addr: Address,
    }

    fn setup_suite(initial_fee_bps: i128) -> Suite {
        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();
        env.ledger().set_timestamp(1_000_000);

        let admin = Address::generate(&env);

        // Deploy LP token.
        let lp_addr = env.register_contract(None, LpToken);
        token::LpTokenClient::new(&env, &lp_addr).initialize(
            &admin, // temporary admin; will be replaced by AMM
            &soroban_sdk::String::from_str(&env, "AMM LP"),
            &soroban_sdk::String::from_str(&env, "ALP"),
            &7u32,
        );

        // Deploy token A and B.
        let ta = env.register_stellar_asset_contract_v2(admin.clone());
        let tb = env.register_stellar_asset_contract_v2(admin.clone());
        let ta_addr = ta.address();
        let tb_addr = tb.address();

        // Deploy AMM.
        let amm_addr = env.register_contract(None, AmmPool);
        amm::AmmPoolClient::new(&env, &amm_addr).initialize(
            &admin,
            &ta_addr,
            &tb_addr,
            &lp_addr,
            &initial_fee_bps,
            &admin,
            &0_i128,
        );

        // Re-initialise LP token with AMM as admin so it can mint/burn.
        // (In production the LP token is deployed with AMM as admin from the start.)
        // For tests we use mock_all_auths so the admin check passes regardless.

        // Deploy governance.
        let gov_addr = env.register_contract(None, Governance);
        GovernanceClient::new(&env, &gov_addr).initialize(&admin, &amm_addr, &lp_addr, &100_i128);
        token::LpTokenClient::new(&env, &lp_addr).set_locker(&gov_addr);

        Suite {
            env,
            gov_addr,
            lp_addr,
        }
    }

    /// Mint LP tokens directly to an address (simulates adding liquidity).
    fn mint_lp(suite: &Suite, to: &Address, amount: i128) {
        token::LpTokenClient::new(&suite.env, &suite.lp_addr).mint(to, &amount);
    }

    // ── Tests ─────────────────────────────────────────────────────────────────

    #[test]
    fn test_passing_proposal_executes() {
        let s = setup_suite(30);
        let gov = GovernanceClient::new(&s.env, &s.gov_addr);

        let lp1 = Address::generate(&s.env);
        let lp2 = Address::generate(&s.env);
        mint_lp(&s, &lp1, 600);
        mint_lp(&s, &lp2, 400);

        // Propose new fee of 50 bps.
        let pid = gov.propose(&lp1, &50_i128);
        assert_eq!(pid, 0);

        // Both vote for.
        gov.vote(&lp1, &pid, &true);
        gov.vote(&lp2, &pid, &true);

        // Advance past voting period + timelock.
        let proposal = gov.get_proposal(&pid);
        s.env.ledger().set_timestamp(proposal.execute_after + 1);

        gov.execute(&pid);

        let executed = gov.get_proposal(&pid);
        assert!(executed.executed);
        assert_eq!(gov.proposal_status(&pid), ProposalStatus::Executed);
    }

    #[test]
    fn test_failing_quorum_defeats_proposal() {
        let s = setup_suite(30);
        let gov = GovernanceClient::new(&s.env, &s.gov_addr);

        let lp1 = Address::generate(&s.env);
        let lp2 = Address::generate(&s.env);
        // lp1 gets 20, lp2 gets 980 — lp1 alone is < 10% quorum.
        mint_lp(&s, &lp1, 20);
        mint_lp(&s, &lp2, 980);

        let pid = gov.propose(&lp1, &50_i128);
        // Only lp1 votes (20 out of 1000 total = 2% < 10% quorum).
        gov.vote(&lp1, &pid, &true);

        let proposal = gov.get_proposal(&pid);
        s.env.ledger().set_timestamp(proposal.execute_after + 1);

        // Execute should panic — quorum not met.
        let result = gov.try_execute(&pid);
        assert!(result.is_err());
        assert_eq!(gov.proposal_status(&pid), ProposalStatus::Defeated);
    }

    #[test]
    fn test_expired_proposal_cannot_execute() {
        let s = setup_suite(30);
        let gov = GovernanceClient::new(&s.env, &s.gov_addr);

        let lp1 = Address::generate(&s.env);
        let lp2 = Address::generate(&s.env);
        mint_lp(&s, &lp1, 600);
        mint_lp(&s, &lp2, 400);

        let pid = gov.propose(&lp1, &50_i128);
        gov.vote(&lp1, &pid, &true);
        gov.vote(&lp2, &pid, &true);

        // Jump past the expiry window.
        let proposal = gov.get_proposal(&pid);
        s.env.ledger().set_timestamp(proposal.expires_at + 1);

        let result = gov.try_execute(&pid);
        assert!(result.is_err());
        assert_eq!(gov.proposal_status(&pid), ProposalStatus::Expired);
    }

    #[test]
    fn test_cannot_vote_twice() {
        let s = setup_suite(30);
        let gov = GovernanceClient::new(&s.env, &s.gov_addr);

        let lp1 = Address::generate(&s.env);
        mint_lp(&s, &lp1, 500);
        mint_lp(&s, &Address::generate(&s.env), 500);

        let pid = gov.propose(&lp1, &50_i128);
        gov.vote(&lp1, &pid, &true);

        let result = gov.try_vote(&lp1, &pid, &false);
        assert!(result.is_err());
    }

    #[test]
    fn test_cannot_vote_after_period_ends() {
        let s = setup_suite(30);
        let gov = GovernanceClient::new(&s.env, &s.gov_addr);

        let lp1 = Address::generate(&s.env);
        mint_lp(&s, &lp1, 500);
        mint_lp(&s, &Address::generate(&s.env), 500);

        let pid = gov.propose(&lp1, &50_i128);
        let proposal = gov.get_proposal(&pid);
        s.env.ledger().set_timestamp(proposal.vote_end + 1);

        let result = gov.try_vote(&lp1, &pid, &true);
        assert!(result.is_err());
    }

    #[test]
    fn test_cannot_execute_before_timelock() {
        let s = setup_suite(30);
        let gov = GovernanceClient::new(&s.env, &s.gov_addr);

        let lp1 = Address::generate(&s.env);
        let lp2 = Address::generate(&s.env);
        mint_lp(&s, &lp1, 600);
        mint_lp(&s, &lp2, 400);

        let pid = gov.propose(&lp1, &50_i128);
        gov.vote(&lp1, &pid, &true);
        gov.vote(&lp2, &pid, &true);

        // Jump past voting but NOT past timelock.
        let proposal = gov.get_proposal(&pid);
        s.env.ledger().set_timestamp(proposal.vote_end + 1);

        let result = gov.try_execute(&pid);
        assert!(result.is_err());
        assert_eq!(gov.proposal_status(&pid), ProposalStatus::Pending);
    }

    #[test]
    fn test_proposal_status_active_then_queued() {
        let s = setup_suite(30);
        let gov = GovernanceClient::new(&s.env, &s.gov_addr);

        let lp1 = Address::generate(&s.env);
        let lp2 = Address::generate(&s.env);
        mint_lp(&s, &lp1, 600);
        mint_lp(&s, &lp2, 400);

        let pid = gov.propose(&lp1, &50_i128);
        assert_eq!(gov.proposal_status(&pid), ProposalStatus::Active);

        gov.vote(&lp1, &pid, &true);
        gov.vote(&lp2, &pid, &true);

        let proposal = gov.get_proposal(&pid);
        s.env.ledger().set_timestamp(proposal.execute_after);
        assert_eq!(gov.proposal_status(&pid), ProposalStatus::Queued);
    }

    #[test]
    fn test_no_lp_tokens_cannot_propose() {
        let s = setup_suite(30);
        let gov = GovernanceClient::new(&s.env, &s.gov_addr);

        let nobody = Address::generate(&s.env);
        // Give someone else tokens so total_supply > 0.
        mint_lp(&s, &Address::generate(&s.env), 1000);

        let result = gov.try_propose(&nobody, &50_i128);
        assert!(result.is_err());
    }

    #[test]
    fn test_invalid_fee_bps_rejected() {
        let s = setup_suite(30);
        let gov = GovernanceClient::new(&s.env, &s.gov_addr);

        let lp1 = Address::generate(&s.env);
        mint_lp(&s, &lp1, 1000);

        let result = gov.try_propose(&lp1, &10_001_i128);
        assert!(result.is_err());
    }

    #[test]
    fn test_below_min_stake_cannot_propose_but_exact_min_can() {
        let s = setup_suite(30);
        let gov = GovernanceClient::new(&s.env, &s.gov_addr);
        let low = Address::generate(&s.env);
        let exact = Address::generate(&s.env);
        let whale = Address::generate(&s.env);

        mint_lp(&s, &low, 9);
        mint_lp(&s, &exact, 10);
        mint_lp(&s, &whale, 981);

        assert!(gov.try_propose(&low, &40_i128).is_err());
        let pid = gov.propose(&exact, &40_i128);
        assert_eq!(pid, 0);
    }

    #[test]
    fn test_vote_locks_balance_until_proposal_concludes() {
        let s = setup_suite(30);
        let gov = GovernanceClient::new(&s.env, &s.gov_addr);
        let lp_client = token::LpTokenClient::new(&s.env, &s.lp_addr);

        let lp1 = Address::generate(&s.env);
        let lp2 = Address::generate(&s.env);
        let receiver = Address::generate(&s.env);
        mint_lp(&s, &lp1, 600);
        mint_lp(&s, &lp2, 400);

        let pid = gov.propose(&lp1, &50_i128);
        gov.vote(&lp1, &pid, &true);
        assert_eq!(lp_client.locked_balance(&lp1), 600);

        // Simulated flash-loan pattern fails: voter cannot move locked weight.
        let transfer_result = lp_client.try_transfer(&lp1, &receiver, &600_i128);
        assert!(transfer_result.is_err());

        gov.vote(&lp2, &pid, &true);
        let proposal = gov.get_proposal(&pid);
        s.env.ledger().set_timestamp(proposal.execute_after + 1);
        gov.execute(&pid);

        gov.unlock_vote(&lp1, &pid);
        assert_eq!(lp_client.locked_balance(&lp1), 0);
        lp_client.transfer(&lp1, &receiver, &600_i128);
    }
}
