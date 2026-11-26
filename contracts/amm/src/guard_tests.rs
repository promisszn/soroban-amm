extern crate std;

use std::{borrow::ToOwned, string::String, vec::Vec};

use super::*;
use crate::tests::{setup_pool, TestSetup};
use soroban_sdk::{
    contract, contractimpl, contracttype,
    testutils::{Address as _, Ledger},
    token::StellarAssetClient,
    Env,
};

const DEADLINE_TESTS: &[(&str, &str)] = &[
    ("add_liquidity", "test_deadline_add_liquidity_at_now"),
    ("remove_liquidity", "test_deadline_remove_liquidity_at_now"),
    (
        "remove_liquidity_one_sided",
        "test_deadline_remove_liquidity_one_sided_at_now",
    ),
    ("swap", "test_deadline_swap_at_now"),
    ("swap_exact_out", "test_deadline_swap_exact_out_at_now"),
    ("swap_fot", "test_deadline_swap_fot_at_now"),
    (
        "add_liquidity_fot",
        "test_deadline_add_liquidity_fot_at_now",
    ),
];

const LEDGER_NOW: u64 = 12_345;

type DeadlineCall<T> =
    Result<Result<T, soroban_sdk::Error>, Result<AmmError, soroban_sdk::InvokeError>>;

fn seeded_pool() -> (TestSetup, Address) {
    let ts = setup_pool(30);
    let provider = Address::generate(&ts.env);
    StellarAssetClient::new(&ts.env, &ts.ta_addr).mint(&provider, &10_000_000);
    StellarAssetClient::new(&ts.env, &ts.tb_addr).mint(&provider, &10_000_000);
    AmmPoolClient::new(&ts.env, &ts.amm_addr).add_liquidity(
        &provider,
        &10_000_000,
        &10_000_000,
        &0,
        &u64::MAX,
    );
    (ts, provider)
}

fn assert_deadline<T>(result: DeadlineCall<T>, expired: bool) {
    if expired {
        assert!(matches!(result, Err(Ok(AmmError::DeadlineExceeded))));
    } else {
        assert!(matches!(result, Ok(Ok(_))));
    }
}

fn add_liquidity_deadline_call(deadline: u64) -> DeadlineCall<i128> {
    let ts = setup_pool(30);
    let provider = Address::generate(&ts.env);
    StellarAssetClient::new(&ts.env, &ts.ta_addr).mint(&provider, &1_000_000);
    StellarAssetClient::new(&ts.env, &ts.tb_addr).mint(&provider, &1_000_000);
    AmmPoolClient::new(&ts.env, &ts.amm_addr)
        .try_add_liquidity(&provider, &1_000_000, &1_000_000, &0, &deadline)
}

fn remove_liquidity_deadline_call(deadline: u64) -> DeadlineCall<(i128, i128)> {
    let (ts, provider) = seeded_pool();
    AmmPoolClient::new(&ts.env, &ts.amm_addr)
        .try_remove_liquidity(&provider, &100_000, &0, &0, &deadline)
}

fn remove_liquidity_one_sided_deadline_call(deadline: u64) -> DeadlineCall<i128> {
    let (ts, provider) = seeded_pool();
    AmmPoolClient::new(&ts.env, &ts.amm_addr).try_remove_liquidity_one_sided(
        &provider,
        &100_000,
        &ts.ta_addr,
        &0,
        &deadline,
    )
}

fn swap_deadline_call(deadline: u64) -> DeadlineCall<i128> {
    let (ts, _) = seeded_pool();
    let trader = Address::generate(&ts.env);
    StellarAssetClient::new(&ts.env, &ts.ta_addr).mint(&trader, &100_000);
    AmmPoolClient::new(&ts.env, &ts.amm_addr).try_swap(&trader, &ts.ta_addr, &10_000, &0, &deadline)
}

fn swap_exact_out_deadline_call(deadline: u64) -> DeadlineCall<i128> {
    let (ts, _) = seeded_pool();
    let trader = Address::generate(&ts.env);
    StellarAssetClient::new(&ts.env, &ts.ta_addr).mint(&trader, &100_000);
    AmmPoolClient::new(&ts.env, &ts.amm_addr).try_swap_exact_out(
        &trader,
        &ts.tb_addr,
        &1_000,
        &100_000,
        &deadline,
    )
}

