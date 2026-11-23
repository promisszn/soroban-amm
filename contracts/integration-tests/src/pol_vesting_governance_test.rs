//! End-to-end `CreatePolVesting` governance regression (issue #1044).
//!
//! The production path to `pol_vesting::create_vesting` is a governance
//! `CreatePolVesting` proposal executed by the governance contract. On main,
//! `create_vesting` never checked the vesting contract's balance, so a proposal
//! that named more tokens than the contract actually held executed successfully
//! and only blew up later, inside `release`, when the transfer found nothing to
//! send. These tests drive the real factory -> pool -> governance -> pol_vesting
//! wiring and assert that an unfunded schedule is refused at execution while a
//! funded one succeeds.

use soroban_sdk::testutils::{Address as _, Ledger};
use soroban_sdk::token::{Client as TokenClient, StellarAssetClient};
use soroban_sdk::{Address, BytesN, Env};

use amm::AmmPoolClient;
use factory::FactoryClient;
use governance::{CreatePolVestingParams, GovernanceClient, ProposalKind, Vote};
use pol_vesting::PolVestingContractClient;

const DEADLINE: u64 = u64::MAX;
const VEST_TOTAL: i128 = 100_000;

fn new_env() -> Env {
    let env = Env::default();
    env.budget().reset_unlimited();
    env.mock_all_auths();
    env.ledger().with_mut(|l| {
        l.timestamp = 1_000_000;
        l.sequence_number = 100;
    });
    env
}

struct Ctx {
    env: Env,
    gov: Address,
    pool: Address,
    lp: Address,
    vesting: Address,
    holder: Address,
}

/// Deploy the full stack and (optionally) pre-fund the vesting contract with
/// `fund_vesting` LP tokens transferred from the liquidity provider.
fn deploy(fund_vesting: i128) -> Ctx {
    let env = new_env();

    let admin = Address::generate(&env);
    let amm_hash: BytesN<32> = env.deployer().upload_contract_wasm(amm::WASM);
    let token_hash: BytesN<32> = env.deployer().upload_contract_wasm(token::WASM);
    let gov_hash: BytesN<32> = env.deployer().upload_contract_wasm(governance::WASM);

    let factory = FactoryClient::new(&env, &env.register_contract(None, factory::Factory));
    factory.initialize(&admin, &amm_hash, &token_hash);

    let token_a = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let token_b = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();

    let (pool, gov_opt) = factory.create_pool(&admin, &token_a, &token_b, &2_i128, &Some(gov_hash));
    let gov = gov_opt.expect("factory deploys governance");
    let lp = factory.get_lp_token(&pool).unwrap();

    // Seed liquidity so the provider holds LP tokens to vote with and to fund
    // the vesting contract from.
    let holder = Address::generate(&env);
    for token in [&token_a, &token_b] {
        StellarAssetClient::new(&env, token).mint(&holder, &1_000_000);
    }
    AmmPoolClient::new(&env, &pool).add_liquidity(&holder, &500_000, &500_000, &1_i128, &DEADLINE);

    // The pool's governance is the vesting contract's governance too, so
    // governance's `execute` may call `create_vesting`.
    let vesting = env.register_contract(None, pol_vesting::PolVestingContract);
    let treasury = Address::generate(&env);
    PolVestingContractClient::new(&env, &vesting).initialize(&gov, &treasury);

    if fund_vesting > 0 {
        TokenClient::new(&env, &lp).transfer(&holder, &vesting, &fund_vesting);
    }

    // Governance snapshots voting power at `sequence - 1` at propose time.
    // Close the ledger that received the liquidity (and the funding transfer)
    // so the snapshot sees the holder's LP balance rather than a stale zero.
    env.ledger().with_mut(|l| l.sequence_number += 1);

    Ctx {
        env,
        gov,
        pool,
        lp,
        vesting,
        holder,
    }
}

/// Propose, vote for and execute a `CreatePolVesting` proposal, returning the
/// proposal id. `execute` is left to the caller so it can assert on failure.
fn drive_proposal(ctx: &Ctx, beneficiary: &Address) -> u32 {
    let env = &ctx.env;
    let gov = GovernanceClient::new(env, &ctx.gov);

    let params = CreatePolVestingParams {
        pol_vesting: ctx.vesting.clone(),
        beneficiary: beneficiary.clone(),
        lp_token: ctx.lp.clone(),
        pool: ctx.pool.clone(),
        total: VEST_TOTAL,
        start_ledger: 200,
        cliff_ledger: 300,
        end_ledger: 1_000,
    };
    let proposal_id = gov.propose(&ctx.holder, &ProposalKind::CreatePolVesting(params));
    gov.vote(&ctx.holder, &proposal_id, &Vote::For);

    let proposal = gov.get_proposal(&proposal_id);
    env.ledger()
        .with_mut(|l| l.timestamp = proposal.execute_after + 1);

    proposal_id
}

#[test]
fn create_pol_vesting_execution_fails_when_vesting_contract_is_unfunded() {
    let ctx = deploy(0);
    let beneficiary = Address::generate(&ctx.env);
    let proposal_id = drive_proposal(&ctx, &beneficiary);

    let gov = GovernanceClient::new(&ctx.env, &ctx.gov);
    assert!(
        gov.try_execute(&proposal_id).is_err(),
        "an unfunded CreatePolVesting proposal must not execute"
    );

    // Nothing was created at the vesting contract.
    let vesting = PolVestingContractClient::new(&ctx.env, &ctx.vesting);
    assert_eq!(vesting.committed(&ctx.lp), 0);
    assert_eq!(vesting.schedule_count(&beneficiary), 0);
}

#[test]
fn create_pol_vesting_execution_succeeds_when_vesting_contract_is_funded() {
    let ctx = deploy(VEST_TOTAL);
    let beneficiary = Address::generate(&ctx.env);
    let proposal_id = drive_proposal(&ctx, &beneficiary);

    let gov = GovernanceClient::new(&ctx.env, &ctx.gov);
    gov.execute(&proposal_id);

    let vesting = PolVestingContractClient::new(&ctx.env, &ctx.vesting);
    let schedule = vesting.get_vesting(&beneficiary, &0);
    assert_eq!(schedule.total, VEST_TOTAL);
    assert_eq!(schedule.lp_token, ctx.lp);
    assert_eq!(vesting.committed(&ctx.lp), VEST_TOTAL);

    // The funded schedule can actually pay out once it has vested fully.
    ctx.env.ledger().with_mut(|l| l.sequence_number = 1_000);
    assert_eq!(vesting.release(&beneficiary, &0), VEST_TOTAL);
    assert_eq!(
        TokenClient::new(&ctx.env, &ctx.lp).balance(&beneficiary),
        VEST_TOTAL
    );
    assert_eq!(vesting.committed(&ctx.lp), 0);
}
