//! Drives the `AmmPoolInterface` entry points added for #1390 through the SDK
//! against the real `amm` contract.
//!
//! Every call goes through `soroban_amm_sdk::client::AmmPoolClient` (the client
//! `#[contractclient]` generates from the SDK's own trait declaration) or the
//! `AmmPoolSdk` wrappers, never through `amm::AmmPoolClient`. A method whose
//! SDK signature, argument order, return type or error codes disagree with the
//! contract therefore fails here: the host cannot decode the arguments or the
//! result, or the typed `SdkAmmError` differs from the one asserted.

use amm::AmmPool;
use soroban_amm_sdk::client::{AmmPoolClient, AmmPoolSdk};
use soroban_amm_sdk::types::{
    CircuitBreakerConfig, EmergencyAnnouncement, MultisigAction, MultisigConfig, MultisigProposal,
    SdkAmmError,
};
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token::{StellarAssetClient, TokenClient},
    vec, Address, Env, String, Vec,
};
use token::{LpToken, LpTokenClient};

const NOW: u64 = 12_345;
const DEADLINE: u64 = 10_000_000;
const SEED: i128 = 1_000_000;
const FEE_BPS: i128 = 30;
/// `DEFAULT_EMERGENCY_DELAY_SECS` in the contract.
const DEFAULT_DELAY: u64 = 86_400;
/// `MAX_EMERGENCY_DELAY_SECS` and `MULTISIG_PROPOSAL_TTL_SECS` in the contract.
const SEVEN_DAYS: u64 = 7 * 86_400;

/// A registered, not yet initialized pool with its LP token and two SACs.
struct Bare {
    env: Env,
    pool: Address,
    lp_token: Address,
    admin: Address,
    token_a: Address,
    token_b: Address,
}

fn bare() -> Bare {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(NOW);
    // The circuit breaker keys its intra-ledger baseline on the sequence
    // number, and the default (0) equals its "never seen" sentinel.
    env.ledger().set_sequence_number(100);

    let admin = Address::generate(&env);
    let pool = env.register_contract(None, AmmPool);
    let lp_token = env.register_contract(None, LpToken);
    LpTokenClient::new(&env, &lp_token).initialize(
        &pool,
        &String::from_str(&env, "AMM LP Token"),
        &String::from_str(&env, "ALP"),
        &7u32,
    );
    let token_a = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let token_b = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    Bare {
        env,
        pool,
        lp_token,
        admin,
        token_a,
        token_b,
    }
}

impl Bare {
    fn client(&self) -> AmmPoolClient<'_> {
        AmmPoolClient::new(&self.env, &self.pool)
    }

    fn sdk(&self) -> AmmPoolSdk<'_> {
        AmmPoolSdk::new(&self.env, &self.pool)
    }

    /// Initialize through the SDK with a 0.30 % fee and no protocol fee.
    fn init(self) -> Self {
        self.client().initialize(
            &self.admin,
            &self.token_a,
            &self.token_b,
            &self.lp_token,
            &FEE_BPS,
            &self.admin,
            &0_i128,
        );
        self
    }

    fn mint(&self, to: &Address, amount: i128) {
        StellarAssetClient::new(&self.env, &self.token_a).mint(to, &amount);
        StellarAssetClient::new(&self.env, &self.token_b).mint(to, &amount);
    }

    fn balance(&self, token: &Address, who: &Address) -> i128 {
        TokenClient::new(&self.env, token).balance(who)
    }

    /// Funds a provider with `SEED` of each token and deposits all of it.
    fn seed(&self) -> (Address, i128) {
        let provider = Address::generate(&self.env);
        self.mint(&provider, SEED);
        let shares = self
            .sdk()
            .add_liquidity(&provider, SEED, SEED, 0, DEADLINE)
            .unwrap();
        (provider, shares)
    }

    /// Installs a multisig guard of `signers` with `quorum`.
    fn multisig(&self, signers: &Vec<Address>, quorum: u32) {
        self.client().set_multisig(&self.admin, signers, &quorum);
    }

    fn advance(&self, secs: u64) {
        let now = self.env.ledger().timestamp();
        self.env.ledger().set_timestamp(now + secs);
    }
}

fn pool() -> Bare {
    bare().init()
}

// ── Initializers ─────────────────────────────────────────────────────────────

