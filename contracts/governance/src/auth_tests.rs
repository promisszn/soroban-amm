//! Regression tests that every admin-only `Governance` entrypoint demands the
//! right signature.
//!
//! These tests never call `env.mock_all_auths()` around the call under test.
//! Each entrypoint is invoked with (a) no authorization, (b) only a stranger's
//! authorization, and (c) only the expected signer's authorization, so deleting
//! a `require_auth()` makes at least one case fail. Functions that also take an
//! explicit `admin` argument get a second check that a stranger passed in that
//! slot is refused with `GovernanceError::Unauthorized`.

extern crate std;

use super::*;
use soroban_sdk::{
    testutils::{Address as _, MockAuth, MockAuthInvoke},
    Address, Env, IntoVal, Symbol, Val,
};

const WASM_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../target/wasm32v1-none/release/governance.wasm"
);

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

/// No authorizations and a stranger's alone must fail with an auth error;
/// `signer_expected`'s alone must succeed.
fn assert_requires_auth(env: &Env, signer_expected: &Address, invoke: &MockAuthInvoke) {
    let stranger = Address::generate(env);

    env.set_auths(&[]);
    assert_auth_err(env, invoke, "no authorizations");

    env.mock_auths(&[MockAuth {
        address: &stranger,
        invoke,
    }]);
    assert_auth_err(env, invoke, "a stranger's authorization");

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
}

struct Fixture {
    env: Env,
    addr: Address,
    admin: Address,
}

impl Fixture {
    fn new() -> Self {
        let env = Env::default();
        env.mock_all_auths();
        env.budget().reset_unlimited();
        let admin = Address::generate(&env);
        let addr = env.register_contract(None, Governance);
        GovernanceClient::new(&env, &addr).initialize(
            &admin,
            &Address::generate(&env), // amm pool: never called by the admin entrypoints
            &Address::generate(&env), // lp token
            &(7 * 24 * 60 * 60_u64),
            &(2 * 24 * 60 * 60_u64),
            &1_000_i128,
            &100_i128,
        );
        Self { env, addr, admin }
    }

    fn client(&self) -> GovernanceClient<'_> {
        GovernanceClient::new(&self.env, &self.addr)
    }

    fn check<A: IntoVal<Env, soroban_sdk::Vec<Val>>>(
        &self,
        signer: &Address,
        fn_name: &'static str,
        args: A,
    ) {
        let invoke = MockAuthInvoke {
            contract: &self.addr,
            fn_name,
            args: args.into_val(&self.env),
            sub_invokes: &[],
        };
        assert_requires_auth(&self.env, signer, &invoke);
    }
}

#[test]
fn test_set_quorum_decay_bps_per_day_requires_admin_auth() {
    let f = Fixture::new();
    f.check(&f.admin, "set_quorum_decay_bps_per_day", (25_i128,));
}

#[test]
fn test_set_min_proposer_stake_bps_requires_admin_auth() {
    let f = Fixture::new();
    f.check(&f.admin, "set_min_proposer_stake_bps", (200_i128,));
}

#[test]
fn test_set_timelock_delay_requires_admin_auth() {
    let f = Fixture::new();
    f.check(&f.admin, "set_timelock_delay", (3 * 24 * 60 * 60_u64,));
}

#[test]
fn test_set_veto_multisig_requires_admin_auth() {
    let f = Fixture::new();
    let multisig = Address::generate(&f.env);
    f.check(&f.admin, "set_veto_multisig", (multisig,));
}

#[test]
fn test_propose_admin_requires_admin_auth() {
    let f = Fixture::new();
    let nominee = Address::generate(&f.env);
    f.check(
        &f.admin,
        "propose_admin",
        (f.admin.clone(), nominee.clone()),
    );
    let stranger = Address::generate(&f.env);
    f.env.mock_all_auths();
    assert_eq!(
        f.client().try_propose_admin(&stranger, &nominee),
        Err(Ok(GovernanceError::Unauthorized))
    );
}

#[test]
fn test_accept_admin_requires_nominee_auth() {
    let f = Fixture::new();
    let nominee = Address::generate(&f.env);
    let stranger = Address::generate(&f.env);
    f.client().propose_admin(&f.admin, &nominee);
    assert_eq!(
        f.client().try_accept_admin(&stranger),
        Err(Ok(GovernanceError::Unauthorized))
    );
    f.check(&nominee, "accept_admin", (nominee.clone(),));
    assert_eq!(f.client().get_admin(), nominee);
}

#[test]
fn test_upgrade_requires_admin_auth() {
    let f = Fixture::new();
    let wasm = soroban_sdk::Bytes::from_slice(&f.env, &std::fs::read(WASM_PATH).unwrap());
    let hash = f.env.deployer().upload_contract_wasm(wasm);
    f.check(&f.admin, "upgrade", (f.admin.clone(), hash.clone()));
    let stranger = Address::generate(&f.env);
    f.env.mock_all_auths();
    assert_eq!(
        f.client().try_upgrade(&stranger, &hash),
        Err(Ok(GovernanceError::Unauthorized))
    );
}
