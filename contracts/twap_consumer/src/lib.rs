#![no_std]

use soroban_sdk::{
    contract, contractclient, contracterror, contractimpl, contracttype, symbol_short, Address,
    Env, Vec,
};

#[contractclient(name = "AmmPoolOracleClient")]
pub trait AmmPoolOracle {
    fn get_price_cumulative(env: Env) -> (i128, i128, u64);
}

#[contractclient(name = "ClPoolOracleClient")]
pub trait ClPoolOracle {
    fn get_tick_cumulative(env: Env) -> (i64, u64);
}

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum TwapError {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    ZeroWindow = 3,
    InsufficientHistory = 4,
    NoSnapshotFound = 5,
    ElapsedZero = 6,
    InvalidSpotPrice = 7,
    InvalidTwapPrice = 8,
    InvalidDeviationBps = 9,
    NegativeCollateral = 10,
    PriceManipulated = 11,
}

#[contracttype]
pub enum DataKey {
    Keeper,
    Snapshot(Address, u64),
    TrackedPoolsPersistent,
    /// Sorted (ascending, deduplicated) ledger timestamps at which a snapshot
    /// was saved for this pool. Lets `get_twap_*` binary-search for the most
    /// recent snapshot at or before an arbitrary `then_ts` instead of
    /// requiring an exact-timestamp hit (issue #469).
    SnapshotTimestamps(Address),
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PriceSnapshot {
    pub cum_a: i128,
    pub cum_b: i128,
    pub pool_ts: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PriceValidation {
    pub spot_price: i128,
    pub twap_price: i128,
    pub deviation_bps: i128,
    pub max_deviation_bps: i128,
    pub is_deviation: bool,
}

#[contract]
pub struct TwapConsumer;

#[contractimpl]
impl TwapConsumer {
    pub const SNAPSHOT_TTL_LEDGERS: u32 = 120_960;
    pub const BPS_DENOMINATOR: i128 = 10_000;
    pub const PRICE_SCALE: i128 = 1_000_000;

    pub fn initialize(env: Env, keeper: Address) -> Result<(), TwapError> {
        if env.storage().instance().has(&DataKey::Keeper) {
            return Err(TwapError::AlreadyInitialized);
        }
        env.storage().instance().set(&DataKey::Keeper, &keeper);
        Ok(())
    }

    pub fn get_keeper(env: Env) -> Result<Address, TwapError> {
        env.storage()
            .instance()
            .get(&DataKey::Keeper)
            .ok_or(TwapError::NotInitialized)
    }

    fn require_keeper(env: &Env) -> Result<(), TwapError> {
        Self::get_keeper(env.clone())?.require_auth();
        Ok(())
    }

    pub fn save_snapshot(env: Env, pool: Address) -> Result<(), TwapError> {
        Self::require_keeper(&env)?;
        let (cum_a, cum_b, pool_ts) = AmmPoolOracleClient::new(&env, &pool).get_price_cumulative();
        let ledger_ts = env.ledger().timestamp();
        let snapshot = PriceSnapshot {
            cum_a,
            cum_b,
            pool_ts,
        };
        let key = DataKey::Snapshot(pool.clone(), ledger_ts);
        env.storage().persistent().set(&key, &snapshot);
        env.storage().persistent().extend_ttl(
            &key,
            Self::SNAPSHOT_TTL_LEDGERS / 2,
            Self::SNAPSHOT_TTL_LEDGERS,
        );
        Self::record_snapshot_timestamp(&env, &pool, ledger_ts);

        let mut tracked: Vec<Address> = env
            .storage()
            .persistent()
            .get(&DataKey::TrackedPoolsPersistent)
            .unwrap_or_else(|| Vec::new(&env));
        let mut already_tracked = false;
        for i in 0..tracked.len() {
            if tracked.get(i).unwrap() == pool {
                already_tracked = true;
                break;
            }
        }
        if !already_tracked {
            tracked.push_back(pool);
            env.storage()
                .persistent()
                .set(&DataKey::TrackedPoolsPersistent, &tracked);
            env.storage().persistent().extend_ttl(
                &DataKey::TrackedPoolsPersistent,
                Self::SNAPSHOT_TTL_LEDGERS / 2,
                Self::SNAPSHOT_TTL_LEDGERS,
            );
        }
        Ok(())
    }

    /// Deletes a price snapshot from persistent storage.
    pub fn delete_snapshot(env: Env, pool: Address, ledger_ts: u64) -> Result<(), TwapError> {
        Self::require_keeper(&env)?;
        let key = DataKey::Snapshot(pool.clone(), ledger_ts);
        env.storage().persistent().remove(&key);
        Self::remove_snapshot_timestamp(&env, &pool, ledger_ts);
        env.events()
            .publish((symbol_short!("snap_del"), pool), ledger_ts);
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

    pub fn get_twap_price(env: Env, pool: Address, window_seconds: u64) -> Result<i128, TwapError> {
        if window_seconds == 0 {
            return Err(TwapError::ZeroWindow);
        }
        let (cum_a_now, _cum_b_now, pool_ts_now) =
            AmmPoolOracleClient::new(&env, &pool).get_price_cumulative();
        let ledger_ts_now = env.ledger().timestamp();
        if ledger_ts_now < window_seconds {
            return Err(TwapError::InsufficientHistory);
        }
        let then_ts = ledger_ts_now - window_seconds;
        let floor_ts =
            Self::floor_snapshot_ts(&env, &pool, then_ts).ok_or(TwapError::InsufficientHistory)?;
        let snapshot: PriceSnapshot = env
            .storage()
            .persistent()
            .get(&DataKey::Snapshot(pool.clone(), floor_ts))
            .ok_or(TwapError::NoSnapshotFound)?;

        let delta_a = (cum_a_now as u128).wrapping_sub(snapshot.cum_a as u128) as i128;
        let elapsed = (pool_ts_now - snapshot.pool_ts) as i128;
        if elapsed <= 0 {
            return Err(TwapError::ElapsedZero);
        }
        Ok(delta_a / elapsed)
    }

    pub fn validate_price(
        spot_price: i128,
        twap_price: i128,
        max_deviation_bps: i128,
    ) -> Result<PriceValidation, TwapError> {
        if spot_price <= 0 {
            return Err(TwapError::InvalidSpotPrice);
        }
        if twap_price <= 0 {
            return Err(TwapError::InvalidTwapPrice);
        }
        if !(0..=Self::BPS_DENOMINATOR).contains(&max_deviation_bps) {
            return Err(TwapError::InvalidDeviationBps);
        }
        let price_delta = if spot_price >= twap_price {
            spot_price - twap_price
        } else {
            twap_price - spot_price
        };
        let deviation_bps = price_delta * Self::BPS_DENOMINATOR / twap_price;
        Ok(PriceValidation {
            spot_price,
            twap_price,
            deviation_bps,
            max_deviation_bps,
            is_deviation: deviation_bps > max_deviation_bps,
        })
    }

    pub fn validate_price_against_twap(
        env: Env,
        pool: Address,
        window_seconds: u64,
        spot_price: i128,
        max_deviation_bps: i128,
    ) -> Result<PriceValidation, TwapError> {
        let twap_price = Self::get_twap_price(env, pool, window_seconds)?;
        Self::validate_price(spot_price, twap_price, max_deviation_bps)
    }

    pub fn assert_lending_price_safe(
        env: Env,
        pool: Address,
        window_seconds: u64,
        spot_price: i128,
        max_deviation_bps: i128,
        collateral_amount: i128,
    ) -> Result<i128, TwapError> {
        if collateral_amount < 0 {
            return Err(TwapError::NegativeCollateral);
        }
        let validation = Self::validate_price_against_twap(
            env,
            pool,
            window_seconds,
            spot_price,
            max_deviation_bps,
        )?;
        if validation.is_deviation {
            return Err(TwapError::PriceManipulated);
        }
        Ok(collateral_amount * validation.spot_price / Self::PRICE_SCALE)
    }

    pub fn get_twap_both(
        env: Env,
        pool: Address,
        window_seconds: u64,
    ) -> Result<(i128, i128), TwapError> {
        if window_seconds == 0 {
            return Err(TwapError::ZeroWindow);
        }
        let (cum_a_now, cum_b_now, pool_ts_now) =
            AmmPoolOracleClient::new(&env, &pool).get_price_cumulative();
        let ledger_ts_now = env.ledger().timestamp();
        if ledger_ts_now < window_seconds {
            return Err(TwapError::InsufficientHistory);
        }
        let then_ts = ledger_ts_now - window_seconds;
        let floor_ts =
            Self::floor_snapshot_ts(&env, &pool, then_ts).ok_or(TwapError::InsufficientHistory)?;
        let snapshot: PriceSnapshot = env
            .storage()
            .persistent()
            .get(&DataKey::Snapshot(pool.clone(), floor_ts))
            .ok_or(TwapError::NoSnapshotFound)?;

        let delta_a = (cum_a_now as u128).wrapping_sub(snapshot.cum_a as u128) as i128;
        let delta_b = (cum_b_now as u128).wrapping_sub(snapshot.cum_b as u128) as i128;
        let elapsed = (pool_ts_now - snapshot.pool_ts) as i128;
        if elapsed <= 0 {
            return Err(TwapError::ElapsedZero);
        }
        Ok((delta_a / elapsed, delta_b / elapsed))
    }

    pub fn get_tracked_pools(env: Env) -> Vec<Address> {
        env.storage()
            .persistent()
            .get(&DataKey::TrackedPoolsPersistent)
            .unwrap_or_else(|| Vec::new(&env))
    }

    pub fn get_twap_all(env: Env, window_seconds: u64) -> Result<Vec<(Address, i128)>, TwapError> {
        let tracked: Vec<Address> = Self::get_tracked_pools(env.clone());
        let mut results: Vec<(Address, i128)> = Vec::new(&env);
        for i in 0..tracked.len() {
            let pool = tracked.get(i).unwrap();
            let twap = Self::get_twap_price(env.clone(), pool.clone(), window_seconds)?;
            results.push_back((pool, twap));
        }
        Ok(results)
    }

    pub fn get_cl_twap(env: Env, pool: Address, window_seconds: u64) -> Result<i64, TwapError> {
        if window_seconds == 0 {
            return Err(TwapError::ZeroWindow);
        }
        let (cum_now, last_ts_now) = ClPoolOracleClient::new(&env, &pool).get_tick_cumulative();
        let ledger_ts_now = env.ledger().timestamp();
        if ledger_ts_now < window_seconds {
            return Err(TwapError::InsufficientHistory);
        }
        let then_ts = ledger_ts_now - window_seconds;
        let floor_ts =
            Self::floor_snapshot_ts(&env, &pool, then_ts).ok_or(TwapError::InsufficientHistory)?;
        let snapshot: PriceSnapshot = env
            .storage()
            .persistent()
            .get(&DataKey::Snapshot(pool.clone(), floor_ts))
            .ok_or(TwapError::NoSnapshotFound)?;

        let cum_then = snapshot.cum_a as i64;
        let elapsed_pool = (last_ts_now - snapshot.pool_ts) as i64;
        if elapsed_pool <= 0 {
            return Err(TwapError::ElapsedZero);
        }
        Ok((cum_now - cum_then) / elapsed_pool)
    }

    pub fn save_cl_snapshot(env: Env, pool: Address) -> Result<(), TwapError> {
        Self::require_keeper(&env)?;
        let (tick_cum, pool_ts) = ClPoolOracleClient::new(&env, &pool).get_tick_cumulative();
        let ledger_ts = env.ledger().timestamp();
        let snapshot = PriceSnapshot {
            cum_a: tick_cum as i128,
            cum_b: 0,
            pool_ts,
        };
        let key = DataKey::Snapshot(pool.clone(), ledger_ts);
        env.storage().persistent().set(&key, &snapshot);
        env.storage().persistent().extend_ttl(
            &key,
            Self::SNAPSHOT_TTL_LEDGERS / 2,
            Self::SNAPSHOT_TTL_LEDGERS,
        );
        Self::record_snapshot_timestamp(&env, &pool, ledger_ts);

        // Fix #537: register the CL pool in TrackedPoolsPersistent so that
        // get_tracked_pools() and get_twap_all() include it. Previously only
        // save_snapshot (V2 pools) performed this registration; CL pools
        // snapshotted exclusively via save_cl_snapshot were silently omitted.
        let mut tracked: Vec<Address> = env
            .storage()
            .persistent()
            .get(&DataKey::TrackedPoolsPersistent)
            .unwrap_or_else(|| Vec::new(&env));
        let mut already_tracked = false;
        for i in 0..tracked.len() {
            if tracked.get(i).unwrap() == pool {
                already_tracked = true;
                break;
            }
        }
        if !already_tracked {
            tracked.push_back(pool);
            env.storage()
                .persistent()
                .set(&DataKey::TrackedPoolsPersistent, &tracked);
            env.storage().persistent().extend_ttl(
                &DataKey::TrackedPoolsPersistent,
                Self::SNAPSHOT_TTL_LEDGERS / 2,
                Self::SNAPSHOT_TTL_LEDGERS,
            );
        }
        Ok(())
    }
}

/// Minimal mock CL pool used by tests only. Satisfies the `ClPoolOracle`
/// interface (`get_tick_cumulative`) without requiring the full CL contract.
#[cfg(test)]
mod mock_cl_pool {
    use soroban_sdk::{contract, contractimpl, Env};

    #[contract]
    pub struct MockClPool;

    #[contractimpl]
    impl MockClPool {
        pub fn get_tick_cumulative(_env: Env) -> (i64, u64) {
            (1_000_i64, 10_000_u64)
        }
    }
}

#[cfg(test)]
use mock_cl_pool::MockClPool;

#[cfg(test)]
mod tests {
    use super::*;

    use amm::{AmmPool, AmmPoolClient};
    use soroban_sdk::{
        testutils::{Address as _, Events as _, Ledger},
        token::{StellarAssetClient, TokenClient as StellarTokenClient},
        Address, Env, IntoVal,
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

    fn setup_pool_and_consumer<'a>(
        env: &'a Env,
        admin: &Address,
        reserve_a: i128,
        reserve_b: i128,
    ) -> (Address, TwapConsumerClient<'a>) {
        let amm_addr = env.register_contract(None, AmmPool);
        let lp_addr = env.register_contract(None, LpToken);
        let consumer_addr = env.register_contract(None, TwapConsumer);

        token::LpTokenClient::new(env, &lp_addr).initialize(
            &amm_addr,
            &soroban_sdk::String::from_str(env, "AMM LP Token"),
            &soroban_sdk::String::from_str(env, "ALP"),
            &7u32,
        );

        let (ta, ta_sac) = create_sac(env, admin);
        let (tb, tb_sac) = create_sac(env, admin);

        let amm = AmmPoolClient::new(env, &amm_addr);
        amm.initialize(
            admin,
            &ta.address,
            &tb.address,
            &lp_addr,
            &30_i128,
            admin,
            &0_i128,
        );

        let provider = Address::generate(env);
        ta_sac.mint(&provider, &reserve_a);
        tb_sac.mint(&provider, &reserve_b);
        amm.add_liquidity(&provider, &reserve_a, &reserve_b, &0_i128, &u64::MAX);

        let consumer = TwapConsumerClient::new(env, &consumer_addr);
        consumer.initialize(admin);
        consumer.save_snapshot(&amm_addr);

        (amm_addr, consumer)
    }

    #[test]
    fn test_get_twap_price_diverges_from_spot_after_large_trade() {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set_timestamp(10_000);

        let admin = Address::generate(&env);
        let amm_addr = env.register_contract(None, AmmPool);
        let lp_addr = env.register_contract(None, LpToken);
        let consumer_addr = env.register_contract(None, TwapConsumer);

        token::LpTokenClient::new(&env, &lp_addr).initialize(
            &amm_addr,
            &soroban_sdk::String::from_str(&env, "AMM LP Token"),
            &soroban_sdk::String::from_str(&env, "ALP"),
            &7u32,
        );

        let (ta, ta_sac) = create_sac(&env, &admin);
        let (tb, tb_sac) = create_sac(&env, &admin);

        let amm = AmmPoolClient::new(&env, &amm_addr);
        amm.initialize(
            &admin,
            &ta.address,
            &tb.address,
            &lp_addr,
            &30_i128,
            &admin,
            &0_i128,
        );

        let provider = Address::generate(&env);
        ta_sac.mint(&provider, &2_000_000_i128);
        tb_sac.mint(&provider, &2_000_000_i128);
        amm.add_liquidity(
            &provider,
            &2_000_000_i128,
            &2_000_000_i128,
            &0_i128,
            &10_000_u64,
        );

        let consumer = TwapConsumerClient::new(&env, &consumer_addr);
        consumer.initialize(&admin);
        consumer.save_snapshot(&amm_addr);

        env.ledger().set_timestamp(10_060);
        let whale = Address::generate(&env);
        ta_sac.mint(&whale, &1_000_000_i128);
        amm.swap(&whale, &ta.address, &1_000_000_i128, &0_i128, &10_060_u64);

        let twap = consumer.get_twap_price(&amm_addr, &60_u64);
        let (spot_a, _spot_b) = amm.price_ratio();

        assert_eq!(twap, 1_000_000);
        assert!(twap > spot_a);
        assert_ne!(twap, spot_a);
    }

    #[test]
    fn test_validate_price_against_twap_flags_large_deviation() {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set_timestamp(10_000);

        let admin = Address::generate(&env);
        let amm_addr = env.register_contract(None, AmmPool);
        let lp_addr = env.register_contract(None, LpToken);
        let consumer_addr = env.register_contract(None, TwapConsumer);

        token::LpTokenClient::new(&env, &lp_addr).initialize(
            &amm_addr,
            &soroban_sdk::String::from_str(&env, "AMM LP Token"),
            &soroban_sdk::String::from_str(&env, "ALP"),
            &7u32,
        );

        let (ta, ta_sac) = create_sac(&env, &admin);
        let (tb, tb_sac) = create_sac(&env, &admin);

        let amm = AmmPoolClient::new(&env, &amm_addr);
        amm.initialize(
            &admin,
            &ta.address,
            &tb.address,
            &lp_addr,
            &30_i128,
            &admin,
            &0_i128,
        );

        let provider = Address::generate(&env);
        ta_sac.mint(&provider, &2_000_000_i128);
        tb_sac.mint(&provider, &2_000_000_i128);
        amm.add_liquidity(
            &provider,
            &2_000_000_i128,
            &2_000_000_i128,
            &0_i128,
            &10_000_u64,
        );

        let consumer = TwapConsumerClient::new(&env, &consumer_addr);
        consumer.initialize(&admin);
        consumer.save_snapshot(&amm_addr);

        env.ledger().set_timestamp(10_060);
        let whale = Address::generate(&env);
        ta_sac.mint(&whale, &1_000_000_i128);
        amm.swap(&whale, &ta.address, &1_000_000_i128, &0_i128, &10_060_u64);

        let (spot_a, _spot_b) = amm.price_ratio();
        let validation =
            consumer.validate_price_against_twap(&amm_addr, &60_u64, &spot_a, &500_i128);

        assert_eq!(validation.twap_price, 1_000_000);
        assert_eq!(validation.max_deviation_bps, 500);
        assert!(validation.deviation_bps > 500);
        assert!(validation.is_deviation);
    }

    #[test]
    fn test_lending_helper_accepts_safe_price_and_rejects_manipulated_price() {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set_timestamp(10_000);

        let admin = Address::generate(&env);
        let amm_addr = env.register_contract(None, AmmPool);
        let lp_addr = env.register_contract(None, LpToken);
        let consumer_addr = env.register_contract(None, TwapConsumer);

        token::LpTokenClient::new(&env, &lp_addr).initialize(
            &amm_addr,
            &soroban_sdk::String::from_str(&env, "AMM LP Token"),
            &soroban_sdk::String::from_str(&env, "ALP"),
            &7u32,
        );

        let (ta, ta_sac) = create_sac(&env, &admin);
        let (tb, tb_sac) = create_sac(&env, &admin);

        let amm = AmmPoolClient::new(&env, &amm_addr);
        amm.initialize(
            &admin,
            &ta.address,
            &tb.address,
            &lp_addr,
            &30_i128,
            &admin,
            &0_i128,
        );

        let provider = Address::generate(&env);
        ta_sac.mint(&provider, &2_000_000_i128);
        tb_sac.mint(&provider, &2_000_000_i128);
        amm.add_liquidity(
            &provider,
            &2_000_000_i128,
            &2_000_000_i128,
            &0_i128,
            &10_000_u64,
        );

        let consumer = TwapConsumerClient::new(&env, &consumer_addr);
        consumer.initialize(&admin);
        consumer.save_snapshot(&amm_addr);

        env.ledger().set_timestamp(10_060);
        let trader = Address::generate(&env);
        ta_sac.mint(&trader, &1_000_i128);
        amm.swap(&trader, &ta.address, &1_000_i128, &0_i128, &10_060_u64);

        let (safe_spot, _spot_b) = amm.price_ratio();
        let collateral_value = consumer.assert_lending_price_safe(
            &amm_addr,
            &60_u64,
            &safe_spot,
            &500_i128,
            &3_000_000_i128,
        );
        assert!(collateral_value > 0);

        let result = consumer.try_assert_lending_price_safe(
            &amm_addr,
            &60_u64,
            &600_000_i128,
            &500_i128,
            &3_000_000_i128,
        );
        assert_eq!(result, Err(Ok(TwapError::PriceManipulated)));
    }

    #[test]
    fn test_get_twap_both() {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set_timestamp(10_000);

        let admin = Address::generate(&env);
        let amm_addr = env.register_contract(None, AmmPool);
        let lp_addr = env.register_contract(None, LpToken);
        let consumer_addr = env.register_contract(None, TwapConsumer);

        token::LpTokenClient::new(&env, &lp_addr).initialize(
            &amm_addr,
            &soroban_sdk::String::from_str(&env, "AMM LP Token"),
            &soroban_sdk::String::from_str(&env, "ALP"),
            &7u32,
        );

        let (ta, ta_sac) = create_sac(&env, &admin);
        let (tb, tb_sac) = create_sac(&env, &admin);

        let amm = AmmPoolClient::new(&env, &amm_addr);
        amm.initialize(
            &admin,
            &ta.address,
            &tb.address,
            &lp_addr,
            &30_i128,
            &admin,
            &0_i128,
        );

        let provider = Address::generate(&env);
        ta_sac.mint(&provider, &2_000_000_i128);
        tb_sac.mint(&provider, &2_000_000_i128);
        amm.add_liquidity(
            &provider,
            &2_000_000_i128,
            &2_000_000_i128,
            &0_i128,
            &10_000_u64,
        );

        let consumer = TwapConsumerClient::new(&env, &consumer_addr);
        consumer.initialize(&admin);
        consumer.save_snapshot(&amm_addr);

        env.ledger().set_timestamp(10_060);
        let whale = Address::generate(&env);
        ta_sac.mint(&whale, &1_000_i128);
        amm.swap(&whale, &ta.address, &1_000_i128, &0_i128, &10_060_u64);

        let (twap_a_to_b, twap_b_to_a) = consumer.get_twap_both(&amm_addr, &60_u64);
        assert_eq!(twap_a_to_b, 1_000_000);
        assert_eq!(twap_b_to_a, 1_000_000);
    }

    #[test]
    fn test_get_twap_both_with_imbalance() {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set_timestamp(10_000);

        let admin = Address::generate(&env);
        let amm_addr = env.register_contract(None, AmmPool);
        let lp_addr = env.register_contract(None, LpToken);
        let consumer_addr = env.register_contract(None, TwapConsumer);

        token::LpTokenClient::new(&env, &lp_addr).initialize(
            &amm_addr,
            &soroban_sdk::String::from_str(&env, "AMM LP Token"),
            &soroban_sdk::String::from_str(&env, "ALP"),
            &7u32,
        );

        let (ta, ta_sac) = create_sac(&env, &admin);
        let (tb, tb_sac) = create_sac(&env, &admin);

        let amm = AmmPoolClient::new(&env, &amm_addr);
        amm.initialize(
            &admin,
            &ta.address,
            &tb.address,
            &lp_addr,
            &30_i128,
            &admin,
            &0_i128,
        );

        let provider = Address::generate(&env);
        ta_sac.mint(&provider, &2_000_000_i128);
        tb_sac.mint(&provider, &4_000_000_i128);
        amm.add_liquidity(
            &provider,
            &2_000_000_i128,
            &4_000_000_i128,
            &0_i128,
            &10_000_u64,
        );

        let consumer = TwapConsumerClient::new(&env, &consumer_addr);
        consumer.initialize(&admin);
        consumer.save_snapshot(&amm_addr);

        env.ledger().set_timestamp(10_060);
        let whale = Address::generate(&env);
        ta_sac.mint(&whale, &1_000_i128);
        amm.swap(&whale, &ta.address, &1_000_i128, &0_i128, &10_060_u64);

        let (twap_a_to_b, twap_b_to_a) = consumer.get_twap_both(&amm_addr, &60_u64);
        assert_eq!(twap_a_to_b, 2_000_000);
        assert_eq!(twap_b_to_a, 500_000);
    }

    #[test]
    fn test_get_tracked_pools_and_twap_all() {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set_timestamp(10_000);

        let admin = Address::generate(&env);

        let amm_addr1 = env.register_contract(None, AmmPool);
        let lp_addr1 = env.register_contract(None, LpToken);
        token::LpTokenClient::new(&env, &lp_addr1).initialize(
            &amm_addr1,
            &soroban_sdk::String::from_str(&env, "LP1"),
            &soroban_sdk::String::from_str(&env, "LP1"),
            &7u32,
        );
        let (ta1, ta1_sac) = create_sac(&env, &admin);
        let (tb1, tb1_sac) = create_sac(&env, &admin);
        let amm1 = AmmPoolClient::new(&env, &amm_addr1);
        amm1.initialize(
            &admin,
            &ta1.address,
            &tb1.address,
            &lp_addr1,
            &30_i128,
            &admin,
            &0_i128,
        );
        let p1 = Address::generate(&env);
        ta1_sac.mint(&p1, &2_000_000_i128);
        tb1_sac.mint(&p1, &2_000_000_i128);
        amm1.add_liquidity(&p1, &2_000_000_i128, &2_000_000_i128, &0_i128, &10_000_u64);

        let amm_addr2 = env.register_contract(None, AmmPool);
        let lp_addr2 = env.register_contract(None, LpToken);
        token::LpTokenClient::new(&env, &lp_addr2).initialize(
            &amm_addr2,
            &soroban_sdk::String::from_str(&env, "LP2"),
            &soroban_sdk::String::from_str(&env, "LP2"),
            &7u32,
        );
        let (ta2, ta2_sac) = create_sac(&env, &admin);
        let (tb2, tb2_sac) = create_sac(&env, &admin);
        let amm2 = AmmPoolClient::new(&env, &amm_addr2);
        amm2.initialize(
            &admin,
            &ta2.address,
            &tb2.address,
            &lp_addr2,
            &30_i128,
            &admin,
            &0_i128,
        );
        let p2 = Address::generate(&env);
        ta2_sac.mint(&p2, &2_000_000_i128);
        tb2_sac.mint(&p2, &4_000_000_i128);
        amm2.add_liquidity(&p2, &2_000_000_i128, &4_000_000_i128, &0_i128, &10_000_u64);

        let consumer_addr = env.register_contract(None, TwapConsumer);
        let consumer = TwapConsumerClient::new(&env, &consumer_addr);
        consumer.initialize(&admin);
        consumer.save_snapshot(&amm_addr1);
        consumer.save_snapshot(&amm_addr2);

        let tracked = consumer.get_tracked_pools();
        assert_eq!(tracked.len(), 2);
        assert!(tracked.contains(&amm_addr1));
        assert!(tracked.contains(&amm_addr2));

        consumer.save_snapshot(&amm_addr1);
        assert_eq!(consumer.get_tracked_pools().len(), 2);

        env.ledger().set_timestamp(10_060);
        let whale1 = Address::generate(&env);
        ta1_sac.mint(&whale1, &1_000_i128);
        amm1.swap(&whale1, &ta1.address, &1_000_i128, &0_i128, &10_060_u64);
        let whale2 = Address::generate(&env);
        ta2_sac.mint(&whale2, &1_000_i128);
        amm2.swap(&whale2, &ta2.address, &1_000_i128, &0_i128, &10_060_u64);

        let all_twaps = consumer.get_twap_all(&60_u64);
        assert_eq!(all_twaps.len(), 2);

        let twap1 = consumer.get_twap_price(&amm_addr1, &60_u64);
        assert_eq!(twap1, 1_000_000);
        let twap2 = consumer.get_twap_price(&amm_addr2, &60_u64);
        assert_eq!(twap2, 2_000_000);

        for i in 0..all_twaps.len() {
            let (pool, twap) = all_twaps.get(i).unwrap();
            if pool == amm_addr1 {
                assert_eq!(twap, twap1);
            } else {
                assert_eq!(twap, twap2);
            }
        }
    }

    #[test]
    fn test_delete_snapshot() {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set_timestamp(10_000);

        let admin = Address::generate(&env);
        let amm_addr = env.register_contract(None, AmmPool);
        let lp_addr = env.register_contract(None, LpToken);
        let consumer_addr = env.register_contract(None, TwapConsumer);

        token::LpTokenClient::new(&env, &lp_addr).initialize(
            &amm_addr,
            &soroban_sdk::String::from_str(&env, "AMM LP Token"),
            &soroban_sdk::String::from_str(&env, "ALP"),
            &7u32,
        );

        let (ta, ta_sac) = create_sac(&env, &admin);
        let (tb, tb_sac) = create_sac(&env, &admin);

        let amm = AmmPoolClient::new(&env, &amm_addr);
        amm.initialize(
            &admin,
            &ta.address,
            &tb.address,
            &lp_addr,
            &30_i128,
            &admin,
            &0_i128,
        );

        let provider = Address::generate(&env);
        ta_sac.mint(&provider, &2_000_000_i128);
        tb_sac.mint(&provider, &2_000_000_i128);
        amm.add_liquidity(
            &provider,
            &2_000_000_i128,
            &2_000_000_i128,
            &0_i128,
            &10_000_u64,
        );

        let consumer = TwapConsumerClient::new(&env, &consumer_addr);
        consumer.initialize(&admin);
        consumer.save_snapshot(&amm_addr);

        env.ledger().set_timestamp(10_060);
        let whale = Address::generate(&env);
        ta_sac.mint(&whale, &1_000_i128);
        amm.swap(&whale, &ta.address, &1_000_i128, &0_i128, &10_060_u64);
        assert_eq!(consumer.get_twap_price(&amm_addr, &60_u64), 1_000_000);

        consumer.delete_snapshot(&amm_addr, &10_000);

        // The only snapshot at/before then_ts=10_000 was removed from the
        // timestamp index along with the snapshot itself, so there is no
        // floor entry left — this is now InsufficientHistory rather than
        // NoSnapshotFound (issue #469).
        let result = consumer.try_get_twap_price(&amm_addr, &60_u64);
        assert_eq!(result, Err(Ok(TwapError::InsufficientHistory)));
    }

    // ── Issue #469: arbitrary window_seconds should hit the nearest older
    // snapshot instead of requiring an exact-timestamp match ──────────────────

    #[test]
    fn test_get_twap_price_with_arbitrary_window_uses_floor_snapshot() {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set_timestamp(10_000);

        let admin = Address::generate(&env);
        let amm_addr = env.register_contract(None, AmmPool);
        let lp_addr = env.register_contract(None, LpToken);
        let consumer_addr = env.register_contract(None, TwapConsumer);

        token::LpTokenClient::new(&env, &lp_addr).initialize(
            &amm_addr,
            &soroban_sdk::String::from_str(&env, "AMM LP Token"),
            &soroban_sdk::String::from_str(&env, "ALP"),
            &7u32,
        );

        let (ta, ta_sac) = create_sac(&env, &admin);
        let (tb, tb_sac) = create_sac(&env, &admin);

        let amm = AmmPoolClient::new(&env, &amm_addr);
        amm.initialize(
            &admin,
            &ta.address,
            &tb.address,
            &lp_addr,
            &30_i128,
            &admin,
            &0_i128,
        );

        let provider = Address::generate(&env);
        ta_sac.mint(&provider, &2_000_000_i128);
        tb_sac.mint(&provider, &2_000_000_i128);
        amm.add_liquidity(
            &provider,
            &2_000_000_i128,
            &2_000_000_i128,
            &0_i128,
            &10_000_u64,
        );

        let consumer = TwapConsumerClient::new(&env, &consumer_addr);
        consumer.initialize(&admin);
        consumer.save_snapshot(&amm_addr);

        // A trade moves the pool's cumulative price, then a second snapshot
        // is saved 30s after the first (at ts=10_030).
        env.ledger().set_timestamp(10_030);
        let whale = Address::generate(&env);
        ta_sac.mint(&whale, &2_000_000_i128);
        amm.swap(&whale, &ta.address, &1_000_000_i128, &0_i128, &10_030_u64);
        consumer.save_snapshot(&amm_addr);

        // A second trade 45s later advances the pool's own cumulative
        // timestamp without a matching snapshot, so both queries below have
        // a positive `elapsed` against their respective floor snapshot.
        env.ledger().set_timestamp(10_075);
        amm.swap(&whale, &ta.address, &1_000_000_i128, &0_i128, &10_075_u64);

        // Query with window=45 at ts=10_075: then_ts = 10_030, an exact hit on
        // the second snapshot, exercising the common case handled today.
        let exact_hit = consumer.get_twap_price(&amm_addr, &45_u64);
        assert!(exact_hit > 0);

        // Query with window=50 at ts=10_075: then_ts = 10_025, which has no
        // exact snapshot. Previously this reverted with NoSnapshotFound; now
        // it must fall back to the floor (the ts=10_000 snapshot) instead.
        let floor_hit = consumer.get_twap_price(&amm_addr, &50_u64);
        assert!(floor_hit > 0);
    }

    #[test]
    fn test_zero_window_returns_error() {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set_timestamp(10_000);

        let admin = Address::generate(&env);
        let pool = Address::generate(&env);
        let consumer_addr = env.register_contract(None, TwapConsumer);
        let consumer = TwapConsumerClient::new(&env, &consumer_addr);
        consumer.initialize(&admin);

        assert_eq!(
            consumer.try_get_twap_price(&pool, &0_u64),
            Err(Ok(TwapError::ZeroWindow))
        );
        assert_eq!(
            consumer.try_get_twap_both(&pool, &0_u64),
            Err(Ok(TwapError::ZeroWindow))
        );
        assert_eq!(
            consumer.try_get_cl_twap(&pool, &0_u64),
            Err(Ok(TwapError::ZeroWindow))
        );
    }

    #[test]
    fn test_save_snapshot_requires_keeper_auth() {
        let env = Env::default();
        env.ledger().set_timestamp(10_000);

        let keeper = Address::generate(&env);
        let pool = Address::generate(&env);
        let consumer_addr = env.register_contract(None, TwapConsumer);
        let consumer = TwapConsumerClient::new(&env, &consumer_addr);
        consumer.initialize(&keeper);

        assert!(consumer.try_save_snapshot(&pool).is_err());
        assert!(consumer.try_delete_snapshot(&pool, &10_000).is_err());
        assert!(consumer.try_save_cl_snapshot(&pool).is_err());
    }

    #[test]
    fn test_save_snapshot_fails_when_uninitialized() {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set_timestamp(10_000);

        let pool = Address::generate(&env);
        let consumer_addr = env.register_contract(None, TwapConsumer);
        let consumer = TwapConsumerClient::new(&env, &consumer_addr);

        assert!(consumer.try_save_snapshot(&pool).is_err());
    }

    #[test]
    fn test_initialize_is_idempotent_guard() {
        let env = Env::default();
        env.mock_all_auths();

        let keeper = Address::generate(&env);
        let consumer_addr = env.register_contract(None, TwapConsumer);
        let consumer = TwapConsumerClient::new(&env, &consumer_addr);

        consumer.initialize(&keeper);
        assert_eq!(consumer.get_keeper(), keeper);
        assert_eq!(
            consumer.try_initialize(&Address::generate(&env)),
            Err(Ok(TwapError::AlreadyInitialized))
        );
    }

    #[test]
    fn test_delete_snapshot_emits_event() {
        let env = Env::default();
        env.mock_all_auths();
        let keeper = Address::generate(&env);
        let pool = Address::generate(&env);
        let ledger_ts = 100u64;
        let consumer_addr = env.register_contract(None, TwapConsumer);
        let consumer = TwapConsumerClient::new(&env, &consumer_addr);
        consumer.initialize(&keeper);

        consumer.delete_snapshot(&pool, &ledger_ts);

        let events = env.events().all();
        let event = events.last().unwrap();
        let (contract_id, topics, data) = event;

        assert_eq!(contract_id, consumer_addr);
        // topics are (symbol_short!("snap_del"), pool)
        let mut expected_topics: Vec<soroban_sdk::Val> = Vec::new(&env);
        expected_topics.push_back(symbol_short!("snap_del").into_val(&env));
        expected_topics.push_back(pool.clone().into_val(&env));
        assert_eq!(topics, expected_topics);
        // data is ledger_ts
        let data_ts: u64 = data.into_val(&env);
        assert_eq!(data_ts, ledger_ts);
    }

    // ── Issue #537: save_cl_snapshot must register the pool in
    // TrackedPoolsPersistent so get_tracked_pools / get_twap_all include it ──

    #[test]
    fn test_save_cl_snapshot_registers_pool_in_tracked_pools() {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set_timestamp(10_000);

        let admin = Address::generate(&env);
        let cl_addr = env.register_contract(None, MockClPool);

        let consumer_addr = env.register_contract(None, TwapConsumer);
        let consumer = TwapConsumerClient::new(&env, &consumer_addr);
        consumer.initialize(&admin);

        // Before any CL snapshot, the tracked list must be empty.
        assert_eq!(
            consumer.get_tracked_pools().len(),
            0,
            "no pools should be tracked before first snapshot"
        );

        // A single save_cl_snapshot call must register the pool (fix #537).
        consumer.save_cl_snapshot(&cl_addr);

        let tracked = consumer.get_tracked_pools();
        assert_eq!(tracked.len(), 1, "CL pool must appear in tracked pools");
        assert!(
            tracked.contains(&cl_addr),
            "tracked pools must contain the CL pool address"
        );

        // Calling save_cl_snapshot again (different ledger timestamp) must not
        // create a duplicate entry in TrackedPoolsPersistent.
        env.ledger().set_timestamp(10_060);
        consumer.save_cl_snapshot(&cl_addr);
        assert_eq!(
            consumer.get_tracked_pools().len(),
            1,
            "duplicate registration must be suppressed"
        );
    }
}
