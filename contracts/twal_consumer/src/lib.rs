#![no_std]

//! TWAL (time-weighted average liquidity) consumer contract.

use soroban_sdk::{
    contract, contractclient, contracterror, contractimpl, contracttype, Address, Env, Symbol, Vec,
};

#[contractclient(name = "AmmPoolLiquidityClient")]
pub trait AmmPoolLiquidityOracle {
    fn get_liquidity_cumulative(env: Env) -> (i128, u64);
}

#[contractclient(name = "ClPoolLiquidityClient")]
pub trait ClPoolLiquidityOracle {
    fn active_liquidity(env: Env) -> i128;
    fn get_tick_cumulative(env: Env) -> (i64, u64);
}

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum TwalError {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    ZeroWindow = 3,
    InsufficientHistory = 4,
    NoSnapshotFound = 5,
    ElapsedZero = 6,
}

#[contracttype]
pub enum DataKey {
    Keeper,
    LiquiditySnapshot(Address, u64),
    TrackedPoolsPersistent,
    /// Sorted (ascending, deduplicated) ledger timestamps at which a
    /// liquidity snapshot was saved for this pool. Lets `get_twal_*`
    /// binary-search for the most recent snapshot at or before an arbitrary
    /// `then_ts` instead of requiring an exact-timestamp hit (issue #469).
    SnapshotTimestamps(Address),
    /// Running liquidity-cumulative state for a CL pool (see `save_cl_snapshot`).
    ClAccumulator(Address),
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PoolType {
    Amm,
    Cl,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrackedPool {
    pub address: Address,
    pub pool_type: PoolType,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LiquiditySnapshot {
    pub cum_liquidity: i128,
    pub pool_ts: u64,
}

/// Running integral of a CL pool's active liquidity over ledger time.
///
/// Concentrated-liquidity pools only expose the *instantaneous* active
/// liquidity, so — unlike the AMM path — there is no pool-side cumulative to
/// difference. This contract builds one itself: on each keeper snapshot it adds
/// `last_active * elapsed` to `cum_liquidity`, turning a series of instantaneous
/// readings into a proper time-weighted accumulator that `get_cl_twal` can
/// difference to recover an average liquidity *level* (issue #462).
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClLiquidityAccumulator {
    /// Integral of active liquidity over ledger time (liquidity-seconds).
    pub cum_liquidity: i128,
    /// Ledger timestamp at which the accumulator was last updated.
    pub last_ts: u64,
    /// Active liquidity recorded at `last_ts`, held constant until the next update.
    pub last_active: i128,
}

#[contract]
pub struct TwalConsumer;

#[contractimpl]
impl TwalConsumer {
    pub const SNAPSHOT_TTL_LEDGERS: u32 = 120_960;

    pub fn initialize(env: Env, keeper: Address) -> Result<(), TwalError> {
        if env.storage().instance().has(&DataKey::Keeper) {
            return Err(TwalError::AlreadyInitialized);
        }
        env.storage().instance().set(&DataKey::Keeper, &keeper);
        Ok(())
    }

    pub fn get_keeper(env: Env) -> Result<Address, TwalError> {
        env.storage()
            .instance()
            .get(&DataKey::Keeper)
            .ok_or(TwalError::NotInitialized)
    }

    fn require_keeper(env: &Env) -> Result<(), TwalError> {
        Self::get_keeper(env.clone())?.require_auth();
        Ok(())
    }

    pub fn save_snapshot(env: Env, pool: Address) -> Result<(), TwalError> {
        Self::require_keeper(&env)?;
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
        Self::register_tracked_pool(&env, &pool, PoolType::Amm);
        Self::record_snapshot_timestamp(&env, &pool, ledger_ts);
        Ok(())
    }

    /// Record `ts` in the pool's sorted snapshot-timestamp index. Ledger
    /// timestamps are non-decreasing across calls, so appending keeps the
    /// index sorted; skip if `ts` is already the most recent entry so a
    /// keeper re-saving within the same ledger doesn't create a duplicate.
    fn record_snapshot_timestamp(env: &Env, pool: &Address, ts: u64) {
        let key = DataKey::SnapshotTimestamps(pool.clone());
        let mut timestamps: Vec<u64> = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or_else(|| Vec::new(env));
        if timestamps.last() != Some(ts) {
            timestamps.push_back(ts);
        }
        env.storage().persistent().set(&key, &timestamps);
        env.storage().persistent().extend_ttl(
            &key,
            Self::SNAPSHOT_TTL_LEDGERS / 2,
            Self::SNAPSHOT_TTL_LEDGERS,
        );
    }

    fn remove_snapshot_timestamp(env: &Env, pool: &Address, ts: u64) {
        let key = DataKey::SnapshotTimestamps(pool.clone());
        let timestamps: Vec<u64> = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or_else(|| Vec::new(env));
        let mut updated: Vec<u64> = Vec::new(env);
        for i in 0..timestamps.len() {
            let t = timestamps.get(i).unwrap();
            if t != ts {
                updated.push_back(t);
            }
        }
        env.storage().persistent().set(&key, &updated);
    }

    /// Binary-search the pool's snapshot-timestamp index for the most recent
    /// entry at or before `then_ts` (the "floor"). Returns `None` if no
    /// snapshot that old exists.
    fn floor_snapshot_ts(env: &Env, pool: &Address, then_ts: u64) -> Option<u64> {
        let timestamps: Vec<u64> = env
            .storage()
            .persistent()
            .get(&DataKey::SnapshotTimestamps(pool.clone()))
            .unwrap_or_else(|| Vec::new(env));
        let mut lo: u32 = 0;
        let mut hi: u32 = timestamps.len();
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if timestamps.get(mid).unwrap() <= then_ts {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        if lo == 0 {
            None
        } else {
            Some(timestamps.get(lo - 1).unwrap())
        }
    }

    fn register_tracked_pool(env: &Env, pool: &Address, pool_type: PoolType) {
        let mut tracked: Vec<TrackedPool> = env
            .storage()
            .persistent()
            .get(&DataKey::TrackedPoolsPersistent)
            .unwrap_or_else(|| Vec::new(env));

        let mut already = false;

        for i in 0..tracked.len() {
            let item = tracked.get(i).unwrap();
            if item.address == *pool {
                already = true;
                break;
            }
        }

        if !already {
            tracked.push_back(TrackedPool {
                address: pool.clone(),
                pool_type,
            });
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

    pub fn get_twal_liquidity(
        env: Env,
        pool: Address,
        window_seconds: u64,
    ) -> Result<i128, TwalError> {
        if window_seconds == 0 {
            return Err(TwalError::ZeroWindow);
        }
        let (cum_now, pool_ts_now) =
            AmmPoolLiquidityClient::new(&env, &pool).get_liquidity_cumulative();
        let ledger_ts_now = env.ledger().timestamp();
        if ledger_ts_now < window_seconds {
            return Err(TwalError::InsufficientHistory);
        }
        let then_ts = ledger_ts_now - window_seconds;
        let floor_ts =
            Self::floor_snapshot_ts(&env, &pool, then_ts).ok_or(TwalError::InsufficientHistory)?;
        let snapshot: LiquiditySnapshot = env
            .storage()
            .persistent()
            .get(&DataKey::LiquiditySnapshot(pool, floor_ts))
            .ok_or(TwalError::NoSnapshotFound)?;

        let delta = (cum_now as u128).wrapping_sub(snapshot.cum_liquidity as u128) as i128;
        let elapsed = (pool_ts_now - snapshot.pool_ts) as i128;
        if elapsed <= 0 {
            return Err(TwalError::ElapsedZero);
        }
        Ok(delta / elapsed)
    }

    pub fn get_twal_all(env: Env, window_seconds: u64) -> Result<Vec<(Address, i128)>, TwalError> {
        let tracked = Self::get_tracked_pools(env.clone());
        let mut results: Vec<(Address, i128)> = Vec::new(&env);
        for i in 0..tracked.len() {
            let tracked_pool = tracked.get(i).unwrap();

            let twal = match tracked_pool.pool_type {
                PoolType::Amm => Self::get_twal_liquidity(
                    env.clone(),
                    tracked_pool.address.clone(),
                    window_seconds,
                )?,

                PoolType::Cl => {
                    Self::get_cl_twal(
                        env.clone(),
                        tracked_pool.address.clone(),
                        window_seconds,
                    )
                }
            };

            results.push_back((tracked_pool.address, twal));
        }
        Ok(results)
    }

    pub fn get_tracked_pools(env: Env) -> Vec<TrackedPool> {
        env.storage()
            .persistent()
            .get(&DataKey::TrackedPoolsPersistent)
            .unwrap_or_else(|| Vec::new(&env))
    }

    /// Save a CL pool snapshot, accumulating the integral of active liquidity.
    ///
    /// A CL pool only exposes its *instantaneous* active liquidity, so each call
    /// advances a running accumulator by `last_active * elapsed` (the liquidity
    /// recorded at the previous snapshot, held constant over the interval since).
    /// The resulting cumulative — not the raw instantaneous reading — is stored
    /// in the snapshot, so `get_cl_twal` can difference two snapshots to recover
    /// an average liquidity *level* rather than a rate of change (issue #462).
    pub fn save_cl_snapshot(env: Env, pool: Address) -> Result<(), TwalError> {
        Self::require_keeper(&env)?;
        let active = ClPoolLiquidityClient::new(&env, &pool).active_liquidity();
        let ledger_ts = env.ledger().timestamp();

        let acc_key = DataKey::ClAccumulator(pool.clone());
        let cum = match env
            .storage()
            .persistent()
            .get::<_, ClLiquidityAccumulator>(&acc_key)
        {
            Some(prev) => {
                let elapsed = ledger_ts.saturating_sub(prev.last_ts) as i128;
                prev.cum_liquidity + prev.last_active * elapsed
            }
            // First snapshot for this pool: cumulative starts at zero.
            None => 0,
        };

        let accumulator = ClLiquidityAccumulator {
            cum_liquidity: cum,
            last_ts: ledger_ts,
            last_active: active,
        };
        env.storage().persistent().set(&acc_key, &accumulator);
        env.storage().persistent().extend_ttl(
            &acc_key,
            Self::SNAPSHOT_TTL_LEDGERS / 2,
            Self::SNAPSHOT_TTL_LEDGERS,
        );

        // Store the running cumulative (and the ledger time it corresponds to)
        // so a later query can difference two points of the integral.
        let snapshot = LiquiditySnapshot {
            cum_liquidity: cum,
            pool_ts: ledger_ts,
        };
        let key = DataKey::LiquiditySnapshot(pool.clone(), ledger_ts);
        env.storage().persistent().set(&key, &snapshot);
        env.storage().persistent().extend_ttl(
            &key,
            Self::SNAPSHOT_TTL_LEDGERS / 2,
            Self::SNAPSHOT_TTL_LEDGERS,
        );
        Self::register_tracked_pool(&env, &pool, PoolType::Cl);
        Self::record_snapshot_timestamp(&env, &pool, ledger_ts);
        Ok(())
    }

    /// Deletes a stored liquidity snapshot from persistent storage. Keeper-only.
    pub fn delete_snapshot(env: Env, pool: Address, ledger_ts: u64) -> Result<(), TwalError> {
        Self::require_keeper(&env)?;
        let key = DataKey::LiquiditySnapshot(pool.clone(), ledger_ts);
        if !env.storage().persistent().has(&key) {
            return Err(TwalError::NoSnapshotFound);
        }
        env.storage().persistent().remove(&key);
        Self::remove_snapshot_timestamp(&env, &pool, ledger_ts);
        env.events()
            .publish((Symbol::new(&env, "snapshot_deleted"),), (pool, ledger_ts));
        Ok(())
    }
    /// Returns the time-weighted average active liquidity for a CL pool over
    /// `window_seconds`.
    ///
    /// Differences the liquidity-cumulative of the floor snapshot (the most
    /// recent one at or before `now - window`) against the accumulator
    /// extrapolated to the current ledger time, then divides by the elapsed
    /// span. A pool whose active liquidity is constant at `L` correctly reports
    /// `L`; the previous implementation differenced two *instantaneous* readings
    /// and so reported a rate of change (0 for constant liquidity) with the
    /// wrong units (issue #462).
    pub fn get_cl_twal(env: Env, pool: Address, window_seconds: u64) -> i128 {
        assert!(window_seconds > 0, "window_seconds must be > 0");

        let ledger_ts_now = env.ledger().timestamp();
        assert!(
            ledger_ts_now >= window_seconds,
            "ledger timestamp is smaller than requested window"
        );

        let then_ts = ledger_ts_now - window_seconds;
        let floor_ts = Self::floor_snapshot_ts(&env, &pool, then_ts)
            .unwrap_or_else(|| panic!("no liquidity snapshot at or before {then_ts}"));
        let snapshot: LiquiditySnapshot = env
            .storage()
            .persistent()
            .get(&DataKey::LiquiditySnapshot(pool.clone(), floor_ts))
            .unwrap_or_else(|| panic!("missing liquidity snapshot at {floor_ts}"));

        let accumulator: ClLiquidityAccumulator = env
            .storage()
            .persistent()
            .get(&DataKey::ClAccumulator(pool))
            .unwrap_or_else(|| panic!("missing CL accumulator; call save_cl_snapshot first"));

        // Extrapolate the cumulative to now using the last recorded active
        // liquidity, mirroring how the AMM accumulator advances between checkpoints.
        let elapsed_since_update = ledger_ts_now.saturating_sub(accumulator.last_ts) as i128;
        let cum_now = accumulator.cum_liquidity + accumulator.last_active * elapsed_since_update;

        let elapsed = (ledger_ts_now - snapshot.pool_ts) as i128;
        assert!(elapsed > 0, "window too small (pool time did not advance)");
        (cum_now - snapshot.cum_liquidity) / elapsed
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

    #[test]
    fn test_zero_window_returns_error() {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set_timestamp(10_000);

        let admin = Address::generate(&env);
        let pool = Address::generate(&env);
        let consumer_addr = env.register_contract(None, TwalConsumer);
        let consumer = TwalConsumerClient::new(&env, &consumer_addr);
        consumer.initialize(&admin);

        assert_eq!(
            consumer.try_get_twal_liquidity(&pool, &0_u64),
            Err(Ok(TwalError::ZeroWindow))
        );
    }

    #[test]
    fn test_save_snapshot_requires_keeper_auth() {
        let env = Env::default();
        env.ledger().set_timestamp(10_000);

        let keeper = Address::generate(&env);
        let pool = Address::generate(&env);
        let consumer_addr = env.register_contract(None, TwalConsumer);
        let consumer = TwalConsumerClient::new(&env, &consumer_addr);
        consumer.initialize(&keeper);

        assert!(consumer.try_save_snapshot(&pool).is_err());
        assert!(consumer.try_save_cl_snapshot(&pool).is_err());
    }

    #[test]
    fn test_save_snapshot_fails_when_uninitialized() {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set_timestamp(10_000);

        let pool = Address::generate(&env);
        let consumer_addr = env.register_contract(None, TwalConsumer);
        let consumer = TwalConsumerClient::new(&env, &consumer_addr);

        assert!(consumer.try_save_snapshot(&pool).is_err());
    }

    #[test]
    fn test_initialize_is_idempotent_guard() {
        let env = Env::default();
        env.mock_all_auths();

        let keeper = Address::generate(&env);
        let consumer_addr = env.register_contract(None, TwalConsumer);
        let consumer = TwalConsumerClient::new(&env, &consumer_addr);

        consumer.initialize(&keeper);
        assert_eq!(consumer.get_keeper(), keeper);
        assert_eq!(
            consumer.try_initialize(&Address::generate(&env)),
            Err(Ok(TwalError::AlreadyInitialized))
        );
    }

    #[test]
    fn test_delete_snapshot_removes_liquidity_snapshot() {
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

        // Snapshot stored at ledger timestamp 10_000; deleting it removes it
        // (and its entry in the timestamp index) so a later TWAL query whose
        // window lands at or before that point has no floor snapshot left.
        consumer.delete_snapshot(&amm_addr, &10_000_u64);

        env.ledger().with_mut(|l| l.timestamp = 10_600);
        assert_eq!(
            consumer.try_get_twal_liquidity(&amm_addr, &600),
            Err(Ok(TwalError::InsufficientHistory))
        );
    }

    // ── Issue #469: arbitrary window_seconds should hit the nearest older
    // snapshot instead of requiring an exact-timestamp match ──────────────────

    #[test]
    fn test_get_twal_liquidity_with_arbitrary_window_uses_floor_snapshot() {
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

        // A second snapshot 30s later, after more liquidity is added.
        env.ledger().with_mut(|l| l.timestamp = 10_030);
        ta_sac.mint(&provider, &100_000_i128);
        tb_sac.mint(&provider, &100_000_i128);
        AmmPoolClient::new(&env, &amm_addr).add_liquidity(
            &provider,
            &100_000_i128,
            &100_000_i128,
            &0_i128,
            &u64::MAX,
        );
        consumer.save_snapshot(&amm_addr);

        // window=50 at ts=10_075 asks for then_ts=10_025, which has no exact
        // snapshot. Previously this reverted with NoSnapshotFound; now it
        // must fall back to the floor (the ts=10_000 snapshot).
        env.ledger().with_mut(|l| l.timestamp = 10_075);
        let twal = consumer.get_twal_liquidity(&amm_addr, &50);
        assert!(twal > 0);
    }

    #[test]
    fn test_delete_snapshot_requires_keeper_auth() {
        let env = Env::default();
        env.ledger().set_timestamp(10_000);

        let keeper = Address::generate(&env);
        let pool = Address::generate(&env);
        let consumer_addr = env.register_contract(None, TwalConsumer);
        let consumer = TwalConsumerClient::new(&env, &consumer_addr);
        consumer.initialize(&keeper);

        assert!(consumer.try_delete_snapshot(&pool, &10_000_u64).is_err());
    }

    // Minimal CL pool stand-in exposing a settable instantaneous active
    // liquidity, matching the `ClPoolLiquidityOracle::active_liquidity` method
    // the consumer reads.
    #[contract]
    pub struct MockClPool;

    #[contractimpl]
    impl MockClPool {
        pub fn set_liquidity(env: Env, value: i128) {
            env.storage()
                .instance()
                .set(&soroban_sdk::symbol_short!("liq"), &value);
        }

        pub fn active_liquidity(env: Env) -> i128 {
            env.storage()
                .instance()
                .get(&soroban_sdk::symbol_short!("liq"))
                .unwrap_or(0)
        }
    }

    #[test]
    fn test_cl_twal_constant_liquidity_reports_level() {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set_timestamp(10_000);

        let keeper = Address::generate(&env);
        let pool_addr = env.register_contract(None, MockClPool);
        let pool = MockClPoolClient::new(&env, &pool_addr);
        let consumer_addr = env.register_contract(None, TwalConsumer);
        let consumer = TwalConsumerClient::new(&env, &consumer_addr);
        consumer.initialize(&keeper);

        // Active liquidity is constant at 5_000 across the whole window.
        pool.set_liquidity(&5_000_i128);
        consumer.save_cl_snapshot(&pool_addr); // baseline accumulator at t=10_000

        env.ledger().with_mut(|l| l.timestamp = 10_600);
        consumer.save_cl_snapshot(&pool_addr); // accumulator advances by 5_000*600

        env.ledger().with_mut(|l| l.timestamp = 11_200);
        let twal = consumer.get_cl_twal(&pool_addr, &600);

        // Constant liquidity must report its level, not a rate of change (0).
        assert_eq!(twal, 5_000);
    }

    #[test]
    fn test_cl_twal_is_time_weighted_average_of_levels() {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set_timestamp(10_000);

        let keeper = Address::generate(&env);
        let pool_addr = env.register_contract(None, MockClPool);
        let pool = MockClPoolClient::new(&env, &pool_addr);
        let consumer_addr = env.register_contract(None, TwalConsumer);
        let consumer = TwalConsumerClient::new(&env, &consumer_addr);
        consumer.initialize(&keeper);

        // 1_000 for the first 300s, then 3_000 for the next 300s.
        pool.set_liquidity(&1_000_i128);
        consumer.save_cl_snapshot(&pool_addr);

        env.ledger().with_mut(|l| l.timestamp = 10_300);
        pool.set_liquidity(&3_000_i128);
        consumer.save_cl_snapshot(&pool_addr);

        env.ledger().with_mut(|l| l.timestamp = 10_600);
        consumer.save_cl_snapshot(&pool_addr);

        // Window covers both segments: (1_000*300 + 3_000*300) / 600 = 2_000.
        let twal = consumer.get_cl_twal(&pool_addr, &600);
        assert_eq!(twal, 2_000);
    }
}
