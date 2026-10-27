//! Authorization tests for privileged `ConcentratedLiquidity` entrypoints.
//!
//! Verifies that all 11 admin functions require proper authorization from the
//! correct signer. Each test checks three cases:
//! 1. No authorization -> Error(Auth, InvalidAction)
//! 2. Stranger's authorization only -> Error(Auth, InvalidAction)
//! 3. Correct signer's authorization -> Success
//!
//! For functions that take an admin argument, we also verify that passing a
//! stranger's address as the admin argument fails with Unauthorized.

extern crate std;

use super::*;
use soroban_sdk::{
    testutils::{Address as _, Ledger, MockAuth, MockAuthInvoke},
    Address, Env, IntoVal, Symbol, Val,
};
use soroban_sdk::token::StellarAssetClient;

#[cfg(feature = "testutils")]
use super::WASM as CL_WASM;
#[cfg(not(feature = "testutils"))]
const CL_WASM: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../target/wasm32v1-none/release/concentrated_liquidity.wasm"
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
        invoke.fn_name, context
    ));
    assert!(
        msg.contains("Error(Auth, InvalidAction)"),
        "{} under {}: expected Error(Auth, InvalidAction), got: {}",
        invoke.fn_name,
        context,
        msg
    );
}

/// Tests the three authorization cases:
/// 1. No authorizations -> Error(Auth, InvalidAction)
/// 2. Only a stranger's authorization -> Error(Auth, InvalidAction)
/// 3. Only the expected signer's authorization -> Success
fn assert_requires_auth(env: &Env, signer_expected: &Address, invoke: &MockAuthInvoke) {
    let stranger = Address::generate(env);

    // Case 1: No authorization
    env.set_auths(&[]);
    assert_auth_err(env, invoke, "no authorizations");

    // Case 2: Stranger's authorization
    env.mock_auths(&[MockAuth {
        address: &stranger,
        invoke,
    }]);
    assert_auth_err(env, invoke, "a stranger's authorization");

    // Case 3: Correct signer's authorization
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
    admin: Address,
    cl_addr: Address,
    client: ConcentratedLiquidityClient<'static>,
    token_a: Address,
    token_b: Address,
}

impl Fixture {
    fn new() -> Self {
        let env = Env::default();
        env.budget().reset_unlimited();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let provider = Address::generate(&env);

        let token_a_sac = env.register_stellar_asset_contract_v2(admin.clone());
        let token_b_sac = env.register_stellar_asset_contract_v2(admin.clone());
        let token_a = token_a_sac.address();
        let token_b = token_b_sac.address();

        let cl_addr = env.register_contract(None, ConcentratedLiquidity);
        let client = ConcentratedLiquidityClient::new(&env, &cl_addr);
        client.initialize(&admin, &token_a, &token_b, &30_i128, &0_i32, &1_i32);

        // Mint tokens for testing
        let sac_a = StellarAssetClient::new(&env, &token_a);
        let sac_b = StellarAssetClient::new(&env, &token_b);
        sac_a.mint(&provider, &10_000_000_i128);
        sac_b.mint(&provider, &10_000_000_i128);

        Self {
            env,
            admin,
            cl_addr,
            client,
            token_a,
            token_b,
        }
    }

    fn env(&self) -> &Env {
        &self.env
    }

    fn check<A: IntoVal<Env, soroban_sdk::Vec<Val>>>(
        &self,
        signer: &Address,
        fn_name: &'static str,
        args: A,
    ) {
        let invoke = MockAuthInvoke {
            contract: &self.cl_addr,
            fn_name,
            args: args.into_val(self.env()),
            sub_invokes: &[],
        };
        assert_requires_auth(self.env(), signer, &invoke);
    }
}

#[test]
fn test_pause_requires_admin_auth() {
    let f = Fixture::new();
    f.check(&f.admin, "pause", (f.admin.clone(),));
    assert!(f.client.is_paused());
}

#[test]
fn test_unpause_requires_admin_auth() {
    let f = Fixture::new();
    f.env().mock_all_auths();
    f.client.pause(&f.admin);
    f.check(&f.admin, "unpause", (f.admin.clone(),));
    assert!(!f.client.is_paused());
}

#[test]
fn test_set_oracle_requires_admin_auth() {
    let f = Fixture::new();
    f.check(
        &f.admin,
        "set_oracle",
        (f.admin.clone(), Option::<Address>::None),
    );
    
    // Also check that a stranger passed as admin is rejected
    let stranger = Address::generate(f.env());
    f.env().mock_all_auths();
    assert_eq!(
        f.client.try_set_oracle(&stranger, &None),
        Err(Ok(ClError::Unauthorized))
    );
}

