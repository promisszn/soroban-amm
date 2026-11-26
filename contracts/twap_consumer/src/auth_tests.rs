extern crate std;

use super::*;
use soroban_sdk::{
    contract, contractimpl, symbol_short,
    testutils::{Address as _, Ledger as _, MockAuth, MockAuthInvoke},
    Address, Env, IntoVal, Symbol, Val,
};

#[contract]
struct AuthMockAmmPool;

#[contractimpl]
impl AuthMockAmmPool {
    pub fn get_price_cumulative(env: Env) -> (i128, i128, u64) {
        env.storage()
            .instance()
            .get(&symbol_short!("cum"))
            .unwrap_or((1_000_000_i128, 1_000_000_i128, 10_000_u64))
    }
}

/// Asserts that a privileged invocation strictly enforces authorization:
/// 1. Fails with `Error(Auth, InvalidAction)` when no authorizations are provided.
/// 2. Fails with `Error(Auth, InvalidAction)` when only a stranger's authorization is provided.
/// 3. Succeeds when only `signer_expected`'s authorization is provided.
fn assert_requires_auth(
    env: &Env,
    _contract_id: &Address,
    signer_expected: &Address,
    invoke: &MockAuthInvoke,
) {
    let stranger = Address::generate(env);
    let stranger_auth = [MockAuth {
        address: &stranger,
        invoke,
    }];
    let expected_auth = [MockAuth {
        address: signer_expected,
        invoke,
    }];

    // 1. No authorizations
    env.set_auths(&[]);
    assert_auth_err(env, invoke, "no authorizations");

    // 2. Stranger authorization only
    env.mock_auths(&stranger_auth);
    assert_auth_err(env, invoke, "stranger authorization");

    // 3. Expected signer authorization only
    env.mock_auths(&expected_auth);
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        env.invoke_contract::<Val>(
            invoke.contract,
            &Symbol::new(env, invoke.fn_name),
            invoke.args.clone(),
        );
    }));
    assert!(
        res.is_ok(),
        "expected {} to succeed with authorization from {:?}, failed: {:?}",
        invoke.fn_name,
        signer_expected,
        res
    );
}

fn assert_auth_err(env: &Env, invoke: &MockAuthInvoke, context: &str) {
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        env.invoke_contract::<Val>(
            invoke.contract,
            &Symbol::new(env, invoke.fn_name),
            invoke.args.clone(),
        );
    }));

    assert!(
        res.is_err(),
        "expected {} to fail under {}, but it succeeded",
        invoke.fn_name,
        context
    );
    let err = res.unwrap_err();
    let msg = if let Some(s) = err.downcast_ref::<&str>() {
        std::string::String::from(*s)
    } else if let Some(s) = err.downcast_ref::<std::string::String>() {
        s.clone()
    } else {
        std::string::String::new()
    };
    assert!(
        msg.contains("Error(Auth, InvalidAction)"),
        "expected Error(Auth, InvalidAction) for {} under {}, got: {}",
        invoke.fn_name,
        context,
        msg
    );
}

struct Fixture {
    env: Env,
    keeper: Address,
    contract_addr: Address,
    client: TwapConsumerClient<'static>,
}

fn setup_fixture() -> Fixture {
    let env = Env::default();
    env.ledger().set_timestamp(10_000);
    env.mock_all_auths();
    let keeper = Address::generate(&env);
    let contract_addr = env.register_contract(None, TwapConsumer);
    let client = TwapConsumerClient::new(&env, &contract_addr);
    client.initialize(&keeper);
    Fixture {
        env,
        keeper,
        contract_addr,
        client,
    }
}

#[test]
fn test_save_snapshot_requires_auth() {
    let f = setup_fixture();
    let pool = f.env.register_contract(None, AuthMockAmmPool);
    let invoke = MockAuthInvoke {
        contract: &f.contract_addr,
        fn_name: "save_snapshot",
        args: (pool.clone(),).into_val(&f.env),
        sub_invokes: &[],
    };
    assert_requires_auth(&f.env, &f.contract_addr, &f.keeper, &invoke);
    assert_eq!(f.client.get_snapshot_count(&pool), 1);
}

