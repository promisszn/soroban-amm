use super::*;
use amm::{AmmPool, AmmPoolClient};
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    Env,
};
use token::{LpToken, LpTokenClient};

struct Suite {
    env: Env,
    gov_addr: Address,
    lp_addr: Address,
    amm_addr: Address,
}

impl Suite {
    fn gov(&self) -> GovernanceClient<'_> {
        GovernanceClient::new(&self.env, &self.gov_addr)
    }

    fn amm(&self) -> AmmPoolClient<'_> {
        AmmPoolClient::new(&self.env, &self.amm_addr)
    }
}

fn setup() -> Suite {
    let env = Env::default();
    env.mock_all_auths_allowing_non_root_auth();
    env.ledger().set_timestamp(1_000_000);
    let admin = Address::generate(&env);

    let lp_addr = env.register_contract(None, LpToken);
    LpTokenClient::new(&env, &lp_addr).initialize(
        &admin,
        &soroban_sdk::String::from_str(&env, "AMM LP"),
        &soroban_sdk::String::from_str(&env, "ALP"),
        &7_u32,
    );
    let token_a = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let token_b = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let gov_addr = env.register_contract(None, Governance);
    let amm_addr = env.register_contract(None, AmmPool);
    AmmPoolClient::new(&env, &amm_addr).initialize(
        &gov_addr, &token_a, &token_b, &lp_addr, &30_i128, &admin, &0_i128,
    );
    GovernanceClient::new(&env, &gov_addr).initialize(
        &admin,
        &amm_addr,
        &lp_addr,
        &(7 * 24 * 60 * 60_u64),
        &(2 * 24 * 60 * 60_u64),
        &1_000_i128,
        &100_i128,
    );
    LpTokenClient::new(&env, &lp_addr).set_locker(&gov_addr);

    Suite {
        env,
        gov_addr,
        lp_addr,
        amm_addr,
    }
}

fn mint_lp(suite: &Suite, to: &Address, amount: i128) {
    LpTokenClient::new(&suite.env, &suite.lp_addr).mint(to, &amount);
}

fn proposal(suite: &Suite, proposer: &Address) -> u32 {
    suite.gov().propose(proposer, &ProposalKind::UpdateFee(50))
}

fn passed_proposal(suite: &Suite) -> (u32, Address, Address) {
    let for_voter = Address::generate(&suite.env);
    let second_voter = Address::generate(&suite.env);
    mint_lp(suite, &for_voter, 600);
    mint_lp(suite, &second_voter, 400);
    let id = proposal(suite, &for_voter);
    suite.gov().vote(&for_voter, &id, &Vote::For);
    suite.gov().vote(&second_voter, &id, &Vote::For);
    (id, for_voter, second_voter)
}

fn execute_at(suite: &Suite, id: u32) {
    let proposal = suite.gov().get_proposal(&id);
    suite.env.ledger().set_timestamp(proposal.execute_after);
    suite.gov().execute(&id);
}

fn assert_tallies_unchanged(before: &Proposal, after: &Proposal) {
    assert_eq!(after.votes_for, before.votes_for);
    assert_eq!(after.votes_against, before.votes_against);
    assert_eq!(after.votes_abstain, before.votes_abstain);
    assert_eq!(after.executed, before.executed);
    assert_eq!(after.cancelled, before.cancelled);
    assert_eq!(after.vetoed, before.vetoed);
}

fn proposal_with_tally(suite: &Suite, votes_for: i128, votes_against: i128) -> u32 {
    let for_voter = Address::generate(&suite.env);
    let against_voter = Address::generate(&suite.env);
    mint_lp(suite, &for_voter, votes_for);
    mint_lp(suite, &against_voter, votes_against);
    let id = proposal(suite, &for_voter);
    suite.gov().vote(&for_voter, &id, &Vote::For);
    suite.gov().vote(&against_voter, &id, &Vote::Against);
    let proposal = suite.gov().get_proposal(&id);
    suite.env.ledger().set_timestamp(proposal.execute_after);
    id
}

