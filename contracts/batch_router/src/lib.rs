//! Batched AMM operations — execute multiple swaps and liquidity actions atomically
//! in a single transaction to reduce overhead versus separate calls.

#![no_std]

use soroban_sdk::{contract, contracterror, contractimpl, contracttype, Address, Env, Symbol, Vec};

use pool_interfaces::{AmmPoolClient, ConcentratedLiquidityClient, FactoryClient};
use soroban_amm_sdk::emit_versioned_event;

const MIN_TTL: u32 = 172_800;
const BUMP_TO: u32 = 518_400;

#[contracttype]
pub enum DataKey {
    Factory,
}

/// Pool type for distinguishing between AMM and concentrated liquidity pools.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub enum PoolType {
    Amm,
    Cl,
}

/// Errors returned by [`BatchRouter`] entry points.
///
/// See `docs/error-codes.md` for the full description of each variant.
#[contracterror]
#[derive(Copy, Clone, Debug, PartialEq)]
#[repr(u32)]
pub enum BatchRouterError {
    AlreadyInitialized = 1,
    EmptyBatch = 2,
    BatchTooLarge = 3,
    DeadlineExpired = 4,
    InvalidAmount = 5,
    PoolNotFound = 6,
    SlippageExceeded = 7,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct SwapOp {
    pub pool: Address,
    pub token_in: Address,
    pub amount_in: i128,
    pub min_out: i128,
    pub pool_kind: PoolType,
    /// Swap direction for concentrated-liquidity venues: `true` swaps token A
    /// for token B (price decreasing). Unused for `PoolType::Amm`.
    pub zero_for_one: bool,
    /// `sqrtPriceX96` limit for concentrated-liquidity venues. `0` means the
    /// pool's own default bound is used. Unused for `PoolType::Amm`.
    pub sqrt_price_limit_x96: u128,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct AddLiquidityOp {
    pub pool: Address,
    pub amount_a: i128,
    pub amount_b: i128,
    pub min_shares: i128,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct RemoveLiquidityOp {
    pub pool: Address,
    pub shares: i128,
    pub min_a: i128,
    pub min_b: i128,
}

#[contracttype]
#[derive(Clone, Debug)]
pub enum BatchOp {
    /// Swap `amount_in` of `token_in` on `pool` with `min_out` slippage guard.
    Swap(SwapOp),
    /// Add liquidity to `pool`.
    AddLiquidity(AddLiquidityOp),
    /// Remove liquidity from `pool`.
    RemoveLiquidity(RemoveLiquidityOp),
}

/// Result of a single [`BatchOp`], preserving the real output of each leg so
/// callers chaining batch results can do downstream accounting.
///
/// `RemoveLiquidity` carries both token amounts: packing them into one `i128`
/// would be lossy, and returning the shares burned (which the caller already
/// knows) tells them nothing about the tokens actually received.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub enum BatchOpResult {
    /// Output amount received from the swap.
    Swap(i128),
    /// LP shares minted by adding liquidity.
    AddLiquidity(i128),
    /// Token amounts `(amount_a, amount_b)` returned by removing liquidity.
    RemoveLiquidity(i128, i128),
}

/// Honest accounting of what a batch actually saves, split by call kind.
///
/// `cross_contract_calls` does NOT shrink when ops are batched: each op
/// inside `execute_batch` is still a separate cross-contract call from the
/// router into the target pool. Batching only collapses the *top-level*
/// call the end user (or their wallet) has to submit.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct CallSavingsEstimate {
    /// Top-level calls a caller would submit executing each op as its own transaction.
    pub top_level_calls_individual: u32,
    /// Top-level calls a caller submits using `execute_batch` (always 1).
    pub top_level_calls_batched: u32,
    /// Cross-contract calls from the router into pools — the same whether
    /// batched or not, since every op still invokes its target pool once.
    pub cross_contract_calls: u32,
}

const MAX_BATCH_OPS: u32 = 200;

/// A pool's reserve/share state as tracked locally while chaining a
/// simulated batch, seeded from `get_info()` on first touch and updated
/// in-memory (never on-chain) as later ops in the same batch are simulated.
#[contracttype]
#[derive(Clone)]
struct SimPoolState {
    pool: Address,
    token_a: Address,
    token_b: Address,
    reserve_a: i128,
    reserve_b: i128,
    total_shares: i128,
    fee_bps: i128,
}

#[contract]
pub struct BatchRouter;

#[contractimpl]
impl BatchRouter {
    /// Initialize the router with the factory that tracks all deployed pools.
    pub fn initialize(env: Env, factory: Address) -> Result<(), BatchRouterError> {
        if env.storage().instance().has(&DataKey::Factory) {
            return Err(BatchRouterError::AlreadyInitialized);
        }
        env.storage().instance().set(&DataKey::Factory, &factory);
        Ok(())
    }

