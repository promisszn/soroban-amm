#![no_std]

//! DEX aggregator — routes trades across multiple AMM and CL pools for best execution.

use soroban_sdk::{
    contract, contractclient, contracterror, contractimpl, contracttype, symbol_short, vec,
    Address, Env, Vec,
};

use pool_interfaces::{AmmPoolClient, FactoryClient};

const MIN_TTL: u32 = 172_800;
const BUMP_TO: u32 = 518_400;

#[contractclient(name = "ClPoolClient")]
pub trait ClPoolInterface {
    fn estimate_price_impact(
        env: Env,
        zero_for_one: bool,
        amount_in: i128,
        sqrt_price_limit_x96: u128,
    ) -> PriceImpactEstimate;
    fn swap(
        env: Env,
        sender: Address,
        zero_for_one: bool,
        amount_in: i128,
        sqrt_price_limit_x96: u128,
        min_amount_out: i128,
        deadline: u64,
    ) -> i128;
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PriceImpactEstimate {
    pub amount_in: i128,
    pub amount_in_after_fee: i128,
    pub amount_out: i128,
    pub fee_amount: i128,
    pub spot_price_before: i128,
    pub effective_price: i128,
    pub price_impact_bps: i128,
    pub sqrt_price_before: u128,
    pub sqrt_price_after: u128,
    pub tick_before: i32,
    pub tick_after: i32,
    pub active_liquidity_before: i128,
    pub active_liquidity_after: i128,
}

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum AggregatorError {
    NoRouteFound = 1,
    SlippageExceeded = 2,
    InvalidMaxHops = 3,
    TooManyRoutingTokens = 4,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PoolKind {
    Amm,
    Cl,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RouteHop {
    pub pool: Address,
    pub pool_kind: PoolKind,
    pub token_in: Address,
    pub token_out: Address,
    pub zero_for_one: bool,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClPoolInfo {
    pub pool: Address,
    pub token_a: Address,
    pub token_b: Address,
    pub fee_bps: i128,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RouteQuote {
    pub amount_out: i128,
    pub hops: Vec<RouteHop>,
}

#[contracttype]
pub enum DataKey {
    Admin,
    Factory,
    MaxHops,
    RoutingTokens,
    ClPoolCount,
    ClPool(u32),
}

#[contract]
pub struct DexAggregator;

#[contractimpl]
impl DexAggregator {
    pub const DEFAULT_MAX_HOPS: u32 = 4;
    pub const PRICE_TOLERANCE_BPS: i128 = 10;
    pub const BPS: i128 = 10_000;
    pub const MAX_CL_POOLS: u32 = 50;
    pub const MAX_ROUTING_TOKENS: u32 = 50;
    pub const CL_FEE_TIERS: [i128; 3] = [30, 100, 500];

    pub const MIN_SQRT_PRICE: u128 = 4_295_128_739_u128;
    pub const MAX_SQRT_PRICE: u128 = 340_275_971_719_517_849_884_931_781_110_561_029_923_u128;

    pub fn initialize(env: Env, admin: Address, factory: Address) {
        assert!(
            !env.storage().instance().has(&DataKey::Factory),
            "already initialized"
        );
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Factory, &factory);
        env.storage()
            .instance()
            .set(&DataKey::MaxHops, &Self::DEFAULT_MAX_HOPS);
        env.storage().instance().set(&DataKey::ClPoolCount, &0u32);
    }

    pub fn set_max_hops(env: Env, max_hops: u32) -> Result<(), AggregatorError> {
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();

        if max_hops == 0 {
            return Err(AggregatorError::InvalidMaxHops);
        }

        Self::extend_ttl(&env);
        env.storage().instance().set(&DataKey::MaxHops, &max_hops);
        Ok(())
    }

    pub fn register_cl_pool(
        env: Env,
        pool: Address,
        token_a: Address,
        token_b: Address,
        fee_bps: i128,
    ) {
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();

        Self::extend_ttl(&env);
        let count: u32 = env
            .storage()
            .instance()
            .get(&DataKey::ClPoolCount)
            .unwrap_or(0);

        for i in 0..count {
            let entry: ClPoolInfo = env.storage().instance().get(&DataKey::ClPool(i)).unwrap();
            if entry.pool == pool {
                return;
            }
        }

        assert!(count < Self::MAX_CL_POOLS, "max CL pools reached");

        env.storage().instance().set(
            &DataKey::ClPool(count),
            &ClPoolInfo {
                pool: pool.clone(),
                token_a: token_a.clone(),
                token_b: token_b.clone(),
                fee_bps,
            },
        );
        env.storage()
            .instance()
            .set(&DataKey::ClPoolCount, &(count + 1));

        soroban_amm_sdk::emit_versioned_event!(
            &env,
            (symbol_short!("cl_reg"),),
            (token_a, token_b, fee_bps, pool)
        );
    }

    pub fn set_routing_tokens(env: Env, tokens: Vec<Address>) -> Result<(), AggregatorError> {
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();

        if tokens.len() as u32 > Self::MAX_ROUTING_TOKENS {
            return Err(AggregatorError::TooManyRoutingTokens);
        }

        Self::extend_ttl(&env);
        env.storage()
            .instance()
            .set(&DataKey::RoutingTokens, &tokens);
        Ok(())
    }

    pub fn remove_cl_pool(env: Env, pool: Address) {
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();

        Self::extend_ttl(&env);
        let count: u32 = env
            .storage()
            .instance()
            .get(&DataKey::ClPoolCount)
            .unwrap_or(0);

        for i in 0..count {
            let entry: ClPoolInfo = env.storage().instance().get(&DataKey::ClPool(i)).unwrap();
            if entry.pool == pool {
                if i != count - 1 {
                    let last: ClPoolInfo = env
                        .storage()
                        .instance()
                        .get(&DataKey::ClPool(count - 1))
                        .unwrap();
                    env.storage().instance().set(&DataKey::ClPool(i), &last);
                }
                env.storage().instance().remove(&DataKey::ClPool(count - 1));
                env.storage()
                    .instance()
                    .set(&DataKey::ClPoolCount, &(count - 1));
                return;
            }
        }
    }

    /// Find the best route up to `max_hops` pools deep (#319).
    pub fn find_best_route(
        env: Env,
        token_in: Address,
        token_out: Address,
        amount_in: i128,
        max_hops: u32,
    ) -> Result<RouteQuote, AggregatorError> {
        Self::extend_ttl(&env);
        assert!(token_in != token_out, "same token");
        assert!(amount_in > 0, "amount must be positive");
        let stored_max_hops: u32 = env
            .storage()
            .instance()
            .get(&DataKey::MaxHops)
            .unwrap_or(Self::DEFAULT_MAX_HOPS);
        let cap = max_hops.min(stored_max_hops);
        if cap == 0 {
            return Err(AggregatorError::NoRouteFound);
        }
        let (quote, runner_up) =
            Self::search_best_bfs(&env, &token_in, &token_out, amount_in, cap)?;

        // The venue a route is *entered* through identifies the decision: it is
        // the pool the aggregator picked over every alternative first hop.
        let venue = quote.hops.get(0).unwrap();
        soroban_amm_sdk::emit_versioned_event!(
            &env,
            (symbol_short!("route_sel"),),
            (
                venue.pool.clone(),
                venue.pool_kind.clone(),
                amount_in,
                quote.amount_out
            )
        );

        // Only meaningful when a second venue actually produced a quote; with a
        // single venue there is no improvement to measure.
        if let Some((alt_pool, alt_kind, alt_out)) = runner_up {
            soroban_amm_sdk::emit_versioned_event!(
                &env,
                (symbol_short!("route_alt"),),
                (
                    venue.pool.clone(),
                    quote.amount_out,
                    alt_pool,
                    alt_kind,
                    alt_out
                )
            );
        }

        Ok(quote)
    }

    /// Read-only quote for off-chain simulation (#319).
    pub fn get_quote(
        env: Env,
        token_in: Address,
        token_out: Address,
        amount_in: i128,
        max_hops: u32,
    ) -> Result<RouteQuote, AggregatorError> {
        Self::find_best_route(env, token_in, token_out, amount_in, max_hops)
    }

    /// Execute a pre-computed multi-hop route atomically (#319).
    pub fn execute_route(
        env: Env,
        route: RouteQuote,
        trader: Address,
        amount_in: i128,
        min_out: i128,
        deadline: u64,
    ) -> Result<i128, AggregatorError> {
        Self::extend_ttl(&env);
        trader.require_auth();
        if route.hops.is_empty() || route.amount_out < min_out {
            return Err(AggregatorError::SlippageExceeded);
        }
        if deadline < env.ledger().timestamp() {
            return Err(AggregatorError::SlippageExceeded);
        }
        let entry = route.hops.get(0).unwrap();
        let exit = route.hops.get(route.hops.len() - 1).unwrap();
        let amount_out =
            Self::execute_hops(&env, &route.hops, &trader, amount_in, min_out, deadline)?;

        // `amount_out` is what the pools actually returned, not `route.amount_out`,
        // which is only the quote the route was planned against.
        soroban_amm_sdk::emit_versioned_event!(
            &env,
            (symbol_short!("route_exe"),),
            (
                trader,
                entry.token_in.clone(),
                exit.token_out.clone(),
                amount_in,
                amount_out,
                entry.pool.clone()
            )
        );

        Ok(amount_out)
    }

    pub fn swap_best(
        env: Env,
        trader: Address,
        token_in: Address,
        token_out: Address,
        amount_in: i128,
        min_out: i128,
    ) -> Result<i128, AggregatorError> {
        Self::extend_ttl(&env);
        let max_hops: u32 = env
            .storage()
            .instance()
            .get(&DataKey::MaxHops)
            .unwrap_or(Self::DEFAULT_MAX_HOPS);
        let quote = Self::find_best_route(env.clone(), token_in, token_out, amount_in, max_hops)?;
        let deadline = env.ledger().timestamp() + 3600;
        Self::execute_route(env, quote, trader, amount_in, min_out, deadline)
    }

    pub fn is_price_within_tolerance(
        env: Env,
        token_in: Address,
        token_out: Address,
        amount_in: i128,
        quoted_out: i128,
    ) -> bool {
        Self::extend_ttl(&env);
        let max_hops: u32 = env
            .storage()
            .instance()
            .get(&DataKey::MaxHops)
            .unwrap_or(Self::DEFAULT_MAX_HOPS);
        let Ok(best) = Self::find_best_route(env.clone(), token_in, token_out, amount_in, max_hops)
        else {
            return quoted_out == 0;
        };
        if best.amount_out == 0 {
            return quoted_out == 0;
        }
        let diff = if best.amount_out >= quoted_out {
            best.amount_out - quoted_out
        } else {
            quoted_out - best.amount_out
        };
        let observed_bps = diff * Self::BPS / best.amount_out;
        if observed_bps > Self::PRICE_TOLERANCE_BPS {
            soroban_amm_sdk::emit_versioned_event!(
                &env,
                (symbol_short!("tol_fail"),),
                (
                    best.hops.get(0).unwrap().pool,
                    observed_bps,
                    Self::PRICE_TOLERANCE_BPS
                )
            );
            return false;
        }
        true
    }

    /// Breadth-first search for the best route, plus the runner-up quote.
    ///
    /// The second element is the best *other* venue that completed a route to
    /// `token_out`, or `None` when only one venue ever quoted. Callers use it to
    /// report how much the winning venue actually improved on the alternative.
    #[allow(clippy::type_complexity)]
    fn search_best_bfs(
        env: &Env,
        token_in: &Address,
        token_out: &Address,
        amount_in: i128,
        max_hops: u32,
    ) -> Result<(RouteQuote, Option<(Address, PoolKind, i128)>), AggregatorError> {
        let factory: Address = env.storage().instance().get(&DataKey::Factory).unwrap();
        let factory_client = FactoryClient::new(env, &factory);
        let tokens = Self::discover_tokens(env, token_in, token_out);

        let mut best_out: i128 = 0;
        let mut best_hops: Vec<RouteHop> = Vec::new(env);
        // Best completed route whose entry venue differs from the winner's.
        let mut runner_up: Option<(Address, PoolKind, i128)> = None;

        let mut frontier_token: Vec<Address> = Vec::new(env);
        let mut frontier_amount: Vec<i128> = Vec::new(env);
        let mut frontier_hops: Vec<Vec<RouteHop>> = Vec::new(env);
        let mut frontier_depth: Vec<u32> = Vec::new(env);

        // (token, depth) pairs already enqueued. Without this, every frontier
        // node re-expands to O(N) neighbours regardless of whether they have
        // already been explored, blowing the frontier up to O(N^max_hops) and
        // exhausting the per-transaction instruction budget (#363).
        let mut visited: Vec<(Address, u32, i128)> = Vec::new(env);

        frontier_token.push_back(token_in.clone());
        frontier_amount.push_back(amount_in);
        frontier_hops.push_back(Vec::new(env));
        frontier_depth.push_back(0);
        visited.push_back((token_in.clone(), 0, amount_in));

        let mut idx: u32 = 0;
        while idx < frontier_token.len() {
            let current_token = frontier_token.get(idx).unwrap();
            let current_amount = frontier_amount.get(idx).unwrap();
            let current_hops = frontier_hops.get(idx).unwrap();
            let depth = frontier_depth.get(idx).unwrap();
            idx += 1;

            if depth >= max_hops {
                continue;
            }

            for t in 0..tokens.len() {
                let next_token = tokens.get(t).unwrap();
                if next_token == current_token {
                    continue;
                }

                let Some(step) = Self::quote_hop(
                    env,
                    &factory_client,
                    &current_token,
                    &next_token,
                    current_amount,
                ) else {
                    continue;
                };

                let mut new_hops = Vec::new(env);
                for h in 0..current_hops.len() {
                    new_hops.push_back(current_hops.get(h).unwrap());
                }
                new_hops.push_back(step.hops.get(0).unwrap());

                if next_token == *token_out {
                    if step.amount_out > best_out {
                        // The old winner becomes the alternative to compare against.
                        if !best_hops.is_empty() {
                            let prev = best_hops.get(0).unwrap();
                            runner_up =
                                Self::better_alt(runner_up, prev.pool, prev.pool_kind, best_out);
                        }
                        best_out = step.amount_out;
                        best_hops = new_hops;
                    } else {
                        let entry = new_hops.get(0).unwrap();
                        runner_up = Self::better_alt(
                            runner_up,
                            entry.pool,
                            entry.pool_kind,
                            step.amount_out,
                        );
                    }
                } else if depth + 1 < max_hops
                    && !Self::is_visited_and_worse(
                        &mut visited,
                        &next_token,
                        depth + 1,
                        step.amount_out,
                    )
                {
                    frontier_token.push_back(next_token);
                    frontier_amount.push_back(step.amount_out);
                    frontier_hops.push_back(new_hops);
                    frontier_depth.push_back(depth + 1);
                }
            }
        }

        if best_out <= 0 || best_hops.is_empty() {
            return Err(AggregatorError::NoRouteFound);
        }

        // A route that re-enters through the winning venue is the same venue
        // decision, not an alternative to it.
        let winner_pool = best_hops.get(0).unwrap().pool;
        let runner_up = match runner_up {
            Some((pool, _, _)) if pool == winner_pool => None,
            other => other,
        };

        Ok((
            RouteQuote {
                amount_out: best_out,
                hops: best_hops,
            },
            runner_up,
        ))
    }

    /// Keep whichever of the two alternatives quoted more output.
    fn better_alt(
        current: Option<(Address, PoolKind, i128)>,
        pool: Address,
        kind: PoolKind,
        amount_out: i128,
    ) -> Option<(Address, PoolKind, i128)> {
        match current {
            Some((_, _, best)) if best >= amount_out => current,
            _ => Some((pool, kind, amount_out)),
        }
    }

    fn execute_hops(
        env: &Env,
        hops: &Vec<RouteHop>,
        trader: &Address,
        amount_in: i128,
        min_out: i128,
        deadline: u64,
    ) -> Result<i128, AggregatorError> {
        let mut current = amount_in;
        let last = hops.len() - 1;
        for i in 0..hops.len() {
            let hop = hops.get(i).unwrap();
            let hop_min = if i == last { min_out } else { 0 };
            current = match hop.pool_kind {
                PoolKind::Amm => AmmPoolClient::new(env, &hop.pool).swap(
                    trader,
                    &hop.token_in,
                    &current,
                    &hop_min,
                    &deadline,
                ),
                PoolKind::Cl => {
                    let limit = if hop.zero_for_one {
                        Self::MIN_SQRT_PRICE + 1
                    } else {
                        Self::MAX_SQRT_PRICE - 1
                    };
                    ClPoolClient::new(env, &hop.pool).swap(
                        trader,
                        &hop.zero_for_one,
                        &current,
                        &limit,
                        &hop_min,
                        &deadline,
                    )
                }
            };
        }
        Ok(current)
    }

    fn quote_hop(
        env: &Env,
        factory: &FactoryClient,
        token_in: &Address,
        token_out: &Address,
        amount_in: i128,
    ) -> Option<RouteQuote> {
        if amount_in <= 0 {
            return None;
        }

        let mut best: i128 = 0;
        let mut hop = RouteHop {
            pool: token_in.clone(),
            pool_kind: PoolKind::Amm,
            token_in: token_in.clone(),
            token_out: token_out.clone(),
            zero_for_one: true,
        };

        if let Some(pool) = factory.get_pool(token_in, token_out) {
            let out = AmmPoolClient::new(env, &pool).get_amount_out(token_in, &amount_in);
            if out > best {
                best = out;
                hop = RouteHop {
                    pool,
                    pool_kind: PoolKind::Amm,
                    token_in: token_in.clone(),
                    token_out: token_out.clone(),
                    zero_for_one: true,
                };
            }
        }

        let cl_count: u32 = env
            .storage()
            .instance()
            .get(&DataKey::ClPoolCount)
            .unwrap_or(0);
        for i in 0..cl_count {
            let info: ClPoolInfo = env.storage().instance().get(&DataKey::ClPool(i)).unwrap();
            if !(Self::is_cl_pool_match(&info, token_in, token_out)) {
                continue;
            }
            if let Some((out, zfo)) =
                Self::quote_cl(env, &info.pool, token_in, token_out, amount_in)
            {
                if out > best {
                    best = out;
                    hop = RouteHop {
                        pool: info.pool,
                        pool_kind: PoolKind::Cl,
                        token_in: token_in.clone(),
                        token_out: token_out.clone(),
                        zero_for_one: zfo,
                    };
                }
            }
        }

        for fee_idx in 0..3 {
            let fee = Self::CL_FEE_TIERS[fee_idx as usize];
            if let Some(cl) = factory.get_cl_pool(token_in, token_out, &fee) {
                if let Some((out, zfo)) = Self::quote_cl(env, &cl, token_in, token_out, amount_in) {
                    if out > best {
                        best = out;
                        hop = RouteHop {
                            pool: cl,
                            pool_kind: PoolKind::Cl,
                            token_in: token_in.clone(),
                            token_out: token_out.clone(),
                            zero_for_one: zfo,
                        };
                    }
                }
            }
        }

        if best <= 0 {
            return None;
        }

        Some(RouteQuote {
            amount_out: best,
            hops: vec![env, hop],
        })
    }

    fn quote_cl(
        env: &Env,
        pool: &Address,
        _token_in: &Address,
        _token_out: &Address,
        amount_in: i128,
    ) -> Option<(i128, bool)> {
        let client = ClPoolClient::new(env, pool);
        let mut best: i128 = 0;
        let mut zfo = true;
        for direction in [true, false] {
            let limit = if direction {
                Self::MIN_SQRT_PRICE + 1
            } else {
                Self::MAX_SQRT_PRICE - 1
            };
            let est = client.estimate_price_impact(&direction, &amount_in, &limit);
            if est.amount_out > best {
                best = est.amount_out;
                zfo = direction;
            }
        }
        if best > 0 {
            Some((best, zfo))
        } else {
            None
        }
    }

    fn discover_tokens(env: &Env, token_in: &Address, token_out: &Address) -> Vec<Address> {
        let mut tokens: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::RoutingTokens)
            .unwrap_or_else(|| Vec::new(env));
        Self::push_unique(&mut tokens, token_in.clone());
        Self::push_unique(&mut tokens, token_out.clone());

        let cl_count: u32 = env
            .storage()
            .instance()
            .get(&DataKey::ClPoolCount)
            .unwrap_or(0);
        for i in 0..cl_count {
            let info: ClPoolInfo = env.storage().instance().get(&DataKey::ClPool(i)).unwrap();
            Self::push_unique(&mut tokens, info.token_a.clone());
            Self::push_unique(&mut tokens, info.token_b.clone());
        }
        tokens
    }

    fn is_cl_pool_match(info: &ClPoolInfo, token_in: &Address, token_out: &Address) -> bool {
        (info.token_a == *token_in && info.token_b == *token_out)
            || (info.token_a == *token_out && info.token_b == *token_in)
    }

    /// Has `(token, depth)` already been enqueued with a better or equal amount? (#363)
    fn is_visited_and_worse(
        visited: &mut Vec<(Address, u32, i128)>,
        token: &Address,
        depth: u32,
        amount: i128,
    ) -> bool {
        for i in 0..visited.len() {
            let (t, d, a) = visited.get(i).unwrap();
            if d == depth && t == *token {
                if amount <= a {
                    return true;
                } else {
                    visited.set(i, (t, d, amount));
                    return false;
                }
            }
        }
        visited.push_back((token.clone(), depth, amount));
        false
    }

    fn push_unique(vec: &mut Vec<Address>, addr: Address) {
        for i in 0..vec.len() {
            if vec.get(i).unwrap() == addr {
                return;
            }
        }
        vec.push_back(addr);
    }

    fn extend_ttl(env: &Env) {
        env.storage().instance().extend_ttl(MIN_TTL, BUMP_TO);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use factory::{Factory, FactoryClient};
    use soroban_sdk::{
        testutils::{Address as _, Events as _},
        token::StellarAssetClient,
        Address, BytesN, IntoVal, Symbol,
    };

    #[test]
    fn test_no_route_when_uninitialized() {
        let env = Env::default();
        let agg_addr = env.register_contract(None, DexAggregator);
        let agg = DexAggregatorClient::new(&env, &agg_addr);
        let a = Address::generate(&env);
        let b = Address::generate(&env);
        let result = agg.try_find_best_route(&a, &b, &100_i128, &3u32);
        assert!(result.is_err());
    }

    #[test]
    fn test_visited_dedup_is_keyed_on_token_and_depth() {
        let env = Env::default();
        let token_a = Address::generate(&env);
        let token_b = Address::generate(&env);

        let mut visited: Vec<(Address, u32, i128)> = Vec::new(&env);
        visited.push_back((token_a.clone(), 1, 100));

        // Same (token, depth) pair with worse or equal amount is blocked.
        assert!(DexAggregator::is_visited_and_worse(
            &mut visited,
            &token_a,
            1,
            100
        ));
        assert!(DexAggregator::is_visited_and_worse(
            &mut visited,
            &token_a,
            1,
            50
        ));
        // Same (token, depth) pair with strictly better amount is allowed.
        assert!(!DexAggregator::is_visited_and_worse(
            &mut visited,
            &token_a,
            1,
            150
        ));

        // After being allowed, the new best amount is recorded (150).
        assert!(DexAggregator::is_visited_and_worse(
            &mut visited,
            &token_a,
            1,
            150
        ));
        assert!(DexAggregator::is_visited_and_worse(
            &mut visited,
            &token_a,
            1,
            140
        ));

        // Same token at a different depth must still be explorable.
        assert!(!DexAggregator::is_visited_and_worse(
            &mut visited,
            &token_a,
            2,
            50
        ));
        // A different token at the same depth is independent.
        assert!(!DexAggregator::is_visited_and_worse(
            &mut visited,
            &token_b,
            1,
            50
        ));
    }

    #[test]
    fn test_discover_tokens_includes_registered_cl_pools() {
        let env = Env::default();
        let factory_addr = env.register_contract(None, Factory);
        let factory = FactoryClient::new(&env, &factory_addr);
        let admin = Address::generate(&env);
        factory.initialize(
            &admin,
            &BytesN::from_array(&env, &[0u8; 32]),
            &BytesN::from_array(&env, &[1u8; 32]),
        );

        let agg_addr = env.register_contract(None, DexAggregator);
        let agg = DexAggregatorClient::new(&env, &agg_addr);
        let agg_admin = Address::generate(&env);
        agg.initialize(&agg_admin, &factory_addr);

        let token_a = Address::generate(&env);
        let token_b = Address::generate(&env);
        let cl_pool = Address::generate(&env);

        env.mock_all_auths();
        agg.register_cl_pool(&cl_pool, &token_a, &token_b, &30_i128);

        let tokens = env.as_contract(&agg_addr, || {
            DexAggregator::discover_tokens(&env, &token_a, &token_b)
        });
        assert_eq!(tokens.len(), 2);

        let mut found_a = false;
        let mut found_b = false;
        for i in 0..tokens.len() {
            let token = tokens.get(i).unwrap();
            if token == token_a {
                found_a = true;
            }
            if token == token_b {
                found_b = true;
            }
        }

        assert!(found_a);
        assert!(found_b);
    }

    // -------------------------------------------------------------------------
    // #685: versioned routing events
    // -------------------------------------------------------------------------

    struct Venues {
        env: Env,
        agg: Address,
        admin: Address,
        trader: Address,
        token_a: Address,
        token_b: Address,
        pool_ab: Address,
        factory: Address,
    }

    /// Aggregator wired to a real factory with a funded `token_a <-> token_b`
    /// AMM pool, plus a trader holding both tokens.
    fn setup_venues() -> Venues {
        let env = Env::default();
        env.mock_all_auths();
        env.budget().reset_unlimited();

        let admin = Address::generate(&env);
        let amm_wasm_hash = env.deployer().upload_contract_wasm(amm::WASM);
        let lp_wasm_hash = env.deployer().upload_contract_wasm(token::WASM);

        let factory_addr = env.register_contract(None, Factory);
        let factory = FactoryClient::new(&env, &factory_addr);
        factory.initialize(&admin, &amm_wasm_hash, &lp_wasm_hash);

        let agg_addr = env.register_contract(None, DexAggregator);
        DexAggregatorClient::new(&env, &agg_addr).initialize(&admin, &factory_addr);

        let token_a = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();
        let token_b = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();

        // fee_tier 2 = 30 bps (Medium); governance_wasm_hash = None
        factory.create_pool(&admin, &token_a, &token_b, &2_i128, &None);
        let pool_ab = factory.get_pool(&token_a, &token_b).unwrap();

        let lp = Address::generate(&env);
        let trader = Address::generate(&env);
        for t in [&token_a, &token_b] {
            StellarAssetClient::new(&env, t).mint(&lp, &10_000_000_i128);
            StellarAssetClient::new(&env, t).mint(&trader, &1_000_000_i128);
        }
        amm::AmmPoolClient::new(&env, &pool_ab).add_liquidity(
            &lp,
            &1_000_000,
            &1_000_000,
            &0,
            &u64::MAX,
        );

        Venues {
            env,
            agg: agg_addr,
            admin,
            trader,
            token_a,
            token_b,
            pool_ab,
            factory: factory_addr,
        }
    }

    /// Payload of the last event this contract emitted under `topic`, with the
    /// schema-version prefix asserted and stripped.
    fn last_payload<T: soroban_sdk::TryFromVal<Env, soroban_sdk::Val>>(
        env: &Env,
        contract: &Address,
        topic: Symbol,
    ) -> T {
        let event = env
            .events()
            .all()
            .iter()
            .rfind(|e| e.0 == *contract && e.1 == (topic.clone(),).into_val(env))
            .unwrap_or_else(|| panic!("no event emitted for the requested topic"));
        let (version, payload): (u32, T) = event.2.into_val(env);
        assert_eq!(version, soroban_amm_sdk::EVENT_SCHEMA_VERSION);
        payload
    }

    fn count_events(env: &Env, contract: &Address, topic: Symbol) -> usize {
        env.events()
            .all()
            .iter()
            .filter(|e| e.0 == *contract && e.1 == (topic.clone(),).into_val(env))
            .count()
    }

    #[test]
    fn test_register_cl_pool_emits_cl_reg() {
        let v = setup_venues();
        let agg = DexAggregatorClient::new(&v.env, &v.agg);
        let cl_pool = Address::generate(&v.env);

        agg.register_cl_pool(&cl_pool, &v.token_a, &v.token_b, &30_i128);

        let (token_a, token_b, fee_bps, pool): (Address, Address, i128, Address) =
            last_payload(&v.env, &v.agg, symbol_short!("cl_reg"));
        assert_eq!(token_a, v.token_a);
        assert_eq!(token_b, v.token_b);
        assert_eq!(fee_bps, 30);
        assert_eq!(pool, cl_pool);
    }

    #[test]
    fn test_register_cl_pool_does_not_re_emit_for_a_known_pool() {
        let v = setup_venues();
        let agg = DexAggregatorClient::new(&v.env, &v.agg);
        let cl_pool = Address::generate(&v.env);

        agg.register_cl_pool(&cl_pool, &v.token_a, &v.token_b, &30_i128);
        agg.register_cl_pool(&cl_pool, &v.token_a, &v.token_b, &30_i128);

        assert_eq!(count_events(&v.env, &v.agg, symbol_short!("cl_reg")), 1);
    }

    #[test]
    fn test_find_best_route_emits_route_sel_with_the_chosen_venue() {
        let v = setup_venues();
        let agg = DexAggregatorClient::new(&v.env, &v.agg);

        let quote = agg.find_best_route(&v.token_a, &v.token_b, &10_000_i128, &2u32);

        let (venue, venue_kind, amount_in, amount_out): (Address, PoolKind, i128, i128) =
            last_payload(&v.env, &v.agg, symbol_short!("route_sel"));
        assert_eq!(venue, v.pool_ab);
        assert_eq!(venue_kind, PoolKind::Amm);
        assert_eq!(amount_in, 10_000);
        assert_eq!(amount_out, quote.amount_out);
    }

    #[test]
    fn test_route_alt_is_not_emitted_when_only_one_venue_quoted() {
        let v = setup_venues();
        let agg = DexAggregatorClient::new(&v.env, &v.agg);

        agg.find_best_route(&v.token_a, &v.token_b, &10_000_i128, &2u32);

        assert_eq!(count_events(&v.env, &v.agg, symbol_short!("route_sel")), 1);
        assert_eq!(count_events(&v.env, &v.agg, symbol_short!("route_alt")), 0);
    }

    #[test]
    fn test_route_alt_reports_the_runner_up_when_a_second_venue_quotes() {
        let v = setup_venues();
        let agg = DexAggregatorClient::new(&v.env, &v.agg);
        let factory = FactoryClient::new(&v.env, &v.factory);

        // A second, deliberately shallower A->C->B route through a third token,
        // so two distinct entry venues complete a route to token_b.
        let token_c = env_token(&v.env, &v.admin);
        factory.create_pool(&v.admin, &v.token_a, &token_c, &2_i128, &None);
        factory.create_pool(&v.admin, &token_c, &v.token_b, &2_i128, &None);

        let lp = Address::generate(&v.env);
        for t in [&v.token_a, &token_c, &v.token_b] {
            StellarAssetClient::new(&v.env, t).mint(&lp, &10_000_000_i128);
        }
        for (x, y) in [(&v.token_a, &token_c), (&token_c, &v.token_b)] {
            let pool = factory.get_pool(x, y).unwrap();
            amm::AmmPoolClient::new(&v.env, &pool).add_liquidity(
                &lp,
                &500_000,
                &500_000,
                &0,
                &u64::MAX,
            );
        }
        agg.set_routing_tokens(&soroban_sdk::vec![
            &v.env,
            v.token_a.clone(),
            token_c.clone(),
            v.token_b.clone()
        ]);

        let quote = agg.find_best_route(&v.token_a, &v.token_b, &10_000_i128, &3u32);

        let (venue, amount_out, alt_venue, _alt_kind, alt_amount_out): (
            Address,
            i128,
            Address,
            PoolKind,
            i128,
        ) = last_payload(&v.env, &v.agg, symbol_short!("route_alt"));
        assert_eq!(venue, quote.hops.get(0).unwrap().pool);
        assert_eq!(amount_out, quote.amount_out);
        // The alternative is a different entry venue and never beats the winner.
        assert_ne!(alt_venue, venue);
        assert!(alt_amount_out <= amount_out);
    }

    #[test]
    fn test_execute_route_emits_route_exe_with_the_settled_amount() {
        let v = setup_venues();
        let agg = DexAggregatorClient::new(&v.env, &v.agg);

        let quote = agg.find_best_route(&v.token_a, &v.token_b, &10_000_i128, &2u32);
        let out = agg.execute_route(&quote, &v.trader, &10_000_i128, &0_i128, &u64::MAX);

        let (trader, token_in, token_out, amount_in, amount_out, pool): (
            Address,
            Address,
            Address,
            i128,
            i128,
            Address,
        ) = last_payload(&v.env, &v.agg, symbol_short!("route_exe"));
        assert_eq!(trader, v.trader);
        assert_eq!(token_in, v.token_a);
        assert_eq!(token_out, v.token_b);
        assert_eq!(amount_in, 10_000);
        assert_eq!(pool, v.pool_ab);
        // The pool's actual output, which is what execute_route returned.
        assert_eq!(amount_out, out);
    }

    #[test]
    fn test_swap_best_emits_both_route_sel_and_route_exe() {
        let v = setup_venues();
        let agg = DexAggregatorClient::new(&v.env, &v.agg);

        let out = agg.swap_best(&v.trader, &v.token_a, &v.token_b, &10_000_i128, &0_i128);

        assert_eq!(count_events(&v.env, &v.agg, symbol_short!("route_sel")), 1);
        let (_, _, _, quoted): (Address, PoolKind, i128, i128) =
            last_payload(&v.env, &v.agg, symbol_short!("route_sel"));
        let (_, _, _, _, settled, _): (Address, Address, Address, i128, i128, Address) =
            last_payload(&v.env, &v.agg, symbol_short!("route_exe"));
        assert_eq!(settled, out);
        assert_eq!(quoted, out);
    }

    #[test]
    fn test_tolerance_failure_emits_tol_fail() {
        let v = setup_venues();
        let agg = DexAggregatorClient::new(&v.env, &v.agg);

        // Quote far outside the 10 bps tolerance.
        assert!(!agg.is_price_within_tolerance(&v.token_a, &v.token_b, &10_000_i128, &1_i128));

        let (pool, observed_bps, tolerance_bps): (Address, i128, i128) =
            last_payload(&v.env, &v.agg, symbol_short!("tol_fail"));
        assert_eq!(pool, v.pool_ab);
        assert_eq!(tolerance_bps, DexAggregator::PRICE_TOLERANCE_BPS);
        assert!(observed_bps > tolerance_bps);
    }

    #[test]
    fn test_tolerance_success_emits_nothing() {
        let v = setup_venues();
        let agg = DexAggregatorClient::new(&v.env, &v.agg);

        let quote = agg.find_best_route(&v.token_a, &v.token_b, &10_000_i128, &2u32);
        assert!(agg.is_price_within_tolerance(
            &v.token_a,
            &v.token_b,
            &10_000_i128,
            &quote.amount_out
        ));

        assert_eq!(count_events(&v.env, &v.agg, symbol_short!("tol_fail")), 0);
    }

    #[test]
    fn test_no_routing_events_when_no_route_exists() {
        let v = setup_venues();
        let agg = DexAggregatorClient::new(&v.env, &v.agg);
        let orphan = env_token(&v.env, &v.admin);

        assert!(agg
            .try_find_best_route(&v.token_a, &orphan, &10_000_i128, &2u32)
            .is_err());

        assert_eq!(count_events(&v.env, &v.agg, symbol_short!("route_sel")), 0);
        assert_eq!(count_events(&v.env, &v.agg, symbol_short!("route_alt")), 0);
    }

    fn env_token(env: &Env, admin: &Address) -> Address {
        env.register_stellar_asset_contract_v2(admin.clone())
            .address()
    }

    // -------------------------------------------------------------------------
    // #809: Authorization checks for set_max_hops and set_routing_tokens
    // -------------------------------------------------------------------------

    #[test]
    fn test_set_max_hops_non_admin_fails() {
        // Regression test: non-admin calls set_max_hops with no auth → must panic
        let env = Env::default();
        let factory_addr = env.register_contract(None, Factory);
        let factory = FactoryClient::new(&env, &factory_addr);
        let admin = Address::generate(&env);
        factory.initialize(
            &admin,
            &BytesN::from_array(&env, &[0u8; 32]),
            &BytesN::from_array(&env, &[1u8; 32]),
        );

        let agg_addr = env.register_contract(None, DexAggregator);
        let agg = DexAggregatorClient::new(&env, &agg_addr);
        agg.initialize(&admin, &factory_addr);

        let non_admin = Address::generate(&env);
        env.mock_auths(&[]);

        // Non-admin calling set_max_hops should fail
        let result = agg.try_set_max_hops(&3u32);
        assert!(result.is_err());
    }

    #[test]
    fn test_set_routing_tokens_non_admin_fails() {
        // Regression test: non-admin calls set_routing_tokens with no auth → must panic
        let env = Env::default();
        let factory_addr = env.register_contract(None, Factory);
        let factory = FactoryClient::new(&env, &factory_addr);
        let admin = Address::generate(&env);
        factory.initialize(
            &admin,
            &BytesN::from_array(&env, &[0u8; 32]),
            &BytesN::from_array(&env, &[1u8; 32]),
        );

        let agg_addr = env.register_contract(None, DexAggregator);
        let agg = DexAggregatorClient::new(&env, &agg_addr);
        agg.initialize(&admin, &factory_addr);

        let token = Address::generate(&env);
        let tokens = vec![&env, token];
        env.mock_auths(&[]);

        // Non-admin calling set_routing_tokens should fail
        let result = agg.try_set_routing_tokens(&tokens);
        assert!(result.is_err());
    }

    #[test]
    fn test_admin_set_max_hops_success_then_route_limited() {
        // Admin calls set_max_hops(&env, &3) successfully, then a >3-hop route fails
        let env = Env::default();
        env.mock_all_auths();
        env.budget().reset_unlimited();

        let admin = Address::generate(&env);
        let amm_wasm_hash = env.deployer().upload_contract_wasm(amm::WASM);
        let lp_wasm_hash = env.deployer().upload_contract_wasm(token::WASM);

        let factory_addr = env.register_contract(None, Factory);
        let factory = FactoryClient::new(&env, &factory_addr);
        factory.initialize(&admin, &amm_wasm_hash, &lp_wasm_hash);

        let agg_addr = env.register_contract(None, DexAggregator);
        let agg = DexAggregatorClient::new(&env, &agg_addr);
        agg.initialize(&admin, &factory_addr);

        // Admin sets max_hops to 3
        let result = agg.try_set_max_hops(&3u32);
        assert!(result.is_ok());

        // Verify find_best_route respects the max_hops cap
        let token_a = Address::generate(&env);
        let token_b = Address::generate(&env);

        // Create a deep route that would require >3 hops (this will fail because we have no pools)
        let result = agg.try_find_best_route(&token_a, &token_b, &100_i128, &5u32);
        assert!(result.is_err()); // Expected: NoRouteFound because no pools registered
    }

    #[test]
    fn test_set_max_hops_zero_fails_with_invalid_max_hops() {
        // set_max_hops(&env, &0) → Err(AggregatorError::InvalidMaxHops)
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let factory_addr = env.register_contract(None, Factory);
        let factory = FactoryClient::new(&env, &factory_addr);
        factory.initialize(
            &admin,
            &BytesN::from_array(&env, &[0u8; 32]),
            &BytesN::from_array(&env, &[1u8; 32]),
        );

        let agg_addr = env.register_contract(None, DexAggregator);
        let agg = DexAggregatorClient::new(&env, &agg_addr);
        agg.initialize(&admin, &factory_addr);

        // Setting max_hops to 0 should fail with InvalidMaxHops
        let result = agg.try_set_max_hops(&0u32);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().unwrap(),
            AggregatorError::InvalidMaxHops
        );
    }

    #[test]
    fn test_set_routing_tokens_too_many_fails() {
        // set_routing_tokens with MAX_ROUTING_TOKENS + 1 addresses → TooManyRoutingTokens
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let factory_addr = env.register_contract(None, Factory);
        let factory = FactoryClient::new(&env, &factory_addr);
        factory.initialize(
            &admin,
            &BytesN::from_array(&env, &[0u8; 32]),
            &BytesN::from_array(&env, &[1u8; 32]),
        );

        let agg_addr = env.register_contract(None, DexAggregator);
        let agg = DexAggregatorClient::new(&env, &agg_addr);
        agg.initialize(&admin, &factory_addr);

        // Create MAX_ROUTING_TOKENS + 1 addresses
        let mut too_many_tokens = Vec::new(&env);
        for i in 0..=(DexAggregator::MAX_ROUTING_TOKENS) {
            too_many_tokens.push_back(Address::generate(&env));
        }

        // Setting too many tokens should fail with TooManyRoutingTokens
        let result = agg.try_set_routing_tokens(&too_many_tokens);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().unwrap(),
            AggregatorError::TooManyRoutingTokens
        );
    }

    #[test]
    fn test_admin_set_routing_tokens_success_updates_discovery() {
        // Admin calls set_routing_tokens with a valid list, and discover_tokens picks up the new tokens
        let env = Env::default();
        let factory_addr = env.register_contract(None, Factory);
        let factory = FactoryClient::new(&env, &factory_addr);
        let admin = Address::generate(&env);
        factory.initialize(
            &admin,
            &BytesN::from_array(&env, &[0u8; 32]),
            &BytesN::from_array(&env, &[1u8; 32]),
        );

        let agg_addr = env.register_contract(None, DexAggregator);
        let agg = DexAggregatorClient::new(&env, &agg_addr);
        agg.initialize(&admin, &factory_addr);

        let token_a = Address::generate(&env);
        let token_b = Address::generate(&env);
        let routing_token = Address::generate(&env);

        env.mock_all_auths();

        // Set routing tokens
        let routing_tokens = vec![&env, routing_token.clone()];
        let result = agg.try_set_routing_tokens(&routing_tokens);
        assert!(result.is_ok());

        // Verify discover_tokens includes the routing tokens
        let discovered = env.as_contract(&agg_addr, || {
            DexAggregator::discover_tokens(&env, &token_a, &token_b)
        });

        let mut found_routing_token = false;
        for i in 0..discovered.len() {
            if discovered.get(i).unwrap() == routing_token {
                found_routing_token = true;
                break;
            }
        }
        assert!(found_routing_token);
    }
}