#[test]
fn test_double_vote_returns_already_voted_without_changing_tally_or_lock() {
    let suite = setup();
    let voter = Address::generate(&suite.env);
    let other = Address::generate(&suite.env);
    mint_lp(&suite, &voter, 600);
    mint_lp(&suite, &other, 400);
    let id = proposal(&suite, &voter);
    suite.gov().vote(&voter, &id, &Vote::For);
    let before = suite.gov().get_proposal(&id);
    let locked_before = LpTokenClient::new(&suite.env, &suite.lp_addr).locked_balance(&voter);

    assert_eq!(
        suite.gov().try_vote(&voter, &id, &Vote::Against),
        Err(Ok(GovernanceError::AlreadyVoted))
    );
    assert_tallies_unchanged(&before, &suite.gov().get_proposal(&id));
    assert_eq!(
        LpTokenClient::new(&suite.env, &suite.lp_addr).locked_balance(&voter),
        locked_before
    );
}

#[test]
fn test_delegate_cannot_revote_after_delegator_was_aggregated() {
    let suite = setup();
    let delegator = Address::generate(&suite.env);
    let delegatee = Address::generate(&suite.env);
    let proposer = Address::generate(&suite.env);
    mint_lp(&suite, &delegator, 300);
    mint_lp(&suite, &delegatee, 300);
    mint_lp(&suite, &proposer, 400);
    let id = proposal(&suite, &proposer);
    suite.gov().delegate(&delegator, &delegatee);
    suite.gov().vote(&delegatee, &id, &Vote::For);
    assert_eq!(suite.gov().get_proposal(&id).votes_for, 600);
    suite.gov().undelegate(&delegator);
    let before = suite.gov().get_proposal(&id);

    assert_eq!(
        suite.gov().try_vote(&delegator, &id, &Vote::Against),
        Err(Ok(GovernanceError::AlreadyVoted))
    );
    assert_tallies_unchanged(&before, &suite.gov().get_proposal(&id));
}

#[test]
fn test_vote_before_vote_start_returns_exact_error() {
    let suite = setup();
    let voter = Address::generate(&suite.env);
    let other = Address::generate(&suite.env);
    mint_lp(&suite, &voter, 600);
    mint_lp(&suite, &other, 400);
    let id = proposal(&suite, &voter);
    let before = suite.gov().get_proposal(&id);
    // Defensive-branch test: `propose` sets `vote_start` to the current
    // ledger time and ledger time never decreases, so normal execution cannot
    // vote before `vote_start`. Rewinding the clock is the only way to reach
    // the guard, which still has to hold if a voting delay is ever added.
    suite.env.ledger().set_timestamp(before.vote_start - 1);

    assert_eq!(
        suite.gov().try_vote(&voter, &id, &Vote::For),
        Err(Ok(GovernanceError::VotingNotStarted))
    );
    assert_tallies_unchanged(&before, &suite.gov().get_proposal(&id));
    assert_eq!(
        suite.gov().get_vote_info(&id, &voter),
        VoteRecord::DidNotVote
    );
}

#[test]
fn test_vote_on_executed_proposal_returns_already_executed() {
    let suite = setup();
    let (id, voter, _) = passed_proposal(&suite);
    let before = suite.gov().get_proposal(&id);
    execute_at(&suite, id);
    // Defensive-branch test: `execute` requires `now > vote_end` and `vote`
    // rejects `now > vote_end` first, so an executed proposal is never open
    // for voting in normal execution. Rewinding the clock into the voting
    // period is the only way to reach the `AlreadyExecuted` guard in `vote`.
    suite.env.ledger().set_timestamp(before.vote_end);
    let executed = suite.gov().get_proposal(&id);

    assert_eq!(
        suite.gov().try_vote(&voter, &id, &Vote::Against),
        Err(Ok(GovernanceError::AlreadyExecuted))
    );
    assert_tallies_unchanged(&executed, &suite.gov().get_proposal(&id));
    assert_eq!(suite.amm().get_info().fee_bps, 50);
}

