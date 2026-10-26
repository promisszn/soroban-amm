//! Proposal lifecycle regression and guard coverage tests (#1187).
//!
//! Verifies all 12 critical proposal lifecycle guards across vote, execute,
//! cancel_proposal, and veto, plus edge boundary conditions.

extern crate std;

use super::*;
use amm::{AmmPool, AmmPoolClient};
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    Address, Env,
};
use token::LpToken;

struct TestFixture {
    env: Env,
    gov_addr: Address,
    lp_addr: Address,
    amm_addr: Address,
    admin: Address,
    multisig: Address,
}

impl TestFixture {
    fn new(initial_fee_bps: i128) -> Self {
        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();
        env.budget().reset_unlimited();
        env.ledger().set_timestamp(1_000_000);

        let admin = Address::generate(&env);
        let multisig = Address::generate(&env);

        let lp_addr = env.register_contract(None, LpToken);
        token::LpTokenClient::new(&env, &lp_addr).initialize(
            &admin,
            &soroban_sdk::String::from_str(&env, "AMM LP"),
            &soroban_sdk::String::from_str(&env, "ALP"),
            &7u32,
        );

        let ta = env.register_stellar_asset_contract_v2(admin.clone());
        let tb = env.register_stellar_asset_contract_v2(admin.clone());

        let gov_addr = env.register_contract(None, Governance);
        let amm_addr = env.register_contract(None, AmmPool);

        amm::AmmPoolClient::new(&env, &amm_addr).initialize(
            &gov_addr,
            &ta.address(),
            &tb.address(),
            &lp_addr,
            &initial_fee_bps,
            &admin,
            &0_i128,
        );

        let gov = GovernanceClient::new(&env, &gov_addr);
        gov.initialize(
            &admin,
            &amm_addr,
            &lp_addr,
            &(7 * 24 * 60 * 60_u64), // 7 days voting
            &(2 * 24 * 60 * 60_u64), // 2 days timelock
            &1_000_i128,             // 10% quorum
            &100_i128,               // 1% min proposer stake
        );
        gov.set_veto_multisig(&multisig);

        token::LpTokenClient::new(&env, &lp_addr).set_locker(&gov_addr);

        Self {
            env,
            gov_addr,
            lp_addr,
            amm_addr,
            admin,
            multisig,
        }
    }

    fn gov(&self) -> GovernanceClient<'_> {
        GovernanceClient::new(&self.env, &self.gov_addr)
    }

    fn amm(&self) -> AmmPoolClient<'_> {
        AmmPoolClient::new(&self.env, &self.amm_addr)
    }

    fn mint_lp(&self, to: &Address, amount: i128) {
        token::LpTokenClient::new(&self.env, &self.lp_addr).mint(to, &amount);
    }
}

// =========================================================================
// 1. vote: VotingNotStarted (line 1006)
// =========================================================================

#[test]
fn test_vote_rejected_before_voting_starts() {
    let f = TestFixture::new(30);
    let gov = f.gov();

    let lp1 = Address::generate(&f.env);
    f.mint_lp(&lp1, 500);
    f.mint_lp(&Address::generate(&f.env), 500);

    let pid = gov.propose(&lp1, &ProposalKind::UpdateFee(50));
    let proposal = gov.get_proposal(&pid);

    f.env
        .ledger()
        .set_timestamp(proposal.vote_start.saturating_sub(10));
    let res = gov.try_vote(&lp1, &pid, &Vote::For);
    assert_eq!(res, Err(Ok(GovernanceError::VotingNotStarted)));
}

// =========================================================================
// 2. vote: AlreadyExecuted (line 1012)
// =========================================================================

#[test]
fn test_vote_rejected_after_proposal_executed() {
    let f = TestFixture::new(30);
    let gov = f.gov();

    let lp1 = Address::generate(&f.env);
    let lp2 = Address::generate(&f.env);
    let late_voter = Address::generate(&f.env);
    f.mint_lp(&lp1, 600);
    f.mint_lp(&lp2, 400);
    f.mint_lp(&late_voter, 100);

    let pid = gov.propose(&lp1, &ProposalKind::UpdateFee(50));
    gov.vote(&lp1, &pid, &Vote::For);
    gov.vote(&lp2, &pid, &Vote::For);

    let p = gov.get_proposal(&pid);
    f.env.ledger().set_timestamp(p.execute_after + 1);
    gov.execute(&pid);

    let res = gov.try_vote(&late_voter, &pid, &Vote::For);
    assert_eq!(res, Err(Ok(GovernanceError::AlreadyExecuted)));
}

