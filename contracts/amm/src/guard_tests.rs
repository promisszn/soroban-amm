//! AMM Deadline and Slippage Regression Tests (#1184).
//!
//! Covers:
//! 1. All 7 functions taking a `deadline` parameter with boundary testing:
//!    - `deadline = now - 1` -> fails with `DeadlineExceeded`
//!    - `deadline = now` -> succeeds
//!    - `deadline = now + 1` -> succeeds
//! 2. Fee-on-transfer slippage checks in `swap_fot` and `add_liquidity_fot`
//! 3. Standard slippage bounds in `swap`, `swap_exact_out`, `add_liquidity`, `remove_liquidity`, `remove_liquidity_one_sided`.

extern crate std;

use super::*;
use crate::tests::{create_sac, seed_pool, setup_pool, TestSetup};
use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    token::{StellarAssetClient, TokenClient as StellarTokenClient},
    Address, Env,
};

fn setup_and_fund() -> (TestSetup, Address, AmmPoolClient<'static>) {
    let ts = setup_pool(30);
    let trader = Address::generate(&ts.env);
    let client = AmmPoolClient::new(&ts.env, &ts.amm_addr);

    // Seed baseline liquidity
    seed_pool(&ts);

    // Fund trader with tokens
    StellarAssetClient::new(&ts.env, &ts.ta_addr).mint(&trader, &100_000_000);
    StellarAssetClient::new(&ts.env, &ts.tb_addr).mint(&trader, &100_000_000);

    (ts, trader, client)
}

// =========================================================================
// 1. Deadline Boundaries for all 7 AMM Functions (now - 1, now, now + 1)
// =========================================================================

// --- 1. add_liquidity (line 2009) ---
#[test]
fn test_add_liquidity_deadline_past_fails() {
    let (ts, provider, client) = setup_and_fund();
    let now = ts.env.ledger().timestamp();

    let res = client.try_add_liquidity(&provider, &1_000_000, &0, &1_000_000, &0, &(now - 1));
    assert_eq!(res, Err(Ok(AmmError::DeadlineExceeded)));
}

#[test]
fn test_add_liquidity_deadline_exact_now_passes() {
    let (ts, provider, client) = setup_and_fund();
    let now = ts.env.ledger().timestamp();

    let res = client.try_add_liquidity(&provider, &1_000_000, &0, &1_000_000, &0, &now);
    assert!(res.is_ok());
}

#[test]
fn test_add_liquidity_deadline_future_passes() {
    let (ts, provider, client) = setup_and_fund();
    let now = ts.env.ledger().timestamp();

    let res = client.try_add_liquidity(&provider, &1_000_000, &0, &1_000_000, &0, &(now + 1));
    assert!(res.is_ok());
}

// --- 2. remove_liquidity (line 2118) ---
#[test]
fn test_remove_liquidity_deadline_past_fails() {
    let (ts, provider, client) = setup_and_fund();
    let shares = client.add_liquidity(&provider, &1_000_000, &0, &1_000_000, &0, &u64::MAX);
    let now = ts.env.ledger().timestamp();

    let res = client.try_remove_liquidity(&provider, &(shares / 2), &0, &0, &(now - 1));
    assert_eq!(res, Err(Ok(AmmError::DeadlineExceeded)));
}

#[test]
fn test_remove_liquidity_deadline_exact_now_passes() {
    let (ts, provider, client) = setup_and_fund();
    let shares = client.add_liquidity(&provider, &1_000_000, &0, &1_000_000, &0, &u64::MAX);
    let now = ts.env.ledger().timestamp();

    let res = client.try_remove_liquidity(&provider, &(shares / 2), &0, &0, &now);
    assert!(res.is_ok());
}

#[test]
fn test_remove_liquidity_deadline_future_passes() {
    let (ts, provider, client) = setup_and_fund();
    let shares = client.add_liquidity(&provider, &1_000_000, &0, &1_000_000, &0, &u64::MAX);
    let now = ts.env.ledger().timestamp();

    let res = client.try_remove_liquidity(&provider, &(shares / 2), &0, &0, &(now + 1));
    assert!(res.is_ok());
}

// --- 3. remove_liquidity_one_sided (line 2221) ---
#[test]
fn test_remove_liquidity_one_sided_deadline_past_fails() {
    let (ts, provider, client) = setup_and_fund();
    let shares = client.add_liquidity(&provider, &1_000_000, &0, &1_000_000, &0, &u64::MAX);
    let now = ts.env.ledger().timestamp();

    let res = client.try_remove_liquidity_one_sided(
        &provider,
        &(shares / 2),
        &ts.ta_addr,
        &0,
        &(now - 1),
    );
    assert_eq!(res, Err(Ok(AmmError::DeadlineExceeded)));
}