#[test]
fn test_vote_on_vetoed_proposal_returns_proposal_vetoed() {
    let suite = setup();
    let (id, voter, _) = passed_proposal(&suite);
    let multisig = Address::generate(&suite.env);
    suite.gov().set_veto_multisig(&multisig);
    let before = suite.gov().get_proposal(&id);
    suite.env.ledger().set_timestamp(before.vote_end + 1);
    suite.gov().veto(&id);
    // Defensive-branch test: `veto` requires `now > vote_end`, so a vetoed
    // proposal is never open for voting in normal execution. Rewinding the
    // clock is the only way to reach the `ProposalVetoed` guard in `vote`.
    suite.env.ledger().set_timestamp(before.vote_end);
    let vetoed = suite.gov().get_proposal(&id);

    assert_eq!(
        suite.gov().try_vote(&voter, &id, &Vote::Against),
        Err(Ok(GovernanceError::ProposalVetoed))
    );
    assert_tallies_unchanged(&vetoed, &suite.gov().get_proposal(&id));
    assert_eq!(suite.amm().get_info().fee_bps, 30);
}

#[test]
fn test_execute_twice_returns_already_executed_and_changes_target_once() {
    let suite = setup();
    let (id, _, _) = passed_proposal(&suite);
    execute_at(&suite, id);
    let executed = suite.gov().get_proposal(&id);
    assert_eq!(suite.amm().get_info().fee_bps, 50);

    assert_eq!(
        suite.gov().try_execute(&id),
        Err(Ok(GovernanceError::AlreadyExecuted))
    );
    assert_tallies_unchanged(&executed, &suite.gov().get_proposal(&id));
    assert_eq!(suite.amm().get_info().fee_bps, 50);
}

#[test]
fn test_execute_during_voting_period_returns_exact_error() {
    let suite = setup();
    let (id, _, _) = passed_proposal(&suite);
    let before = suite.gov().get_proposal(&id);

    assert_eq!(
        suite.gov().try_execute(&id),
        Err(Ok(GovernanceError::VotingPeriodActive))
    );
    assert_tallies_unchanged(&before, &suite.gov().get_proposal(&id));
    assert_eq!(suite.amm().get_info().fee_bps, 30);
}

#[test]
fn test_execute_equal_for_and_against_is_defeated() {
    let suite = setup();
    let id = proposal_with_tally(&suite, 500, 500);
    let before = suite.gov().get_proposal(&id);

    assert_eq!(
        suite.gov().try_execute(&id),
        Err(Ok(GovernanceError::ProposalDefeated))
    );
    assert_tallies_unchanged(&before, &suite.gov().get_proposal(&id));
    assert_eq!(suite.amm().get_info().fee_bps, 30);
}

#[test]
fn test_execute_more_against_than_for_is_defeated() {
    let suite = setup();
    let id = proposal_with_tally(&suite, 400, 600);
    let before = suite.gov().get_proposal(&id);

    assert_eq!(
        suite.gov().try_execute(&id),
        Err(Ok(GovernanceError::ProposalDefeated))
    );
    assert_tallies_unchanged(&before, &suite.gov().get_proposal(&id));
    assert_eq!(suite.amm().get_info().fee_bps, 30);
}

#[test]
fn test_execute_passes_with_one_more_vote_for_than_against() {
    let suite = setup();
    let id = proposal_with_tally(&suite, 501, 499);

    assert_eq!(suite.gov().try_execute(&id), Ok(Ok(())));
    assert!(suite.gov().get_proposal(&id).executed);
    assert_eq!(suite.amm().get_info().fee_bps, 50);
}

#[test]
fn test_cancel_executed_proposal_returns_already_executed() {
    let suite = setup();
    let (id, proposer, _) = passed_proposal(&suite);
    execute_at(&suite, id);
    // No clock rewind: `cancel_proposal` checks `executed` before the
    // voting-period bound, so the guard is reached at the execution time.
    let executed = suite.gov().get_proposal(&id);

    assert_eq!(
        suite.gov().try_cancel_proposal(&id, &proposer),
        Err(Ok(GovernanceError::AlreadyExecuted))
    );
    assert_tallies_unchanged(&executed, &suite.gov().get_proposal(&id));
    assert!(!suite.gov().get_proposal(&id).cancelled);
}

#[test]
fn test_cancel_open_proposal_succeeds_and_marks_cancelled() {
    let suite = setup();
    let proposer = Address::generate(&suite.env);
    mint_lp(&suite, &proposer, 1_000);
    let id = proposal(&suite, &proposer);

    suite.gov().cancel_proposal(&id, &proposer);

    assert!(suite.gov().get_proposal(&id).cancelled);
    assert_eq!(suite.gov().proposal_status(&id), ProposalStatus::Cancelled);
}