#[test]
fn test_save_cl_snapshot_requires_auth() {
    let f = setup_fixture();
    let pool = f.env.register_contract(None, MockClPool);
    let invoke = MockAuthInvoke {
        contract: &f.contract_addr,
        fn_name: "save_cl_snapshot",
        args: (pool.clone(),).into_val(&f.env),
        sub_invokes: &[],
    };
    assert_requires_auth(&f.env, &f.contract_addr, &f.keeper, &invoke);
    assert_eq!(f.client.get_snapshot_count(&pool), 1);
}

#[test]
fn test_delete_snapshot_requires_auth() {
    let f = setup_fixture();
    let pool = f.env.register_contract(None, AuthMockAmmPool);

    // Save 3 snapshots so deleting the oldest leaves 2 retained snapshots.
    f.env.ledger().set_timestamp(10_000);
    f.client.save_snapshot(&pool);
    f.env.ledger().set_timestamp(10_060);
    f.client.save_snapshot(&pool);
    f.env.ledger().set_timestamp(10_120);
    f.client.save_snapshot(&pool);
    assert_eq!(f.client.get_snapshot_count(&pool), 3);

    let invoke = MockAuthInvoke {
        contract: &f.contract_addr,
        fn_name: "delete_snapshot",
        args: (pool.clone(), 10_000_u64).into_val(&f.env),
        sub_invokes: &[],
    };
    assert_requires_auth(&f.env, &f.contract_addr, &f.keeper, &invoke);
    assert_eq!(f.client.get_snapshot_count(&pool), 2);
}

#[test]
fn test_set_retention_policy_requires_auth() {
    let f = setup_fixture();
    let policy = RetentionPolicy {
        max_age_seconds: 172_800,
        max_snapshots_per_pool: 50,
    };
    let invoke = MockAuthInvoke {
        contract: &f.contract_addr,
        fn_name: "set_retention_policy",
        args: (f.keeper.clone(), policy.clone()).into_val(&f.env),
        sub_invokes: &[],
    };
    assert_requires_auth(&f.env, &f.contract_addr, &f.keeper, &invoke);
    assert_eq!(f.client.get_retention_policy(), policy);
}

#[test]
fn test_set_retention_policy_rejects_stranger_as_admin() {
    let f = setup_fixture();
    let stranger = Address::generate(&f.env);
    let policy = RetentionPolicy {
        max_age_seconds: 172_800,
        max_snapshots_per_pool: 50,
    };
    let invoke = MockAuthInvoke {
        contract: &f.contract_addr,
        fn_name: "set_retention_policy",
        args: (stranger.clone(), policy.clone()).into_val(&f.env),
        sub_invokes: &[],
    };
    f.env.mock_auths(&[MockAuth {
        address: &stranger,
        invoke: &invoke,
    }]);
    let res = f.client.try_set_retention_policy(&stranger, &policy);
    assert_eq!(res, Err(Ok(TwapError::Unauthorized)));
}

#[test]
fn test_propose_keeper_requires_auth() {
    let f = setup_fixture();
    let new_keeper = Address::generate(&f.env);
    let invoke = MockAuthInvoke {
        contract: &f.contract_addr,
        fn_name: "propose_keeper",
        args: (new_keeper.clone(),).into_val(&f.env),
        sub_invokes: &[],
    };
    assert_requires_auth(&f.env, &f.contract_addr, &f.keeper, &invoke);
    assert_eq!(f.client.get_pending_keeper(), Some(new_keeper));
}

#[test]
fn test_accept_keeper_requires_auth() {
    let f = setup_fixture();
    let new_keeper = Address::generate(&f.env);
    f.client.propose_keeper(&new_keeper);

    let invoke = MockAuthInvoke {
        contract: &f.contract_addr,
        fn_name: "accept_keeper",
        args: (new_keeper.clone(),).into_val(&f.env),
        sub_invokes: &[],
    };
    assert_requires_auth(&f.env, &f.contract_addr, &new_keeper, &invoke);
    assert_eq!(f.client.get_keeper(), new_keeper);
}

