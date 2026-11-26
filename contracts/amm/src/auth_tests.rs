//! Regression tests that every privileged `AmmPool` entrypoint, and every
//! fund-moving user entrypoint (#1295), demands a signature from the right
//! address.
//!
//! Each test drives one entrypoint through [`assert_requires_auth`], which
//! checks that the call is rejected with `Error(Auth, InvalidAction)` when
//! nobody signs and when only a stranger signs, and succeeds when exactly the
//! expected signer authorizes that call. A second, narrower check per function
//! with an `admin` argument confirms a stranger passed as `admin` is refused
//! even when every signature is mocked.

extern crate std;

use super::*;
use crate::tests::{seed_pool, setup_pool};
use soroban_sdk::{
    testutils::{Address as _, AuthorizedFunction, Ledger, MockAuth, MockAuthInvoke},
    Address, Env, IntoVal, Symbol, Val, Vec,
};

#[cfg(feature = "testutils")]
use super::WASM as AMM_WASM;
#[cfg(not(feature = "testutils"))]
const AMM_WASM: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../target/wasm32v1-none/release/amm.wasm"
));

/// Panics from the host are expected while probing; keep them off stderr.
fn invoke_catching(env: &Env, invoke: &MockAuthInvoke) -> Result<(), std::string::String> {
    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(std::boxed::Box::new(|_| {}));
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        env.invoke_contract::<Val>(
            invoke.contract,
            &Symbol::new(env, invoke.fn_name),
            invoke.args.clone(),
        );
    }));
    std::panic::set_hook(prev_hook);
    res.map_err(|err| {
        if let Some(s) = err.downcast_ref::<&str>() {
            std::string::String::from(*s)
        } else if let Some(s) = err.downcast_ref::<std::string::String>() {
            s.clone()
        } else {
            std::string::String::new()
        }
    })
}

fn assert_auth_err(env: &Env, invoke: &MockAuthInvoke, context: &str) {
    let msg = invoke_catching(env, invoke).expect_err(&std::format!(
        "{} should fail under {}",
        invoke.fn_name,
        context
    ));
    assert!(
        msg.contains("Error(Auth, InvalidAction)"),
        "{} under {}: expected Error(Auth, InvalidAction), got: {}",
        invoke.fn_name,
        context,
        msg
    );
}

/// 1. No authorizations -> `Error(Auth, InvalidAction)`.
/// 2. Only a stranger's authorization -> `Error(Auth, InvalidAction)`.
/// 3. Only `signer_expected`'s authorization -> succeeds.
fn assert_requires_auth(env: &Env, signer_expected: &Address, invoke: &MockAuthInvoke) {
    assert_requires_auth_with_checks(env, signer_expected, invoke, || {}, || {});
}

/// [`assert_requires_auth`], additionally running `check_rejected_state`
/// after each rejected call and `check_success` after the authorized one.
fn assert_requires_auth_with_checks(
    env: &Env,
    signer_expected: &Address,
    invoke: &MockAuthInvoke,
    mut check_rejected_state: impl FnMut(),
    check_success: impl FnOnce(),
) {
    let stranger = Address::generate(env);

    env.set_auths(&[]);
    assert_auth_err(env, invoke, "no authorizations");
    check_rejected_state();

    env.mock_auths(&[MockAuth {
        address: &stranger,
        invoke,
    }]);
    assert_auth_err(env, invoke, "a stranger's authorization");
    check_rejected_state();

    env.mock_auths(&[MockAuth {
        address: signer_expected,
        invoke,
    }]);
    if let Err(msg) = invoke_catching(env, invoke) {
        panic!(
            "{} should succeed with authorization from {:?}, failed: {}",
            invoke.fn_name, signer_expected, msg
        );
    }
    check_success();
}

/// Asserts the last invocation's authorization tree for `signer` is rooted at
/// `contract.fn_name(args)` itself, not only at a nested token `transfer`.
///
/// The fund-moving entrypoints pull tokens from the user, so the token's own
/// `require_auth` would still reject an unsigned call even if the
/// entrypoint's `require_auth` were deleted; the rejection legs alone cannot
/// tell the two apart. Without the entrypoint's check, the mocked tree rooted
/// at the entrypoint no longer matches and the authorized leg fails, and this
/// assertion pins the recorded root so the test does not rely on that
/// matching detail alone.
fn assert_root_auth(
    env: &Env,
    signer: &Address,
    contract: &Address,
    fn_name: &str,
    args: Vec<Val>,
) {
    let auths = env.auths();
    let root = auths
        .iter()
        .find(|(addr, _)| addr == signer)
        .map(|(_, invocation)| invocation.function.clone())
        .unwrap_or_else(|| panic!("{fn_name}: no authorization recorded for the signer"));
    assert_eq!(
        root,
        AuthorizedFunction::Contract((contract.clone(), Symbol::new(env, fn_name), args)),
        "{fn_name}: the signer's authorization must be rooted at the entrypoint itself"
    );
}