#[test]
fn initialize_with_emergency_delay_sets_the_delay() {
    let p = bare();
    p.client().initialize_with_emergency_delay(
        &p.admin,
        &p.token_a,
        &p.token_b,
        &p.lp_token,
        &FEE_BPS,
        &p.admin,
        &0_i128,
        &3_600,
    );
    assert_eq!(p.sdk().emergency_delay(), 3_600);
    assert_eq!(p.client().get_emergency_delay(), 3_600);
}

#[test]
fn initialize_with_emergency_delay_rejects_more_than_seven_days() {
    let p = bare();
    assert_eq!(
        p.client().try_initialize_with_emergency_delay(
            &p.admin,
            &p.token_a,
            &p.token_b,
            &p.lp_token,
            &FEE_BPS,
            &p.admin,
            &0_i128,
            &(SEVEN_DAYS + 1),
        ),
        Err(Ok(SdkAmmError::InvalidEmergencyDelay))
    );
}

#[test]
fn initialize_with_flash_loan_fee_sets_the_fee() {
    let p = bare();
    p.client().initialize_with_flash_loan_fee(
        &p.admin,
        &p.token_a,
        &p.token_b,
        &p.lp_token,
        &FEE_BPS,
        &p.admin,
        &0_i128,
        &10,
    );
    assert_eq!(p.sdk().info().flash_loan_fee_bps, 10);
    assert_eq!(
        p.client().try_initialize(
            &p.admin,
            &p.token_a,
            &p.token_b,
            &p.lp_token,
            &FEE_BPS,
            &p.admin,
            &0_i128,
        ),
        Err(Ok(SdkAmmError::AlreadyInitialized))
    );
}

// ── Oracle and LP locker admin ───────────────────────────────────────────────

#[test]
fn set_oracle_is_admin_only() {
    let p = pool();
    let oracle = Address::generate(&p.env);
    let stranger = Address::generate(&p.env);
    assert_eq!(
        p.client().try_set_oracle(&stranger, &Some(oracle.clone())),
        Err(Ok(SdkAmmError::Unauthorized))
    );
    assert_eq!(
        p.client().try_set_oracle(&p.admin, &Some(oracle)),
        Ok(Ok(()))
    );
    assert_eq!(p.client().try_set_oracle(&p.admin, &None), Ok(Ok(())));
}

#[test]
fn set_max_oracle_deviation_bps_validates_caller_and_range() {
    let p = pool();
    let stranger = Address::generate(&p.env);
    assert_eq!(
        p.client().try_set_max_oracle_deviation_bps(&stranger, &500),
        Err(Ok(SdkAmmError::Unauthorized))
    );
    assert_eq!(
        p.client()
            .try_set_max_oracle_deviation_bps(&p.admin, &10_001),
        Err(Ok(SdkAmmError::InvalidFeeBps))
    );
    assert_eq!(
        p.client().try_set_max_oracle_deviation_bps(&p.admin, &500),
        Ok(Ok(()))
    );
}

#[test]
fn set_lp_locker_points_the_lp_token_at_the_locker() {
    let p = pool();
    let locker = Address::generate(&p.env);
    p.client().set_lp_locker(&locker);
    assert_eq!(LpTokenClient::new(&p.env, &p.lp_token).locker(), locker);
}

// ── Views ────────────────────────────────────────────────────────────────────

#[test]
fn lock_and_state_views_on_an_idle_pool() {
    let p = pool();
    let sdk = p.sdk();
    assert!(!sdk.is_locked());
    assert!(!p.client().is_locked());
    assert!(!sdk.flash_loan_locked());
    assert!(!sdk.drained());
    assert!(sdk.exits_allowed_when_paused());
    assert_eq!(sdk.emergency_delay(), DEFAULT_DELAY);
    assert_eq!(sdk.emergency_announcement(), None);
    assert_eq!(sdk.lp_rebate_bps(), 0);
    assert_eq!(sdk.multisig(), (Vec::new(&p.env), 0));
}

#[test]
fn get_fee_info_reports_the_swap_fee() {
    let p = pool();
    assert_eq!(p.client().get_fee_info(), FEE_BPS);
    p.client().update_fee(&50);
    assert_eq!(p.client().try_get_fee_info(), Ok(Ok(50)));

    let uninitialized = bare();
    assert_eq!(
        uninitialized.client().try_get_fee_info(),
        Err(Ok(SdkAmmError::NotInitialized))
    );
}