#[test]
fn test_cancelled_proposal_cannot_be_cancelled_again() {
    let suite = setup();
    let proposer = Address::generate(&suite.env);
    mint_lp(&suite, &proposer, 1_000);
    let id = proposal(&suite, &proposer);
    suite.gov().cancel_proposal(&id, &proposer);
    let before = suite.gov().get_proposal(&id);

    assert_eq!(
        suite.gov().try_cancel_proposal(&id, &proposer),
        Err(Ok(GovernanceError::ProposalCancelled))
    );
    assert_tallies_unchanged(&before, &suite.gov().get_proposal(&id));
}

#[test]
fn test_cancel_after_voting_closes_returns_voting_period_ended() {
    let suite = setup();
    let proposer = Address::generate(&suite.env);
    mint_lp(&suite, &proposer, 1_000);
    let id = proposal(&suite, &proposer);
    let before = suite.gov().get_proposal(&id);
    suite.env.ledger().set_timestamp(before.vote_end + 1);

    assert_eq!(
        suite.gov().try_cancel_proposal(&id, &proposer),
        Err(Ok(GovernanceError::VotingPeriodEnded))
    );
    assert_tallies_unchanged(&before, &suite.gov().get_proposal(&id));
}

#[test]
fn test_veto_executed_proposal_returns_already_executed() {
    let suite = setup();
    let (id, _, _) = passed_proposal(&suite);
    let multisig = Address::generate(&suite.env);
    suite.gov().set_veto_multisig(&multisig);
    execute_at(&suite, id);
    // No clock rewind: `veto` checks `executed` before the veto window.
    let executed = suite.gov().get_proposal(&id);

    assert_eq!(
        suite.gov().try_veto(&id),
        Err(Ok(GovernanceError::AlreadyExecuted))
    );
    assert_tallies_unchanged(&executed, &suite.gov().get_proposal(&id));
    assert_eq!(suite.gov().get_veto_audit(&id), None);
}

#[test]
fn test_veto_twice_returns_proposal_vetoed() {
    let suite = setup();
    let (id, _, _) = passed_proposal(&suite);
    let multisig = Address::generate(&suite.env);
    suite.gov().set_veto_multisig(&multisig);
    let proposal = suite.gov().get_proposal(&id);
    suite.env.ledger().set_timestamp(proposal.vote_end + 1);
    suite.gov().veto(&id);
    let vetoed = suite.gov().get_proposal(&id);

    assert_eq!(
        suite.gov().try_veto(&id),
        Err(Ok(GovernanceError::ProposalVetoed))
    );
    assert_tallies_unchanged(&vetoed, &suite.gov().get_proposal(&id));
    assert_eq!(suite.amm().get_info().fee_bps, 30);
}

#[test]
fn test_veto_during_voting_period_returns_voting_period_active() {
    let suite = setup();
    let (id, _, _) = passed_proposal(&suite);
    let multisig = Address::generate(&suite.env);
    suite.gov().set_veto_multisig(&multisig);
    let before = suite.gov().get_proposal(&id);

    assert_eq!(
        suite.gov().try_veto(&id),
        Err(Ok(GovernanceError::VotingPeriodActive))
    );
    assert_tallies_unchanged(&before, &suite.gov().get_proposal(&id));
    assert_eq!(suite.gov().get_veto_audit(&id), None);
}

#[test]
fn test_veto_defeated_proposal_returns_proposal_defeated() {
    let suite = setup();
    let id = proposal_with_tally(&suite, 400, 600);
    let multisig = Address::generate(&suite.env);
    suite.gov().set_veto_multisig(&multisig);
    let before = suite.gov().get_proposal(&id);
    suite.env.ledger().set_timestamp(before.vote_end + 1);

    assert_eq!(
        suite.gov().try_veto(&id),
        Err(Ok(GovernanceError::ProposalDefeated))
    );
    assert_tallies_unchanged(&before, &suite.gov().get_proposal(&id));
    assert_eq!(suite.gov().get_veto_audit(&id), None);
    assert_eq!(suite.amm().get_info().fee_bps, 30);
}