// =========================================================================
// 3. vote: ProposalVetoed (line 1018)
// =========================================================================

#[test]
fn test_vote_rejected_after_proposal_vetoed() {
    let f = TestFixture::new(30);
    let gov = f.gov();

    let lp1 = Address::generate(&f.env);
    let lp2 = Address::generate(&f.env);
    let voter = Address::generate(&f.env);
    f.mint_lp(&lp1, 600);
    f.mint_lp(&lp2, 400);
    f.mint_lp(&voter, 100);

    let pid = gov.propose(&lp1, &ProposalKind::UpdateFee(50));
    gov.vote(&lp1, &pid, &Vote::For);
    gov.vote(&lp2, &pid, &Vote::For);

    let p = gov.get_proposal(&pid);
    f.env.ledger().set_timestamp(p.vote_end + 1);
    gov.veto(&pid);

    let res = gov.try_vote(&voter, &pid, &Vote::For);
    assert_eq!(res, Err(Ok(GovernanceError::ProposalVetoed)));
}

// =========================================================================
// 4. vote: AlreadyVoted (double voting, line 1023)
// =========================================================================

#[test]
fn test_vote_double_voting_rejected() {
    let f = TestFixture::new(30);
    let gov = f.gov();

    let lp1 = Address::generate(&f.env);
    f.mint_lp(&lp1, 500);
    f.mint_lp(&Address::generate(&f.env), 500);

    let pid = gov.propose(&lp1, &ProposalKind::UpdateFee(50));
    gov.vote(&lp1, &pid, &Vote::For);

    let res = gov.try_vote(&lp1, &pid, &Vote::For);
    assert_eq!(res, Err(Ok(GovernanceError::AlreadyVoted)));

    let res_against = gov.try_vote(&lp1, &pid, &Vote::Against);
    assert_eq!(res_against, Err(Ok(GovernanceError::AlreadyVoted)));
}

// =========================================================================
// 5. execute: AlreadyExecuted (re-execution, line 1095)
// =========================================================================

#[test]
fn test_execute_already_executed_rejected() {
    let f = TestFixture::new(30);
    let gov = f.gov();

    let lp1 = Address::generate(&f.env);
    let lp2 = Address::generate(&f.env);
    f.mint_lp(&lp1, 600);
    f.mint_lp(&lp2, 400);

    let pid = gov.propose(&lp1, &ProposalKind::UpdateFee(65));
    gov.vote(&lp1, &pid, &Vote::For);
    gov.vote(&lp2, &pid, &Vote::For);

    let p = gov.get_proposal(&pid);
    f.env.ledger().set_timestamp(p.execute_after + 1);
    gov.execute(&pid);

    assert_eq!(f.amm().get_fee_bps(), 65);

    let res = gov.try_execute(&pid);
    assert_eq!(res, Err(Ok(GovernanceError::AlreadyExecuted)));
    // Invariant: Pool fee unchanged on repeated execution failure
    assert_eq!(f.amm().get_fee_bps(), 65);
}

// =========================================================================
// 6. execute: VotingPeriodActive (line 1107)
// =========================================================================

#[test]
fn test_execute_rejected_while_voting_active() {
    let f = TestFixture::new(30);
    let gov = f.gov();

    let lp1 = Address::generate(&f.env);
    f.mint_lp(&lp1, 600);
    f.mint_lp(&Address::generate(&f.env), 400);

    let pid = gov.propose(&lp1, &ProposalKind::UpdateFee(50));
    gov.vote(&lp1, &pid, &Vote::For);

    let p = gov.get_proposal(&pid);
    f.env.ledger().set_timestamp(p.vote_start + 100);

    let res = gov.try_execute(&pid);
    assert_eq!(res, Err(Ok(GovernanceError::VotingPeriodActive)));
    assert_eq!(f.amm().get_fee_bps(), 30);
}

// =========================================================================
// 7. execute: ProposalDefeated (line 1128)
// =========================================================================

#[test]
fn test_execute_rejected_when_proposal_defeated_more_against() {
    let f = TestFixture::new(30);
    let gov = f.gov();

    let lp1 = Address::generate(&f.env);
    let lp2 = Address::generate(&f.env);
    f.mint_lp(&lp1, 300);
    f.mint_lp(&lp2, 500);
    f.mint_lp(&Address::generate(&f.env), 200);

    let pid = gov.propose(&lp1, &ProposalKind::UpdateFee(75));
    gov.vote(&lp1, &pid, &Vote::For);     // 300 for
    gov.vote(&lp2, &pid, &Vote::Against); // 500 against

    let p = gov.get_proposal(&pid);
    f.env.ledger().set_timestamp(p.execute_after + 1);

    let res = gov.try_execute(&pid);
    assert_eq!(res, Err(Ok(GovernanceError::ProposalDefeated)));
    // Invariant: AMM pool fee remains unchanged
    assert_eq!(f.amm().get_fee_bps(), 30);
}