#[test]
fn test_accept_keeper_rejects_stranger_as_new_keeper() {
    let f = setup_fixture();
    let pending_keeper = Address::generate(&f.env);
    let stranger = Address::generate(&f.env);
    f.client.propose_keeper(&pending_keeper);

    let invoke = MockAuthInvoke {
        contract: &f.contract_addr,
        fn_name: "accept_keeper",
        args: (stranger.clone(),).into_val(&f.env),
        sub_invokes: &[],
    };
    f.env.mock_auths(&[MockAuth {
        address: &stranger,
        invoke: &invoke,
    }]);
    let res = f.client.try_accept_keeper(&stranger);
    assert_eq!(res, Err(Ok(TwapError::WrongPendingKeeper)));
}

#[test]
fn test_configure_public_snapshots_requires_auth() {
    let f = setup_fixture();
    let factory = Address::generate(&f.env);
    let invoke = MockAuthInvoke {
        contract: &f.contract_addr,
        fn_name: "configure_public_snapshots",
        args: (factory, 300_u64).into_val(&f.env),
        sub_invokes: &[],
    };
    assert_requires_auth(&f.env, &f.contract_addr, &f.keeper, &invoke);
}

#[test]
fn test_add_tracked_pool_requires_auth() {
    let f = setup_fixture();
    let pool = Address::generate(&f.env);
    let invoke = MockAuthInvoke {
        contract: &f.contract_addr,
        fn_name: "add_tracked_pool",
        args: (pool.clone(), PoolType::Amm).into_val(&f.env),
        sub_invokes: &[],
    };
    assert_requires_auth(&f.env, &f.contract_addr, &f.keeper, &invoke);
    assert!(f.client.is_tracked(&pool));
}

#[test]
fn test_remove_tracked_pool_requires_auth() {
    let f = setup_fixture();
    let pool = Address::generate(&f.env);
    f.client.add_tracked_pool(&pool, &PoolType::Amm);

    let invoke = MockAuthInvoke {
        contract: &f.contract_addr,
        fn_name: "remove_tracked_pool",
        args: (pool.clone(),).into_val(&f.env),
        sub_invokes: &[],
    };
    assert_requires_auth(&f.env, &f.contract_addr, &f.keeper, &invoke);
    assert!(!f.client.is_tracked(&pool));
}

/// Seeds three snapshots for `pool`, all age-eligible once the ledger is moved
/// past the default 7-day retention window.
fn seed_prunable(f: &Fixture, pool: &Address) {
    for ts in [10_000_u64, 10_060, 10_120] {
        f.env.ledger().set_timestamp(ts);
        f.client.save_snapshot(pool);
    }
    f.env
        .ledger()
        .set_timestamp(10_120 + TwapConsumer::DEFAULT_MAX_AGE_SECONDS + 1);
}

#[test]
fn test_prune_snapshots_requires_auth() {
    // #1333: pruning deletes TWAP history, so it is keeper-only like
    // delete_snapshot.
    let f = setup_fixture();
    let pool = f.env.register_contract(None, AuthMockAmmPool);
    seed_prunable(&f, &pool);

    let invoke = MockAuthInvoke {
        contract: &f.contract_addr,
        fn_name: "prune_snapshots",
        args: (pool.clone(), 2_u32).into_val(&f.env),
        sub_invokes: &[],
    };
    assert_requires_auth(&f.env, &f.contract_addr, &f.keeper, &invoke);
    assert_eq!(f.client.get_snapshot_count(&pool), 1);
}

#[test]
fn test_prune_all_requires_auth() {
    let f = setup_fixture();
    let pool = f.env.register_contract(None, AuthMockAmmPool);
    seed_prunable(&f, &pool);

    let invoke = MockAuthInvoke {
        contract: &f.contract_addr,
        fn_name: "prune_all",
        args: (2_u32,).into_val(&f.env),
        sub_invokes: &[],
    };
    assert_requires_auth(&f.env, &f.contract_addr, &f.keeper, &invoke);
    assert_eq!(f.client.get_snapshot_count(&pool), 1);
}