struct Fixture {
    ts: crate::tests::TestSetup,
}

impl Fixture {
    fn new() -> Self {
        Self { ts: setup_pool(30) }
    }
    fn env(&self) -> &Env {
        &self.ts.env
    }
    fn client(&self) -> AmmPoolClient<'_> {
        AmmPoolClient::new(&self.ts.env, &self.ts.amm_addr)
    }
    fn check<A: IntoVal<Env, soroban_sdk::Vec<Val>>>(
        &self,
        signer: &Address,
        fn_name: &'static str,
        args: A,
    ) {
        let invoke = MockAuthInvoke {
            contract: &self.ts.amm_addr,
            fn_name,
            args: args.into_val(self.env()),
            sub_invokes: &[],
        };
        assert_requires_auth(self.env(), signer, &invoke);
    }

    fn add_funded_provider(&self) -> (Address, i128) {
        let provider = Address::generate(self.env());
        soroban_sdk::token::StellarAssetClient::new(self.env(), &self.ts.ta_addr)
            .mint(&provider, &1_000_000);
        soroban_sdk::token::StellarAssetClient::new(self.env(), &self.ts.tb_addr)
            .mint(&provider, &1_000_000);
        let shares = self
            .client()
            .add_liquidity(&provider, &100_000, &100_000, &0, &u64::MAX);
        (provider, shares)
    }

    /// Drives a fund-moving entrypoint through [`assert_requires_auth_with_checks`].
    ///
    /// `pulls` lists the `(token, amount)` transfers the entrypoint makes from
    /// `signer` to the pool. Each is a nested `transfer` that itself requires
    /// `signer`'s authorization, so it is included as a sub-invocation of the
    /// mocked authorization; without it even the expected signer would be
    /// rejected. Every rejected attempt must leave the reserves, total shares
    /// and `share_holder`'s balance unchanged, and the authorized call must
    /// record its root authorization on the entrypoint itself.
    fn check_fund_entrypoint<A: IntoVal<Env, Vec<Val>>>(
        &self,
        signer: &Address,
        fn_name: &'static str,
        args: A,
        pulls: &[(&Address, i128)],
        share_holder: &Address,
    ) {
        let env = self.env();
        let pool = &self.ts.amm_addr;
        let transfers: std::vec::Vec<MockAuthInvoke> = pulls
            .iter()
            .map(|(token, amount)| MockAuthInvoke {
                contract: token,
                fn_name: "transfer",
                args: (signer.clone(), pool.clone(), *amount).into_val(env),
                sub_invokes: &[],
            })
            .collect();
        let args: Vec<Val> = args.into_val(env);
        let invoke = MockAuthInvoke {
            contract: pool,
            fn_name,
            args: args.clone(),
            sub_invokes: &transfers,
        };
        let before = self.user_pool_state(share_holder);
        assert_requires_auth_with_checks(
            env,
            signer,
            &invoke,
            || assert_eq!(self.user_pool_state(share_holder), before),
            || assert_root_auth(env, signer, pool, fn_name, args.clone()),
        );
        assert_ne!(
            self.user_pool_state(share_holder),
            before,
            "{fn_name}: the authorized call should have changed pool state"
        );
    }

    fn user_pool_state(&self, share_holder: &Address) -> (i128, i128, i128, i128) {
        let info = self.client().get_info();
        (
            info.reserve_a,
            info.reserve_b,
            info.total_shares,
            self.client().shares_of(share_holder),
        )
    }
}

fn funded_fixture() -> (Fixture, Address, i128) {
    let f = Fixture::new();
    seed_pool(&f.ts);
    let (provider, shares) = f.add_funded_provider();
    (f, provider, shares)
}

#[test]
fn test_swap_requires_trader_auth_without_state_change() {
    let (f, trader, _) = funded_fixture();
    let ta = f.ts.ta_addr.clone();
    f.check_fund_entrypoint(
        &trader,
        "swap",
        (trader.clone(), ta.clone(), 1_000_i128, 0_i128, u64::MAX),
        &[(&ta, 1_000)],
        &trader,
    );
}