#[test]
fn test_execute_rejected_when_votes_tied() {
    let f = TestFixture::new(30);
    let gov = f.gov();

    let lp1 = Address::generate(&f.env);
    let lp2 = Address::generate(&f.env);
    f.mint_lp(&lp1, 400);
    f.mint_lp(&lp2, 400);
    f.mint_lp(&Address::generate(&f.env), 200);

    let pid = gov.propose(&lp1, &ProposalKind::UpdateFee(80));
    gov.vote(&lp1, &pid, &Vote::For);     // 400 for
    gov.vote(&lp2, &pid, &Vote::Against); // 400 against (equal)

    let p = gov.get_proposal(&pid);
    f.env.ledger().set_timestamp(p.execute_after + 1);

    let res = gov.try_execute(&pid);
    assert_eq!(res, Err(Ok(GovernanceError::ProposalDefeated)));
    assert_eq!(f.amm().get_fee_bps(), 30);
}

// =========================================================================
// 8. cancel_proposal: AlreadyExecuted (line 1278)
// =========================================================================

#[test]
fn test_cancel_proposal_rejected_after_execution() {
    let f = TestFixture::new(30);
    let gov = f.gov();

    let lp1 = Address::generate(&f.env);
    let lp2 = Address::generate(&f.env);
    f.mint_lp(&lp1, 600);
    f.mint_lp(&lp2, 400);

    let pid = gov.propose(&lp1, &ProposalKind::UpdateFee(50));
    gov.vote(&lp1, &pid, &Vote::For);
    gov.vote(&lp2, &pid, &Vote::For);

    let p = gov.get_proposal(&pid);
    f.env.ledger().set_timestamp(p.execute_after + 1);
    gov.execute(&pid);

    let res = gov.try_cancel_proposal(&pid, &lp1);
    assert_eq!(res, Err(Ok(GovernanceError::AlreadyExecuted)));
}

// =========================================================================
// 9. veto: AlreadyExecuted (line 1477)
// =========================================================================

#[test]
fn test_veto_rejected_after_proposal_executed() {
    let f = TestFixture::new(30);
    let gov = f.gov();

    let lp1 = Address::generate(&f.env);
    let lp2 = Address::generate(&f.env);
    f.mint_lp(&lp1, 600);
    f.mint_lp(&lp2, 400);

    let pid = gov.propose(&lp1, &ProposalKind::UpdateFee(50));
    gov.vote(&lp1, &pid, &Vote::For);
    gov.vote(&lp2, &pid, &Vote::For);

    let p = gov.get_proposal(&pid);
    f.env.ledger().set_timestamp(p.execute_after + 1);
    gov.execute(&pid);

    let res = gov.try_veto(&pid);
    assert_eq!(res, Err(Ok(GovernanceError::AlreadyExecuted)));
}

// =========================================================================
// 10. veto: ProposalVetoed (line 1483)
// =========================================================================

#[test]
fn test_veto_rejected_when_already_vetoed() {
    let f = TestFixture::new(30);
    let gov = f.gov();

    let lp1 = Address::generate(&f.env);
    let lp2 = Address::generate(&f.env);
    f.mint_lp(&lp1, 600);
    f.mint_lp(&lp2, 400);

    let pid = gov.propose(&lp1, &ProposalKind::UpdateFee(50));
    gov.vote(&lp1, &pid, &Vote::For);
    gov.vote(&lp2, &pid, &Vote::For);

    let p = gov.get_proposal(&pid);
    f.env.ledger().set_timestamp(p.vote_end + 1);
    gov.veto(&pid);

    let res = gov.try_veto(&pid);
    assert_eq!(res, Err(Ok(GovernanceError::ProposalVetoed)));
}

// =========================================================================
// 11. veto: VotingPeriodActive (line 1488)
// =========================================================================

#[test]
fn test_veto_rejected_while_voting_period_active() {
    let f = TestFixture::new(30);
    let gov = f.gov();

    let lp1 = Address::generate(&f.env);
    f.mint_lp(&lp1, 600);
    f.mint_lp(&Address::generate(&f.env), 400);

    let pid = gov.propose(&lp1, &ProposalKind::UpdateFee(50));
    gov.vote(&lp1, &pid, &Vote::For);

    let p = gov.get_proposal(&pid);
    f.env.ledger().set_timestamp(p.vote_start + 100);

    let res = gov.try_veto(&pid);
    assert_eq!(res, Err(Ok(GovernanceError::VotingPeriodActive)));
}

