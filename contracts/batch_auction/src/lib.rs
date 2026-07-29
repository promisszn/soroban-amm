//! MEV-resistant batch auction contract.
//!
//! Collects swap orders within a configurable time window and settles them
//! atomically in a single transaction. Because no external trade can be
//! inserted between batched orders during settlement, sandwich attacks are
//! structurally impossible for orders in the same batch window.
//!
//! Flow:
//!   1. Deploy and `initialize` with an admin and a batch window duration.
//!   2. Traders call `submit_order` — tokens are escrowed immediately until
//!      the current batch reaches the configured order cap.
//!   3. After the window elapses, anyone calls `settle_batch`.
//!   4. Settlement executes all orders atomically; output tokens go to traders.
//!   5. Any trader may call `cancel_order` before settlement to reclaim tokens.

#![no_std]

use amm::AmmPoolClient;
use concentrated_liquidity::ConcentratedLiquidityClient;
use soroban_sdk::token::Client as SepTokenClient;
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, symbol_short, Address, Env, Symbol, Vec,
};

const DEFAULT_MAX_ORDERS: u32 = 50;
const MAX_ORDERS_CEILING: u32 = 200;

// ── Errors ────────────────────────────────────────────────────────────────────

#[contracterror]
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum AuctionError {
    AlreadyInitialized = 1,
    Unauthorized = 2,
    OrderNotFound = 3,
    BatchWindowOpen = 4,
    NoOrders = 5,
    ZeroAmount = 6,
    DeadlineExceeded = 7,
    BatchFull = 8,
    InvalidMaxOrders = 9,
    /// `token_in`/`token_out` do not match the pool's token pair (issue #361).
    InvalidPoolTokenPair = 10,
    /// Token transfer to/from a trader failed (issue #546).
    TransferFailed = 11,
    /// `accept_admin` called without a prior `propose_admin` (issue #553).
    NoPendingAdmin = 12,
    /// `accept_admin` caller is not the pending nominee (issue #553).
    WrongAdmin = 13,
}

// ── Storage types ─────────────────────────────────────────────────────────────

/// Settlement venue an order may be routed to (issue #351).
///
/// `Amm` dispatches through [`AmmPoolClient`] (constant-product pool); `Cl`
/// dispatches through [`ConcentratedLiquidityClient`] (Uniswap-v3-style
/// concentrated-liquidity pool). Both venues escrow the input from, and pay the
/// output to, the batch-auction contract, so they are interchangeable from a
/// trader's perspective.
#[contracttype]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PoolType {
    Amm,
    Cl,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct Order {
    pub id: u64,
    pub trader: Address,
    pub pool: Address,
    pub token_in: Address,
    pub token_out: Address,
    pub amount_in: i128,
    pub min_out: i128,
    pub submitted_at: u64,
    /// Trader-supplied deadline (ledger timestamp). Honored verbatim at
    /// settlement time: if it has passed by the time `settle_batch` runs, the
    /// order is expired and refunded rather than executed against a
    /// freshly-computed deadline.
    pub deadline: u64,
    /// Venue type of `pool`.
    pub pool_type: PoolType,
    /// Swap direction for concentrated-liquidity venues: `true` swaps token A
    /// for token B (price decreasing). Unused for `PoolType::Amm`.
    pub zero_for_one: bool,
    /// `sqrtPriceX96` limit for concentrated-liquidity venues. `0` means the
    /// pool's own default bound is used. Unused for `PoolType::Amm`.
    pub sqrt_price_limit: u128,
    /// Optional alternate venue of the *opposite* `PoolType`, trading the same
    /// `token_in → token_out` pair. When present, settlement quotes both venues
    /// and routes the swap to whichever returns more output (issue #351).
    pub alt_pool: Option<Address>,
}

#[contracttype]
pub enum DataKey {
    Admin,
    /// Nominated admin awaiting `accept_admin` (two-step rotation, issue #553).
    PendingAdmin,
    BatchWindowSecs,
    BatchOpenedAt,
    MaxOrders,
    NextOrderId,
    Order(u64),
    PendingOrders,
}

fn max_orders(env: &Env) -> u32 {
    env.storage()
        .instance()
        .get(&DataKey::MaxOrders)
        .unwrap_or(DEFAULT_MAX_ORDERS)
}

// ── Contract ──────────────────────────────────────────────────────────────────

#[contract]
pub struct BatchAuction;