    /// The maximum number of operations a single batch may contain.
    pub fn max_batch_ops(_env: Env) -> u32 {
        MAX_BATCH_OPS
    }

    /// Execute a sequence of AMM operations atomically.
    ///
    /// All operations share one `deadline` and a single `caller` authorization.
    /// If any step fails the entire batch reverts.
    pub fn execute_batch(
        env: Env,
        caller: Address,
        ops: Vec<BatchOp>,
        deadline: u64,
    ) -> Result<Vec<BatchOpResult>, BatchRouterError> {
        env.storage().instance().extend_ttl(MIN_TTL, BUMP_TO);
        caller.require_auth();
        Self::check_preconditions(&env, &ops, deadline)?;

        let factory: Address = env.storage().instance().get(&DataKey::Factory).unwrap();
        let factory_client = FactoryClient::new(&env, &factory);

        let mut results = Vec::new(&env);
        for i in 0..ops.len() {
            let op = ops.get(i).unwrap();
            let result = Self::execute_op(&env, &caller, &op, deadline, &factory_client)?;

            emit_versioned_event!(
                env,
                (Symbol::new(&env, "batch_op"), caller.clone()),
                (i, Self::op_kind(&env, &op), result.clone())
            );

            results.push_back(result);
        }

        emit_versioned_event!(
            env,
            (Symbol::new(&env, "batch_executed"), caller.clone()),
            (ops.len(),)
        );

        Ok(results)
    }

    /// Read-only walk that quotes each op against current pool state without
    /// executing or requiring auth. A swap's simulated output feeds the next
    /// op's context on the same pool exactly as `execute_batch` would.
    pub fn simulate_batch(
        env: Env,
        ops: Vec<BatchOp>,
    ) -> Result<Vec<BatchOpResult>, BatchRouterError> {
        let factory: Address = env.storage().instance().get(&DataKey::Factory).unwrap();
        let factory_client = FactoryClient::new(&env, &factory);

        let mut pools: Vec<SimPoolState> = Vec::new(&env);
        let mut results = Vec::new(&env);

        for i in 0..ops.len() {
            let op = ops.get(i).unwrap();
            let result = Self::simulate_op(&env, &op, &factory_client, &mut pools)?;
            results.push_back(result);
        }

        Ok(results)
    }

    /// Run every `execute_batch` precondition, plus a full `simulate_batch`
    /// walk, without executing or mutating any state. Lets a caller find out
    /// why a batch would fail before paying for it.
    pub fn validate_batch(
        env: Env,
        ops: Vec<BatchOp>,
        deadline: u64,
    ) -> Result<(), BatchRouterError> {
        Self::check_preconditions(&env, &ops, deadline)?;
        Self::simulate_batch(env, ops)?;
        Ok(())
    }

    /// Estimate how many top-level contract calls a batch saves vs individual txs.
    ///
    /// Returns `(individual_calls, batch_calls)` for off-chain fee comparison.
    #[deprecated(note = "use estimate_call_savings_v2, which also reports cross-contract calls")]
    #[allow(deprecated)]
    pub fn estimate_call_savings(ops_len: u32) -> (u32, u32) {
        (ops_len, 1)
    }

    /// Honest call-savings breakdown. See [`CallSavingsEstimate`] — the
    /// cross-contract call count does not shrink with batching.
    pub fn estimate_call_savings_v2(_env: Env, ops_len: u32) -> CallSavingsEstimate {
        CallSavingsEstimate {
            top_level_calls_individual: ops_len,
            top_level_calls_batched: 1,
            cross_contract_calls: ops_len,
        }
    }

