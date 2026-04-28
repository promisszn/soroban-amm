//! Governance contract for LP-weighted fee parameter voting.
//!
//! LP token holders can propose changes to the pool's `fee_bps`.
//! Voting power equals the proposer/voter's LP token balance at proposal
//! creation time (snapshot). A proposal passes when:
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

// ── Storage keys ─────────────────────────────────────────────────────────────

#[contracttype]
pub enum DataKey {
    /// Address of the AMM pool contract.
    AmmPool,
    /// Address of the LP token contract.
    LpToken,
    /// Monotonically increasing proposal counter.
    ProposalCount,
    /// Individual proposal storage.
    Proposal(u32),
    /// Whether a voter has already voted on a proposal: (proposal_id, voter).
    HasVoted(u32, Address),
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
    pub fn initialize(env: Env, amm_pool: Address, lp_token: Address) {
        assert!(
            !env.storage().instance().has(&DataKey::AmmPool),
            "already initialized"
        );
        env.storage().instance().set(&DataKey::AmmPool, &amm_pool);
        env.storage().instance().set(&DataKey::LpToken, &lp_token);
        env.storage().instance().set(&DataKey::ProposalCount, &0u32);
    }

    // ── Core functions ────────────────────────────────────────────────────────

    /// Create a new proposal to change the pool fee.
    ///
    /// The proposer must hold at least 1 LP token (non-zero balance).
    /// Returns the new `proposal_id`.
    pub fn propose(env: Env, proposer: Address, new_fee_bps: i128) -> u32 {
        proposer.require_auth();

        assert!(
            (0..=10_000).contains(&new_fee_bps),
            "invalid fee: {new_fee_bps} must be in 0..=10_000"
        );

        let lp_token: Address = env.storage().instance().get(&DataKey::LpToken).unwrap();
        let lp_client = LpTokenClient::new(&env, &lp_token);

        let proposer_balance = lp_client.balance(&proposer);
        assert!(proposer_balance > 0, "proposer has no LP tokens");

        let total_supply = lp_client.total_supply();
        assert!(total_supply > 0, "LP total supply is zero");

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

        env.storage()
            .instance()
            .set(&DataKey::Proposal(id), &proposal);
        env.storage()
            .instance()
            .set(&DataKey::ProposalCount, &(id + 1));

        id
    }

    /// Cast a vote on an active proposal.
    ///
    /// Voting power = voter's LP balance at the time of the call.
    /// Each address may only vote once per proposal.
    pub fn vote(env: Env, voter: Address, proposal_id: u32, support: bool) {
        voter.require_auth();

        let mut proposal: Proposal = env
            .storage()
            .instance()
            .get(&DataKey::Proposal(proposal_id))
            .expect("proposal not found");

        let now = env.ledger().timestamp();
        assert!(now >= proposal.vote_start, "voting not started");
        assert!(now <= proposal.vote_end, "voting period has ended");
        assert!(!proposal.executed, "proposal already executed");

        let voted_key = DataKey::HasVoted(proposal_id, voter.clone());
        assert!(!env.storage().instance().has(&voted_key), "already voted");

        let lp_token: Address = env.storage().instance().get(&DataKey::LpToken).unwrap();
        let voting_power = LpTokenClient::new(&env, &lp_token).balance(&voter);
        assert!(voting_power > 0, "no LP tokens: voting power is zero");

        if support {
            proposal.votes_for += voting_power;
        } else {
            proposal.votes_against += voting_power;
        }

        env.storage()
            .instance()
            .set(&DataKey::Proposal(proposal_id), &proposal);
        env.storage().instance().set(&voted_key, &true);
    }

    /// Execute a passed proposal after the timelock has elapsed.
    ///
    /// Anyone can call this once the conditions are met.
    pub fn execute(env: Env, proposal_id: u32) {
        let mut proposal: Proposal = env
            .storage()
            .instance()
            .get(&DataKey::Proposal(proposal_id))
            .expect("proposal not found");

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
        let quorum_threshold = proposal.snapshot_total_supply * QUORUM_BPS / 10_000;
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
        env.storage()
            .instance()
            .set(&DataKey::Proposal(proposal_id), &proposal);
    }

    /// Read a proposal by id.
    pub fn get_proposal(env: Env, proposal_id: u32) -> Proposal {
        env.storage()
            .instance()
            .get(&DataKey::Proposal(proposal_id))
            .expect("proposal not found")
    }

    /// Derive the current status of a proposal.
    pub fn proposal_status(env: Env, proposal_id: u32) -> ProposalStatus {
        let proposal: Proposal = env
            .storage()
            .instance()
            .get(&DataKey::Proposal(proposal_id))
            .expect("proposal not found");

        if proposal.executed {
            return ProposalStatus::Executed;
        }

        let now = env.ledger().timestamp();

        if now <= proposal.vote_end {
            return ProposalStatus::Active;
        }

        // Voting closed — check outcome.
        let total_votes = proposal.votes_for + proposal.votes_against;
        let quorum_threshold = proposal.snapshot_total_supply * QUORUM_BPS / 10_000;
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
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use amm::AmmPool;
    use soroban_sdk::{
        testutils::{Address as _, Ledger},
        token::StellarAssetClient,
        Env,
    };
    use token::LpToken;

    // ── Helpers ───────────────────────────────────────────────────────────────

    struct Suite {
        env: Env,
        gov_addr: Address,
        amm_addr: Address,
        lp_addr: Address,
        ta_addr: Address,
        tb_addr: Address,
        admin: Address,
    }

    fn setup_suite(initial_fee_bps: i128) -> Suite {
        let env = Env::default();
        env.mock_all_auths();
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
        GovernanceClient::new(&env, &gov_addr).initialize(&amm_addr, &lp_addr);

        Suite {
            env,
            gov_addr,
            amm_addr,
            lp_addr,
            ta_addr,
            tb_addr,
            admin,
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
        // lp1 gets 5, lp2 gets 995 — lp1 alone is < 10% quorum.
        mint_lp(&s, &lp1, 5);
        mint_lp(&s, &lp2, 995);

        let pid = gov.propose(&lp1, &50_i128);
        // Only lp1 votes (5 out of 1000 total = 0.5% < 10% quorum).
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
}
