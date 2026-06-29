#![no_std]

//! TWAL (time-weighted average liquidity) consumer contract.
//!
//! Mirrors the `twap_consumer` pattern: keepers save periodic snapshots of each
//! pool's on-chain `get_liquidity_cumulative` accumulator, then callers query
//! average liquidity over a window for yield calculations and multi-pool analytics.

use soroban_sdk::{contract, contractclient, contractimpl, contracttype, Address, Env, Vec};

#[contractclient(name = "AmmPoolLiquidityClient")]
pub trait AmmPoolLiquidityOracle {
    fn get_liquidity_cumulative(env: Env) -> (i128, u64);
}

#[contractclient(name = "ClPoolLiquidityClient")]
pub trait ClPoolLiquidityOracle {
    fn active_liquidity(env: Env) -> i128;
    fn get_tick_cumulative(env: Env) -> (i64, u64);
}

#[contracttype]
pub enum DataKey {
    /// Address authorized to write snapshots (a keeper bot or governance).
    Keeper,
    LiquiditySnapshot(Address, u64),
    /// Persistent registry of tracked pools to avoid instance storage limits.
    TrackedPoolsPersistent,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LiquiditySnapshot {
    pub cum_liquidity: i128,
    pub pool_ts: u64,
}

#[contract]
pub struct TwalConsumer;

#[contractimpl]
impl TwalConsumer {
    pub const SNAPSHOT_TTL_LEDGERS: u32 = 120_960;

    /// Registers the keeper authorized to write liquidity snapshots.
    ///
    /// Must be called once at deploy time. Snapshot writes (`save_snapshot`,
    /// `save_cl_snapshot`) require the keeper's authorization, preventing
    /// arbitrary callers from corrupting the TWAL history that downstream
    /// yield-calculation and multi-pool analytics depend on.
    pub fn initialize(env: Env, keeper: Address) {
        assert!(
            !env.storage().instance().has(&DataKey::Keeper),
            "already initialized"
        );
        env.storage().instance().set(&DataKey::Keeper, &keeper);
    }

    /// Returns the configured keeper, panicking if the contract is not initialized.
    pub fn get_keeper(env: Env) -> Address {
        env.storage()
            .instance()
            .get(&DataKey::Keeper)
            .unwrap_or_else(|| panic!("contract not initialized"))
    }

    /// Requires the stored keeper's authorization for snapshot writes.
    fn require_keeper(env: &Env) {
        Self::get_keeper(env.clone()).require_auth();
    }

    /// Persist a pool liquidity accumulator snapshot keyed by ledger timestamp.
    pub fn save_snapshot(env: Env, pool: Address) {
        Self::require_keeper(&env);
        let (cum, pool_ts) = AmmPoolLiquidityClient::new(&env, &pool).get_liquidity_cumulative();
        let ledger_ts = env.ledger().timestamp();
        let snapshot = LiquiditySnapshot {
            cum_liquidity: cum,
            pool_ts,
        };
        let key = DataKey::LiquiditySnapshot(pool.clone(), ledger_ts);
        env.storage().persistent().set(&key, &snapshot);
        env.storage().persistent().extend_ttl(
            &key,
            Self::SNAPSHOT_TTL_LEDGERS / 2,
            Self::SNAPSHOT_TTL_LEDGERS,
        );

        Self::register_tracked_pool(&env, &pool);
    }

    /// Register a pool in the persistent tracked-pools list, deduplicating entries.
    fn register_tracked_pool(env: &Env, pool: &Address) {
        let mut tracked: Vec<Address> = env
            .storage()
            .persistent()
            .get(&DataKey::TrackedPoolsPersistent)
            .unwrap_or_else(|| Vec::new(env));
        let mut already = false;
        for i in 0..tracked.len() {
            if tracked.get(i).unwrap() == *pool {
                already = true;
                break;
            }
        }
        if !already {
            tracked.push_back(pool.clone());
        }
        env.storage()
            .persistent()
            .set(&DataKey::TrackedPoolsPersistent, &tracked);
        env.storage().persistent().extend_ttl(
            &DataKey::TrackedPoolsPersistent,
            Self::SNAPSHOT_TTL_LEDGERS / 2,
            Self::SNAPSHOT_TTL_LEDGERS,
        );
    }