#[test]
fn test_set_position_nft_requires_admin_auth() {
    let f = Fixture::new();
    f.check(
        &f.admin,
        "set_position_nft",
        (f.admin.clone(), Option::<Address>::None),
    );
    
    // Also check that a stranger passed as admin is rejected
    let stranger = Address::generate(f.env());
    f.env().mock_all_auths();
    assert_eq!(
        f.client.try_set_position_nft(&stranger, &None),
        Err(Ok(ClError::Unauthorized))
    );
}

#[test]
fn test_set_max_oracle_deviation_bps_requires_admin_auth() {
    let f = Fixture::new();
    f.check(
        &f.admin,
        "set_max_oracle_deviation_bps",
        (f.admin.clone(), 600_i128),
    );
    
    // Also check that a stranger passed as admin is rejected
    let stranger = Address::generate(f.env());
    f.env().mock_all_auths();
    assert_eq!(
        f.client.try_set_max_oracle_deviation_bps(&stranger, &600),
        Err(Ok(ClError::Unauthorized))
    );
}

#[test]
fn test_propose_admin_requires_admin_auth() {
    let f = Fixture::new();
    let nominee = Address::generate(f.env());
    f.check(
        &f.admin,
        "propose_admin",
        (f.admin.clone(), nominee.clone()),
    );
    
    // Also check that a stranger passed as admin is rejected
    let stranger = Address::generate(f.env());
    f.env().mock_all_auths();
    assert_eq!(
        f.client.try_propose_admin(&stranger, &nominee),
        Err(Ok(ClError::Unauthorized))
    );
}

#[test]
fn test_accept_admin_requires_nominee_auth() {
    let f = Fixture::new();
    let nominee = Address::generate(f.env());
    let stranger = Address::generate(f.env());
    
    // Set up: propose a new admin
    f.env().mock_all_auths();
    f.client.propose_admin(&f.admin, &nominee);
    
    // Verify a stranger cannot accept
    assert_eq!(
        f.client.try_accept_admin(&stranger),
        Err(Ok(ClError::Unauthorized))
    );
    
    // Verify the nominee needs proper authorization
    f.check(&nominee, "accept_admin", (nominee.clone(),));
}

#[test]
fn test_upgrade_requires_admin_auth() {
    let f = Fixture::new();
    f.env().budget().reset_unlimited();
    let wasm_hash = f.env().deployer().upload_contract_wasm(CL_WASM);
    f.check(&f.admin, "upgrade", (f.admin.clone(), wasm_hash));
}

#[test]
fn test_upgrade_rejects_stranger_as_admin() {
    let f = Fixture::new();
    f.env().budget().reset_unlimited();
    let wasm_hash = f.env().deployer().upload_contract_wasm(CL_WASM);
    let stranger = Address::generate(f.env());
    f.env().mock_all_auths();
    assert_eq!(
        f.client.try_upgrade(&stranger, &wasm_hash),
        Err(Ok(ClError::Unauthorized))
    );
}

#[test]
fn test_set_protocol_fee_requires_admin_auth() {
    let f = Fixture::new();
    let recipient = Address::generate(f.env());
    f.check(
        &f.admin,
        "set_protocol_fee",
        (f.admin.clone(), recipient.clone(), 10_i128),
    );
    
    // Also check that a stranger passed as admin is rejected
    let stranger = Address::generate(f.env());
    f.env().mock_all_auths();
    assert_eq!(
        f.client.try_set_protocol_fee(&stranger, &recipient, &10),
        Err(Ok(ClError::Unauthorized))
    );
}

#[test]
fn test_withdraw_protocol_fees_requires_admin_auth() {
    let f = Fixture::new();
    f.check(&f.admin, "withdraw_protocol_fees", (f.admin.clone(),));
    
    // Also check that a stranger passed as admin is rejected
    let stranger = Address::generate(f.env());
    f.env().mock_all_auths();
    assert_eq!(
        f.client.try_withdraw_protocol_fees(&stranger),
        Err(Ok(ClError::Unauthorized))
    );
}

#[test]
fn test_set_oracle_cardinality_requires_admin_auth() {
    let f = Fixture::new();
    f.check(
        &f.admin,
        "set_oracle_cardinality",
        (f.admin.clone(), 1024_u32),
    );
    
    // Also check that a stranger passed as admin is rejected
    let stranger = Address::generate(f.env());
    f.env().mock_all_auths();
    assert_eq!(
        f.client.try_set_oracle_cardinality(&stranger, &1024),
        Err(Ok(ClError::Unauthorized))
    );
}