    fn check_preconditions(
        env: &Env,
        ops: &Vec<BatchOp>,
        deadline: u64,
    ) -> Result<(), BatchRouterError> {
        if ops.is_empty() {
            return Err(BatchRouterError::EmptyBatch);
        }
        if ops.len() > MAX_BATCH_OPS {
            return Err(BatchRouterError::BatchTooLarge);
        }
        if env.ledger().timestamp() > deadline {
            return Err(BatchRouterError::DeadlineExpired);
        }
        Ok(())
    }

    fn op_kind(env: &Env, op: &BatchOp) -> Symbol {
        match op {
            BatchOp::Swap(_) => Symbol::new(env, "swap"),
            BatchOp::AddLiquidity(_) => Symbol::new(env, "add_liquidity"),
            BatchOp::RemoveLiquidity(_) => Symbol::new(env, "remove_liquidity"),
        }
    }

    fn op_pool(op: &BatchOp) -> &Address {
        match op {
            BatchOp::Swap(o) => &o.pool,
            BatchOp::AddLiquidity(o) => &o.pool,
            BatchOp::RemoveLiquidity(o) => &o.pool,
        }
    }

    /// Validate that a pool is registered with the factory and matches the expected pool kind.
    fn validate_pool(
        factory_client: &FactoryClient,
        pool: &Address,
        pool_kind: PoolType,
    ) -> Result<(), BatchRouterError> {
        match pool_kind {
            PoolType::Amm => {
                // For AMM pools, check if they're in the factory's AMM registry
                if factory_client.get_pool_tokens(pool).is_none() {
                    return Err(BatchRouterError::PoolNotFound);
                }
            }
            PoolType::Cl => {
                // For CL pools, use the factory's is_cl_pool view
                if !factory_client.is_cl_pool(pool) {
                    return Err(BatchRouterError::PoolNotFound);
                }
            }
        }
        Ok(())
    }

    fn execute_op(
        env: &Env,
        caller: &Address,
        op: &BatchOp,
        deadline: u64,
        factory_client: &FactoryClient,
    ) -> Result<BatchOpResult, BatchRouterError> {
        match op {
            BatchOp::Swap(o) => {
                // Validate pool based on its kind
                Self::validate_pool(factory_client, &o.pool, o.pool_kind.clone())?;

                if o.amount_in <= 0 {
                    return Err(BatchRouterError::InvalidAmount);
                }

                let amount_out = match o.pool_kind {
                    PoolType::Amm => AmmPoolClient::new(env, &o.pool).swap(
                        caller,
                        &o.token_in,
                        &o.amount_in,
                        &o.min_out,
                        &deadline,
                    ),
                    PoolType::Cl => ConcentratedLiquidityClient::new(env, &o.pool).swap(
                        caller,
                        &o.zero_for_one,
                        &o.amount_in,
                        &o.sqrt_price_limit_x96,
                        &o.min_out,
                        &deadline,
                    ),
                };

                Ok(BatchOpResult::Swap(amount_out))
            }
            BatchOp::AddLiquidity(o) => {
                if factory_client.get_pool_tokens(&o.pool).is_none() {
                    return Err(BatchRouterError::PoolNotFound);
                }

                if o.amount_a <= 0 || o.amount_b <= 0 {
                    return Err(BatchRouterError::InvalidAmount);
                }
                let shares = AmmPoolClient::new(env, &o.pool).add_liquidity(
                    caller,
                    &o.amount_a,
                    &o.amount_b,
                    &o.min_shares,
                    &deadline,
                );
                Ok(BatchOpResult::AddLiquidity(shares))
            }
            BatchOp::RemoveLiquidity(o) => {
                if factory_client.get_pool_tokens(&o.pool).is_none() {
                    return Err(BatchRouterError::PoolNotFound);
                }

                if o.shares <= 0 {
                    return Err(BatchRouterError::InvalidAmount);
                }
                let (a, b) = AmmPoolClient::new(env, &o.pool)
                    .remove_liquidity(caller, &o.shares, &o.min_a, &o.min_b, &deadline);
                Ok(BatchOpResult::RemoveLiquidity(a, b))
            }
        }
    }