// =========================================================================
// 12. veto: ProposalDefeated (line 1502)
// =========================================================================

#[test]
fn test_veto_rejected_on_defeated_proposal() {
    let f = TestFixture::new(30);
    let gov = f.gov();

    let lp1 = Address::generate(&f.env);
    let lp2 = Address::generate(&f.env);
    f.mint_lp(&lp1, 300);
    f.mint_lp(&lp2, 500);
    f.mint_lp(&Address::generate(&f.env), 200);

    let pid = gov.propose(&lp1, &ProposalKind::UpdateFee(50));
    gov.vote(&lp1, &pid, &Vote::For);
    gov.vote(&lp2, &pid, &Vote::Against);

    let p = gov.get_proposal(&pid);
    f.env.ledger().set_timestamp(p.vote_end + 1);

    let res = gov.try_veto(&pid);
    assert_eq!(res, Err(Ok(GovernanceError::ProposalDefeated)));
}

// =========================================================================
// Boundary Conditions & Additional Edge Cases (#1187 requirements)
// =========================================================================

#[test]
fn test_boundary_vote_end_exact_second() {
    let f = TestFixture::new(30);
    let gov = f.gov();

    let lp1 = Address::generate(&f.env);
    let lp2 = Address::generate(&f.env);
    f.mint_lp(&lp1, 600);
    f.mint_lp(&lp2, 400);

    let pid = gov.propose(&lp1, &ProposalKind::UpdateFee(50));

    let p = gov.get_proposal(&pid);
    // At exactly vote_end, voting is still allowed
    f.env.ledger().set_timestamp(p.vote_end);
    let vote_res = gov.try_vote(&lp1, &pid, &Vote::For);
    assert!(vote_res.is_ok());

    // But execute is rejected (now <= proposal.vote_end)
    let exec_res = gov.try_execute(&pid);
    assert_eq!(exec_res, Err(Ok(GovernanceError::VotingPeriodActive)));

    // And veto is rejected (now <= proposal.vote_end)
    let veto_res = gov.try_veto(&pid);
    assert_eq!(veto_res, Err(Ok(GovernanceError::VotingPeriodActive)));
}

#[test]
fn test_boundary_vote_end_plus_one_second() {
    let f = TestFixture::new(30);
    let gov = f.gov();

    let lp1 = Address::generate(&f.env);
    let lp2 = Address::generate(&f.env);
    f.mint_lp(&lp1, 600);
    f.mint_lp(&lp2, 400);

    let pid = gov.propose(&lp1, &ProposalKind::UpdateFee(50));
    gov.vote(&lp1, &pid, &Vote::For);

    let p = gov.get_proposal(&pid);
    // At vote_end + 1, voting period has ended
    f.env.ledger().set_timestamp(p.vote_end + 1);
    let vote_res = gov.try_vote(&lp2, &pid, &Vote::For);
    assert_eq!(vote_res, Err(Ok(GovernanceError::VotingPeriodEnded)));
}

#[test]
fn test_boundary_execute_after_exact_second() {
    let f = TestFixture::new(30);
    let gov = f.gov();

    let lp1 = Address::generate(&f.env);
    let lp2 = Address::generate(&f.env);
    f.mint_lp(&lp1, 600);
    f.mint_lp(&lp2, 400);

    let pid = gov.propose(&lp1, &ProposalKind::UpdateFee(45));
    gov.vote(&lp1, &pid, &Vote::For);
    gov.vote(&lp2, &pid, &Vote::For);

    let p = gov.get_proposal(&pid);
    // 1 second before timelock: fails
    f.env.ledger().set_timestamp(p.execute_after - 1);
    let early = gov.try_execute(&pid);
    assert_eq!(early, Err(Ok(GovernanceError::TimelockNotElapsed)));

    // Exactly at execute_after: succeeds!
    f.env.ledger().set_timestamp(p.execute_after);
    let exact = gov.try_execute(&pid);
    assert!(exact.is_ok());
    assert_eq!(f.amm().get_fee_bps(), 45);
}