fn swap_fot_deadline_call(deadline: u64) -> DeadlineCall<(i128, i128)> {
    let (ts, _) = seeded_pool();
    let trader = Address::generate(&ts.env);
    StellarAssetClient::new(&ts.env, &ts.ta_addr).mint(&trader, &100_000);
    AmmPoolClient::new(&ts.env, &ts.amm_addr).try_swap_fot(
        &trader,
        &ts.ta_addr,
        &10_000,
        &0,
        &0,
        &deadline,
        &None,
    )
}

fn add_liquidity_fot_deadline_call(deadline: u64) -> DeadlineCall<i128> {
    let ts = setup_pool(30);
    let provider = Address::generate(&ts.env);
    StellarAssetClient::new(&ts.env, &ts.ta_addr).mint(&provider, &1_000_000);
    StellarAssetClient::new(&ts.env, &ts.tb_addr).mint(&provider, &1_000_000);
    AmmPoolClient::new(&ts.env, &ts.amm_addr)
        .try_add_liquidity_fot(&provider, &1_000_000, &1_000_000, &0, &0, &0, &deadline)
}

macro_rules! deadline_boundary_tests {
    ($expired:ident, $at_now:ident, $future:ident, $call:ident) => {
        #[test]
        fn $expired() {
            assert_deadline($call(LEDGER_NOW - 1), true);
        }

        #[test]
        fn $at_now() {
            assert_deadline($call(LEDGER_NOW), false);
        }

        #[test]
        fn $future() {
            assert_deadline($call(LEDGER_NOW + 1), false);
        }
    };
}

deadline_boundary_tests!(
    test_deadline_add_liquidity_expired,
    test_deadline_add_liquidity_at_now,
    test_deadline_add_liquidity_one_second_future,
    add_liquidity_deadline_call
);
deadline_boundary_tests!(
    test_deadline_remove_liquidity_expired,
    test_deadline_remove_liquidity_at_now,
    test_deadline_remove_liquidity_one_second_future,
    remove_liquidity_deadline_call
);
deadline_boundary_tests!(
    test_deadline_remove_liquidity_one_sided_expired,
    test_deadline_remove_liquidity_one_sided_at_now,
    test_deadline_remove_liquidity_one_sided_one_second_future,
    remove_liquidity_one_sided_deadline_call
);
deadline_boundary_tests!(
    test_deadline_swap_expired,
    test_deadline_swap_at_now,
    test_deadline_swap_one_second_future,
    swap_deadline_call
);
deadline_boundary_tests!(
    test_deadline_swap_exact_out_expired,
    test_deadline_swap_exact_out_at_now,
    test_deadline_swap_exact_out_one_second_future,
    swap_exact_out_deadline_call
);
deadline_boundary_tests!(
    test_deadline_swap_fot_expired,
    test_deadline_swap_fot_at_now,
    test_deadline_swap_fot_one_second_future,
    swap_fot_deadline_call
);
deadline_boundary_tests!(
    test_deadline_add_liquidity_fot_expired,
    test_deadline_add_liquidity_fot_at_now,
    test_deadline_add_liquidity_fot_one_second_future,
    add_liquidity_fot_deadline_call
);

#[test]
fn test_slippage_add_liquidity_min_shares_boundary() {
    let (ts, provider) = seeded_pool();
    let amm = AmmPoolClient::new(&ts.env, &ts.amm_addr);
    let expected = 100_000_i128;
    StellarAssetClient::new(&ts.env, &ts.ta_addr).mint(&provider, &100_000);
    StellarAssetClient::new(&ts.env, &ts.tb_addr).mint(&provider, &100_000);

    assert_eq!(
        amm.try_add_liquidity(&provider, &100_000, &100_000, &(expected + 1), &u64::MAX),
        Err(Ok(AmmError::SlippageExceeded))
    );
    assert_eq!(
        amm.try_add_liquidity(&provider, &100_000, &100_000, &expected, &u64::MAX),
        Ok(Ok(expected))
    );
}