#[test]
fn test_swap_exact_out_requires_trader_auth_without_state_change() {
    let (f, trader, _) = funded_fixture();
    let (ta, tb) = (f.ts.ta_addr.clone(), f.ts.tb_addr.clone());
    let amount_in = f.client().get_amount_in(&tb, &1_000);
    f.check_fund_entrypoint(
        &trader,
        "swap_exact_out",
        (trader.clone(), tb, 1_000_i128, 10_000_i128, u64::MAX),
        &[(&ta, amount_in)],
        &trader,
    );
}

#[test]
fn test_swap_fot_requires_trader_auth_without_state_change() {
    let (f, trader, _) = funded_fixture();
    let ta = f.ts.ta_addr.clone();
    f.check_fund_entrypoint(
        &trader,
        "swap_fot",
        (
            trader.clone(),
            ta.clone(),
            1_000_i128,
            0_i128,
            0_i128,
            u64::MAX,
            Option::<Address>::None,
        ),
        &[(&ta, 1_000)],
        &trader,
    );
}

#[test]
fn test_add_liquidity_requires_provider_auth_without_state_change() {
    let (f, provider, _) = funded_fixture();
    let (ta, tb) = (f.ts.ta_addr.clone(), f.ts.tb_addr.clone());
    f.check_fund_entrypoint(
        &provider,
        "add_liquidity",
        (provider.clone(), 1_000_i128, 1_000_i128, 0_i128, u64::MAX),
        &[(&ta, 1_000), (&tb, 1_000)],
        &provider,
    );
}

#[test]
fn test_add_liquidity_fot_requires_provider_auth_without_state_change() {
    let (f, provider, _) = funded_fixture();
    let (ta, tb) = (f.ts.ta_addr.clone(), f.ts.tb_addr.clone());
    f.check_fund_entrypoint(
        &provider,
        "add_liquidity_fot",
        (
            provider.clone(),
            1_000_i128,
            1_000_i128,
            0_i128,
            0_i128,
            0_i128,
            u64::MAX,
        ),
        &[(&ta, 1_000), (&tb, 1_000)],
        &provider,
    );
}

#[test]
fn test_remove_liquidity_requires_provider_auth_without_state_change() {
    let (f, provider, shares) = funded_fixture();
    f.check_fund_entrypoint(
        &provider,
        "remove_liquidity",
        (provider.clone(), shares / 10, 0_i128, 0_i128, u64::MAX),
        &[],
        &provider,
    );
}

#[test]
fn test_remove_liquidity_one_sided_requires_provider_auth_without_state_change() {
    let (f, provider, shares) = funded_fixture();
    f.check_fund_entrypoint(
        &provider,
        "remove_liquidity_one_sided",
        (
            provider.clone(),
            shares / 10,
            f.ts.ta_addr.clone(),
            0_i128,
            u64::MAX,
        ),
        &[],
        &provider,
    );
}

#[test]
fn test_pause_requires_admin_auth() {
    let f = Fixture::new();
    f.check(&f.ts.admin, "pause", ());
    assert!(f.client().is_paused());
}

#[test]
fn test_unpause_requires_admin_auth() {
    let f = Fixture::new();
    f.env().mock_all_auths();
    f.client().pause();
    f.check(&f.ts.admin, "unpause", ());
    assert!(!f.client().is_paused());
}

#[test]
fn test_update_fee_requires_admin_auth() {
    let f = Fixture::new();
    f.check(&f.ts.admin, "update_fee", (20_i128,));
}

#[test]
fn test_update_flash_loan_fee_requires_admin_auth() {
    let f = Fixture::new();
    f.check(&f.ts.admin, "update_flash_loan_fee", (9_i128,));
}

#[test]
fn test_set_lp_locker_requires_admin_auth() {
    let f = Fixture::new();
    let locker = Address::generate(f.env());
    f.check(&f.ts.admin, "set_lp_locker", (locker,));
}

#[test]
fn test_set_circuit_breaker_config_requires_admin_auth() {
    let f = Fixture::new();
    f.check(
        &f.ts.admin,
        "set_circuit_breaker_config",
        (5_000_i128, 300_u64),
    );
}

#[test]
fn test_force_unlock_requires_admin_auth() {
    let f = Fixture::new();
    let callers = soroban_sdk::Vec::<Address>::new(f.env());
    f.check(&f.ts.admin, "force_unlock", (callers,));
}

#[test]
fn test_upgrade_requires_admin_auth() {
    let f = Fixture::new();
    // Parsing the full pool WASM on upload exceeds the default test budget.
    f.env().budget().reset_unlimited();
    let wasm_hash = f.env().deployer().upload_contract_wasm(AMM_WASM);
    f.check(&f.ts.admin, "upgrade", (wasm_hash,));
}