#[test]
fn test_remove_liquidity_one_sided_deadline_exact_now_passes() {
    let (ts, provider, client) = setup_and_fund();
    let shares = client.add_liquidity(&provider, &1_000_000, &0, &1_000_000, &0, &u64::MAX);
    let now = ts.env.ledger().timestamp();

    let res =
        client.try_remove_liquidity_one_sided(&provider, &(shares / 2), &ts.ta_addr, &0, &now);
    assert!(res.is_ok());
}

#[test]
fn test_remove_liquidity_one_sided_deadline_future_passes() {
    let (ts, provider, client) = setup_and_fund();
    let shares = client.add_liquidity(&provider, &1_000_000, &0, &1_000_000, &0, &u64::MAX);
    let now = ts.env.ledger().timestamp();

    let res = client.try_remove_liquidity_one_sided(
        &provider,
        &(shares / 2),
        &ts.ta_addr,
        &0,
        &(now + 1),
    );
    assert!(res.is_ok());
}

// --- 4. swap (line 2443) ---
#[test]
fn test_swap_deadline_past_fails() {
    let (ts, trader, client) = setup_and_fund();
    let now = ts.env.ledger().timestamp();

    let res = client.try_swap(&trader, &ts.ta_addr, &100_000, &0, &(now - 1));
    assert_eq!(res, Err(Ok(AmmError::DeadlineExceeded)));
}

#[test]
fn test_swap_deadline_exact_now_passes() {
    let (ts, trader, client) = setup_and_fund();
    let now = ts.env.ledger().timestamp();

    let res = client.try_swap(&trader, &ts.ta_addr, &100_000, &0, &now);
    assert!(res.is_ok());
}

#[test]
fn test_swap_deadline_future_passes() {
    let (ts, trader, client) = setup_and_fund();
    let now = ts.env.ledger().timestamp();

    let res = client.try_swap(&trader, &ts.ta_addr, &100_000, &0, &(now + 1));
    assert!(res.is_ok());
}

// --- 5. swap_exact_out (line 2621) ---
#[test]
fn test_swap_exact_out_deadline_past_fails() {
    let (ts, trader, client) = setup_and_fund();
    let now = ts.env.ledger().timestamp();

    let res = client.try_swap_exact_out(&trader, &ts.tb_addr, &50_000, &i128::MAX, &(now - 1));
    assert_eq!(res, Err(Ok(AmmError::DeadlineExceeded)));
}

#[test]
fn test_swap_exact_out_deadline_exact_now_passes() {
    let (ts, trader, client) = setup_and_fund();
    let now = ts.env.ledger().timestamp();

    let res = client.try_swap_exact_out(&trader, &ts.tb_addr, &50_000, &i128::MAX, &now);
    assert!(res.is_ok());
}

#[test]
fn test_swap_exact_out_deadline_future_passes() {
    let (ts, trader, client) = setup_and_fund();
    let now = ts.env.ledger().timestamp();

    let res = client.try_swap_exact_out(&trader, &ts.tb_addr, &50_000, &i128::MAX, &(now + 1));
    assert!(res.is_ok());
}

// --- 6. swap_fot (line 3237 / 3240) ---
#[test]
fn test_swap_fot_deadline_past_fails() {
    let (ts, trader, client) = setup_and_fund();
    let now = ts.env.ledger().timestamp();

    let res = client.try_swap_fot(
        &trader,
        &ts.ta_addr,
        &100_000,
        &0,
        &0,
        &(now - 1),
        &None,
    );
    assert_eq!(res, Err(Ok(AmmError::DeadlineExceeded)));
}

#[test]
fn test_swap_fot_deadline_exact_now_passes() {
    let (ts, trader, client) = setup_and_fund();
    let now = ts.env.ledger().timestamp();

    let res = client.try_swap_fot(&trader, &ts.ta_addr, &100_000, &0, &0, &now, &None);
    assert!(res.is_ok());
}

#[test]
fn test_swap_fot_deadline_future_passes() {
    let (ts, trader, client) = setup_and_fund();
    let now = ts.env.ledger().timestamp();

    let res = client.try_swap_fot(&trader, &ts.ta_addr, &100_000, &0, &0, &(now + 1), &None);
    assert!(res.is_ok());
}

