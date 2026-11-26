use super::*;
use amm::{AmmPool, AmmPoolClient};
use concentrated_liquidity::{ConcentratedLiquidity, ConcentratedLiquidityClient};
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token::{StellarAssetClient, TokenClient as StellarTokenClient},
    Env,
};
use token::{LpToken, LpTokenClient};

struct Fixture {
    env: Env,
    admin: Address,
    auction: Address,
    token_a: Address,
    token_b: Address,
    amm_pool: Address,
    cl_pool: Address,
}

fn setup() -> Fixture {
    let env = Env::default();
    env.mock_all_auths_allowing_non_root_auth();
    env.ledger().set_timestamp(1_000);
    let admin = Address::generate(&env);
    let token_a = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let token_b = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();

    let amm_pool = env.register_contract(None, AmmPool);
    let lp_addr = env.register_contract(None, LpToken);
    LpTokenClient::new(&env, &lp_addr).initialize(
        &admin,
        &soroban_sdk::String::from_str(&env, "LP"),
        &soroban_sdk::String::from_str(&env, "LP"),
        &7_u32,
    );
    AmmPoolClient::new(&env, &amm_pool).initialize(
        &admin, &token_a, &token_b, &lp_addr, &30_i128, &admin, &0_i128,
    );

    let cl_pool = env.register_contract(None, ConcentratedLiquidity);
    ConcentratedLiquidityClient::new(&env, &cl_pool)
        .initialize(&admin, &token_a, &token_b, &30_i128, &0_i32, &10_i32);

    let auction = env.register_contract(None, BatchAuction);
    let client = BatchAuctionClient::new(&env, &auction);
    client.initialize(&admin, &30_u64);
    client.add_venue(&admin, &amm_pool, &PoolType::Amm);
    client.add_venue(&admin, &cl_pool, &PoolType::Cl);

    Fixture {
        env,
        admin,
        auction,
        token_a,
        token_b,
        amm_pool,
        cl_pool,
    }
}

fn client(fixture: &Fixture) -> BatchAuctionClient<'_> {
    BatchAuctionClient::new(&fixture.env, &fixture.auction)
}

fn funded_trader(fixture: &Fixture) -> Address {
    let trader = Address::generate(&fixture.env);
    StellarAssetClient::new(&fixture.env, &fixture.token_a).mint(&trader, &100_000_i128);
    trader
}

fn submit_amm(
    fixture: &Fixture,
    trader: &Address,
    amount: i128,
    deadline: u64,
) -> Result<u64, AuctionError> {
    match client(fixture).try_submit_order(
        trader,
        &fixture.amm_pool,
        &fixture.token_a,
        &fixture.token_b,
        &amount,
        &0_i128,
        &deadline,
    ) {
        Ok(Ok(order_id)) => Ok(order_id),
        Err(Ok(error)) => Err(error),
        Ok(Err(_)) | Err(Err(_)) => panic!("submit_order invocation failed"),
    }
}

fn submit_cl(
    fixture: &Fixture,
    trader: &Address,
    amount: i128,
    deadline: u64,
) -> Result<u64, AuctionError> {
    match client(fixture).try_submit_order_cl(
        trader,
        &fixture.cl_pool,
        &fixture.token_a,
        &fixture.token_b,
        &amount,
        &0_i128,
        &true,
        &0_u128,
        &None,
        &deadline,
    ) {
        Ok(Ok(order_id)) => Ok(order_id),
        Err(Ok(error)) => Err(error),
        Ok(Err(_)) | Err(Err(_)) => panic!("submit_order_cl invocation failed"),
    }
}

fn assert_no_escrow_change(fixture: &Fixture, trader: &Address, before: i128) {
    let token = StellarTokenClient::new(&fixture.env, &fixture.token_a);
    assert_eq!(token.balance(trader), before);
    assert_eq!(token.balance(&fixture.auction), 0);
    assert!(client(fixture).get_pending_orders().is_empty());
    assert_eq!(client(fixture).get_batch_info().0, 0);
}

fn assert_order_unchanged(before: &Order, after: &Order) {
    assert_eq!(after.id, before.id);
    assert_eq!(after.trader, before.trader);
    assert_eq!(after.pool, before.pool);
    assert_eq!(after.token_in, before.token_in);
    assert_eq!(after.token_out, before.token_out);
    assert_eq!(after.amount_in, before.amount_in);
    assert_eq!(after.min_out, before.min_out);
    assert_eq!(after.submitted_at, before.submitted_at);
    assert_eq!(after.deadline, before.deadline);
    assert_eq!(after.pool_type, before.pool_type);
    assert_eq!(after.zero_for_one, before.zero_for_one);
    assert_eq!(after.sqrt_price_limit, before.sqrt_price_limit);
    assert_eq!(after.alt_pool, before.alt_pool);
}