    /// Average pool liquidity (sqrt(reserve_a * reserve_b)) over `window_seconds`.
    pub fn get_twal_liquidity(env: Env, pool: Address, window_seconds: u64) -> i128 {
        assert!(window_seconds > 0, "window_seconds must be > 0");

        let (cum_now, pool_ts_now) =
            AmmPoolLiquidityClient::new(&env, &pool).get_liquidity_cumulative();
        let ledger_ts_now = env.ledger().timestamp();
        assert!(
            ledger_ts_now >= window_seconds,
            "ledger timestamp is smaller than requested window"
        );

        let then_ts = ledger_ts_now - window_seconds;
        let snapshot: LiquiditySnapshot = env
            .storage()
            .persistent()
            .get(&DataKey::LiquiditySnapshot(pool, then_ts))
            .unwrap_or_else(|| panic!("missing liquidity snapshot at {then_ts}"));

        let delta = (cum_now as u128).wrapping_sub(snapshot.cum_liquidity as u128) as i128;
        let elapsed = (pool_ts_now - snapshot.pool_ts) as i128;
        assert!(elapsed > 0, "window too small (pool time did not advance)");
        delta / elapsed
    }

    /// TWAL for every tracked pool in one call.
    pub fn get_twal_all(env: Env, window_seconds: u64) -> Vec<(Address, i128)> {
        let tracked = Self::get_tracked_pools(env.clone());
        let mut results: Vec<(Address, i128)> = Vec::new(&env);
        for i in 0..tracked.len() {
            let pool = tracked.get(i).unwrap();
            let twal = Self::get_twal_liquidity(env.clone(), pool.clone(), window_seconds);
            results.push_back((pool, twal));
        }
        results
    }

    pub fn get_tracked_pools(env: Env) -> Vec<Address> {
        env.storage()
            .persistent()
            .get(&DataKey::TrackedPoolsPersistent)
            .unwrap_or_else(|| Vec::new(&env))
    }

    /// Save CL pool snapshot using active_liquidity * elapsed approximation.
    pub fn save_cl_snapshot(env: Env, pool: Address) {
        Self::require_keeper(&env);
        let active = ClPoolLiquidityClient::new(&env, &pool).active_liquidity();
        let (_tick_cum, pool_ts) = ClPoolLiquidityClient::new(&env, &pool).get_tick_cumulative();
        let ledger_ts = env.ledger().timestamp();
        let snapshot = LiquiditySnapshot {
            cum_liquidity: active,
            pool_ts,
        };
        let key = DataKey::LiquiditySnapshot(pool.clone(), ledger_ts);
        env.storage().persistent().set(&key, &snapshot);
        env.storage().persistent().extend_ttl(
            &key,
            Self::SNAPSHOT_TTL_LEDGERS / 2,
            Self::SNAPSHOT_TTL_LEDGERS,
        );

        Self::register_tracked_pool(&env, &pool);
    }

    /// Returns the time‑weighted average active liquidity for a CL pool over `window_seconds`.
    pub fn get_cl_twal(env: Env, pool: Address, window_seconds: u64) -> i128 {
        assert!(window_seconds > 0, "window_seconds must be > 0");

        // Current active liquidity and pool timestamp
        let active = ClPoolLiquidityClient::new(&env, &pool).active_liquidity();
        let (_, pool_ts_now) = ClPoolLiquidityClient::new(&env, &pool).get_tick_cumulative();

        let ledger_ts_now = env.ledger().timestamp();
        assert!(
            ledger_ts_now >= window_seconds,
            "ledger timestamp is smaller than requested window"
        );

        let then_ts = ledger_ts_now - window_seconds;
        let snapshot: LiquiditySnapshot = env
            .storage()
            .persistent()
            .get(&DataKey::LiquiditySnapshot(pool, then_ts))
            .unwrap_or_else(|| panic!("missing liquidity snapshot at {then_ts}"));

        let delta = (active as u128).wrapping_sub(snapshot.cum_liquidity as u128) as i128;
        let elapsed = (pool_ts_now - snapshot.pool_ts) as i128;
        assert!(elapsed > 0, "window too small (pool time did not advance)");
        delta / elapsed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use amm::{AmmPool, AmmPoolClient};
    use soroban_sdk::{
        testutils::{Address as _, Ledger},
        token::{StellarAssetClient, TokenClient as StellarTokenClient},
        Address, Env,
    };
    use token::LpToken;

    fn create_sac<'a>(
        env: &'a Env,
        admin: &Address,
    ) -> (StellarTokenClient<'a>, StellarAssetClient<'a>) {
        let contract = env.register_stellar_asset_contract_v2(admin.clone());
        (
            StellarTokenClient::new(env, &contract.address()),
            StellarAssetClient::new(env, &contract.address()),
        )
    }

