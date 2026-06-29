#![no_std]

//! TWAL (time-weighted average liquidity) consumer contract.

use soroban_sdk::{contract, contractclient, contractimpl, contracterror, contracttype, Address, Env, Symbol, Vec};

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
        let snapshot = LiquiditySnapshot { cum_liquidity: cum, pool_ts };
        let key = DataKey::LiquiditySnapshot(pool.clone(), ledger_ts);
        env.storage().persistent().set(&key, &snapshot);
        env.storage().persistent().extend_ttl(
            &key,
            Self::SNAPSHOT_TTL_LEDGERS / 2,
            Self::SNAPSHOT_TTL_LEDGERS,
        );
        Self::register_tracked_pool(&env, &pool);
        Ok(())
    }

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

    pub fn get_twal_liquidity(env: Env, pool: Address, window_seconds: u64) -> Result<i128, TwalError> {
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
        let snapshot: LiquiditySnapshot = env
            .storage()
            .persistent()
            .get(&DataKey::LiquiditySnapshot(pool, then_ts))
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
            let pool = tracked.get(i).unwrap();
            let twal = Self::get_twal_liquidity(env.clone(), pool.clone(), window_seconds)?;
            results.push_back((pool, twal));
        }
        Ok(results)
    }

    pub fn get_tracked_pools(env: Env) -> Vec<Address> {
        env.storage()
            .persistent()
            .get(&DataKey::TrackedPoolsPersistent)
            .unwrap_or_else(|| Vec::new(&env))
    }

    pub fn save_cl_snapshot(env: Env, pool: Address) -> Result<(), TwalError> {
        Self::require_keeper(&env)?;
        let active = ClPoolLiquidityClient::new(&env, &pool).active_liquidity();
        let (_tick_cum, pool_ts) = ClPoolLiquidityClient::new(&env, &pool).get_tick_cumulative();
        let ledger_ts = env.ledger().timestamp();
        let snapshot = LiquiditySnapshot { cum_liquidity: active, pool_ts };
        let key = DataKey::LiquiditySnapshot(pool.clone(), ledger_ts);
        env.storage().persistent().set(&key, &snapshot);
        env.storage().persistent().extend_ttl(
            &key,
            Self::SNAPSHOT_TTL_LEDGERS / 2,
            Self::SNAPSHOT_TTL_LEDGERS,
        );
        Self::register_tracked_pool(&env, &pool);
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
        env.events()
            .publish((Symbol::new(&env, "snapshot_deleted"),), (pool, ledger_ts));
        Ok(())
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
            &admin, &ta.address, &tb.address, &lp_addr, &30_i128, &admin, &0_i128,
        );

        let provider = Address::generate(&env);
        ta_sac.mint(&provider, &1_000_000_i128);
        tb_sac.mint(&provider, &1_000_000_i128);
        AmmPoolClient::new(&env, &amm_addr).add_liquidity(
            &provider, &1_000_000_i128, &1_000_000_i128, &0_i128, &u64::MAX,
        );

        let consumer = TwalConsumerClient::new(&env, &consumer_addr);
        consumer.initialize(&admin);
        consumer.save_snapshot(&amm_addr);

        env.ledger().with_mut(|l| l.timestamp = 10_600);
        ta_sac.mint(&provider, &200_000_i128);
        tb_sac.mint(&provider, &200_000_i128);
        AmmPoolClient::new(&env, &amm_addr).add_liquidity(
            &provider, &100_000_i128, &100_000_i128, &0_i128, &u64::MAX,
        );
        consumer.save_snapshot(&amm_addr);

        env.ledger().with_mut(|l| l.timestamp = 11_200);
        let trader = Address::generate(&env);
        ta_sac.mint(&trader, &1_000_i128);
        AmmPoolClient::new(&env, &amm_addr).swap(
            &trader, &ta.address, &1_000_i128, &0_i128, &u64::MAX,
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
            &admin, &ta.address, &tb.address, &lp_addr, &30_i128, &admin, &0_i128,
        );

        let provider = Address::generate(&env);
        ta_sac.mint(&provider, &1_000_000_i128);
        tb_sac.mint(&provider, &1_000_000_i128);
        AmmPoolClient::new(&env, &amm_addr).add_liquidity(
            &provider, &1_000_000_i128, &1_000_000_i128, &0_i128, &u64::MAX,
        );

        let consumer = TwalConsumerClient::new(&env, &consumer_addr);
        consumer.initialize(&admin);
        consumer.save_snapshot(&amm_addr);

        // Snapshot stored at ledger timestamp 10_000; deleting it removes it so a
        // later TWAL query over that window can no longer find it.
        consumer.delete_snapshot(&amm_addr, &10_000_u64);

        env.ledger().with_mut(|l| l.timestamp = 10_600);
        assert_eq!(
            consumer.try_get_twal_liquidity(&amm_addr, &600),
            Err(Ok(TwalError::NoSnapshotFound))
        );
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
}