#[test]
fn test_stranger_cannot_cancel_order_or_change_escrow() {
    let fixture = setup();
    let trader = funded_trader(&fixture);
    let token = StellarTokenClient::new(&fixture.env, &fixture.token_a);
    let order_id = submit_amm(
        &fixture,
        &trader,
        10_000,
        fixture.env.ledger().timestamp() + 100,
    )
    .unwrap();
    let stranger = Address::generate(&fixture.env);
    let before_trader = token.balance(&trader);
    let before_escrow = token.balance(&fixture.auction);
    let before_pending = client(&fixture).get_pending_orders();
    let before_batch = client(&fixture).get_batch_info();

    assert_eq!(
        client(&fixture).try_cancel_order(&stranger, &order_id),
        Err(Ok(AuctionError::Unauthorized))
    );
    assert_eq!(token.balance(&trader), before_trader);
    assert_eq!(token.balance(&fixture.auction), before_escrow);
    let after_pending = client(&fixture).get_pending_orders();
    assert_eq!(after_pending.len(), before_pending.len());
    assert_order_unchanged(
        &before_pending.get(0).unwrap(),
        &after_pending.get(0).unwrap(),
    );
    assert_eq!(client(&fixture).get_batch_info(), before_batch);
}

#[test]
fn test_trader_cancels_order_and_receives_escrow() {
    let fixture = setup();
    let trader = funded_trader(&fixture);
    let token = StellarTokenClient::new(&fixture.env, &fixture.token_a);
    let starting_balance = token.balance(&trader);
    let order_id = submit_amm(
        &fixture,
        &trader,
        10_000,
        fixture.env.ledger().timestamp() + 100,
    )
    .unwrap();

    assert_eq!(token.balance(&trader), starting_balance - 10_000);
    client(&fixture).cancel_order(&trader, &order_id);

    assert_eq!(token.balance(&trader), starting_balance);
    assert_eq!(token.balance(&fixture.auction), 0);
    assert!(client(&fixture).get_pending_orders().is_empty());
}

#[test]
fn test_already_cancelled_order_cannot_be_cancelled_again() {
    let fixture = setup();
    let trader = funded_trader(&fixture);
    let order_id = submit_amm(
        &fixture,
        &trader,
        10_000,
        fixture.env.ledger().timestamp() + 100,
    )
    .unwrap();
    client(&fixture).cancel_order(&trader, &order_id);
    let balance = StellarTokenClient::new(&fixture.env, &fixture.token_a).balance(&trader);

    assert_eq!(
        client(&fixture).try_cancel_order(&trader, &order_id),
        Err(Ok(AuctionError::OrderNotFound))
    );
    assert_eq!(
        StellarTokenClient::new(&fixture.env, &fixture.token_a).balance(&trader),
        balance
    );
    assert!(client(&fixture).get_pending_orders().is_empty());
}

#[test]
fn test_unfillable_order_refunded_at_settlement_cannot_be_cancelled() {
    let fixture = setup();
    let trader = funded_trader(&fixture);
    let order_id = submit_amm(
        &fixture,
        &trader,
        10_000,
        fixture.env.ledger().timestamp() + 100,
    )
    .unwrap();
    fixture.env.ledger().set_timestamp(1_031);
    client(&fixture).settle_batch();
    let balance = StellarTokenClient::new(&fixture.env, &fixture.token_a).balance(&trader);

    assert_eq!(
        client(&fixture).try_cancel_order(&trader, &order_id),
        Err(Ok(AuctionError::OrderNotFound))
    );
    assert_eq!(
        StellarTokenClient::new(&fixture.env, &fixture.token_a).balance(&trader),
        balance
    );
    assert!(client(&fixture).get_pending_orders().is_empty());
}

#[test]
fn test_submit_order_rejects_expired_deadline_without_escrow() {
    let fixture = setup();
    let trader = funded_trader(&fixture);
    let before = StellarTokenClient::new(&fixture.env, &fixture.token_a).balance(&trader);

    assert_eq!(
        submit_amm(
            &fixture,
            &trader,
            1_000,
            fixture.env.ledger().timestamp() - 1
        ),
        Err(AuctionError::DeadlineExceeded)
    );
    assert_no_escrow_change(&fixture, &trader, before);
}

#[test]
fn test_submit_order_accepts_deadline_equal_to_now() {
    let fixture = setup();
    let trader = funded_trader(&fixture);

    assert!(submit_amm(&fixture, &trader, 1_000, fixture.env.ledger().timestamp()).is_ok());
    assert_eq!(client(&fixture).get_pending_orders().len(), 1);
    assert_eq!(
        StellarTokenClient::new(&fixture.env, &fixture.token_a).balance(&fixture.auction),
        1_000
    );
}