#[test]
fn test_boundary_expires_at_exact_second() {
    let f = TestFixture::new(30);
    let gov = f.gov();

    let lp1 = Address::generate(&f.env);
    let lp2 = Address::generate(&f.env);
    f.mint_lp(&lp1, 600);
    f.mint_lp(&lp2, 400);

    let pid = gov.propose(&lp1, &ProposalKind::UpdateFee(55));
    gov.vote(&lp1, &pid, &Vote::For);
    gov.vote(&lp2, &pid, &Vote::For);

    let p = gov.get_proposal(&pid);
    // Exactly at expires_at: succeeds
    f.env.ledger().set_timestamp(p.expires_at);
    let exact = gov.try_execute(&pid);
    assert!(exact.is_ok());
    assert_eq!(f.amm().get_fee_bps(), 55);
}

#[test]
fn test_boundary_expires_at_plus_one_second() {
    let f = TestFixture::new(30);
    let gov = f.gov();

    let lp1 = Address::generate(&f.env);
    let lp2 = Address::generate(&f.env);
    f.mint_lp(&lp1, 600);
    f.mint_lp(&lp2, 400);

    let pid = gov.propose(&lp1, &ProposalKind::UpdateFee(55));
    gov.vote(&lp1, &pid, &Vote::For);
    gov.vote(&lp2, &pid, &Vote::For);

    let p = gov.get_proposal(&pid);
    // 1 second past expires_at: fails
    f.env.ledger().set_timestamp(p.expires_at + 1);
    let expired = gov.try_execute(&pid);
    assert_eq!(expired, Err(Ok(GovernanceError::ProposalExpired)));
}

#[test]
fn test_boundary_passing_by_one_vote() {
    let f = TestFixture::new(30);
    let gov = f.gov();

    let lp1 = Address::generate(&f.env);
    let lp2 = Address::generate(&f.env);
    // Total 1000 supply, quorum 10% (100)
    // votes_for = 301, votes_against = 300
    f.mint_lp(&lp1, 301);
    f.mint_lp(&lp2, 300);
    f.mint_lp(&Address::generate(&f.env), 399);

    let pid = gov.propose(&lp1, &ProposalKind::UpdateFee(70));
    gov.vote(&lp1, &pid, &Vote::For);
    gov.vote(&lp2, &pid, &Vote::Against);

    let p = gov.get_proposal(&pid);
    f.env.ledger().set_timestamp(p.execute_after + 1);

    let res = gov.try_execute(&pid);
    assert!(res.is_ok());
    assert_eq!(f.amm().get_fee_bps(), 70);
}

#[test]
fn test_cancel_proposal_rejected_when_voting_ended() {
    let f = TestFixture::new(30);
    let gov = f.gov();

    let lp1 = Address::generate(&f.env);
    f.mint_lp(&lp1, 600);
    f.mint_lp(&Address::generate(&f.env), 400);

    let pid = gov.propose(&lp1, &ProposalKind::UpdateFee(50));
    let p = gov.get_proposal(&pid);
    f.env.ledger().set_timestamp(p.vote_end + 1);

    let res = gov.try_cancel_proposal(&pid, &lp1);
    assert_eq!(res, Err(Ok(GovernanceError::VotingPeriodEnded)));
}

#[test]
fn test_cancel_proposal_rejected_when_already_cancelled() {
    let f = TestFixture::new(30);
    let gov = f.gov();

    let lp1 = Address::generate(&f.env);
    f.mint_lp(&lp1, 600);
    f.mint_lp(&Address::generate(&f.env), 400);

    let pid = gov.propose(&lp1, &ProposalKind::UpdateFee(50));
    let cancel1 = gov.try_cancel_proposal(&pid, &lp1);
    assert!(cancel1.is_ok());

    let cancel2 = gov.try_cancel_proposal(&pid, &lp1);
    assert_eq!(cancel2, Err(Ok(GovernanceError::ProposalCancelled)));
}

#[test]
fn test_veto_rejected_after_veto_window_expired() {
    let f = TestFixture::new(30);
    let gov = f.gov();

    let lp1 = Address::generate(&f.env);
    let lp2 = Address::generate(&f.env);
    f.mint_lp(&lp1, 600);
    f.mint_lp(&lp2, 400);

    let pid = gov.propose(&lp1, &ProposalKind::UpdateFee(50));
    gov.vote(&lp1, &pid, &Vote::For);
    gov.vote(&lp2, &pid, &Vote::For);

    let p = gov.get_proposal(&pid);
    // Veto window is 24 hours (86,400s) past vote_end
    f.env
        .ledger()
        .set_timestamp(p.vote_end + 86_400 + 10);

    let res = gov.try_veto(&pid);
    assert_eq!(res, Err(Ok(GovernanceError::VetoWindowExpired)));
}