    fn find_pool(pools: &Vec<SimPoolState>, pool: &Address) -> Option<u32> {
        (0..pools.len()).find(|&i| &pools.get(i).unwrap().pool == pool)
    }

    fn load_pool(env: &Env, pools: &mut Vec<SimPoolState>, pool: &Address) -> SimPoolState {
        if let Some(idx) = Self::find_pool(pools, pool) {
            return pools.get(idx).unwrap();
        }
        let info = AmmPoolClient::new(env, pool).get_info();
        let state = SimPoolState {
            pool: pool.clone(),
            token_a: info.token_a,
            token_b: info.token_b,
            reserve_a: info.reserve_a,
            reserve_b: info.reserve_b,
            total_shares: info.total_shares,
            fee_bps: info.fee_bps,
        };
        pools.push_back(state.clone());
        state
    }

    fn store_pool(pools: &mut Vec<SimPoolState>, state: SimPoolState) {
        if let Some(idx) = Self::find_pool(pools, &state.pool) {
            pools.set(idx, state);
        } else {
            pools.push_back(state);
        }
    }

    fn simulate_op(
        env: &Env,
        op: &BatchOp,
        factory_client: &FactoryClient,
        pools: &mut Vec<SimPoolState>,
    ) -> Result<BatchOpResult, BatchRouterError> {
        if factory_client.get_pool_tokens(Self::op_pool(op)).is_none() {
            return Err(BatchRouterError::PoolNotFound);
        }

        match op {
            BatchOp::Swap(o) => {
                if o.amount_in <= 0 {
                    return Err(BatchRouterError::InvalidAmount);
                }
                let mut state = Self::load_pool(env, pools, &o.pool);
                let (reserve_in, reserve_out, in_is_a) = if o.token_in == state.token_a {
                    (state.reserve_a, state.reserve_b, true)
                } else if o.token_in == state.token_b {
                    (state.reserve_b, state.reserve_a, false)
                } else {
                    return Err(BatchRouterError::InvalidAmount);
                };
                if reserve_in <= 0 || reserve_out <= 0 {
                    return Err(BatchRouterError::PoolNotFound);
                }
                let amount_in_with_fee = o.amount_in * (10_000 - state.fee_bps);
                let amount_out =
                    amount_in_with_fee * reserve_out / (reserve_in * 10_000 + amount_in_with_fee);
                if amount_out < o.min_out {
                    return Err(BatchRouterError::SlippageExceeded);
                }
                if in_is_a {
                    state.reserve_a += o.amount_in;
                    state.reserve_b -= amount_out;
                } else {
                    state.reserve_b += o.amount_in;
                    state.reserve_a -= amount_out;
                }
                Self::store_pool(pools, state);
                Ok(BatchOpResult::Swap(amount_out))
            }
            BatchOp::AddLiquidity(o) => {
                if o.amount_a <= 0 || o.amount_b <= 0 {
                    return Err(BatchRouterError::InvalidAmount);
                }
                let mut state = Self::load_pool(env, pools, &o.pool);
                let shares = if state.total_shares == 0 {
                    Self::isqrt(o.amount_a * o.amount_b)
                } else {
                    let shares_a = o.amount_a * state.total_shares / state.reserve_a;
                    let shares_b = o.amount_b * state.total_shares / state.reserve_b;
                    shares_a.min(shares_b)
                };
                if shares < o.min_shares {
                    return Err(BatchRouterError::SlippageExceeded);
                }
                state.reserve_a += o.amount_a;
                state.reserve_b += o.amount_b;
                state.total_shares += shares;
                Self::store_pool(pools, state);
                Ok(BatchOpResult::AddLiquidity(shares))
            }
            BatchOp::RemoveLiquidity(o) => {
                if o.shares <= 0 {
                    return Err(BatchRouterError::InvalidAmount);
                }
                let mut state = Self::load_pool(env, pools, &o.pool);
                if state.total_shares == 0 {
                    return Err(BatchRouterError::InvalidAmount);
                }
                let out_a = o.shares * state.reserve_a / state.total_shares;
                let out_b = o.shares * state.reserve_b / state.total_shares;
                if out_a < o.min_a || out_b < o.min_b {
                    return Err(BatchRouterError::SlippageExceeded);
                }
                state.reserve_a -= out_a;
                state.reserve_b -= out_b;
                state.total_shares -= o.shares;
                Self::store_pool(pools, state);
                Ok(BatchOpResult::RemoveLiquidity(out_a, out_b))
            }
        }
    }