#[test]
fn test_submit_order_rejects_deadline_beyond_maximum_without_escrow() {
    let fixture = setup();
    let trader = funded_trader(&fixture);
    let before = StellarTokenClient::new(&fixture.env, &fixture.token_a).balance(&trader);

    assert_eq!(
        submit_amm(
            &fixture,
            &trader,
            1_000,
            fixture.env.ledger().timestamp() + MAX_ORDER_LIFETIME_SECS + 1
        ),
        Err(AuctionError::DeadlineTooFar)
    );
    assert_no_escrow_change(&fixture, &trader, before);
}

#[test]
fn test_submit_order_rejects_zero_amount_without_escrow() {
    let fixture = setup();
    let trader = funded_trader(&fixture);
    let before = StellarTokenClient::new(&fixture.env, &fixture.token_a).balance(&trader);

    assert_eq!(
        submit_amm(&fixture, &trader, 0, fixture.env.ledger().timestamp() + 10),
        Err(AuctionError::ZeroAmount)
    );
    assert_no_escrow_change(&fixture, &trader, before);
}

#[test]
fn test_submit_order_rejects_negative_amount_without_escrow() {
    let fixture = setup();
    let trader = funded_trader(&fixture);
    let before = StellarTokenClient::new(&fixture.env, &fixture.token_a).balance(&trader);

    assert_eq!(
        submit_amm(&fixture, &trader, -1, fixture.env.ledger().timestamp() + 10),
        Err(AuctionError::ZeroAmount)
    );
    assert_no_escrow_change(&fixture, &trader, before);
}

#[test]
fn test_submit_order_cl_rejects_expired_deadline_without_escrow() {
    let fixture = setup();
    let trader = funded_trader(&fixture);
    let before = StellarTokenClient::new(&fixture.env, &fixture.token_a).balance(&trader);

    assert_eq!(
        submit_cl(
            &fixture,
            &trader,
            1_000,
            fixture.env.ledger().timestamp() - 1
        ),
        Err(AuctionError::DeadlineExceeded)
    );
    assert_no_escrow_change(&fixture, &trader, before);
}

#[test]
fn test_submit_order_cl_accepts_deadline_equal_to_now() {
    let fixture = setup();
    let trader = funded_trader(&fixture);

    assert!(submit_cl(&fixture, &trader, 1_000, fixture.env.ledger().timestamp()).is_ok());
    assert_eq!(client(&fixture).get_pending_orders().len(), 1);
    assert_eq!(
        StellarTokenClient::new(&fixture.env, &fixture.token_a).balance(&fixture.auction),
        1_000
    );
}

#[test]
fn test_submit_order_cl_rejects_deadline_beyond_maximum_without_escrow() {
    let fixture = setup();
    let trader = funded_trader(&fixture);
    let before = StellarTokenClient::new(&fixture.env, &fixture.token_a).balance(&trader);

    assert_eq!(
        submit_cl(
            &fixture,
            &trader,
            1_000,
            fixture.env.ledger().timestamp() + MAX_ORDER_LIFETIME_SECS + 1
        ),
        Err(AuctionError::DeadlineTooFar)
    );
    assert_no_escrow_change(&fixture, &trader, before);
}

#[test]
fn test_submit_order_cl_rejects_zero_amount_without_escrow() {
    let fixture = setup();
    let trader = funded_trader(&fixture);
    let before = StellarTokenClient::new(&fixture.env, &fixture.token_a).balance(&trader);

    assert_eq!(
        submit_cl(&fixture, &trader, 0, fixture.env.ledger().timestamp() + 10),
        Err(AuctionError::ZeroAmount)
    );
    assert_no_escrow_change(&fixture, &trader, before);
}

#[test]
fn test_submit_order_cl_rejects_negative_amount_without_escrow() {
    let fixture = setup();
    let trader = funded_trader(&fixture);
    let before = StellarTokenClient::new(&fixture.env, &fixture.token_a).balance(&trader);

    assert_eq!(
        submit_cl(&fixture, &trader, -1, fixture.env.ledger().timestamp() + 10),
        Err(AuctionError::ZeroAmount)
    );
    assert_no_escrow_change(&fixture, &trader, before);
}

#[test]
fn test_empty_batch_cannot_be_settled_after_window() {
    let fixture = setup();
    fixture.env.ledger().set_timestamp(1_030);

    assert_eq!(
        client(&fixture).try_settle_batch(),
        Err(Ok(AuctionError::NoOrders))
    );
    assert_eq!(client(&fixture).get_batch_info().0, 0);
}

#[test]
fn test_initialize_twice_preserves_first_admin_and_batch_state() {
    let fixture = setup();
    let before = client(&fixture).get_batch_info();
    let second_admin = Address::generate(&fixture.env);

    assert_eq!(
        client(&fixture).try_initialize(&second_admin, &60),
        Err(Ok(AuctionError::AlreadyInitialized))
    );
    assert_eq!(client(&fixture).get_admin(), fixture.admin);
    assert_eq!(client(&fixture).get_batch_info(), before);
}