#[test]
fn get_liquidity_cumulative_starts_at_zero_and_grows_with_time() {
    let p = pool();
    // `initialize` starts the accumulator clock.
    assert_eq!(p.client().get_liquidity_cumulative(), (0, NOW));

    let (provider, shares) = p.seed();
    let (cum_after_seed, ts_after_seed) = p.client().get_liquidity_cumulative();
    assert_eq!(ts_after_seed, NOW);

    p.advance(100);
    p.sdk()
        .remove_liquidity(&provider, shares / 2, 0, 0, DEADLINE)
        .unwrap();
    let (cum, ts) = p.client().get_liquidity_cumulative();
    assert_eq!(ts, NOW + 100);
    assert!(cum > cum_after_seed, "cumulative did not grow: {cum}");
}

// ── Emergency withdrawal (single admin) ──────────────────────────────────────

#[test]
fn announce_emergency_withdraw_records_the_announcement() {
    let p = pool();
    let recipient = Address::generate(&p.env);
    p.client().announce_emergency_withdraw(&p.admin, &recipient);
    let expected = EmergencyAnnouncement {
        to: recipient,
        announced_at: NOW,
    };
    assert_eq!(p.sdk().emergency_announcement(), Some(expected.clone()));
    assert_eq!(p.client().get_emergency_announcement(), Some(expected));
}

#[test]
fn announce_emergency_withdraw_rejects_bad_callers_and_recipients() {
    let p = pool();
    let stranger = Address::generate(&p.env);
    let recipient = Address::generate(&p.env);
    assert_eq!(
        p.client()
            .try_announce_emergency_withdraw(&stranger, &recipient),
        Err(Ok(SdkAmmError::Unauthorized))
    );
    assert_eq!(
        p.client()
            .try_announce_emergency_withdraw(&p.admin, &p.pool),
        Err(Ok(SdkAmmError::InvalidRecipient))
    );
    assert_eq!(
        p.client()
            .try_announce_emergency_withdraw(&p.admin, &p.lp_token),
        Err(Ok(SdkAmmError::InvalidRecipient))
    );
}

#[test]
fn cancel_emergency_withdraw_clears_the_announcement() {
    let p = pool();
    let recipient = Address::generate(&p.env);
    assert_eq!(
        p.client().try_cancel_emergency_withdraw(&p.admin),
        Err(Ok(SdkAmmError::NoPendingProposal))
    );
    p.client().announce_emergency_withdraw(&p.admin, &recipient);
    p.client().cancel_emergency_withdraw(&p.admin);
    assert_eq!(p.sdk().emergency_announcement(), None);
}

#[test]
fn emergency_withdraw_needs_announcement_pause_and_delay_then_drains() {
    let p = pool();
    p.seed();
    let recipient = Address::generate(&p.env);
    let other = Address::generate(&p.env);
    let c = p.client();

    assert_eq!(
        c.try_emergency_withdraw(&p.admin, &recipient),
        Err(Ok(SdkAmmError::NoPendingProposal))
    );
    c.announce_emergency_withdraw(&p.admin, &recipient);
    assert_eq!(
        c.try_emergency_withdraw(&p.admin, &other),
        Err(Ok(SdkAmmError::InvalidRecipient))
    );
    assert_eq!(
        c.try_emergency_withdraw(&p.admin, &recipient),
        Err(Ok(SdkAmmError::Paused))
    );
    c.pause();
    assert_eq!(
        c.try_emergency_withdraw(&p.admin, &recipient),
        Err(Ok(SdkAmmError::EmergencyDelayNotElapsed))
    );

    p.advance(DEFAULT_DELAY);
    assert_eq!(c.try_emergency_withdraw(&p.admin, &recipient), Ok(Ok(())));
    assert!(p.sdk().drained());
    assert_eq!(p.balance(&p.token_a, &recipient), SEED);
    assert_eq!(p.balance(&p.token_b, &recipient), SEED);
    assert_eq!(
        p.client().try_try_circuit_breaker_recovery(),
        Err(Ok(SdkAmmError::PoolDrained))
    );
}

#[test]
fn emergency_withdraw_is_refused_while_a_multisig_guard_is_active() {
    let p = pool();
    let recipient = Address::generate(&p.env);
    p.client().announce_emergency_withdraw(&p.admin, &recipient);
    p.multisig(&vec![&p.env, Address::generate(&p.env)], 1);
    p.client().pause();
    p.advance(DEFAULT_DELAY);
    assert_eq!(
        p.client().try_emergency_withdraw(&p.admin, &recipient),
        Err(Ok(SdkAmmError::Unauthorized))
    );
}