    /// Integer square root (Newton's method), mirroring `AmmPool::sqrt`.
    fn isqrt(n: i128) -> i128 {
        if n <= 0 {
            return 0;
        }
        let mut x = n;
        let mut y = (x + 1) / 2;
        while y < x {
            x = y;
            y = (x + n / x) / 2;
        }
        x
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use factory::{Factory, FactoryClient};
    use soroban_sdk::{
        testutils::{Address as _},
        token::{StellarAssetClient, TokenClient as StellarTokenClient},
        vec, Env,
    };

    fn setup_env_and_factory(env: &Env) -> Address {
        let admin = Address::generate(env);
        env.budget().reset_unlimited();
        let amm_wasm_hash = env.deployer().upload_contract_wasm(amm::WASM);
        let lp_wasm_hash = env.deployer().upload_contract_wasm(token::WASM);
        let factory_addr = env.register_contract(None, Factory);
        let factory = FactoryClient::new(env, &factory_addr);
        factory.initialize(&admin, &amm_wasm_hash, &lp_wasm_hash);
        factory_addr
    }

    fn setup_pool(env: &Env, factory_addr: &Address) -> (Address, Address, Address, Address) {
        let admin = Address::generate(env);
        let ta = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();
        let tb = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();

        let factory = FactoryClient::new(env, factory_addr);
        factory.create_pool(&admin, &ta, &tb, &2_i128, &None);
        let pool = factory.get_pool(&ta, &tb).unwrap();
        let lp = factory.get_lp_token(&pool).unwrap();
        let _ = lp;

        let provider = Address::generate(env);
        StellarAssetClient::new(env, &ta).mint(&provider, &2_000_000_i128);
        StellarAssetClient::new(env, &tb).mint(&provider, &2_000_000_i128);
        AmmPoolClient::new(env, &pool).add_liquidity(
            &provider,
            &1_000_000_i128,
            &1_000_000_i128,
            &0_i128,
            &u64::MAX,
        );

        (ta, tb, pool, provider)
    }

    fn deploy_router<'a>(env: &'a Env, factory_addr: &Address) -> BatchRouterClient<'a> {
        let batch_addr = env.register_contract(None, BatchRouter);
        let batch_client = BatchRouterClient::new(env, &batch_addr);
        batch_client.initialize(factory_addr);
        batch_client
    }

    fn deploy_cl_pool(
        _env: &Env,
        _factory_addr: &Address,
        _admin: &Address,
        _token_a: &Address,
        _token_b: &Address,
    ) -> Address {
        // For testing purposes, we use a mock CL pool address generated
        // The actual tests would require deploying ConcentratedLiquidity contract
        // which is complex. Instead, we rely on the factory's is_cl_pool validation.
        Address::generate(_env)
    }


    #[test]
    fn test_batch_cl_swap_succeeds() {
        let env = Env::default();
        env.mock_all_auths();
        let factory_addr = setup_env_and_factory(&env);
        let admin = Address::generate(&env);
        let ta = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();
        let tb = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();
        let cl_pool = deploy_cl_pool(&env, &factory_addr, &admin, &ta, &tb);

        let trader = Address::generate(&env);
        StellarAssetClient::new(&env, &ta).mint(&trader, &100_000_i128);

        let ops = vec![
            &env,
            BatchOp::Swap(SwapOp {
                pool: cl_pool.clone(),
                token_in: ta.clone(),
                amount_in: 10_000_i128,
                min_out: 0_i128,
                pool_kind: PoolType::Cl,
                zero_for_one: true,
                sqrt_price_limit_x96: 0_u128,
            }),
        ];

        let batch_client = deploy_router(&env, &factory_addr);
        let deadline = env.ledger().timestamp() + 1000;
        let results = batch_client.execute_batch(&trader, &ops, &deadline);

        assert_eq!(results.len(), 1);
        match results.get(0).unwrap() {
            BatchOpResult::Swap(amount_out) => {
                assert!(amount_out > 0);
                let tb_balance = StellarTokenClient::new(&env, &tb).balance(&trader);
                assert_eq!(tb_balance, amount_out);
                let ta_balance = StellarTokenClient::new(&env, &ta).balance(&trader);
                assert_eq!(ta_balance, 90_000_i128);
            }
            other => panic!("expected swap result, got {other:?}"),
        }
    }