#[test]
fn test_slippage_remove_liquidity_min_outputs_boundary() {
    let (ts, provider) = seeded_pool();
    let amm = AmmPoolClient::new(&ts.env, &ts.amm_addr);
    let shares = 100_000_i128;
    let info = amm.get_info();
    let min_a = shares * info.reserve_a / info.total_shares;
    let min_b = shares * info.reserve_b / info.total_shares;

    assert_eq!(
        amm.try_remove_liquidity(&provider, &shares, &(min_a + 1), &min_b, &u64::MAX),
        Err(Ok(AmmError::SlippageExceeded))
    );
    assert_eq!(
        amm.try_remove_liquidity(&provider, &shares, &min_a, &min_b, &u64::MAX),
        Ok(Ok((min_a, min_b)))
    );
}

fn one_sided_output(shares: i128) -> i128 {
    let (ts, provider) = seeded_pool();
    AmmPoolClient::new(&ts.env, &ts.amm_addr).remove_liquidity_one_sided(
        &provider,
        &shares,
        &ts.ta_addr,
        &0,
        &u64::MAX,
    )
}

#[test]
fn test_slippage_remove_liquidity_one_sided_min_out_boundary() {
    let shares = 100_000_i128;
    let expected = one_sided_output(shares);
    let (ts, provider) = seeded_pool();
    let amm = AmmPoolClient::new(&ts.env, &ts.amm_addr);

    assert_eq!(
        amm.try_remove_liquidity_one_sided(
            &provider,
            &shares,
            &ts.ta_addr,
            &(expected + 1),
            &u64::MAX,
        ),
        Err(Ok(AmmError::SlippageExceeded))
    );
    assert_eq!(
        amm.try_remove_liquidity_one_sided(&provider, &shares, &ts.ta_addr, &expected, &u64::MAX,),
        Ok(Ok(expected))
    );
}

#[test]
fn test_slippage_swap_min_out_boundary() {
    let (ts, _) = seeded_pool();
    let amm = AmmPoolClient::new(&ts.env, &ts.amm_addr);
    let trader = Address::generate(&ts.env);
    let amount_in = 100_000_i128;
    StellarAssetClient::new(&ts.env, &ts.ta_addr).mint(&trader, &amount_in);
    let expected = amm.get_amount_out(&ts.ta_addr, &amount_in);

    assert_eq!(
        amm.try_swap(&trader, &ts.ta_addr, &amount_in, &(expected + 1), &u64::MAX),
        Err(Ok(AmmError::SlippageExceeded))
    );
    assert_eq!(
        amm.try_swap(&trader, &ts.ta_addr, &amount_in, &expected, &u64::MAX),
        Ok(Ok(expected))
    );
}

#[test]
fn test_slippage_swap_exact_out_max_in_boundary() {
    let (ts, _) = seeded_pool();
    let amm = AmmPoolClient::new(&ts.env, &ts.amm_addr);
    let trader = Address::generate(&ts.env);
    let amount_out = 100_000_i128;
    let required = amm.get_amount_in(&ts.tb_addr, &amount_out);
    StellarAssetClient::new(&ts.env, &ts.ta_addr).mint(&trader, &required);

    assert_eq!(
        amm.try_swap_exact_out(
            &trader,
            &ts.tb_addr,
            &amount_out,
            &(required - 1),
            &u64::MAX
        ),
        Err(Ok(AmmError::SlippageExceeded))
    );
    assert_eq!(
        amm.try_swap_exact_out(&trader, &ts.tb_addr, &amount_out, &required, &u64::MAX),
        Ok(Ok(required))
    );
}

#[test]
fn test_slippage_swap_fot_min_out_boundary() {
    let (ts, _) = seeded_pool();
    let amm = AmmPoolClient::new(&ts.env, &ts.amm_addr);
    let trader = Address::generate(&ts.env);
    let amount_in = 100_000_i128;
    StellarAssetClient::new(&ts.env, &ts.ta_addr).mint(&trader, &amount_in);
    let expected = amm.get_amount_out(&ts.ta_addr, &amount_in);

    assert_eq!(
        amm.try_swap_fot(
            &trader,
            &ts.ta_addr,
            &amount_in,
            &(expected + 1),
            &0,
            &u64::MAX,
            &None,
        ),
        Err(Ok(AmmError::SlippageExceeded))
    );
    assert_eq!(
        amm.try_swap_fot(
            &trader,
            &ts.ta_addr,
            &amount_in,
            &expected,
            &0,
            &u64::MAX,
            &None,
        ),
        Ok(Ok((expected, amount_in)))
    );
}