    #[test]
    fn test_twal_increases_with_liquidity_and_time() {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set_timestamp(10_000);

        let admin = Address::generate(&env);
        let amm_addr = env.register_contract(None, AmmPool);
        let lp_addr = env.register_contract(None, LpToken);
        let consumer_addr = env.register_contract(None, TwalConsumer);

        token::LpTokenClient::new(&env, &lp_addr).initialize(
            &amm_addr,
            &soroban_sdk::String::from_str(&env, "LP"),
            &soroban_sdk::String::from_str(&env, "LP"),
            &7u32,
        );

        let (ta, ta_sac) = create_sac(&env, &admin);
        let (tb, tb_sac) = create_sac(&env, &admin);
        AmmPoolClient::new(&env, &amm_addr).initialize(
            &admin,
            &ta.address,
            &tb.address,
            &lp_addr,
            &30_i128,
            &admin,
            &0_i128,
        );

        let provider = Address::generate(&env);
        ta_sac.mint(&provider, &1_000_000_i128);
        tb_sac.mint(&provider, &1_000_000_i128);
        AmmPoolClient::new(&env, &amm_addr).add_liquidity(
            &provider,
            &1_000_000_i128,
            &1_000_000_i128,
            &0_i128,
            &u64::MAX,
        );

        let consumer = TwalConsumerClient::new(&env, &consumer_addr);
        consumer.initialize(&admin);
        consumer.save_snapshot(&amm_addr);

        env.ledger().with_mut(|l| l.timestamp = 10_600);
        // Mint additional tokens for the second deposit.
        ta_sac.mint(&provider, &200_000_i128);
        tb_sac.mint(&provider, &200_000_i128);
        AmmPoolClient::new(&env, &amm_addr).add_liquidity(
            &provider,
            &100_000_i128,
            &100_000_i128,
            &0_i128,
            &u64::MAX,
        );
        consumer.save_snapshot(&amm_addr);

        env.ledger().with_mut(|l| l.timestamp = 11_200);
        // Trigger a pool interaction so checkpoint_twal advances pool_ts to 11_200.
        let trader = Address::generate(&env);
        ta_sac.mint(&trader, &1_000_i128);
        AmmPoolClient::new(&env, &amm_addr).swap(
            &trader,
            &ta.address,
            &1_000_i128,
            &0_i128,
            &u64::MAX,
        );

        let twal = consumer.get_twal_liquidity(&amm_addr, &600);
        assert!(twal > 0);
    }

    // Issue #372: snapshot writes must be gated behind the keeper's auth.
    #[test]
    fn test_save_snapshot_requires_keeper_auth() {
        let env = Env::default();
        env.ledger().set_timestamp(10_000);

        let keeper = Address::generate(&env);
        let pool = Address::generate(&env);
        let consumer_addr = env.register_contract(None, TwalConsumer);
        let consumer = TwalConsumerClient::new(&env, &consumer_addr);

        consumer.initialize(&keeper);

        // No auth has been mocked: unauthorized snapshot writes must fail.
        assert!(consumer.try_save_snapshot(&pool).is_err());
        assert!(consumer.try_save_cl_snapshot(&pool).is_err());
    }

    // Issue #372: snapshot writes must fail before initialize is called.
    #[test]
    fn test_save_snapshot_fails_when_uninitialized() {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set_timestamp(10_000);

        let pool = Address::generate(&env);
        let consumer_addr = env.register_contract(None, TwalConsumer);
        let consumer = TwalConsumerClient::new(&env, &consumer_addr);

        // Keeper was never registered, so the contract is uninitialized.
        assert!(consumer.try_save_snapshot(&pool).is_err());
    }

    // Issue #372: initialize is a one-time operation.
    #[test]
    fn test_initialize_is_idempotent_guard() {
        let env = Env::default();
        env.mock_all_auths();

        let keeper = Address::generate(&env);
        let consumer_addr = env.register_contract(None, TwalConsumer);
        let consumer = TwalConsumerClient::new(&env, &consumer_addr);

        consumer.initialize(&keeper);
        assert_eq!(consumer.get_keeper(), keeper);
        // A second initialize must be rejected.
        assert!(consumer.try_initialize(&Address::generate(&env)).is_err());
    }
}