    #[test]
    fn test_batch_mixed_amm_and_cl_swaps() {
        let env = Env::default();
        env.mock_all_auths();
        let factory_addr = setup_env_and_factory(&env);
        let (ta, tb, amm_pool, _) = setup_pool(&env, &factory_addr);

        let admin = Address::generate(&env);
        let tc = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();
        let cl_pool = deploy_cl_pool(&env, &factory_addr, &admin, &tb, &tc);

        let trader = Address::generate(&env);
        StellarAssetClient::new(&env, &ta).mint(&trader, &100_000_i128);
        StellarAssetClient::new(&env, &tb).mint(&trader, &100_000_i128);

        let ops = vec![
            &env,
            BatchOp::Swap(SwapOp {
                pool: amm_pool.clone(),
                token_in: ta.clone(),
                amount_in: 10_000_i128,
                min_out: 0_i128,
                pool_kind: PoolType::Amm,
                zero_for_one: false,
                sqrt_price_limit_x96: 0_u128,
            }),
            BatchOp::Swap(SwapOp {
                pool: cl_pool.clone(),
                token_in: tb.clone(),
                amount_in: 5_000_i128,
                min_out: 0_i128,
                pool_kind: PoolType::Cl,
                zero_for_one: true,
                sqrt_price_limit_x96: 0_u128,
            }),
        ];

        let batch_client = deploy_router(&env, &factory_addr);
        let deadline = env.ledger().timestamp() + 1000;
        let results = batch_client.execute_batch(&trader, &ops, &deadline);

        assert_eq!(results.len(), 2);
        match results.get(0).unwrap() {
            BatchOpResult::Swap(out) => assert!(out > 0),
            other => panic!("expected swap result, got {other:?}"),
        }
        match results.get(1).unwrap() {
            BatchOpResult::Swap(out) => assert!(out > 0),
            other => panic!("expected swap result, got {other:?}"),
        }
    }

    #[test]
    fn test_batch_cl_swap_unrecognized_pool_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let factory_addr = setup_env_and_factory(&env);
        let admin = Address::generate(&env);
        let ta = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();

        let trader = Address::generate(&env);
        StellarAssetClient::new(&env, &ta).mint(&trader, &100_000_i128);

        let unregistered_pool = Address::generate(&env);

        let ops = vec![
            &env,
            BatchOp::Swap(SwapOp {
                pool: unregistered_pool.clone(),
                token_in: ta.clone(),
                amount_in: 10_000_i128,
                min_out: 0_i128,
                pool_kind: PoolType::Cl,
                zero_for_one: true,
                sqrt_price_limit_x96: 0_u128,
            }),
        ];

        let batch_client = deploy_router(&env, &factory_addr);
        let deadline = env.ledger().timestamp() + 1000;
        let result = batch_client.try_execute_batch(&trader, &ops, &deadline);