// ── Reentrancy lock ──────────────────────────────────────────────────────────

#[test]
fn force_unlock_needs_admin_or_a_signer_quorum() {
    let p = pool();
    assert_eq!(p.client().try_force_unlock(&Vec::new(&p.env)), Ok(Ok(())));
    assert!(!p.sdk().is_locked());

    let signer = Address::generate(&p.env);
    p.multisig(&vec![&p.env, signer.clone()], 1);
    assert_eq!(
        p.client().try_force_unlock(&Vec::new(&p.env)),
        Err(Ok(SdkAmmError::Unauthorized))
    );
    assert_eq!(
        p.client().try_force_unlock(&vec![&p.env, signer]),
        Ok(Ok(()))
    );
}

// ── Circuit breaker ──────────────────────────────────────────────────────────

#[test]
fn circuit_breaker_config_round_trips_and_validates() {
    let p = pool();
    let defaults = CircuitBreakerConfig {
        threshold_bps: 5_000,
        cooldown_secs: 600,
        triggered_at: 0,
        tripped: false,
    };
    assert_eq!(p.sdk().circuit_breaker_config(), defaults);

    p.client().set_circuit_breaker_config(&2_500, &900);
    assert_eq!(
        p.client().get_circuit_breaker_config(),
        CircuitBreakerConfig {
            threshold_bps: 2_500,
            cooldown_secs: 900,
            ..defaults
        }
    );
    assert_eq!(
        p.client().try_set_circuit_breaker_config(&0, &900),
        Err(Ok(SdkAmmError::InvalidFeeBps))
    );
}

#[test]
fn circuit_breaker_trips_and_recovers_after_the_cooldown() {
    let p = pool();
    p.seed();
    let c = p.client();
    // Untripped, idle pool: nothing to trip or recover.
    assert!(!c.trip_circuit_breaker());
    assert!(!c.try_circuit_breaker_recovery());

    // A 10 % swap moves the price ~20 %: under the 50 % default threshold,
    // but it records this ledger's baseline price.
    let trader = Address::generate(&p.env);
    p.mint(&trader, SEED);
    p.sdk()
        .execute_swap(&trader, &p.token_a, SEED / 10, 0, DEADLINE)
        .unwrap();
    assert!(!p.sdk().paused());

    // With a 1 % threshold the keeper entry point now trips it.
    c.set_circuit_breaker_config(&100, &600);
    assert_eq!(c.try_trip_circuit_breaker(), Ok(Ok(true)));
    assert!(p.sdk().paused());
    let tripped = p.sdk().circuit_breaker_config();
    assert!(tripped.tripped);
    assert_eq!(tripped.triggered_at, NOW);
    assert!(!c.trip_circuit_breaker(), "already paused");

    p.advance(599);
    assert_eq!(c.try_try_circuit_breaker_recovery(), Ok(Ok(false)));
    assert!(p.sdk().paused());
    p.advance(1);
    assert_eq!(c.try_try_circuit_breaker_recovery(), Ok(Ok(true)));
    assert!(!p.sdk().paused());
    assert!(!p.sdk().circuit_breaker_config().tripped);
}

// ── LP rebate ────────────────────────────────────────────────────────────────

#[test]
fn lp_rebate_round_trips_and_validates() {
    let p = pool();
    let stranger = Address::generate(&p.env);
    p.client().set_lp_rebate(&p.admin, &2_500);
    assert_eq!(p.client().get_lp_rebate(), 2_500);
    assert_eq!(p.sdk().lp_rebate_bps(), 2_500);
    assert_eq!(
        p.client().try_set_lp_rebate(&p.admin, &10_001),
        Err(Ok(SdkAmmError::InvalidFeeBps))
    );
    assert_eq!(
        p.client().try_set_lp_rebate(&stranger, &100),
        Err(Ok(SdkAmmError::Unauthorized))
    );
}

// ── Multisig ─────────────────────────────────────────────────────────────────

#[test]
fn set_multisig_installs_a_guard_once() {
    let p = pool();
    let signers = vec![&p.env, Address::generate(&p.env), Address::generate(&p.env)];
    assert_eq!(
        p.client().try_set_multisig(&p.admin, &signers, &3),
        Err(Ok(SdkAmmError::InvalidQuorum))
    );
    p.multisig(&signers, 2);
    assert_eq!(
        p.client().get_multisig_config(),
        MultisigConfig {
            signers: signers.clone(),
            quorum: 2
        }
    );
    assert_eq!(p.client().get_multisig(), (signers.clone(), 2));
    assert_eq!(p.sdk().multisig(), (signers.clone(), 2));
    assert_eq!(
        p.client().try_set_multisig(&p.admin, &signers, &1),
        Err(Ok(SdkAmmError::MultisigGuardActive))
    );
}