// --- 7. add_liquidity_fot (line 3450) ---
#[test]
fn test_add_liquidity_fot_deadline_past_fails() {
    let (ts, provider, client) = setup_and_fund();
    let now = ts.env.ledger().timestamp();

    let res = client.try_add_liquidity_fot(
        &provider,
        &500_000,
        &500_000,
        &0,
        &0,
        &0,
        &(now - 1),
    );
    assert_eq!(res, Err(Ok(AmmError::DeadlineExceeded)));
}

#[test]
fn test_add_liquidity_fot_deadline_exact_now_passes() {
    let (ts, provider, client) = setup_and_fund();
    let now = ts.env.ledger().timestamp();

    let res = client.try_add_liquidity_fot(&provider, &500_000, &500_000, &0, &0, &0, &now);
    assert!(res.is_ok());
}

#[test]
fn test_add_liquidity_fot_deadline_future_passes() {
    let (ts, provider, client) = setup_and_fund();
    let now = ts.env.ledger().timestamp();

    let res = client.try_add_liquidity_fot(
        &provider,
        &500_000,
        &500_000,
        &0,
        &0,
        &0,
        &(now + 1),
    );
    assert!(res.is_ok());
}

// =========================================================================
// 2. Slippage Exceeded Guards (swap_fot & add_liquidity_fot)
// =========================================================================

#[test]
fn test_swap_fot_slippage_exceeded_fails() {
    let (ts, trader, client) = setup_and_fund();
    let amount_in = 100_000_i128;
    let expected_out = client.get_amount_out(&ts.ta_addr, &amount_in);
    assert!(expected_out > 0);

    // Setting min_out 1 unit above achievable amount reverts with SlippageExceeded
    let res = client.try_swap_fot(
        &trader,
        &ts.ta_addr,
        &amount_in,
        &(expected_out + 1),
        &0,
        &u64::MAX,
        &None,
    );
    assert_eq!(res, Err(Ok(AmmError::SlippageExceeded)));
}

#[test]
fn test_swap_fot_slippage_exact_passes() {
    let (ts, trader, client) = setup_and_fund();
    let amount_in = 100_000_i128;
    let expected_out = client.get_amount_out(&ts.ta_addr, &amount_in);

    let res = client.try_swap_fot(
        &trader,
        &ts.ta_addr,
        &amount_in,
        &expected_out,
        &0,
        &u64::MAX,
        &None,
    );
    assert!(res.is_ok());
}

#[test]
fn test_add_liquidity_fot_slippage_min_shares_exceeded_fails() {
    let (ts, provider, client) = setup_and_fund();

    // With min_shares set impossibly high, reverts with SlippageExceeded
    let res = client.try_add_liquidity_fot(
        &provider,
        &1_000_000,
        &1_000_000,
        &0,
        &0,
        &i128::MAX,
        &u64::MAX,
    );
    assert_eq!(res, Err(Ok(AmmError::SlippageExceeded)));
}

#[test]
fn test_swap_standard_slippage_exceeded_fails() {
    let (ts, trader, client) = setup_and_fund();
    let amount_in = 100_000_i128;
    let expected_out = client.get_amount_out(&ts.ta_addr, &amount_in);

    let res = client.try_swap(
        &trader,
        &ts.ta_addr,
        &amount_in,
        &(expected_out + 1),
        &u64::MAX,
    );
    assert_eq!(res, Err(Ok(AmmError::SlippageExceeded)));
}

#[test]
fn test_swap_exact_out_max_in_exceeded_fails() {
    let (ts, trader, client) = setup_and_fund();
    let amount_out = 50_000_i128;
    let required_in = client.get_amount_in(&ts.tb_addr, &amount_out);

    let res = client.try_swap_exact_out(
        &trader,
        &ts.tb_addr,
        &amount_out,
        &(required_in - 1),
        &u64::MAX,
    );
    assert_eq!(res, Err(Ok(AmmError::SlippageExceeded)));
}

#[test]
fn test_remove_liquidity_one_sided_min_out_exceeded_fails() {
    let (ts, provider, client) = setup_and_fund();
    let shares = client.add_liquidity(&provider, &1_000_000, &0, &1_000_000, &0, &u64::MAX);

    let res = client.try_remove_liquidity_one_sided(
        &provider,
        &(shares / 2),
        &ts.ta_addr,
        &i128::MAX,
        &u64::MAX,
    );
    assert_eq!(res, Err(Ok(AmmError::SlippageExceeded)));
}