#[test]
fn test_vote_is_allowed_at_vote_end() {
    let suite = setup();
    let voter = Address::generate(&suite.env);
    let other = Address::generate(&suite.env);
    mint_lp(&suite, &voter, 600);
    mint_lp(&suite, &other, 400);
    let id = proposal(&suite, &voter);
    let proposal = suite.gov().get_proposal(&id);
    suite.env.ledger().set_timestamp(proposal.vote_end);

    assert_eq!(suite.gov().try_vote(&voter, &id, &Vote::For), Ok(Ok(())));
    assert_eq!(suite.gov().get_proposal(&id).votes_for, 600);
}

#[test]
fn test_vote_one_second_after_vote_end_is_rejected() {
    let suite = setup();
    let voter = Address::generate(&suite.env);
    let other = Address::generate(&suite.env);
    mint_lp(&suite, &voter, 600);
    mint_lp(&suite, &other, 400);
    let id = proposal(&suite, &voter);
    let before = suite.gov().get_proposal(&id);
    suite.env.ledger().set_timestamp(before.vote_end + 1);

    assert_eq!(
        suite.gov().try_vote(&voter, &id, &Vote::For),
        Err(Ok(GovernanceError::VotingPeriodEnded))
    );
    assert_tallies_unchanged(&before, &suite.gov().get_proposal(&id));
}

#[test]
fn test_execute_succeeds_exactly_at_execute_after() {
    let suite = setup();
    let (id, _, _) = passed_proposal(&suite);
    let proposal = suite.gov().get_proposal(&id);
    suite.env.ledger().set_timestamp(proposal.execute_after);

    assert_eq!(suite.gov().try_execute(&id), Ok(Ok(())));
    assert!(suite.gov().get_proposal(&id).executed);
    assert_eq!(suite.amm().get_info().fee_bps, 50);
}

#[test]
fn test_execute_one_second_before_execute_after_is_rejected() {
    let suite = setup();
    let (id, _, _) = passed_proposal(&suite);
    let proposal = suite.gov().get_proposal(&id);
    suite.env.ledger().set_timestamp(proposal.execute_after - 1);

    assert_eq!(
        suite.gov().try_execute(&id),
        Err(Ok(GovernanceError::TimelockNotElapsed))
    );
    assert!(!suite.gov().get_proposal(&id).executed);
    assert_eq!(suite.amm().get_info().fee_bps, 30);
}

#[test]
fn test_execute_succeeds_exactly_at_expires_at() {
    let suite = setup();
    let (id, _, _) = passed_proposal(&suite);
    let proposal = suite.gov().get_proposal(&id);
    suite.env.ledger().set_timestamp(proposal.expires_at);

    assert_eq!(suite.gov().try_execute(&id), Ok(Ok(())));
    assert!(suite.gov().get_proposal(&id).executed);
    assert_eq!(suite.amm().get_info().fee_bps, 50);
}

#[test]
fn test_execute_one_second_after_expires_at_is_rejected() {
    let suite = setup();
    let (id, _, _) = passed_proposal(&suite);
    let proposal = suite.gov().get_proposal(&id);
    suite.env.ledger().set_timestamp(proposal.expires_at + 1);

    assert_eq!(
        suite.gov().try_execute(&id),
        Err(Ok(GovernanceError::ProposalExpired))
    );
    assert!(!suite.gov().get_proposal(&id).executed);
    assert_eq!(suite.amm().get_info().fee_bps, 30);
}

#[test]
fn test_defeated_execution_keeps_target_and_proposal_state_unchanged() {
    let suite = setup();
    let id = proposal_with_tally(&suite, 400, 600);
    let before = suite.gov().get_proposal(&id);
    let fee_before = suite.amm().get_info().fee_bps;

    assert_eq!(
        suite.gov().try_execute(&id),
        Err(Ok(GovernanceError::ProposalDefeated))
    );
    assert!(!suite.gov().get_proposal(&id).executed);
    assert_eq!(suite.gov().get_proposal(&id).votes_for, before.votes_for);
    assert_eq!(
        suite.gov().get_proposal(&id).votes_against,
        before.votes_against
    );
    assert_eq!(suite.amm().get_info().fee_bps, fee_before);
}
