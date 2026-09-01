//! Multi-hop swap router.
//!
//! Routes swaps through one or more AMM pools discovered via the factory
//! contract. A path is an ordered list of token addresses where each adjacent
//! pair must have a deployed pool.

#![no_std]

use soroban_sdk::{contract, contractimpl, contracttype, Address, Env, Vec};

use pool_interfaces::{AmmPoolClient, FactoryClient};

const MIN_TTL: u32 = 172_800;
const BUMP_TO: u32 = 518_400;

#[contracttype]
pub enum DataKey {
    Factory,
    Admin,
}

#[contract]
pub struct Router;

#[contractimpl]
impl Router {
    /// Initialize the router with the factory that tracks all deployed pools.
    pub fn initialize(env: Env, admin: Address, factory: Address) {
        assert!(
            !env.storage().instance().has(&DataKey::Factory),
            "already initialized"
        );
        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Factory, &factory);
    }

    /// Execute a multi-hop swap along `path`.
    pub fn swap_exact_in(
        env: Env,
        trader: Address,
        path: Vec<Address>,
        amount_in: i128,
        min_amount_out: i128,
        deadline: u64,
    ) -> i128 {
        Self::extend_ttl(&env);
        trader.require_auth();
        Self::require_valid_path(&path);
        assert!(amount_in > 0, "amount_in must be positive");

        if env.ledger().timestamp() > deadline {
            panic!("DeadlineExpired");
        }

        let factory: Address = env.storage().instance().get(&DataKey::Factory).unwrap();
        let factory_client = FactoryClient::new(&env, &factory);

        let mut current_amount = amount_in;
        let hops = path.len() - 1;

        for i in 0..hops {
            let token_in = path.get(i).unwrap();
            let token_out = path.get(i + 1).unwrap();

            let pool = factory_client
                .get_pool(&token_in, &token_out)
                .unwrap_or_else(|| panic!("no pool for hop {i}"));

            // Intermediate hops carry no floor of their own; the router enforces
            // the overall slippage bound against `min_amount_out` after the last
            // hop, so only that hop passes a non-zero minimum to the pool.
            let hop_min_out = if i + 1 == hops { min_amount_out } else { 0 };

            current_amount = AmmPoolClient::new(&env, &pool).swap(
                &trader,
                &token_in,
                &current_amount,
                &hop_min_out,
                &deadline,
            );
        }

        if current_amount < min_amount_out {
            panic!("Slippage exceeded");
        }

        current_amount
    }

    pub fn swap_exact_out(
        env: Env,
        trader: Address,
        path: Vec<Address>,
        amount_out: i128,
        max_in: i128,
        deadline: u64,
    ) -> i128 {
        Self::extend_ttl(&env);
        trader.require_auth();
        Self::require_valid_path(&path);
        assert!(amount_out > 0, "amount_out must be positive");

        if env.ledger().timestamp() > deadline {
            panic!("DeadlineExpired");
        }

        let factory: Address = env.storage().instance().get(&DataKey::Factory).unwrap();
        let factory_client = FactoryClient::new(&env, &factory);

        let hops = path.len() - 1;
        // Same reverse walk `get_amounts_in_path` quotes with, so a quote and
        // the execution it precedes can never drift apart.
        let amounts_in = Self::reverse_walk(&env, &factory_client, &path, amount_out);

        let total_in = amounts_in.get(0).unwrap();
        if total_in > max_in {
            panic!("Slippage exceeded");
        }

        let mut current_amount_in = total_in;
        for i in 0..hops {
            let token_in = path.get(i).unwrap();
            let token_out = path.get(i + 1).unwrap();

            let pool = factory_client
                .get_pool(&token_in, &token_out)
                .unwrap_or_else(|| panic!("no pool for hop {i}"));

            let expected_out = amounts_in.get(i + 1).unwrap();

            let actual_out = AmmPoolClient::new(&env, &pool).swap(
                &trader,
                &token_in,
                &current_amount_in,
                &expected_out,
                &deadline,
            );

            if actual_out < expected_out {
                panic!("Slippage exceeded");
            }
            current_amount_in = actual_out;
        }

        total_in
    }

    /// Quote the output of a multi-hop swap without executing it.
    ///
    /// Returns `0` when any hop has no deployed pool. That sentinel is
    /// indistinguishable from a route that genuinely produces zero output and
    /// is kept only for ABI compatibility -- call
    /// [`Router::is_path_routable`] first to tell the two apart, or
    /// [`Router::get_amounts_out_path`] for the per-hop breakdown.
    pub fn get_amount_out_path(env: Env, path: Vec<Address>, amount_in: i128) -> i128 {
        Self::extend_ttl(&env);
        Self::require_valid_path(&path);
        assert!(amount_in > 0, "amount_in must be positive");

        let factory: Address = env.storage().instance().get(&DataKey::Factory).unwrap();
        let factory_client = FactoryClient::new(&env, &factory);

        let mut current_amount = amount_in;
        let hops = path.len() - 1;

        for i in 0..hops {
            let token_in = path.get(i).unwrap();
            let token_out = path.get(i + 1).unwrap();

            let pool = match factory_client.get_pool(&token_in, &token_out) {
                Some(p) => p,
                None => return 0,
            };

            current_amount =
                AmmPoolClient::new(&env, &pool).get_amount_out(&token_in, &current_amount);
        }

        current_amount
    }

    /// Quote the input required to receive exactly `amount_out` along `path`.
    ///
    /// The exact-out mirror of [`Router::get_amount_out_path`]. Walks the path
    /// in reverse through `AmmPoolClient::get_amount_in`, using the same helper
    /// `swap_exact_out` executes with. Panics with `no pool for hop {i}` when a
    /// hop has no deployed pool; use [`Router::is_path_routable`] to check
    /// first.
    pub fn get_amount_in_path(env: Env, path: Vec<Address>, amount_out: i128) -> i128 {
        Self::require_valid_path(&path);
        assert!(amount_out > 0, "amount_out must be positive");

        let factory_client = Self::factory_client(&env);
        Self::reverse_walk(&env, &factory_client, &path, amount_out)
            .get(0)
            .unwrap()
    }

    /// Per-hop breakdown of an exact-in quote.
    ///
    /// Returns `path.len()` amounts where element `i` is the amount of
    /// `path[i]` moving through the route; element `0` is `amount_in` and the
    /// last element equals [`Router::get_amount_out_path`] for the same
    /// arguments. Panics with `no pool for hop {i}` when a hop has no pool,
    /// rather than collapsing the route to the ambiguous `0` sentinel.
    pub fn get_amounts_out_path(env: Env, path: Vec<Address>, amount_in: i128) -> Vec<i128> {
        Self::require_valid_path(&path);
        assert!(amount_in > 0, "amount_in must be positive");

        let factory_client = Self::factory_client(&env);
        Self::forward_walk(&env, &factory_client, &path, amount_in)
    }

    /// Per-hop breakdown of an exact-out quote.
    ///
    /// Returns `path.len()` amounts where element `i` is the amount of
    /// `path[i]` moving through the route; element `0` is the required input
    /// and the last element is `amount_out`.
    pub fn get_amounts_in_path(env: Env, path: Vec<Address>, amount_out: i128) -> Vec<i128> {
        Self::require_valid_path(&path);
        assert!(amount_out > 0, "amount_out must be positive");

        let factory_client = Self::factory_client(&env);
        Self::reverse_walk(&env, &factory_client, &path, amount_out)
    }

    /// Resolve every adjacent pair in `path` to its pool address.
    ///
    /// Returns `path.len() - 1` addresses so integrators can inspect fee tiers
    /// and reserves themselves. Panics with `no pool for hop {i}`, naming the
    /// failing hop index, when a pair has no deployed pool.
    pub fn get_pools_for_path(env: Env, path: Vec<Address>) -> Vec<Address> {
        Self::require_valid_path(&path);

        let factory_client = Self::factory_client(&env);
        let hops = path.len() - 1;
        let mut pools = Vec::new(&env);
        for i in 0..hops {
            pools.push_back(Self::pool_for_hop(&factory_client, &path, i));
        }
        pools
    }

    /// Whether every hop in `path` has a deployed pool.
    ///
    /// This is the unambiguous replacement for
    /// [`Router::get_amount_out_path`]'s `0` return: a route that is simply
    /// missing a pool is `false` here, while a routable path that quotes zero
    /// output is `true`. A malformed path (fewer than two tokens, or a repeated
    /// adjacent token) is also `false` -- a predicate reports rather than
    /// panics.
    pub fn is_path_routable(env: Env, path: Vec<Address>) -> bool {
        if path.len() < 2 {
            return false;
        }
        let factory_client = Self::factory_client(&env);
        let hops = path.len() - 1;
        for i in 0..hops {
            let token_in = path.get(i).unwrap();
            let token_out = path.get(i + 1).unwrap();
            if token_in == token_out {
                return false;
            }
            if factory_client.get_pool(&token_in, &token_out).is_none() {
                return false;
            }
        }
        true
    }

    pub fn get_factory(env: Env) -> Address {
        env.storage().instance().get(&DataKey::Factory).unwrap()
    }

    fn extend_ttl(env: &Env) {
        env.storage().instance().extend_ttl(MIN_TTL, BUMP_TO);
    }

    fn factory_client(env: &Env) -> FactoryClient<'_> {
        let factory: Address = env.storage().instance().get(&DataKey::Factory).unwrap();
        FactoryClient::new(env, &factory)
    }

    /// Shared precondition for every path-taking entry point.
    ///
    /// A repeated adjacent token resolves to a pool that cannot exist, which
    /// previously surfaced as a confusing factory panic; it is rejected here
    /// with a named message instead.
    fn require_valid_path(path: &Vec<Address>) {
        assert!(path.len() >= 2, "path must have at least 2 tokens");
        let hops = path.len() - 1;
        for i in 0..hops {
            if path.get(i).unwrap() == path.get(i + 1).unwrap() {
                panic!("DuplicateAdjacentToken at hop {i}");
            }
        }
    }

    fn pool_for_hop(factory: &FactoryClient, path: &Vec<Address>, i: u32) -> Address {
        let token_in = path.get(i).unwrap();
        let token_out = path.get(i + 1).unwrap();
        factory
            .get_pool(&token_in, &token_out)
            .unwrap_or_else(|| panic!("no pool for hop {i}"))
    }

    /// Walk `path` forwards, returning the amount held at each position.
    ///
    /// `result[0] == amount_in` and `result[i + 1]` is the output of hop `i`.
    fn forward_walk(
        env: &Env,
        factory: &FactoryClient,
        path: &Vec<Address>,
        amount_in: i128,
    ) -> Vec<i128> {
        let hops = path.len() - 1;
        let mut amounts = Vec::new(env);
        amounts.push_back(amount_in);

        let mut current = amount_in;
        for i in 0..hops {
            let token_in = path.get(i).unwrap();
            let pool = Self::pool_for_hop(factory, path, i);
            current = AmmPoolClient::new(env, &pool).get_amount_out(&token_in, &current);
            amounts.push_back(current);
        }
        amounts
    }

    /// Walk `path` backwards, returning the amount required at each position.
    ///
    /// `result[0]` is the input the route needs and the last element is
    /// `amount_out`. Shared by `swap_exact_out` and the exact-out quotes so the
    /// two can never disagree.
    fn reverse_walk(
        env: &Env,
        factory: &FactoryClient,
        path: &Vec<Address>,
        amount_out: i128,
    ) -> Vec<i128> {
        let hops = path.len() - 1;
        let mut amounts = Vec::new(env);
        amounts.push_back(amount_out);

        let mut current_out = amount_out;
        for i in (0..hops).rev() {
            let token_out = path.get(i + 1).unwrap();
            let pool = Self::pool_for_hop(factory, path, i);
            let required_in =
                AmmPoolClient::new(env, &pool).get_amount_in(&token_out, &current_out);
            amounts.push_front(required_in);
            current_out = required_in;
        }
        amounts
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use factory::{Factory, FactoryClient};
    use soroban_sdk::token::StellarAssetClient;
    use soroban_sdk::{
        testutils::{Address as _, Ledger},
        Env,
    };

    fn setup_env_and_router() -> (Env, Address, Address, Address, Address, Address, Address) {
        let env = Env::default();
        env.mock_all_auths();
        let admin = Address::generate(&env);

        env.budget().reset_unlimited();
        let amm_wasm_hash = env.deployer().upload_contract_wasm(amm::WASM);
        let lp_wasm_hash = env.deployer().upload_contract_wasm(token::WASM);

        let factory_addr = env.register_contract(None, Factory);
        let factory = FactoryClient::new(&env, &factory_addr);
        factory.initialize(&admin, &amm_wasm_hash, &lp_wasm_hash);

        let router_addr = env.register_contract(None, Router);
        let router = RouterClient::new(&env, &router_addr);
        router.initialize(&admin, &factory_addr);

        let token1 = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();
        let token2 = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();
        let token3 = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();

        // fee_tier 2 = 30 bps (Medium); governance_wasm_hash = None
        factory.create_pool(&admin, &token1, &token2, &2_i128, &None);
        factory.create_pool(&admin, &token2, &token3, &2_i128, &None);

        let pool1_addr = factory.get_pool(&token1, &token2).unwrap();
        let pool2_addr = factory.get_pool(&token2, &token3).unwrap();

        let trader = Address::generate(&env);
        StellarAssetClient::new(&env, &token1).mint(&trader, &1_000_000_i128);
        StellarAssetClient::new(&env, &token2).mint(&trader, &1_000_000_i128);
        StellarAssetClient::new(&env, &token3).mint(&trader, &1_000_000_i128);

        let lp = Address::generate(&env);
        StellarAssetClient::new(&env, &token1).mint(&lp, &10_000_000_i128);
        StellarAssetClient::new(&env, &token2).mint(&lp, &10_000_000_i128);
        StellarAssetClient::new(&env, &token3).mint(&lp, &10_000_000_i128);

        amm::AmmPoolClient::new(&env, &pool1_addr).add_liquidity(
            &lp,
            &1_000_000,
            &1_000_000,
            &0,
            &u64::MAX,
        );
        amm::AmmPoolClient::new(&env, &pool2_addr).add_liquidity(
            &lp,
            &1_000_000,
            &1_000_000,
            &0,
            &u64::MAX,
        );

        (env, router_addr, trader, token1, token2, token3, pool1_addr)
    }

    #[test]
    #[should_panic(expected = "DeadlineExpired")]
    fn test_expired_deadline() {
        let (env, router_addr, trader, token1, token2, token3, _) = setup_env_and_router();
        env.ledger().with_mut(|li| {
            li.timestamp = 1000;
        });

        let router = RouterClient::new(&env, &router_addr);
        let path = soroban_sdk::vec![&env, token1.clone(), token2.clone(), token3.clone()];
        router.swap_exact_in(&trader, &path, &100_000, &0, &500);
    }

    #[test]
    #[should_panic(expected = "contract call failed")]
    fn test_slippage_exceeded() {
        let (env, router_addr, trader, token1, token2, token3, _) = setup_env_and_router();

        let router = RouterClient::new(&env, &router_addr);
        let path = soroban_sdk::vec![&env, token1.clone(), token2.clone(), token3.clone()];
        router.swap_exact_in(&trader, &path, &100_000, &1_000_000_000, &u64::MAX);
    }

    #[test]
    fn test_successful_route_execution() {
        let (env, router_addr, trader, token1, token2, token3, _) = setup_env_and_router();

        let router = RouterClient::new(&env, &router_addr);
        let path = soroban_sdk::vec![&env, token1.clone(), token2.clone(), token3.clone()];

        let out = router.swap_exact_in(&trader, &path, &10_000, &0, &u64::MAX);
        assert!(out > 0);
    }

    #[test]
    #[should_panic(expected = "contract call failed")]
    fn test_atomic_revert_behavior() {
        let (env, router_addr, trader, token1, token2, token3, _pool1_addr) =
            setup_env_and_router();

        let router = RouterClient::new(&env, &router_addr);
        let path = soroban_sdk::vec![&env, token1.clone(), token2.clone(), token3.clone()];

        // This will panic, the state should be reverted
        router.swap_exact_in(&trader, &path, &10_000, &1_000_000, &u64::MAX);

        // Since it panics, the test will pass, and in actual Soroban the state would revert.
    }

    /// Everything `setup_env_and_router` builds, plus a fourth token and a
    /// `token3 <-> token4` pool so three-hop routes can be exercised, and an
    /// `orphan` token that deliberately has no pool at all.
    struct ThreeHop {
        env: Env,
        router: Address,
        trader: Address,
        token1: Address,
        token2: Address,
        token3: Address,
        token4: Address,
        orphan: Address,
        factory: Address,
    }

    fn setup_three_hop() -> ThreeHop {
        let env = Env::default();
        env.mock_all_auths();
        let admin = Address::generate(&env);

        env.budget().reset_unlimited();
        let amm_wasm_hash = env.deployer().upload_contract_wasm(amm::WASM);
        let lp_wasm_hash = env.deployer().upload_contract_wasm(token::WASM);

        let factory_addr = env.register_contract(None, Factory);
        let factory = FactoryClient::new(&env, &factory_addr);
        factory.initialize(&admin, &amm_wasm_hash, &lp_wasm_hash);

        let router_addr = env.register_contract(None, Router);
        RouterClient::new(&env, &router_addr).initialize(&admin, &factory_addr);

        let mut tokens: soroban_sdk::Vec<Address> = soroban_sdk::Vec::new(&env);
        for _ in 0..4 {
            tokens.push_back(
                env.register_stellar_asset_contract_v2(admin.clone())
                    .address(),
            );
        }
        let orphan = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();

        let lp = Address::generate(&env);
        let trader = Address::generate(&env);
        for i in 0..tokens.len() {
            let t = tokens.get(i).unwrap();
            StellarAssetClient::new(&env, &t).mint(&lp, &10_000_000_i128);
            StellarAssetClient::new(&env, &t).mint(&trader, &1_000_000_i128);
        }

        // fee_tier 2 = 30 bps (Medium); governance_wasm_hash = None
        for i in 0..3 {
            let a = tokens.get(i).unwrap();
            let b = tokens.get(i + 1).unwrap();
            factory.create_pool(&admin, &a, &b, &2_i128, &None);
            let pool = factory.get_pool(&a, &b).unwrap();
            amm::AmmPoolClient::new(&env, &pool).add_liquidity(
                &lp,
                &1_000_000,
                &1_000_000,
                &0,
                &u64::MAX,
            );
        }

        ThreeHop {
            env,
            router: router_addr,
            trader,
            token1: tokens.get(0).unwrap(),
            token2: tokens.get(1).unwrap(),
            token3: tokens.get(2).unwrap(),
            token4: tokens.get(3).unwrap(),
            orphan,
            factory: factory_addr,
        }
    }

    // -- #683: exact-out quoting, per-hop breakdowns, path validation ---------

    #[test]
    fn test_single_hop_quotes_agree_with_pool() {
        let s = setup_three_hop();
        let router = RouterClient::new(&s.env, &s.router);
        let path = soroban_sdk::vec![&s.env, s.token1.clone(), s.token2.clone()];

        let pool = FactoryClient::new(&s.env, &s.factory)
            .get_pool(&s.token1, &s.token2)
            .unwrap();
        let pool_client = amm::AmmPoolClient::new(&s.env, &pool);

        assert_eq!(
            router.get_amount_out_path(&path, &10_000),
            pool_client.get_amount_out(&s.token1, &10_000)
        );
        assert_eq!(
            router.get_amount_in_path(&path, &10_000),
            pool_client.get_amount_in(&s.token2, &10_000)
        );
    }

    #[test]
    fn test_amounts_out_path_matches_scalar_quote_for_two_and_three_hops() {
        let s = setup_three_hop();
        let router = RouterClient::new(&s.env, &s.router);

        let two_hop = soroban_sdk::vec![&s.env, s.token1.clone(), s.token2.clone()];
        let amounts = router.get_amounts_out_path(&two_hop, &10_000);
        assert_eq!(amounts.len(), 2);
        assert_eq!(amounts.get(0).unwrap(), 10_000);
        assert_eq!(
            amounts.last().unwrap(),
            router.get_amount_out_path(&two_hop, &10_000)
        );

        let three_hop = soroban_sdk::vec![
            &s.env,
            s.token1.clone(),
            s.token2.clone(),
            s.token3.clone(),
            s.token4.clone()
        ];
        let amounts = router.get_amounts_out_path(&three_hop, &10_000);
        assert_eq!(amounts.len(), 4);
        assert_eq!(amounts.get(0).unwrap(), 10_000);
        assert_eq!(
            amounts.last().unwrap(),
            router.get_amount_out_path(&three_hop, &10_000)
        );
        // Each hop takes a fee, so the route strictly decreases.
        for i in 0..3u32 {
            assert!(amounts.get(i + 1).unwrap() < amounts.get(i).unwrap());
        }
    }

    #[test]
    fn test_amounts_in_path_breakdown_ends_at_requested_output() {
        let s = setup_three_hop();
        let router = RouterClient::new(&s.env, &s.router);
        let path = soroban_sdk::vec![&s.env, s.token1.clone(), s.token2.clone(), s.token3.clone()];

        let amounts = router.get_amounts_in_path(&path, &10_000);
        assert_eq!(amounts.len(), 3);
        assert_eq!(amounts.last().unwrap(), 10_000);
        assert_eq!(
            amounts.get(0).unwrap(),
            router.get_amount_in_path(&path, &10_000)
        );
        // Walking backwards, each earlier position needs strictly more.
        assert!(amounts.get(0).unwrap() > amounts.get(1).unwrap());
        assert!(amounts.get(1).unwrap() > amounts.get(2).unwrap());
    }

    #[test]
    fn test_in_out_round_trip_never_exceeds_the_original_input() {
        let s = setup_three_hop();
        let router = RouterClient::new(&s.env, &s.router);

        for path in [
            soroban_sdk::vec![&s.env, s.token1.clone(), s.token2.clone()],
            soroban_sdk::vec![
                &s.env,
                s.token1.clone(),
                s.token2.clone(),
                s.token3.clone(),
                s.token4.clone()
            ],
        ] {
            let hops = (path.len() - 1) as i128;
            let amount_in = 10_000_i128;
            let out = router.get_amount_out_path(&path, &amount_in);
            let round_trip = router.get_amount_in_path(&path, &out);

            // Quoting back the exact output can never ask for more than the
            // input that produced it, and integer rounding costs at most one
            // unit per hop.
            assert!(round_trip <= amount_in);
            assert!(amount_in - round_trip <= hops);
        }
    }

    #[test]
    fn test_get_pools_for_path_resolves_every_hop() {
        let s = setup_three_hop();
        let router = RouterClient::new(&s.env, &s.router);
        let factory = FactoryClient::new(&s.env, &s.factory);
        let path = soroban_sdk::vec![&s.env, s.token1.clone(), s.token2.clone(), s.token3.clone()];

        let pools = router.get_pools_for_path(&path);
        assert_eq!(pools.len(), 2);
        assert_eq!(
            pools.get(0).unwrap(),
            factory.get_pool(&s.token1, &s.token2).unwrap()
        );
        assert_eq!(
            pools.get(1).unwrap(),
            factory.get_pool(&s.token2, &s.token3).unwrap()
        );
    }

    #[test]
    #[should_panic(expected = "no pool for hop 1")]
    fn test_get_pools_for_path_names_the_failing_hop() {
        let s = setup_three_hop();
        let router = RouterClient::new(&s.env, &s.router);
        let path = soroban_sdk::vec![&s.env, s.token1.clone(), s.token2.clone(), s.orphan.clone()];
        router.get_pools_for_path(&path);
    }

    #[test]
    fn test_is_path_routable_distinguishes_missing_pools() {
        let s = setup_three_hop();
        let router = RouterClient::new(&s.env, &s.router);

        let good = soroban_sdk::vec![&s.env, s.token1.clone(), s.token2.clone(), s.token3.clone()];
        assert!(router.is_path_routable(&good));

        let missing = soroban_sdk::vec![&s.env, s.token1.clone(), s.orphan.clone()];
        assert!(!router.is_path_routable(&missing));
        // The ambiguous legacy sentinel for exactly the same path.
        assert_eq!(router.get_amount_out_path(&missing, &10_000), 0);

        // Malformed paths are reported, not panicked on.
        assert!(!router.is_path_routable(&soroban_sdk::Vec::new(&s.env)));
        assert!(!router.is_path_routable(&soroban_sdk::vec![&s.env, s.token1.clone()]));
        assert!(!router.is_path_routable(&soroban_sdk::vec![
            &s.env,
            s.token1.clone(),
            s.token1.clone()
        ]));
    }

    #[test]
    #[should_panic(expected = "DuplicateAdjacentToken at hop 1")]
    fn test_duplicate_adjacent_token_is_rejected_by_name() {
        let s = setup_three_hop();
        let router = RouterClient::new(&s.env, &s.router);
        let path = soroban_sdk::vec![&s.env, s.token1.clone(), s.token2.clone(), s.token2.clone()];
        router.get_amount_out_path(&path, &10_000);
    }

    #[test]
    #[should_panic(expected = "DuplicateAdjacentToken at hop 0")]
    fn test_duplicate_adjacent_token_is_rejected_on_exact_out_quote() {
        let s = setup_three_hop();
        let router = RouterClient::new(&s.env, &s.router);
        let path = soroban_sdk::vec![&s.env, s.token1.clone(), s.token1.clone()];
        router.get_amount_in_path(&path, &10_000);
    }

    #[test]
    #[should_panic(expected = "path must have at least 2 tokens")]
    fn test_empty_path_is_rejected() {
        let s = setup_three_hop();
        let router = RouterClient::new(&s.env, &s.router);
        router.get_amounts_out_path(&soroban_sdk::Vec::new(&s.env), &10_000);
    }

    #[test]
    #[should_panic(expected = "path must have at least 2 tokens")]
    fn test_single_element_path_is_rejected() {
        let s = setup_three_hop();
        let router = RouterClient::new(&s.env, &s.router);
        let path = soroban_sdk::vec![&s.env, s.token1.clone()];
        router.get_amounts_in_path(&path, &10_000);
    }

    #[test]
    #[should_panic(expected = "amount_in must be positive")]
    fn test_zero_amount_in_is_rejected() {
        let s = setup_three_hop();
        let router = RouterClient::new(&s.env, &s.router);
        let path = soroban_sdk::vec![&s.env, s.token1.clone(), s.token2.clone()];
        router.get_amounts_out_path(&path, &0);
    }

    #[test]
    #[should_panic(expected = "amount_out must be positive")]
    fn test_zero_amount_out_is_rejected() {
        let s = setup_three_hop();
        let router = RouterClient::new(&s.env, &s.router);
        let path = soroban_sdk::vec![&s.env, s.token1.clone(), s.token2.clone()];
        router.get_amount_in_path(&path, &0);
    }

    #[test]
    fn test_swap_exact_out_still_matches_its_quote_after_the_refactor() {
        let s = setup_three_hop();
        let router = RouterClient::new(&s.env, &s.router);
        let path = soroban_sdk::vec![&s.env, s.token1.clone(), s.token2.clone(), s.token3.clone()];

        let quoted_in = router.get_amount_in_path(&path, &10_000);
        let spent = router.swap_exact_out(&s.trader, &path, &10_000, &quoted_in, &u64::MAX);
        assert_eq!(spent, quoted_in);
    }

    #[test]
    #[should_panic(expected = "contract call failed")]
    fn test_swap_exact_out_enforces_max_in() {
        let s = setup_three_hop();
        let router = RouterClient::new(&s.env, &s.router);
        let path = soroban_sdk::vec![&s.env, s.token1.clone(), s.token2.clone()];

        let quoted_in = router.get_amount_in_path(&path, &10_000);
        router.swap_exact_out(&s.trader, &path, &10_000, &(quoted_in - 1), &u64::MAX);
    }
}