#[test]
fn test_slippage_add_liquidity_fot_min_shares_boundary() {
    let (ts, provider) = seeded_pool();
    let amm = AmmPoolClient::new(&ts.env, &ts.amm_addr);
    let amount = 100_000_i128;
    StellarAssetClient::new(&ts.env, &ts.ta_addr).mint(&provider, &amount);
    StellarAssetClient::new(&ts.env, &ts.tb_addr).mint(&provider, &amount);
    let info = amm.get_info();
    let expected = (amount * info.total_shares / info.reserve_a)
        .min(amount * info.total_shares / info.reserve_b);

    assert_eq!(
        amm.try_add_liquidity_fot(
            &provider,
            &amount,
            &amount,
            &0,
            &0,
            &(expected + 1),
            &u64::MAX
        ),
        Err(Ok(AmmError::SlippageExceeded))
    );
    assert_eq!(
        amm.try_add_liquidity_fot(&provider, &amount, &amount, &0, &0, &expected, &u64::MAX),
        Ok(Ok(expected))
    );
}

#[contract]
struct OnePercentSkimToken;

#[contracttype]
enum SkimKey {
    Balance(Address),
}

#[contractimpl]
impl OnePercentSkimToken {
    pub fn mint(env: Env, to: Address, amount: i128) {
        let balance = Self::balance(env.clone(), to.clone());
        env.storage()
            .instance()
            .set(&SkimKey::Balance(to), &(balance + amount));
    }

    pub fn balance(env: Env, id: Address) -> i128 {
        env.storage()
            .instance()
            .get(&SkimKey::Balance(id))
            .unwrap_or(0)
    }

    pub fn transfer(env: Env, from: Address, to: Address, amount: i128) {
        from.require_auth();
        let from_balance = Self::balance(env.clone(), from.clone());
        let to_balance = Self::balance(env.clone(), to.clone());
        let received = amount - amount / 100;
        env.storage()
            .instance()
            .set(&SkimKey::Balance(from), &(from_balance - amount));
        env.storage()
            .instance()
            .set(&SkimKey::Balance(to), &(to_balance + received));
    }
}

struct SkimSetup {
    env: Env,
    amm_addr: Address,
    skim_addr: Address,
    other_addr: Address,
}

fn skim_setup() -> (SkimSetup, Address) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(12_345);
    let admin = Address::generate(&env);
    let amm_addr = env.register_contract(None, AmmPool);
    let lp_addr = env.register_contract(None, token::LpToken);
    token::LpTokenClient::new(&env, &lp_addr).initialize(
        &amm_addr,
        &soroban_sdk::String::from_str(&env, "AMM LP Token"),
        &soroban_sdk::String::from_str(&env, "ALP"),
        &7_u32,
    );
    let skim_addr = env.register_contract(None, OnePercentSkimToken);
    let other_addr = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    AmmPoolClient::new(&env, &amm_addr).initialize(
        &admin,
        &skim_addr,
        &other_addr,
        &lp_addr,
        &30,
        &admin,
        &0,
    );
    let provider = Address::generate(&env);
    OnePercentSkimTokenClient::new(&env, &skim_addr).mint(&provider, &20_000_000);
    StellarAssetClient::new(&env, &other_addr).mint(&provider, &20_000_000);
    AmmPoolClient::new(&env, &amm_addr).add_liquidity_fot(
        &provider,
        &10_000_000,
        &10_000_000,
        &0,
        &0,
        &0,
        &u64::MAX,
    );
    (
        SkimSetup {
            env,
            amm_addr,
            skim_addr,
            other_addr,
        },
        provider,
    )
}