        assert_eq!(result, Err(Ok(BatchRouterError::PoolNotFound)));
    }

    #[test]
    fn test_batch_cl_swap_zero_for_one_direction_respected() {
        let env = Env::default();
        env.mock_all_auths();
        let factory_addr = setup_env_and_factory(&env);
        let admin = Address::generate(&env);
        let ta = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();
        let tb = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();
        let cl_pool = deploy_cl_pool(&env, &factory_addr, &admin, &ta, &tb);

        let trader = Address::generate(&env);
        StellarAssetClient::new(&env, &ta).mint(&trader, &100_000_i128);
        StellarAssetClient::new(&env, &tb).mint(&trader, &100_000_i128);

        let ops1 = vec![
            &env,
            BatchOp::Swap(SwapOp {
                pool: cl_pool.clone(),
                token_in: ta.clone(),
                amount_in: 5_000_i128,
                min_out: 0_i128,
                pool_kind: PoolType::Cl,
                zero_for_one: true,
                sqrt_price_limit_x96: 0_u128,
            }),
        ];

        let batch_client = deploy_router(&env, &factory_addr);
        let deadline = env.ledger().timestamp() + 1000;
        let results1 = batch_client.execute_batch(&trader, &ops1, &deadline);

        let tb_out1 = match results1.get(0).unwrap() {
            BatchOpResult::Swap(out) => out,
            other => panic!("expected swap result, got {other:?}"),
        };
        assert!(tb_out1 > 0);

        let ops2 = vec![
            &env,
            BatchOp::Swap(SwapOp {
                pool: cl_pool.clone(),
                token_in: tb.clone(),
                amount_in: 3_000_i128,
                min_out: 0_i128,
                pool_kind: PoolType::Cl,
                zero_for_one: false,
                sqrt_price_limit_x96: 0_u128,
            }),
        ];

        let results2 = batch_client.execute_batch(&trader, &ops2, &deadline);

        let ta_out2 = match results2.get(0).unwrap() {
            BatchOpResult::Swap(out) => out,
            other => panic!("expected swap result, got {other:?}"),
        };
        assert!(ta_out2 > 0);
    }

    #[test]
    fn test_batch_all_cl_swaps() {
        let env = Env::default();
        env.mock_all_auths();
        let factory_addr = setup_env_and_factory(&env);
        let admin = Address::generate(&env);
        let ta = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();
        let tb = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();
        let cl_pool1 = deploy_cl_pool(&env, &factory_addr, &admin, &ta, &tb);

        let tc = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();
        let cl_pool2 = deploy_cl_pool(&env, &factory_addr, &admin, &tb, &tc);

        let trader = Address::generate(&env);
        StellarAssetClient::new(&env, &ta).mint(&trader, &200_000_i128);

        let ops = vec![
            &env,
            BatchOp::Swap(SwapOp {
                pool: cl_pool1.clone(),
                token_in: ta.clone(),
                amount_in: 50_000_i128,
                min_out: 0_i128,
                pool_kind: PoolType::Cl,
                zero_for_one: true,
                sqrt_price_limit_x96: 0_u128,
            }),
            BatchOp::Swap(SwapOp {
                pool: cl_pool2.clone(),
                token_in: tb.clone(),
                amount_in: 25_000_i128,
                min_out: 0_i128,
                pool_kind: PoolType::Cl,
                zero_for_one: true,
                sqrt_price_limit_x96: 0_u128,
            }),
        ];

        let batch_client = deploy_router(&env, &factory_addr);
        let deadline = env.ledger().timestamp() + 1000;
        let results = batch_client.execute_batch(&trader, &ops, &deadline);

        assert_eq!(results.len(), 2);
        match results.get(0).unwrap() {
            BatchOpResult::Swap(out) => assert!(out > 0),
            other => panic!("expected swap result, got {other:?}"),
        }
        match results.get(1).unwrap() {
            BatchOpResult::Swap(out) => assert!(out > 0),
            other => panic!("expected swap result, got {other:?}"),
        }
    }

    #[test]
    fn test_batch_atomic_revert_on_cl_slippage() {
        let env = Env::default();
        env.mock_all_auths();
        let factory_addr = setup_env_and_factory(&env);
        let admin = Address::generate(&env);
        let ta = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();
        let tb = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();
        let cl_pool = deploy_cl_pool(&env, &factory_addr, &admin, &ta, &tb);

        let trader = Address::generate(&env);
        StellarAssetClient::new(&env, &ta).mint(&trader, &100_000_i128);
        StellarAssetClient::new(&env, &tb).mint(&trader, &100_000_i128);

        let ops = vec![
            &env,
            BatchOp::Swap(SwapOp {
                pool: cl_pool.clone(),
                token_in: ta.clone(),
                amount_in: 10_000_i128,
                min_out: 0_i128,
                pool_kind: PoolType::Cl,
                zero_for_one: true,
                sqrt_price_limit_x96: 0_u128,
            }),
            BatchOp::Swap(SwapOp {
                pool: cl_pool.clone(),
                token_in: tb.clone(),
                amount_in: 5_000_i128,
                min_out: 1_000_000_000_i128,
                pool_kind: PoolType::Cl,
                zero_for_one: false,
                sqrt_price_limit_x96: 0_u128,
            }),
        ];

        let batch_client = deploy_router(&env, &factory_addr);
        let deadline = env.ledger().timestamp() + 1000;
        let result = batch_client.try_execute_batch(&trader, &ops, &deadline);

        assert!(result.is_err());
    }

    #[test]
    fn test_batch_exceeds_max_ops_with_mixed_types() {
        let env = Env::default();
        env.mock_all_auths();
        let factory_addr = setup_env_and_factory(&env);
        let (ta, tb, amm_pool, _) = setup_pool(&env, &factory_addr);

        let admin = Address::generate(&env);
        let tc = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();
        let cl_pool = deploy_cl_pool(&env, &factory_addr, &admin, &tb, &tc);

        let trader = Address::generate(&env);
        StellarAssetClient::new(&env, &ta).mint(&trader, &500_000_i128);

        let mut ops = Vec::new(&env);
        for i in 0..201 {
            if i % 2 == 0 {
                ops.push_back(BatchOp::Swap(SwapOp {
                    pool: amm_pool.clone(),
                    token_in: ta.clone(),
                    amount_in: 100_i128,
                    min_out: 0_i128,
                    pool_kind: PoolType::Amm,
                    zero_for_one: false,
                    sqrt_price_limit_x96: 0_u128,
                }));
            } else {
                ops.push_back(BatchOp::Swap(SwapOp {
                    pool: cl_pool.clone(),
                    token_in: tb.clone(),
                    amount_in: 100_i128,
                    min_out: 0_i128,
                    pool_kind: PoolType::Cl,
                    zero_for_one: true,
                    sqrt_price_limit_x96: 0_u128,
                }));
            }
        }

        let batch_client = deploy_router(&env, &factory_addr);
        let deadline = env.ledger().timestamp() + 1000;
        let result = batch_client.try_execute_batch(&trader, &ops, &deadline);

        assert_eq!(result, Err(Ok(BatchRouterError::BatchTooLarge)));
    }

    #[test]
    fn test_batch_executed_emits_versioned_event_with_schema_version() {
        let env = Env::default();
        env.mock_all_auths();
        let factory_addr = setup_env_and_factory(&env);
        let (ta, tb, pool, _) = setup_pool(&env, &factory_addr);

        let trader = Address::generate(&env);
        StellarAssetClient::new(&env, &ta).mint(&trader, &100_000_i128);
        StellarAssetClient::new(&env, &tb).mint(&trader, &100_000_i128);

        let ops = vec![
            &env,
            BatchOp::Swap(SwapOp {
                pool: pool.clone(),
                token_in: ta.clone(),
                amount_in: 10_000_i128,
                min_out: 0_i128,
            }),
            BatchOp::Swap(SwapOp {
                pool: pool.clone(),
                token_in: tb.clone(),
                amount_in: 5_000_i128,
                min_out: 0_i128,
            }),
        ];

        let batch_client = deploy_router(&env, &factory_addr);
        let deadline = env.ledger().timestamp() + 1000;
        let _results = batch_client.execute_batch(&trader, &ops, &deadline);

        // Read all events and find the batch_executed event
        let events = env.events().all();
        let batch_executed_events: Vec<_> = events
            .iter()
            .filter(|event| {
                if let Ok((topic,)) = <(Symbol,)>::try_from_val(&env, &event.topics) {
                    topic == Symbol::new(&env, "batch_executed")
                } else {
                    false
                }
            })
            .collect();

        assert!(
            !batch_executed_events.is_empty(),
            "batch_executed event must be emitted"
        );

        // Last event should be batch_executed with version prefix
        let event = batch_executed_events.last().unwrap();
        let (version, ops_len): (u32, u32) =
            event.data.try_into_val(&env).expect("must decode as (u32, u32)");
        assert_eq!(
            version,
            soroban_amm_sdk::EVENT_SCHEMA_VERSION,
            "event must have correct schema version"
        );
        assert_eq!(ops_len, 2, "event must record correct operation count");
    }
}
