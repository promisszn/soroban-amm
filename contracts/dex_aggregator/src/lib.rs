#![no_std]

//! DEX aggregator — routes trades across multiple AMM and CL pools for best execution.

use soroban_sdk::{
    contract, contractclient, contracterror, contractimpl, contracttype, vec, Address, Env, Vec,
};

use amm::{AmmPoolClient, PoolInfo};
use factory::FactoryClient;

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
    Factory,
    MaxHops,
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
    pub const CL_FEE_TIERS: [i128; 3] = [30, 100, 500];

    pub fn initialize(env: Env, factory: Address) {
        assert!(
            !env.storage().instance().has(&DataKey::Factory),
            "already initialized"
        );
        env.storage().instance().set(&DataKey::Factory, &factory);
        env.storage()
            .instance()
            .set(&DataKey::MaxHops, &Self::DEFAULT_MAX_HOPS);
        env.storage()
            .instance()
            .set(&DataKey::ClPoolCount, &0u32);
    }

    pub fn set_max_hops(env: Env, max_hops: u32) {
        Self::extend_ttl(&env);
        env.storage()
            .instance()
            .set(&DataKey::MaxHops, &max_hops);
    }

    pub fn register_cl_pool(
        env: Env,
        pool: Address,
        token_a: Address,
        token_b: Address,
        fee_bps: i128,
    ) {
        Self::extend_ttl(&env);
        let count: u32 = env
            .storage()
            .instance()
            .get(&DataKey::ClPoolCount)
            .unwrap_or(0);

        for i in 0..count {
            let entry: ClPoolInfo = env
                .storage()
                .instance()
                .get(&DataKey::ClPool(i))
                .unwrap();
            if entry.pool == pool {
                return;
            }
        }

        assert!(
            count < Self::MAX_CL_POOLS,
            "max CL pools reached"
        );

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
        Self::search_best_bfs(&env, &token_in, &token_out, amount_in, cap)
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
        Self::execute_hops(&env, &route.hops, &trader, amount_in, min_out, deadline)
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
        let Ok(best) = Self::find_best_route(env, token_in, token_out, amount_in, max_hops) else {
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
        diff * Self::BPS / best.amount_out <= Self::PRICE_TOLERANCE_BPS
    }

    fn search_best_bfs(
        env: &Env,
        token_in: &Address,
        token_out: &Address,
        amount_in: i128,
        max_hops: u32,
    ) -> Result<RouteQuote, AggregatorError> {
        let factory: Address = env.storage().instance().get(&DataKey::Factory).unwrap();
        let factory_client = FactoryClient::new(env, &factory);
        let tokens = Self::discover_tokens(env, &factory_client);

        let mut best_out: i128 = 0;
        let mut best_hops: Vec<RouteHop> = Vec::new(env);

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
                        best_out = step.amount_out;
                        best_hops = new_hops;
                    }
                } else if depth + 1 < max_hops
                    && !Self::is_visited_and_worse(&mut visited, &next_token, depth + 1, step.amount_out)
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
        Ok(RouteQuote {
            amount_out: best_out,
            hops: best_hops,
        })
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
                PoolKind::Cl => ClPoolClient::new(env, &hop.pool).swap(
                    trader,
                    &hop.zero_for_one,
                    &current,
                    &0u128,
                    &hop_min,
                    &deadline,
                ),
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

        let cl_count: u32 = env.storage().instance().get(&DataKey::ClPoolCount).unwrap_or(0);
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
            let est = client.estimate_price_impact(&direction, &amount_in, &0u128);
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

    fn discover_tokens(env: &Env, factory: &FactoryClient) -> Vec<Address> {
        let pools = factory.all_pools();
        let mut tokens: Vec<Address> = Vec::new(env);
        for i in 0..pools.len() {
            let pool = pools.get(i).unwrap();
            let info: PoolInfo = AmmPoolClient::new(env, &pool).get_info();
            Self::push_unique(&mut tokens, info.token_a);
            Self::push_unique(&mut tokens, info.token_b);
        }

        let cl_count: u32 = env.storage().instance().get(&DataKey::ClPoolCount).unwrap_or(0);
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
    use soroban_sdk::{testutils::Address as _, Address, BytesN};

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
        assert!(DexAggregator::is_visited_and_worse(&mut visited, &token_a, 1, 100));
        assert!(DexAggregator::is_visited_and_worse(&mut visited, &token_a, 1, 50));
        // Same (token, depth) pair with strictly better amount is allowed.
        assert!(!DexAggregator::is_visited_and_worse(&mut visited, &token_a, 1, 150));
        
        // After being allowed, the new best amount is recorded (150).
        assert!(DexAggregator::is_visited_and_worse(&mut visited, &token_a, 1, 150));
        assert!(DexAggregator::is_visited_and_worse(&mut visited, &token_a, 1, 140));

        // Same token at a different depth must still be explorable.
        assert!(!DexAggregator::is_visited_and_worse(&mut visited, &token_a, 2, 50));
        // A different token at the same depth is independent.
        assert!(!DexAggregator::is_visited_and_worse(&mut visited, &token_b, 1, 50));
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
        agg.initialize(&factory_addr);

        let token_a = Address::generate(&env);
        let token_b = Address::generate(&env);
        let cl_pool = Address::generate(&env);
        agg.register_cl_pool(&cl_pool, &token_a, &token_b, &30_i128);

        let tokens = DexAggregator::discover_tokens(&env, &factory);
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
}