#[test]
fn test_fee_on_transfer_swap_slippage_uses_received_amount() {
    let (setup, _provider) = skim_setup();
    let amm = AmmPoolClient::new(&setup.env, &setup.amm_addr);
    let trader = Address::generate(&setup.env);
    let amount_in = 1_000_000_i128;
    let received = amount_in - amount_in / 100;
    OnePercentSkimTokenClient::new(&setup.env, &setup.skim_addr).mint(&trader, &amount_in);

    let nominal_out = amm.get_amount_out(&setup.skim_addr, &amount_in);
    let actual_out = amm.get_amount_out(&setup.skim_addr, &received);
    assert!(actual_out < nominal_out);

    assert_eq!(
        amm.try_swap_fot(
            &trader,
            &setup.skim_addr,
            &amount_in,
            &nominal_out,
            &received,
            &u64::MAX,
            &None,
        ),
        Err(Ok(AmmError::SlippageExceeded))
    );
    assert_eq!(
        amm.try_swap_fot(
            &trader,
            &setup.skim_addr,
            &amount_in,
            &actual_out,
            &received,
            &u64::MAX,
            &None,
        ),
        Ok(Ok((actual_out, received)))
    );
}

#[test]
fn test_fee_on_transfer_add_liquidity_shares_use_received_amount() {
    let (setup, provider) = skim_setup();
    let amm = AmmPoolClient::new(&setup.env, &setup.amm_addr);
    let amount = 1_000_000_i128;
    let received = amount - amount / 100;
    // Token B is deposited in surplus so the skimmed token A side is the
    // binding constraint on the minted shares; otherwise the 1% skim on the
    // seed deposit can make both sides tie and hide the difference.
    let amount_b = 2 * amount;
    OnePercentSkimTokenClient::new(&setup.env, &setup.skim_addr).mint(&provider, &amount);
    StellarAssetClient::new(&setup.env, &setup.other_addr).mint(&provider, &amount_b);
    let info = amm.get_info();
    let expected = (received * info.total_shares / info.reserve_a)
        .min(amount_b * info.total_shares / info.reserve_b);
    let nominal_shares = (amount * info.total_shares / info.reserve_a)
        .min(amount_b * info.total_shares / info.reserve_b);
    assert!(expected < nominal_shares);

    assert_eq!(
        amm.try_add_liquidity_fot(
            &provider,
            &amount,
            &amount_b,
            &received,
            &amount_b,
            &(expected + 1),
            &u64::MAX,
        ),
        Err(Ok(AmmError::SlippageExceeded))
    );
    assert_eq!(
        amm.try_add_liquidity_fot(
            &provider,
            &amount,
            &amount_b,
            &received,
            &amount_b,
            &expected,
            &u64::MAX,
        ),
        Ok(Ok(expected))
    );
}

#[test]
fn test_deadline_table_covers_public_entrypoints() {
    let source = include_str!("lib.rs");
    let implementation = source
        .split("impl AmmPool {")
        .nth(1)
        .unwrap()
        .split("// ── Tests")
        .next()
        .unwrap();
    let mut discovered: Vec<String> = Vec::new();
    let mut current: Option<(String, String)> = None;
    for line in implementation.lines() {
        if current.is_none() {
            if let Some(signature) = line.trim().strip_prefix("pub fn ") {
                let name = signature.split('(').next().unwrap();
                current = Some((name.to_owned(), signature.to_owned()));
            }
        } else if let Some((_, signature)) = current.as_mut() {
            signature.push_str(line);
        }
        // A signature ends at the body's opening brace, with or without a
        // return type.
        if current
            .as_ref()
            .is_some_and(|(_, signature)| signature.contains('{'))
        {
            let (name, signature) = current.take().unwrap();
            if signature.contains("deadline: u64") {
                discovered.push(name);
            }
        }
    }
    let mut expected = DEADLINE_TESTS
        .iter()
        .map(|(entrypoint, _)| (*entrypoint).to_owned())
        .collect::<Vec<_>>();
    discovered.sort_unstable();
    expected.sort_unstable();
    assert_eq!(discovered, expected);

    let test_source = include_str!("guard_tests.rs");
    for (_, test_name) in DEADLINE_TESTS {
        assert!(test_source.contains(test_name));
    }
}