#[test]
fn propose_emergency_withdraw_creates_a_typed_proposal() {
    let p = pool();
    let signer = Address::generate(&p.env);
    let stranger = Address::generate(&p.env);
    let recipient = Address::generate(&p.env);
    p.multisig(&vec![&p.env, signer.clone()], 1);

    assert_eq!(
        p.client()
            .try_propose_emergency_withdraw(&stranger, &recipient),
        Err(Ok(SdkAmmError::Unauthorized))
    );
    let id = p.client().propose_emergency_withdraw(&signer, &recipient);
    assert_eq!(id, 1);
    assert_eq!(
        p.client().get_multisig_proposal(&id),
        Some(MultisigProposal {
            action: MultisigAction::EmergencyWithdraw(recipient),
            approvals: vec![&p.env, signer],
            // Quorum (1) is reached on creation, which extends the lifetime
            // by the emergency delay.
            expires_at: NOW + SEVEN_DAYS + DEFAULT_DELAY,
            quorum_reached_at: NOW,
            epoch: 1,
            executed: false,
        })
    );
    assert_eq!(p.client().get_multisig_proposal(&99), None);
}

#[test]
fn approve_multisig_proposal_records_approvals_up_to_quorum() {
    let p = pool();
    let s1 = Address::generate(&p.env);
    let s2 = Address::generate(&p.env);
    let recipient = Address::generate(&p.env);
    p.multisig(&vec![&p.env, s1.clone(), s2.clone()], 2);

    let id = p.client().propose_emergency_withdraw(&s1, &recipient);
    assert_eq!(
        p.client()
            .get_multisig_proposal(&id)
            .unwrap()
            .quorum_reached_at,
        0
    );
    p.advance(10);
    p.client().approve_multisig_proposal(&s2, &id);
    let proposal = p.client().get_multisig_proposal(&id).unwrap();
    assert_eq!(proposal.approvals, vec![&p.env, s1, s2.clone()]);
    assert_eq!(proposal.quorum_reached_at, NOW + 10);
    assert_eq!(
        p.client().try_approve_multisig_proposal(&s2, &(id + 1)),
        Err(Ok(SdkAmmError::NoPendingProposal))
    );
}

#[test]
fn exec_multisig_emergency_wd_enforces_quorum_delay_and_pause() {
    let p = pool();
    p.seed();
    let s1 = Address::generate(&p.env);
    let s2 = Address::generate(&p.env);
    let recipient = Address::generate(&p.env);
    p.multisig(&vec![&p.env, s1.clone(), s2.clone()], 2);
    let c = p.client();

    let id = c.propose_emergency_withdraw(&s1, &recipient);
    assert_eq!(
        c.try_exec_multisig_emergency_wd(&s1, &id),
        Err(Ok(SdkAmmError::QuorumNotReached))
    );
    c.approve_multisig_proposal(&s2, &id);
    assert_eq!(
        c.try_exec_multisig_emergency_wd(&s1, &id),
        Err(Ok(SdkAmmError::EmergencyDelayNotElapsed))
    );
    p.advance(DEFAULT_DELAY);
    assert_eq!(
        c.try_exec_multisig_emergency_wd(&s1, &id),
        Err(Ok(SdkAmmError::Paused))
    );
    c.pause();
    assert_eq!(c.try_exec_multisig_emergency_wd(&s1, &id), Ok(Ok(())));
    assert!(p.sdk().drained());
    assert_eq!(p.balance(&p.token_a, &recipient), SEED);
    assert_eq!(p.balance(&p.token_b, &recipient), SEED);
    assert!(c.get_multisig_proposal(&id).unwrap().executed);
    assert_eq!(
        c.try_exec_multisig_emergency_wd(&s1, &id),
        Err(Ok(SdkAmmError::AlreadyExecuted))
    );
}