#[test]
fn test_set_oracle_requires_admin_auth() {
    let f = Fixture::new();
    f.check(
        &f.ts.admin,
        "set_oracle",
        (f.ts.admin.clone(), Option::<Address>::None),
    );
    let stranger = Address::generate(f.env());
    f.env().mock_all_auths();
    assert_eq!(
        f.client().try_set_oracle(&stranger, &None),
        Err(Ok(AmmError::Unauthorized))
    );
}

#[test]
fn test_set_max_oracle_deviation_bps_requires_admin_auth() {
    let f = Fixture::new();
    f.check(
        &f.ts.admin,
        "set_max_oracle_deviation_bps",
        (f.ts.admin.clone(), 500_i128),
    );
    let stranger = Address::generate(f.env());
    f.env().mock_all_auths();
    assert_eq!(
        f.client().try_set_max_oracle_deviation_bps(&stranger, &500),
        Err(Ok(AmmError::Unauthorized))
    );
}

#[test]
fn test_set_protocol_fee_requires_admin_auth() {
    let f = Fixture::new();
    let recipient = Address::generate(f.env());
    f.check(
        &f.ts.admin,
        "set_protocol_fee",
        (f.ts.admin.clone(), recipient.clone(), 10_i128),
    );
    let stranger = Address::generate(f.env());
    f.env().mock_all_auths();
    assert_eq!(
        f.client().try_set_protocol_fee(&stranger, &recipient, &10),
        Err(Ok(AmmError::Unauthorized))
    );
}

#[test]
fn test_set_lp_rebate_requires_admin_auth() {
    let f = Fixture::new();
    f.check(
        &f.ts.admin,
        "set_lp_rebate",
        (f.ts.admin.clone(), 1_000_i128),
    );
    let stranger = Address::generate(f.env());
    f.env().mock_all_auths();
    assert_eq!(
        f.client().try_set_lp_rebate(&stranger, &1_000),
        Err(Ok(AmmError::Unauthorized))
    );
}

#[test]
fn test_set_multisig_requires_admin_auth() {
    let f = Fixture::new();
    let signers = soroban_sdk::Vec::<Address>::new(f.env());
    f.check(
        &f.ts.admin,
        "set_multisig",
        (f.ts.admin.clone(), signers.clone(), 0_u32),
    );
    let stranger = Address::generate(f.env());
    f.env().mock_all_auths();
    assert_eq!(
        f.client().try_set_multisig(&stranger, &signers, &0),
        Err(Ok(AmmError::Unauthorized))
    );
}

#[test]
fn test_propose_admin_requires_admin_auth() {
    let f = Fixture::new();
    let nominee = Address::generate(f.env());
    f.check(
        &f.ts.admin,
        "propose_admin",
        (f.ts.admin.clone(), nominee.clone()),
    );
    let stranger = Address::generate(f.env());
    f.env().mock_all_auths();
    assert_eq!(
        f.client().try_propose_admin(&stranger, &nominee),
        Err(Ok(AmmError::Unauthorized))
    );
}

#[test]
fn test_accept_admin_requires_nominee_auth() {
    let f = Fixture::new();
    let nominee = Address::generate(f.env());
    let stranger = Address::generate(f.env());
    f.env().mock_all_auths();
    f.client().propose_admin(&f.ts.admin, &nominee);
    assert_eq!(
        f.client().try_accept_admin(&stranger),
        Err(Ok(AmmError::WrongAdmin))
    );
    f.check(&nominee, "accept_admin", (nominee.clone(),));
}

#[test]
fn test_announce_emergency_withdraw_requires_admin_auth() {
    let f = Fixture::new();
    let to = Address::generate(f.env());
    f.env().mock_all_auths();
    f.client().pause();
    f.check(
        &f.ts.admin,
        "announce_emergency_withdraw",
        (f.ts.admin.clone(), to),
    );
}

#[test]
fn test_emergency_withdraw_requires_admin_auth() {
    let f = Fixture::new();
    seed_pool(&f.ts);
    let to = Address::generate(f.env());
    f.env().mock_all_auths();
    f.client().pause();
    f.client().announce_emergency_withdraw(&f.ts.admin, &to);
    f.env()
        .ledger()
        .set_timestamp(f.env().ledger().timestamp() + 86_400);
    f.check(
        &f.ts.admin,
        "emergency_withdraw",
        (f.ts.admin.clone(), to.clone()),
    );
    let stranger = Address::generate(f.env());
    f.env().mock_all_auths();
    assert_eq!(
        f.client().try_emergency_withdraw(&stranger, &to),
        Err(Ok(AmmError::Unauthorized))
    );
}