#[contractimpl]
impl BatchAuction {
    /// Initialize the auction contract.
    ///
    /// - `batch_window_secs` — how long (in ledger seconds) a batch window stays
    ///   open before it can be settled.
    pub fn initialize(
        env: Env,
        admin: Address,
        batch_window_secs: u64,
    ) -> Result<(), AuctionError> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(AuctionError::AlreadyInitialized);
        }
        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage()
            .instance()
            .set(&DataKey::BatchWindowSecs, &batch_window_secs);
        env.storage().instance().set(&DataKey::NextOrderId, &0_u64);
        env.storage()
            .instance()
            .set(&DataKey::MaxOrders, &DEFAULT_MAX_ORDERS);
        env.storage()
            .instance()
            .set(&DataKey::PendingOrders, &Vec::<u64>::new(&env));
        env.storage()
            .instance()
            .set(&DataKey::BatchOpenedAt, &env.ledger().timestamp());
        Ok(())
    }

    /// Submit a constant-product (AMM) swap order and escrow `amount_in` of
    /// `token_in`.
    ///
    /// Tokens are pulled from `trader` immediately so the batch holds a firm
    /// commitment. `token_in`/`token_out` must be the pool's token pair; this
    /// is validated here so a mismatched order can never reach `settle_batch`.
    ///
    /// Returns the new order ID.
    pub fn submit_order(
        env: Env,
        trader: Address,
        pool: Address,
        token_in: Address,
        token_out: Address,
        amount_in: i128,
        min_out: i128,
        deadline: u64,
    ) -> Result<u64, AuctionError> {
        Self::record_order(
            env,
            trader,
            pool,
            token_in,
            token_out,
            amount_in,
            min_out,
            deadline,
            PoolType::Amm,
            false,
            0,
            None,
        )
    }

    /// Submit a concentrated-liquidity (CL) swap order and escrow `amount_in`
    /// of `token_in` (issue #351).
    ///
    /// `zero_for_one` selects the CL swap direction (token A → token B when
    /// `true`) and `sqrt_price_limit` is the `sqrtPriceX96` bound passed to the
    /// CL pool (`0` lets the pool walk to its own bound).
    ///
    /// `alt_amm_pool` may name a constant-product pool trading the same
    /// `token_in → token_out` pair. When supplied, settlement quotes both the
    /// CL pool and the AMM pool and routes the swap to whichever returns more
    /// output, giving batched traders best execution across venue types.
    ///
    /// Returns the new order ID.
    #[allow(clippy::too_many_arguments)]
    pub fn submit_order_cl(
        env: Env,
        trader: Address,
        pool: Address,
        token_in: Address,
        token_out: Address,
        amount_in: i128,
        min_out: i128,
        zero_for_one: bool,
        sqrt_price_limit: u128,
        alt_amm_pool: Option<Address>,
        deadline: u64,
    ) -> Result<u64, AuctionError> {
        Self::record_order(
            env,
            trader,
            pool,
            token_in,
            token_out,
            amount_in,
            min_out,
            deadline,
            PoolType::Cl,
            zero_for_one,
            sqrt_price_limit,
            alt_amm_pool,
        )
    }

    /// Shared order-intake path: validate, escrow `amount_in`, persist the
    /// order, and enqueue it into the current batch window.
    #[allow(clippy::too_many_arguments)]
    fn record_order(
        env: Env,
        trader: Address,
        pool: Address,
        token_in: Address,
        token_out: Address,
        amount_in: i128,
        min_out: i128,
        deadline: u64,
        pool_type: PoolType,
        zero_for_one: bool,
        sqrt_price_limit: u128,
        alt_pool: Option<Address>,
    ) -> Result<u64, AuctionError> {
        if deadline < env.ledger().timestamp() {
            return Err(AuctionError::DeadlineExceeded);
        }
        if amount_in <= 0 {
            return Err(AuctionError::ZeroAmount);
        }

        // Validate the order's tokens against the primary pool's actual pair up
        // front. If a mismatched pair slipped through, settle_batch would call
        // swap with the wrong tokens and panic; since settlement is atomic, that
        // panic reverts the whole batch and locks every other trader's escrow
        // until each order is cancelled individually (issue #361). AMM and CL
        // pools expose their token pair through different interfaces, so the
        // lookup must branch on pool_type (issue #470).
        if !Self::pool_matches_pair(&env, &pool, pool_type, &token_in, &token_out) {
            return Err(AuctionError::InvalidPoolTokenPair);
        }
        // An alternate venue must trade the same pair as the primary venue.
        // Otherwise best-venue routing could quote and select a real pool over
        // `token_in -> token_x`, causing settlement to receive the wrong asset.
        if let Some(ref alt) = alt_pool {
            let alt_type = match pool_type {
                PoolType::Amm => PoolType::Cl,
                PoolType::Cl => PoolType::Amm,
            };
            if !Self::pool_matches_pair(&env, alt, alt_type, &token_in, &token_out) {
                return Err(AuctionError::InvalidPoolTokenPair);
            }
        }

        let mut pending: Vec<u64> = env
            .storage()
            .instance()
            .get(&DataKey::PendingOrders)
            .unwrap_or_else(|| Vec::new(&env));
        if pending.len() >= max_orders(&env) {
            return Err(AuctionError::BatchFull);
        }

        trader.require_auth();

        // Escrow input tokens immediately so the commitment is firm.
        SepTokenClient::new(&env, &token_in).transfer(
            &trader,
            &env.current_contract_address(),
            &amount_in,
        );

        let id: u64 = env
            .storage()
            .instance()
            .get(&DataKey::NextOrderId)
            .unwrap_or(0);

        let order = Order {
            id,
            trader: trader.clone(),
            pool,
            token_in,
            token_out,
            amount_in,
            min_out,
            submitted_at: env.ledger().timestamp(),
            deadline,
            pool_type,
            zero_for_one,
            sqrt_price_limit,
            alt_pool,
        };

        env.storage().instance().set(&DataKey::Order(id), &order);

        pending.push_back(id);
        env.storage()
            .instance()
            .set(&DataKey::PendingOrders, &pending);
        env.storage()
            .instance()
            .set(&DataKey::NextOrderId, &(id + 1));

        env.events().publish(
            (Symbol::new(&env, "order_submitted"), trader),
            (id, amount_in),
        );

        Ok(id)
    }

    /// Cancel a pending order and refund escrowed tokens.
    ///
    /// Only the original trader may cancel their own order.
    pub fn cancel_order(env: Env, trader: Address, order_id: u64) -> Result<(), AuctionError> {
        trader.require_auth();

        let order: Order = env
            .storage()
            .instance()
            .get(&DataKey::Order(order_id))
            .ok_or(AuctionError::OrderNotFound)?;

        if order.trader != trader {
            return Err(AuctionError::Unauthorized);
        }

        // Refund escrowed tokens.
        SepTokenClient::new(&env, &order.token_in).transfer(
            &env.current_contract_address(),
            &trader,
            &order.amount_in,
        );

        env.storage().instance().remove(&DataKey::Order(order_id));

        let pending: Vec<u64> = env
            .storage()
            .instance()
            .get(&DataKey::PendingOrders)
            .unwrap_or_else(|| Vec::new(&env));
        let mut updated = Vec::<u64>::new(&env);
        for i in 0..pending.len() {
            let oid = pending.get(i).unwrap();
            if oid != order_id {
                updated.push_back(oid);
            }
        }
        env.storage()
            .instance()
            .set(&DataKey::PendingOrders, &updated);

        env.events()
            .publish((Symbol::new(&env, "order_cancelled"), trader), (order_id,));

        Ok(())
    }

    /// Settle the current batch atomically.
    ///
    /// Callable by anyone once the batch window has elapsed. Pending orders
    /// are processed sequentially within a single transaction — no external
    /// trade can be inserted between them, which structurally prevents
    /// sandwich attacks.
    ///
    /// Each order's swap is attempted in isolation (issue #473): if an order
    /// has become unfillable at settlement time (its `min_out` can no longer
    /// be met, its venue is paused, or in-range liquidity is insufficient),
    /// that single order is skipped — its escrow is refunded to the trader
    /// and it is dropped from the batch — instead of reverting the whole
    /// settlement and freezing every other trader's escrow.
    ///
    /// Token transfers (payout on success, refund on failure/expiry) are also
    /// isolated via `try_transfer` (issue #546): a single trader's inability
    /// to receive tokens — no trustline, insufficient limit, frozen/revoked
    /// SAC authorization, clawback-enabled asset — does not revert the entire
    /// batch. The failing order is logged as `order_failed` and settlement
    /// continues with the remaining orders.
    ///
    /// Returns the output amounts for each order that settled successfully,
    /// in submission order. Orders that failed and were refunded are omitted;
    /// callers should compare against [`get_pending_orders`] before and after
    /// to see which orders failed, and use `order_failed` events to react.
    pub fn settle_batch(env: Env) -> Result<Vec<i128>, AuctionError> {
        let opened_at: u64 = env
            .storage()
            .instance()
            .get(&DataKey::BatchOpenedAt)
            .unwrap_or(0);
        let window_secs: u64 = env
            .storage()
            .instance()
            .get(&DataKey::BatchWindowSecs)
            .unwrap_or(60);
        let now = env.ledger().timestamp();
        if now < opened_at + window_secs {
            return Err(AuctionError::BatchWindowOpen);
        }

        let pending: Vec<u64> = env
            .storage()
            .instance()
            .get(&DataKey::PendingOrders)
            .unwrap_or_else(|| Vec::new(&env));
        if pending.is_empty() {
            return Err(AuctionError::NoOrders);
        }

        let auction_addr = env.current_contract_address();
        let order_limit = max_orders(&env);
        let process_count = if pending.len() > order_limit {
            order_limit
        } else {
            pending.len()
        };
        let mut results = Vec::<i128>::new(&env);

        for i in 0..process_count {
            let order_id = pending.get(i).unwrap();
            let order: Order = env
                .storage()
                .instance()
                .get(&DataKey::Order(order_id))
                .unwrap();

            // Honor the trader's own deadline. If it has already passed by
            // settlement time, expire the order and refund its escrow rather
            // than silently substituting a freshly-computed deadline.
            if now > order.deadline {
                let refund_ok = SepTokenClient::new(&env, &order.token_in)
                    .try_transfer(&auction_addr, &order.trader, &order.amount_in)
                    .ok()
                    .and_then(|r| r.ok());
                if refund_ok.is_none() {
                    env.events().publish(
                        (Symbol::new(&env, "order_failed"), order.trader.clone()),
                        (order_id,),
                    );
                } else {
                    results.push_back(0);
                    env.events().publish(
                        (Symbol::new(&env, "order_expired"), order.trader.clone()),
                        (order_id,),
                    );
                }
                env.storage().instance().remove(&DataKey::Order(order_id));
                continue;
            }

            // Execute the swap on behalf of the batch auction contract, routing
            // to whichever supported venue gives the best output. Authorization
            // for the token pull (auction → pool) is automatically satisfied
            // because the batch_auction is the invoking contract. A failure
            // here (min_out no longer met, venue paused, insufficient in-range
            // liquidity) is isolated to this order rather than reverting the
            // whole batch (issue #473). The deadline passed in is the trader's
            // own submitted bound, not a freshly-computed one.
            match Self::execute_op(&env, &order, &auction_addr, order.deadline) {
                Ok(amount_out) => {
                    // Forward output tokens to the original trader.
                    let payout_ok = SepTokenClient::new(&env, &order.token_out)
                        .try_transfer(&auction_addr, &order.trader, &amount_out)
                        .ok()
                        .and_then(|r| r.ok());
                    if payout_ok.is_none() {
                        // Swap succeeded but payout to the trader failed (no
                        // trustline, frozen/revoked auth, clawback, etc.).
                        // Tokens are stranded in the auction contract — the
                        // order is still dropped so the batch can continue.
                        env.events().publish(
                            (Symbol::new(&env, "order_failed"), order.trader.clone()),
                            (order_id,),
                        );
                    } else {
                        results.push_back(amount_out);
                        env.events().publish(
                            (Symbol::new(&env, "order_settled"), order.trader.clone()),
                            (order_id, amount_out),
                        );
                    }
                }
                Err(()) => {
                    // Unfillable order: the failed swap attempt was rolled back
                    // by the runtime, so the full escrow is still held by the
                    // auction contract. Refund it and drop the order instead of
                    // letting it block every other trader's settlement.
                    let refund_ok = SepTokenClient::new(&env, &order.token_in)
                        .try_transfer(&auction_addr, &order.trader, &order.amount_in)
                        .ok()
                        .and_then(|r| r.ok());
                    if refund_ok.is_none() {
                        // Refund transfer itself failed — escrow is stranded
                        // but the order is still dropped so other traders are
                        // not blocked.
                        env.events().publish(
                            (
                                Symbol::new(&env, "order_refund_failed"),
                                order.trader.clone(),
                            ),
                            (order_id,),
                        );
                    }
                    env.events().publish(
                        (Symbol::new(&env, "order_failed"), order.trader.clone()),
                        (order_id,),
                    );
                }
            }
            env.storage().instance().remove(&DataKey::Order(order_id));
        }

        let mut remaining = Vec::<u64>::new(&env);
        for i in process_count..pending.len() {
            remaining.push_back(pending.get(i).unwrap());
        }

        env.storage()
            .instance()
            .set(&DataKey::PendingOrders, &remaining);
        env.storage().instance().set(&DataKey::BatchOpenedAt, &now);

        env.events()
            .publish((symbol_short!("settled"),), (process_count,));

        Ok(results)
    }

    /// Quote the output an order would receive on each candidate venue and
    /// return the best `(amount_out, pool, pool_type)` triple (issue #351).
    ///
    /// Read-only: callers can preview the venue settlement would choose. The
    /// quote walks the same pool math used at execution, so the chosen venue
    /// matches `settle_batch`'s routing for an unchanged pool state.
    pub fn quote_order(env: Env, order_id: u64) -> Result<(i128, Address, PoolType), AuctionError> {
        let order: Order = env
            .storage()
            .instance()
            .get(&DataKey::Order(order_id))
            .ok_or(AuctionError::OrderNotFound)?;
        Ok(Self::best_venue(&env, &order))
    }

    /// Dispatch a single order's swap to the best available venue and return
    /// the realized output amount.
    ///
    /// Branches on [`PoolType`]: `Amm` settles through [`AmmPoolClient`], `Cl`
    /// through [`ConcentratedLiquidityClient`]. When the chosen venue consumes
    /// less than `amount_in` (a concentrated-liquidity pool can fill partially
    /// once it exhausts in-range liquidity), the unspent escrow is refunded to
    /// the trader so no funds are stranded in the auction contract.
    ///
    /// Calls the venue through its `try_` entry point (issue #473): a pool
    /// error (slippage/`min_out` not met, paused, insufficient liquidity) is
    /// returned as `Err(())` instead of escalating to a host panic, so the
    /// caller can isolate the failure to this one order.
    ///
    /// The unspent-refund transfer is also wrapped with `try_transfer`
    /// (issue #546) so a trustline/auth failure on the refund leg cannot
    /// revert the caller.
    fn execute_op(env: &Env, order: &Order, sender: &Address, deadline: u64) -> Result<i128, ()> {
        let (_, venue, venue_type) = Self::best_venue(env, order);

        let spent_before = SepTokenClient::new(env, &order.token_in).balance(sender);
        let swap_result = match venue_type {
            PoolType::Amm => AmmPoolClient::new(env, &venue)
                .try_swap(
                    sender,
                    &order.token_in,
                    &order.amount_in,
                    &order.min_out,
                    &deadline,
                )
                .ok()
                .and_then(|r| r.ok()),
            PoolType::Cl => ConcentratedLiquidityClient::new(env, &venue)
                .try_swap(
                    sender,
                    &order.zero_for_one,
                    &order.amount_in,
                    &order.sqrt_price_limit,
                    &order.min_out,
                    &deadline,
                )
                .ok()
                .and_then(|r| r.ok()),
        };
        let amount_out = match swap_result {
            Some(amount_out) => amount_out,
            None => return Err(()),
        };
        let spent_after = SepTokenClient::new(env, &order.token_in).balance(sender);

        // Refund any input the venue did not consume (partial fill).
        let spent = spent_before - spent_after;
        let unspent = order.amount_in - spent;
        if unspent > 0 {
            SepTokenClient::new(env, &order.token_in)
                .try_transfer(sender, &order.trader, &unspent)
                .ok()
                .and_then(|r| r.ok());
        }

        Ok(amount_out)
    }

    /// Pick the venue that quotes the most output for `order`.
    ///
    /// Always considers the primary `(pool, pool_type)`. If `alt_pool` is set it
    /// is treated as a venue of the opposite type and compared; the higher quote
    /// wins, with the primary kept on ties or when the alternate cannot be
    /// quoted. Quotes are best-effort: a venue that fails to quote is simply not
    /// selected, so a stale or unrelated alternate can never block settlement.
    fn best_venue(env: &Env, order: &Order) -> (i128, Address, PoolType) {
        let primary_q = Self::try_quote(env, &order.pool, order.pool_type, order).unwrap_or(0);

        if let Some(alt) = order.alt_pool.clone() {
            let alt_type = match order.pool_type {
                PoolType::Amm => PoolType::Cl,
                PoolType::Cl => PoolType::Amm,
            };
            // Re-check the alternate venue's token pair at routing time so a
            // malformed order written before validation was added can never be
            // routed into the wrong output asset.
            if Self::pool_matches_pair(env, &alt, alt_type, &order.token_in, &order.token_out) {
                if let Some(alt_q) = Self::try_quote(env, &alt, alt_type, order) {
                    if alt_q > primary_q {
                        return (alt_q, alt, alt_type);
                    }
                }
            }
        }
        (primary_q, order.pool.clone(), order.pool_type)
    }

    /// Return whether `pool` trades exactly the unordered `(token_in, token_out)`
    /// pair for the requested venue type.
    fn pool_matches_pair(
        env: &Env,
        pool: &Address,
        pool_type: PoolType,
        token_in: &Address,
        token_out: &Address,
    ) -> bool {
        let (pool_token_a, pool_token_b) = match pool_type {
            PoolType::Amm => {
                let info = AmmPoolClient::new(env, pool).get_info();
                (info.token_a, info.token_b)
            }
            PoolType::Cl => ConcentratedLiquidityClient::new(env, pool).get_tokens(),
        };
        (token_in == &pool_token_a && token_out == &pool_token_b)
            || (token_in == &pool_token_b && token_out == &pool_token_a)
    }

    /// Read-only output quote for `order` on `pool` interpreted as `pool_type`.
    /// Returns `None` if the venue rejects the quote (e.g. wrong token pair).
    fn try_quote(env: &Env, pool: &Address, pool_type: PoolType, order: &Order) -> Option<i128> {
        match pool_type {
            PoolType::Amm => AmmPoolClient::new(env, pool)
                .try_get_amount_out(&order.token_in, &order.amount_in)
                .ok()?
                .ok(),
            PoolType::Cl => Some(
                ConcentratedLiquidityClient::new(env, pool)
                    .try_estimate_price_impact(
                        &order.zero_for_one,
                        &order.amount_in,
                        &order.sqrt_price_limit,
                    )
                    .ok()?
                    .ok()?
                    .amount_out,
            ),
        }
    }

    /// Return all pending orders in the current batch window.
    pub fn get_pending_orders(env: Env) -> Vec<Order> {
        let pending: Vec<u64> = env
            .storage()
            .instance()
            .get(&DataKey::PendingOrders)
            .unwrap_or_else(|| Vec::new(&env));
        let mut orders = Vec::<Order>::new(&env);
        for i in 0..pending.len() {
            let id = pending.get(i).unwrap();
            if let Some(order) = env.storage().instance().get(&DataKey::Order(id)) {
                orders.push_back(order);
            }
        }
        orders
    }

    /// Return compact batch capacity and timing metadata.
    ///
    /// The tuple is `(pending_count, max_orders, batch_opened_at,
    /// batch_window_secs)`.
    pub fn get_batch_info(env: Env) -> (u32, u32, u64, u64) {
        let pending: Vec<u64> = env
            .storage()
            .instance()
            .get(&DataKey::PendingOrders)
            .unwrap_or_else(|| Vec::new(&env));
        let opened_at: u64 = env
            .storage()
            .instance()
            .get(&DataKey::BatchOpenedAt)
            .unwrap_or(0);
        let window_secs: u64 = env
            .storage()
            .instance()
            .get(&DataKey::BatchWindowSecs)
            .unwrap_or(60);

        (pending.len(), max_orders(&env), opened_at, window_secs)
    }

    /// Update the batch window duration. Admin-only.
    pub fn set_batch_window(env: Env, batch_window_secs: u64) -> Result<(), AuctionError> {
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();
        env.storage()
            .instance()
            .set(&DataKey::BatchWindowSecs, &batch_window_secs);
        env.events()
            .publish((Symbol::new(&env, "window_updated"),), (batch_window_secs,));
        Ok(())
    }

    /// Update the maximum number of orders accepted into a batch. Admin-only.
    ///
    /// `n` must be between 1 and `MAX_ORDERS_CEILING`, inclusive. The ceiling
    /// keeps settlement cost bounded even if governance/admin configuration is
    /// changed under production load.
    pub fn set_max_orders(env: Env, admin: Address, n: u32) -> Result<(), AuctionError> {
        let stored_admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        if stored_admin != admin {
            return Err(AuctionError::Unauthorized);
        }
        admin.require_auth();
        if n == 0 || n > MAX_ORDERS_CEILING {
            return Err(AuctionError::InvalidMaxOrders);
        }

        env.storage().instance().set(&DataKey::MaxOrders, &n);
        env.events()
            .publish((Symbol::new(&env, "max_orders_updated"),), (n,));
        Ok(())
    }

    /// Nominate a new admin. The nominee must call `accept_admin` to complete
    /// the transfer (two-step rotation, matching `pol_vesting` / AMM).
    ///
    /// Prevents a mistyped address from permanently locking `set_batch_window`
    /// and `set_max_orders` (issue #553).
    pub fn propose_admin(
        env: Env,
        current_admin: Address,
        new_admin: Address,
    ) -> Result<(), AuctionError> {
        let stored: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        if current_admin != stored {
            return Err(AuctionError::Unauthorized);
        }
        current_admin.require_auth();
        env.storage()
            .instance()
            .set(&DataKey::PendingAdmin, &Some(new_admin.clone()));
        env.events().publish(
            (Symbol::new(&env, "admin_nominated"),),
            (current_admin, new_admin),
        );
        Ok(())
    }

    /// Accept the pending admin nomination. Caller becomes the new admin.
    pub fn accept_admin(env: Env, new_admin: Address) -> Result<(), AuctionError> {
        let pending: Option<Address> = env
            .storage()
            .instance()
            .get(&DataKey::PendingAdmin)
            .unwrap_or(None);
        let nominee = pending.ok_or(AuctionError::NoPendingAdmin)?;
        if new_admin != nominee {
            return Err(AuctionError::WrongAdmin);
        }
        new_admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &new_admin);
        env.storage()
            .instance()
            .set(&DataKey::PendingAdmin, &Option::<Address>::None);
        env.events()
            .publish((Symbol::new(&env, "admin_changed"),), (new_admin,));
        Ok(())
    }

    /// Return the active admin address.
    pub fn get_admin(env: Env) -> Address {
        env.storage().instance().get(&DataKey::Admin).unwrap()
    }

    /// Return the pending admin nominee, or `None` if no transfer is in progress.
    pub fn get_pending_admin(env: Env) -> Option<Address> {
        env.storage()
            .instance()
            .get(&DataKey::PendingAdmin)
            .unwrap_or(None)
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use amm::AmmPool;
    use concentrated_liquidity::{ConcentratedLiquidity, ConcentratedLiquidityClient};
    use soroban_sdk::{
        testutils::{Address as _, Ledger},
        token::{StellarAssetClient, TokenClient as StellarTokenClient},
        Env, String,
    };
    use token::{LpToken, LpTokenClient};

    /// Deploy a concentrated-liquidity pool over `(token_a, token_b)`, seed a
    /// wide in-range position, and return the pool address. The pool starts at
    /// tick 0 with tick spacing 10 and a 30 bps fee.
    fn deploy_cl_pool(env: &Env, admin: &Address, token_a: &Address, token_b: &Address) -> Address {
        let cl_addr = env.register_contract(None, ConcentratedLiquidity);
        let cl = ConcentratedLiquidityClient::new(env, &cl_addr);
        cl.initialize(admin, token_a, token_b, &30_i128, &0_i32, &10_i32);

        let lp = Address::generate(env);
        StellarAssetClient::new(env, token_a).mint(&lp, &100_000_000_i128);
        StellarAssetClient::new(env, token_b).mint(&lp, &100_000_000_i128);
        cl.mint_position(
            &lp,
            &-1_000_i32,
            &1_000_i32,
            &50_000_000_i128,
            &50_000_000_i128,
            &0_i128,
            &0_i128,
        );
        cl_addr
    }

    fn deploy_pool(env: &Env, token_a: &Address, token_b: &Address) -> Address {
        let amm_addr = env.register_contract(None, AmmPool);
        let lp_addr = env.register_contract(None, LpToken);
        LpTokenClient::new(env, &lp_addr).initialize(
            &amm_addr,
            &String::from_str(env, "LP"),
            &String::from_str(env, "LP"),
            &7u32,
        );
        AmmPoolClient::new(env, &amm_addr).initialize(
            &amm_addr, token_a, token_b, &lp_addr, &30_i128, &amm_addr, &0_i128,
        );
        amm_addr
    }

    fn setup(env: &Env) -> (Address, Address, Address, Address) {
        let admin = Address::generate(env);
        let ta = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();
        let tb = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();
        let pool = deploy_pool(env, &ta, &tb);

        let lp = Address::generate(env);
        StellarAssetClient::new(env, &ta).mint(&lp, &2_000_000_i128);
        StellarAssetClient::new(env, &tb).mint(&lp, &2_000_000_i128);
        AmmPoolClient::new(env, &pool).add_liquidity(
            &lp,
            &1_000_000_i128,
            &1_000_000_i128,
            &0_i128,
            &u64::MAX,
        );
        (ta, tb, pool, admin)
    }

    #[test]
    fn test_submit_and_settle() {
        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();
        env.ledger().set_timestamp(1000);

        let (ta, tb, pool, admin) = setup(&env);

        let auction_addr = env.register_contract(None, BatchAuction);
        BatchAuctionClient::new(&env, &auction_addr).initialize(&admin, &30_u64);

        let trader = Address::generate(&env);
        StellarAssetClient::new(&env, &ta).mint(&trader, &100_000_i128);

        BatchAuctionClient::new(&env, &auction_addr).submit_order(
            &trader,
            &pool,
            &ta,
            &tb,
            &10_000_i128,
            &0_i128,
            &u64::MAX,
        );

        // Advance past the batch window.
        env.ledger().set_timestamp(1031);

        let results = BatchAuctionClient::new(&env, &auction_addr).settle_batch();

        assert_eq!(results.len(), 1);
        assert!(results.get(0).unwrap() > 0);

        // Trader received token_b.
        let tb_balance = StellarTokenClient::new(&env, &tb).balance(&trader);
        assert!(tb_balance > 0);
    }

    #[test]
    fn test_submit_order_rejects_mismatched_pool_tokens() {
        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();
        env.ledger().set_timestamp(1000);

        let (ta, _tb, pool, admin) = setup(&env);

        // A token that is not part of the pool's pair.
        let foreign_admin = Address::generate(&env);
        let foreign = env
            .register_stellar_asset_contract_v2(foreign_admin)
            .address();

        let auction_addr = env.register_contract(None, BatchAuction);
        let client = BatchAuctionClient::new(&env, &auction_addr);
        client.initialize(&admin, &30_u64);

        let trader = Address::generate(&env);
        StellarAssetClient::new(&env, &ta).mint(&trader, &100_000_i128);

        // token_out is not the pool's other token → rejected up front.
        let result = client.try_submit_order(
            &trader,
            &pool,
            &ta,
            &foreign,
            &10_000_i128,
            &0_i128,
            &u64::MAX,
        );
        assert_eq!(result, Err(Ok(AuctionError::InvalidPoolTokenPair)));

        // The order is rejected before any escrow, so the trader keeps its funds
        // and no order is recorded in the batch.
        assert_eq!(
            StellarTokenClient::new(&env, &ta).balance(&trader),
            100_000_i128
        );
        assert_eq!(client.get_pending_orders().len(), 0);
    }

    #[test]
    fn test_cancel_order_refunds_tokens() {
        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();
        env.ledger().set_timestamp(1000);

        let (ta, tb, pool, admin) = setup(&env);

        let auction_addr = env.register_contract(None, BatchAuction);
        BatchAuctionClient::new(&env, &auction_addr).initialize(&admin, &30_u64);

        let trader = Address::generate(&env);
        StellarAssetClient::new(&env, &ta).mint(&trader, &100_000_i128);

        let order_id = BatchAuctionClient::new(&env, &auction_addr).submit_order(
            &trader,
            &pool,
            &ta,
            &tb,
            &10_000_i128,
            &0_i128,
            &u64::MAX,
        );

        // Tokens were escrowed — trader's balance decreased.
        let balance_after_submit = StellarTokenClient::new(&env, &ta).balance(&trader);
        assert_eq!(balance_after_submit, 90_000_i128);

        BatchAuctionClient::new(&env, &auction_addr).cancel_order(&trader, &order_id);

        // Tokens returned after cancel.
        let balance_after_cancel = StellarTokenClient::new(&env, &ta).balance(&trader);
        assert_eq!(balance_after_cancel, 100_000_i128);

        let orders = BatchAuctionClient::new(&env, &auction_addr).get_pending_orders();
        assert_eq!(orders.len(), 0);
    }

    #[test]
    fn test_settle_before_window_reverts() {
        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();
        env.ledger().set_timestamp(1000);

        let (ta, tb, pool, admin) = setup(&env);

        let auction_addr = env.register_contract(None, BatchAuction);
        BatchAuctionClient::new(&env, &auction_addr).initialize(&admin, &30_u64);

        let trader = Address::generate(&env);
        StellarAssetClient::new(&env, &ta).mint(&trader, &100_000_i128);
        BatchAuctionClient::new(&env, &auction_addr).submit_order(
            &trader,
            &pool,
            &ta,
            &tb,
            &10_000_i128,
            &0_i128,
            &u64::MAX,
        );

        // Window has not elapsed — should return BatchWindowOpen error.
        let result = BatchAuctionClient::new(&env, &auction_addr).try_settle_batch();
        assert!(result.is_err());
    }

    #[test]
    fn test_settle_batch_honors_order_deadline_not_freshly_computed_one() {
        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();
        env.ledger().set_timestamp(1000);

        let (ta, tb, pool, admin) = setup(&env);

        let auction_addr = env.register_contract(None, BatchAuction);
        let client = BatchAuctionClient::new(&env, &auction_addr);
        client.initialize(&admin, &30_u64);

        let trader = Address::generate(&env);
        StellarAssetClient::new(&env, &ta).mint(&trader, &100_000_i128);

        // Trader wants this order to expire well before the batch settles.
        client.submit_order(&trader, &pool, &ta, &tb, &10_000_i128, &0_i128, &1_010_u64);

        // Tokens are escrowed on submission.
        assert_eq!(
            StellarTokenClient::new(&env, &ta).balance(&trader),
            90_000_i128
        );

        // Advance past both the batch window and the order's own deadline.
        env.ledger().set_timestamp(1031);

        let results = client.settle_batch();
        assert_eq!(results.len(), 1);
        // The order was expired, not executed against a freshly-computed
        // deadline, so no output was produced.
        assert_eq!(results.get(0).unwrap(), 0);

        // Escrow was refunded in full rather than swapped.
        assert_eq!(
            StellarTokenClient::new(&env, &ta).balance(&trader),
            100_000_i128
        );
        assert_eq!(StellarTokenClient::new(&env, &tb).balance(&trader), 0);
        assert_eq!(client.get_pending_orders().len(), 0);
    }

    #[test]
    fn test_settle_batch_still_executes_orders_within_their_own_deadline() {
        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();
        env.ledger().set_timestamp(1000);

        let (ta, tb, pool, admin) = setup(&env);

        let auction_addr = env.register_contract(None, BatchAuction);
        let client = BatchAuctionClient::new(&env, &auction_addr);
        client.initialize(&admin, &30_u64);

        let trader = Address::generate(&env);
        StellarAssetClient::new(&env, &ta).mint(&trader, &100_000_i128);

        // Deadline is after settlement time, so the order must still execute.
        client.submit_order(&trader, &pool, &ta, &tb, &10_000_i128, &0_i128, &2_000_u64);

        env.ledger().set_timestamp(1031);

        let results = client.settle_batch();
        assert_eq!(results.len(), 1);
        assert!(results.get(0).unwrap() > 0);
        assert!(StellarTokenClient::new(&env, &tb).balance(&trader) > 0);
    }

    #[test]
    fn test_multiple_traders_in_same_batch() {
        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();
        env.ledger().set_timestamp(1000);

        let (ta, tb, pool, admin) = setup(&env);

        let auction_addr = env.register_contract(None, BatchAuction);
        BatchAuctionClient::new(&env, &auction_addr).initialize(&admin, &60_u64);

        let trader1 = Address::generate(&env);
        let trader2 = Address::generate(&env);
        StellarAssetClient::new(&env, &ta).mint(&trader1, &50_000_i128);
        StellarAssetClient::new(&env, &ta).mint(&trader2, &50_000_i128);

        BatchAuctionClient::new(&env, &auction_addr).submit_order(
            &trader1,
            &pool,
            &ta,
            &tb,
            &5_000_i128,
            &0_i128,
            &u64::MAX,
        );
        BatchAuctionClient::new(&env, &auction_addr).submit_order(
            &trader2,
            &pool,
            &ta,
            &tb,
            &5_000_i128,
            &0_i128,
            &u64::MAX,
        );

        env.ledger().set_timestamp(1061);

        let results = BatchAuctionClient::new(&env, &auction_addr).settle_batch();

        assert_eq!(results.len(), 2);
        assert!(results.get(0).unwrap() > 0);
        assert!(results.get(1).unwrap() > 0);

        // Both traders received token_b.
        assert!(StellarTokenClient::new(&env, &tb).balance(&trader1) > 0);
        assert!(StellarTokenClient::new(&env, &tb).balance(&trader2) > 0);
    }

    // ── Issue #473: an unfillable order must not block the rest of the batch ──

    #[test]
    fn test_unfillable_order_is_refunded_and_does_not_block_batch() {
        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();
        env.ledger().set_timestamp(1000);

        let (ta, tb, pool, admin) = setup(&env);

        let auction_addr = env.register_contract(None, BatchAuction);
        let client = BatchAuctionClient::new(&env, &auction_addr);
        client.initialize(&admin, &30_u64);

        let good_trader = Address::generate(&env);
        let bad_trader = Address::generate(&env);
        StellarAssetClient::new(&env, &ta).mint(&good_trader, &10_000_i128);
        StellarAssetClient::new(&env, &ta).mint(&bad_trader, &10_000_i128);

        // An impossibly high min_out: this order can never clear at
        // settlement, no matter how the pool moves.
        client.submit_order(
            &bad_trader,
            &pool,
            &ta,
            &tb,
            &5_000_i128,
            &1_000_000_000_i128,
            &u64::MAX,
        );
        client.submit_order(
            &good_trader,
            &pool,
            &ta,
            &tb,
            &5_000_i128,
            &0_i128,
            &u64::MAX,
        );

        env.ledger().set_timestamp(1031);

        // Previously this reverted the whole batch, locking both escrows.
        let results = client.settle_batch();

        // Only the fillable order settled.
        assert_eq!(results.len(), 1);
        assert!(results.get(0).unwrap() > 0);

        // The good trader received token_b.
        assert!(StellarTokenClient::new(&env, &tb).balance(&good_trader) > 0);

        // The unfillable order's escrow was refunded in full, not stranded.
        assert_eq!(
            StellarTokenClient::new(&env, &ta).balance(&bad_trader),
            10_000_i128
        );
        assert_eq!(StellarTokenClient::new(&env, &tb).balance(&bad_trader), 0);

        // Both orders are cleared from the batch — the bad one is not left
        // pending to jam every future settlement attempt.
        assert_eq!(client.get_pending_orders().len(), 0);
    }

    #[test]
    fn test_submit_beyond_cap_returns_batch_full() {
        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();
        env.ledger().set_timestamp(1000);

        let (ta, tb, pool, admin) = setup(&env);

        let auction_addr = env.register_contract(None, BatchAuction);
        let client = BatchAuctionClient::new(&env, &auction_addr);
        client.initialize(&admin, &60_u64);
        client.set_max_orders(&admin, &2_u32);

        let trader = Address::generate(&env);
        StellarAssetClient::new(&env, &ta).mint(&trader, &10_000_i128);

        client.submit_order(&trader, &pool, &ta, &tb, &1_000_i128, &0_i128, &u64::MAX);
        client.submit_order(&trader, &pool, &ta, &tb, &1_000_i128, &0_i128, &u64::MAX);

        let result =
            client.try_submit_order(&trader, &pool, &ta, &tb, &1_000_i128, &0_i128, &u64::MAX);
        assert_eq!(result, Err(Ok(AuctionError::BatchFull)));

        let (pending_count, max_orders, opened_at, window_secs) = client.get_batch_info();
        assert_eq!(pending_count, 2);
        assert_eq!(max_orders, 2);
        assert_eq!(opened_at, 1000);
        assert_eq!(window_secs, 60);

        let trader_balance = StellarTokenClient::new(&env, &ta).balance(&trader);
        assert_eq!(trader_balance, 8_000_i128);
    }

    #[test]
    fn test_settlement_with_exactly_max_orders_succeeds() {
        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();
        env.ledger().set_timestamp(1000);

        let (ta, tb, pool, admin) = setup(&env);

        let auction_addr = env.register_contract(None, BatchAuction);
        let client = BatchAuctionClient::new(&env, &auction_addr);
        client.initialize(&admin, &30_u64);
        client.set_max_orders(&admin, &3_u32);

        let trader = Address::generate(&env);
        StellarAssetClient::new(&env, &ta).mint(&trader, &10_000_i128);

        for _ in 0..3 {
            client.submit_order(&trader, &pool, &ta, &tb, &1_000_i128, &0_i128, &u64::MAX);
        }

        env.ledger().set_timestamp(1031);

        let results = client.settle_batch();
        assert_eq!(results.len(), 3);
        for i in 0..results.len() {
            assert!(results.get(i).unwrap() > 0);
        }

        let (pending_count, max_orders, opened_at, window_secs) = client.get_batch_info();
        assert_eq!(pending_count, 0);
        assert_eq!(max_orders, 3);
        assert_eq!(opened_at, 1031);
        assert_eq!(window_secs, 30);
        assert_eq!(client.get_pending_orders().len(), 0);
        assert!(StellarTokenClient::new(&env, &tb).balance(&trader) > 0);
    }

    // ── Issue #351: concentrated-liquidity settlement venue ────────────────────

    #[test]
    fn test_submit_and_settle_cl_order() {
        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();
        env.ledger().set_timestamp(1000);

        let admin = Address::generate(&env);
        let ta = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();
        let tb = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();
        let cl_pool = deploy_cl_pool(&env, &admin, &ta, &tb);

        let auction_addr = env.register_contract(None, BatchAuction);
        let client = BatchAuctionClient::new(&env, &auction_addr);
        client.initialize(&admin, &30_u64);

        let trader = Address::generate(&env);
        StellarAssetClient::new(&env, &ta).mint(&trader, &100_000_i128);

        // token A → token B is the zero_for_one direction; limit 0 lets the pool
        // walk down to its own bound.
        client.submit_order_cl(
            &trader,
            &cl_pool,
            &ta,
            &tb,
            &10_000_i128,
            &0_i128,
            &true,
            &0_u128,
            &None,
            &u64::MAX,
        );

        env.ledger().set_timestamp(1031);

        let results = client.settle_batch();
        assert_eq!(results.len(), 1);
        assert!(results.get(0).unwrap() > 0);

        // Trader received token_b from the CL pool.
        assert!(StellarTokenClient::new(&env, &tb).balance(&trader) > 0);
        // Escrowed token_a was fully consumed by the swap.
        assert_eq!(
            StellarTokenClient::new(&env, &ta).balance(&trader),
            90_000_i128
        );
    }

    #[test]
    fn test_quote_and_settle_route_to_best_venue() {
        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();
        env.ledger().set_timestamp(1000);

        let admin = Address::generate(&env);
        let ta = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();
        let tb = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();

        // Two venues over the same pair: a constant-product AMM and a CL pool.
        let amm_pool = deploy_pool(&env, &ta, &tb);
        let lp = Address::generate(&env);
        StellarAssetClient::new(&env, &ta).mint(&lp, &2_000_000_i128);
        StellarAssetClient::new(&env, &tb).mint(&lp, &2_000_000_i128);
        AmmPoolClient::new(&env, &amm_pool).add_liquidity(
            &lp,
            &1_000_000_i128,
            &1_000_000_i128,
            &0_i128,
            &u64::MAX,
        );
        let cl_pool = deploy_cl_pool(&env, &admin, &ta, &tb);

        let auction_addr = env.register_contract(None, BatchAuction);
        let client = BatchAuctionClient::new(&env, &auction_addr);
        client.initialize(&admin, &30_u64);

        let trader = Address::generate(&env);
        StellarAssetClient::new(&env, &ta).mint(&trader, &100_000_i128);

        // CL order with the AMM pool as alternate venue.
        let order_id = client.submit_order_cl(
            &trader,
            &cl_pool,
            &ta,
            &tb,
            &10_000_i128,
            &0_i128,
            &true,
            &0_u128,
            &Some(amm_pool.clone()),
            &u64::MAX,
        );

        // Independently quote both venues; the best of the two must match the
        // contract's chosen quote.
        let amm_q = AmmPoolClient::new(&env, &amm_pool).get_amount_out(&ta, &10_000_i128);
        let cl_q = ConcentratedLiquidityClient::new(&env, &cl_pool)
            .estimate_price_impact(&true, &10_000_i128, &0_u128)
            .amount_out;
        let expected_best = amm_q.max(cl_q);
        let expected_pool = if amm_q > cl_q {
            amm_pool.clone()
        } else {
            cl_pool.clone()
        };

        let (best_out, best_pool, _ptype) = client.quote_order(&order_id);
        assert_eq!(best_out, expected_best);
        assert_eq!(best_pool, expected_pool);

        env.ledger().set_timestamp(1031);
        let results = client.settle_batch();
        assert_eq!(results.len(), 1);
        // Realized output is at least the best quote's min_out and positive.
        assert!(results.get(0).unwrap() > 0);
        assert!(StellarTokenClient::new(&env, &tb).balance(&trader) > 0);
    }

    #[test]
    fn test_submit_order_cl_rejects_alt_pool_with_wrong_pair() {
        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();
        env.ledger().set_timestamp(1000);

        let admin = Address::generate(&env);
        let ta = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();
        let tb = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();
        let tc = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();
        let cl_pool = deploy_cl_pool(&env, &admin, &ta, &tb);
        let wrong_amm_pool = deploy_pool(&env, &ta, &tc);

        let auction_addr = env.register_contract(None, BatchAuction);
        let client = BatchAuctionClient::new(&env, &auction_addr);
        client.initialize(&admin, &30_u64);

        let trader = Address::generate(&env);
        StellarAssetClient::new(&env, &ta).mint(&trader, &100_000_i128);

        let result = client.try_submit_order_cl(
            &trader,
            &cl_pool,
            &ta,
            &tb,
            &10_000_i128,
            &0_i128,
            &true,
            &0_u128,
            &Some(wrong_amm_pool),
            &u64::MAX,
        );
        assert_eq!(result, Err(Ok(AuctionError::InvalidPoolTokenPair)));
        assert_eq!(
            StellarTokenClient::new(&env, &ta).balance(&trader),
            100_000_i128
        );
        assert_eq!(client.get_pending_orders().len(), 0);
    }

    #[test]
    fn test_quote_order_ignores_stored_alt_pool_with_wrong_pair() {
        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();
        env.ledger().set_timestamp(1000);

        let admin = Address::generate(&env);
        let ta = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();
        let tb = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();
        let tc = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();

        let primary_pool = deploy_cl_pool(&env, &admin, &ta, &tb);
        let wrong_alt_pool = deploy_pool(&env, &ta, &tc);

        let auction_addr = env.register_contract(None, BatchAuction);
        let client = BatchAuctionClient::new(&env, &auction_addr);
        client.initialize(&admin, &30_u64);

        let trader = Address::generate(&env);
        let order = Order {
            id: 7,
            trader,
            pool: primary_pool.clone(),
            token_in: ta.clone(),
            token_out: tb.clone(),
            amount_in: 10_000_i128,
            min_out: 0_i128,
            submitted_at: 1000,
            deadline: u64::MAX,
            pool_type: PoolType::Cl,
            zero_for_one: true,
            sqrt_price_limit: 0_u128,
            alt_pool: Some(wrong_alt_pool),
        };
        env.as_contract(&auction_addr, || {
            env.storage()
                .instance()
                .set(&DataKey::Order(order.id), &order);
        });

        let (best_out, best_pool, best_type) = client.quote_order(&order.id);
        let primary_q = ConcentratedLiquidityClient::new(&env, &primary_pool)
            .estimate_price_impact(&true, &10_000_i128, &0_u128)
            .amount_out;
        assert_eq!(best_out, primary_q);
        assert_eq!(best_pool, primary_pool);
        assert_eq!(best_type, PoolType::Cl);
    }

    #[test]
    fn test_amm_order_still_defaults_to_amm_pool_type() {
        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();
        env.ledger().set_timestamp(1000);

        let (ta, tb, pool, admin) = setup(&env);

        let auction_addr = env.register_contract(None, BatchAuction);
        let client = BatchAuctionClient::new(&env, &auction_addr);
        client.initialize(&admin, &30_u64);

        let trader = Address::generate(&env);
        StellarAssetClient::new(&env, &ta).mint(&trader, &100_000_i128);
        let id = client.submit_order(&trader, &pool, &ta, &tb, &10_000_i128, &0_i128, &u64::MAX);

        let order = client.get_pending_orders().get(0).unwrap();
        assert_eq!(order.pool_type, PoolType::Amm);
        assert!(order.alt_pool.is_none());

        // The quote resolves through the AMM venue.
        let (best_out, best_pool, ptype) = client.quote_order(&id);
        assert_eq!(best_pool, pool);
        assert_eq!(ptype, PoolType::Amm);
        assert!(best_out > 0);
    }

    #[test]
    fn test_partial_settlement_refreshes_batch_opened_at() {
        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();
        env.ledger().set_timestamp(1000);

        let (ta, tb, pool, admin) = setup(&env);

        let auction_addr = env.register_contract(None, BatchAuction);
        let client = BatchAuctionClient::new(&env, &auction_addr);
        client.initialize(&admin, &30_u64);
        client.set_max_orders(&admin, &2_u32);

        let trader1 = Address::generate(&env);
        let trader2 = Address::generate(&env);
        StellarAssetClient::new(&env, &ta).mint(&trader1, &100_000_i128);
        StellarAssetClient::new(&env, &ta).mint(&trader2, &100_000_i128);

        client.submit_order(&trader1, &pool, &ta, &tb, &10_000_i128, &0_i128, &u64::MAX);
        client.submit_order(&trader2, &pool, &ta, &tb, &10_000_i128, &0_i128, &u64::MAX);

        // Increase max_orders so we can submit a 3rd order, but set max_orders to 1 before settlement
        client.set_max_orders(&admin, &1_u32);

        // Advance timestamp past window (1000 + 30 = 1030)
        env.ledger().set_timestamp(1030);

        let results = client.settle_batch();
        assert_eq!(results.len(), 1);
        assert_eq!(client.get_pending_orders().len(), 1);

        let (_pending_count, _max_orders, opened_at, _window_secs) = client.get_batch_info();
        assert_eq!(
            opened_at, 1030,
            "BatchOpenedAt must be refreshed to timestamp of settlement"
        );

        // Submit another order at timestamp 1035
        env.ledger().set_timestamp(1035);
        client.set_max_orders(&admin, &5_u32);
        let trader3 = Address::generate(&env);
        StellarAssetClient::new(&env, &ta).mint(&trader3, &100_000_i128);
        client.submit_order(&trader3, &pool, &ta, &tb, &10_000_i128, &0_i128, &u64::MAX);

        // Attempting settle_batch at t=1035 must fail because opened_at is 1030 and window is 30s
        let err = client.try_settle_batch().err().unwrap().unwrap();
        assert_eq!(err, AuctionError::BatchWindowOpen);
    }

    // ── Issue #546: transfer failure isolation ──────────────────────────────────

    /// Verify that a swap failure followed by a *successful* refund still
    /// isolates the order (issue #473) and that `try_transfer` wrapping on
    /// the refund leg (issue #546) is exercised. The order is dropped and
    /// the batch continues.
    #[test]
    fn test_swap_failure_refund_via_try_transfer() {
        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();
        env.ledger().set_timestamp(1000);

        let (ta, tb, pool, admin) = setup(&env);

        let auction_addr = env.register_contract(None, BatchAuction);
        let client = BatchAuctionClient::new(&env, &auction_addr);
        client.initialize(&admin, &30_u64);

        let bad_trader = Address::generate(&env);
        StellarAssetClient::new(&env, &ta).mint(&bad_trader, &10_000_i128);

        client.submit_order(
            &bad_trader,
            &pool,
            &ta,
            &tb,
            &5_000_i128,
            &1_000_000_000_i128, // impossible min_out
            &u64::MAX,
        );

        env.ledger().set_timestamp(1031);

        let results = client.settle_batch();
        assert_eq!(results.len(), 0);

        // Refund went through (try_transfer succeeded here).
        assert_eq!(
            StellarTokenClient::new(&env, &ta).balance(&bad_trader),
            10_000_i128
        );
        assert_eq!(client.get_pending_orders().len(), 0);
    }

    /// Issue #546: payout transfer failure isolation.
    ///
    /// Submit two orders. Before settlement, modify the first order's
    /// `token_out` in storage to point at a non-existent contract. The swap
    /// succeeds (AMM swap doesn't reference `token_out` directly) but the
    /// payout `try_transfer` traps on the fake address. The failing order is
    /// dropped and the second order still settles — proving a single bad
    /// payout cannot revert the entire batch.
    #[test]
    fn test_failed_payout_does_not_revert_batch() {
        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();
        env.ledger().set_timestamp(1000);

        let (ta, tb, pool, admin) = setup(&env);

        let auction_addr = env.register_contract(None, BatchAuction);
        let client = BatchAuctionClient::new(&env, &auction_addr);
        client.initialize(&admin, &30_u64);
        client.set_max_orders(&admin, &2_u32);

        let bad_trader = Address::generate(&env);
        let good_trader = Address::generate(&env);
        StellarAssetClient::new(&env, &ta).mint(&bad_trader, &100_000_i128);
        StellarAssetClient::new(&env, &ta).mint(&good_trader, &100_000_i128);

        // Both orders submit with valid tokens — escrow succeeds.
        client.submit_order(
            &bad_trader,
            &pool,
            &ta,
            &tb,
            &10_000_i128,
            &0_i128,
            &u64::MAX,
        );
        client.submit_order(
            &good_trader,
            &pool,
            &ta,
            &tb,
            &10_000_i128,
            &0_i128,
            &u64::MAX,
        );

        // Mutate the bad order's token_out to a non-existent contract
        // address so that the payout try_transfer will trap.
        let fake_token_out = Address::generate(&env);
        env.as_contract(&auction_addr, || {
            let mut bad_order: Order = env.storage().instance().get(&DataKey::Order(0)).unwrap();
            bad_order.token_out = fake_token_out;
            env.storage().instance().set(&DataKey::Order(0), &bad_order);
        });

        env.ledger().set_timestamp(1031);

        // Batch must NOT revert — the good order settles despite the bad
        // order's payout transfer failure.
        let results = client.settle_batch();
        assert_eq!(results.len(), 1);
        assert!(results.get(0).unwrap() > 0);
        assert!(StellarTokenClient::new(&env, &tb).balance(&good_trader) > 0);

        // Both orders are cleared from the batch.
        assert_eq!(client.get_pending_orders().len(), 0);
    }

    /// Issue #546: deadline-expiry refund transfer failure isolation.
    ///
    /// Submit an order with a short deadline that has already passed and
    /// mutate its `token_in` to a non-existent contract address before
    /// settlement. The order hits the deadline-expired path and the refund
    /// `try_transfer` traps on the fake address. The batch must complete
    /// without reverting.
    #[test]
    fn test_failed_deadline_refund_does_not_revert_batch() {
        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();
        env.ledger().set_timestamp(1000);

        let (ta, tb, pool, admin) = setup(&env);

        let auction_addr = env.register_contract(None, BatchAuction);
        let client = BatchAuctionClient::new(&env, &auction_addr);
        client.initialize(&admin, &30_u64);

        let trader = Address::generate(&env);
        StellarAssetClient::new(&env, &ta).mint(&trader, &100_000_i128);

        // Submit an order with a short deadline (1010) that will expire.
        client.submit_order(&trader, &pool, &ta, &tb, &10_000_i128, &0_i128, &1010_u64);

        // Mutate token_in to a non-existent contract so the deadline-expiry
        // refund try_transfer traps.
        let fake_token_in = Address::generate(&env);
        env.as_contract(&auction_addr, || {
            let mut order: Order = env.storage().instance().get(&DataKey::Order(0)).unwrap();
            order.token_in = fake_token_in;
            env.storage().instance().set(&DataKey::Order(0), &order);
        });

        // Advance past the batch window AND the deadline.
        env.ledger().set_timestamp(1040);

        // Batch must NOT revert — the expired order is dropped despite the
        // refund transfer failure.
        let results = client.settle_batch();
        assert_eq!(results.len(), 0);
        assert_eq!(client.get_pending_orders().len(), 0);
    }

    // ── Issue #553: two-step admin rotation ─────────────────────────────────────

    #[test]
    fn test_propose_and_accept_admin() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let nominee = Address::generate(&env);
        let auction_addr = env.register_contract(None, BatchAuction);
        let client = BatchAuctionClient::new(&env, &auction_addr);
        client.initialize(&admin, &30_u64);

        assert_eq!(client.get_admin(), admin);
        assert_eq!(client.get_pending_admin(), None);

        client.propose_admin(&admin, &nominee);
        assert_eq!(client.get_pending_admin(), Some(nominee.clone()));
        // Admin is unchanged until accept.
        assert_eq!(client.get_admin(), admin);

        client.accept_admin(&nominee);
        assert_eq!(client.get_admin(), nominee);
        assert_eq!(client.get_pending_admin(), None);

        // New admin can call admin-only setters; old admin cannot.
        client.set_max_orders(&nominee, &10_u32);
        let err = client
            .try_set_max_orders(&admin, &5_u32)
            .err()
            .unwrap()
            .unwrap();
        assert_eq!(err, AuctionError::Unauthorized);
    }

    #[test]
    fn test_propose_admin_requires_current_admin() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let rando = Address::generate(&env);
        let nominee = Address::generate(&env);
        let auction_addr = env.register_contract(None, BatchAuction);
        let client = BatchAuctionClient::new(&env, &auction_addr);
        client.initialize(&admin, &30_u64);

        let err = client
            .try_propose_admin(&rando, &nominee)
            .err()
            .unwrap()
            .unwrap();
        assert_eq!(err, AuctionError::Unauthorized);
    }

    #[test]
    fn test_accept_admin_requires_pending_nominee() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let nominee = Address::generate(&env);
        let other = Address::generate(&env);
        let auction_addr = env.register_contract(None, BatchAuction);
        let client = BatchAuctionClient::new(&env, &auction_addr);
        client.initialize(&admin, &30_u64);

        let err = client.try_accept_admin(&nominee).err().unwrap().unwrap();
        assert_eq!(err, AuctionError::NoPendingAdmin);

        client.propose_admin(&admin, &nominee);
        let err = client.try_accept_admin(&other).err().unwrap().unwrap();
        assert_eq!(err, AuctionError::WrongAdmin);
    }

    #[test]
    fn test_new_admin_controls_batch_window_after_acceptance() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let nominee = Address::generate(&env);
        let auction_addr = env.register_contract(None, BatchAuction);
        let client = BatchAuctionClient::new(&env, &auction_addr);
        client.initialize(&admin, &30_u64);

        client.propose_admin(&admin, &nominee);
        client.accept_admin(&nominee);

        // Nominee (new admin) can update the window; auth is checked against
        // the stored admin inside set_batch_window.
        client.set_batch_window(&60_u64);
        let (_, _, _, window) = client.get_batch_info();
        assert_eq!(window, 60);
    }
}