#[test]
fn exec_multisig_set_multisig_replaces_the_signer_set() {
    let p = pool();
    let s1 = Address::generate(&p.env);
    let s2 = Address::generate(&p.env);
    p.multisig(&vec![&p.env, s1.clone()], 1);
    let c = p.client();

    let new_signers = vec![&p.env, s2.clone()];
    assert_eq!(
        c.try_propose_set_multisig(&s1, &new_signers, &2),
        Err(Ok(SdkAmmError::InvalidQuorum))
    );
    let id = c.propose_set_multisig(&s1, &new_signers, &1);
    assert_eq!(
        c.get_multisig_proposal(&id).unwrap().action,
        MultisigAction::SetMultisig(new_signers.clone(), 1)
    );
    // The proposal is not an emergency withdrawal.
    assert_eq!(
        c.try_exec_multisig_emergency_wd(&s1, &id),
        Err(Ok(SdkAmmError::NoPendingProposal))
    );
    assert_eq!(c.try_exec_multisig_set_multisig(&s1, &id), Ok(Ok(())));
    assert_eq!(c.get_multisig(), (new_signers, 1));
    // The old signer is no longer a signer.
    assert_eq!(
        c.try_exec_multisig_set_multisig(&s1, &id),
        Err(Ok(SdkAmmError::Unauthorized))
    );
}

// ── Fee-on-transfer and one-sided liquidity (AmmPoolSdk wrappers) ────────────

#[test]
fn sdk_remove_liquidity_one_sided_pays_out_one_token() {
    let p = pool();
    let (provider, shares) = p.seed();
    // Small enough that the internal swap stays under the circuit breaker's
    // 50 % threshold (a trip would return 0 without executing).
    let burn = shares / 100;
    let sdk = p.sdk();
    assert_eq!(
        sdk.remove_liquidity_one_sided(&provider, burn, &p.lp_token, 0, DEADLINE),
        Err(SdkAmmError::InvalidToken)
    );

    let before_a = p.balance(&p.token_a, &provider);
    let before_b = p.balance(&p.token_b, &provider);
    let out = sdk
        .remove_liquidity_one_sided(&provider, burn, &p.token_a, 0, DEADLINE)
        .unwrap();
    // Roughly twice the pro-rata share of token_a, less the swap fee.
    let pro_rata = SEED * burn / (shares + 1_000);
    assert!(out > pro_rata && out < 2 * pro_rata, "out = {out}");
    assert_eq!(p.balance(&p.token_a, &provider), before_a + out);
    assert_eq!(p.balance(&p.token_b, &provider), before_b);
    assert_eq!(sdk.shares_of(&provider), shares - burn);
    assert_eq!(
        sdk.remove_liquidity_one_sided(&provider, burn, &p.token_a, out * 2, DEADLINE),
        Err(SdkAmmError::SlippageExceeded)
    );

    p.client().pause();
    assert_eq!(
        sdk.remove_liquidity_one_sided(&provider, burn, &p.token_a, 0, DEADLINE),
        Err(SdkAmmError::Paused)
    );
}

#[test]
fn sdk_execute_swap_fot_matches_the_quote_for_a_standard_token() {
    let p = pool();
    p.seed();
    let trader = Address::generate(&p.env);
    p.mint(&trader, SEED);
    let sdk = p.sdk();
    let amount_in = 10_000;

    assert_eq!(
        sdk.execute_swap_fot(
            &trader,
            &p.token_a,
            amount_in,
            0,
            amount_in + 1,
            DEADLINE,
            None
        ),
        Err(SdkAmmError::FotSlippage)
    );

    let quote = sdk.quote_swap_in(&p.token_a, amount_in).unwrap();
    let (amount_out, received) = sdk
        .execute_swap_fot(&trader, &p.token_a, amount_in, 0, amount_in, DEADLINE, None)
        .unwrap();
    assert_eq!(received, amount_in);
    assert_eq!(amount_out, quote.amount_out);
    assert_eq!(p.balance(&p.token_b, &trader), SEED + amount_out);
}

#[test]
fn sdk_add_liquidity_fot_mints_the_quoted_shares() {
    let p = pool();
    p.seed();
    let provider = Address::generate(&p.env);
    p.mint(&provider, SEED);
    let sdk = p.sdk();

    assert_eq!(
        sdk.add_liquidity_fot(&provider, 1_000, 1_000, 1_001, 0, 0, DEADLINE),
        Err(SdkAmmError::FotSlippage)
    );

    let quote = sdk.quote_add_liquidity(50_000, 50_000).unwrap();
    let shares = sdk
        .add_liquidity_fot(&provider, 50_000, 50_000, 50_000, 50_000, 0, DEADLINE)
        .unwrap();
    assert_eq!(shares, quote.shares);
    assert_eq!(sdk.shares_of(&provider), shares);
}
