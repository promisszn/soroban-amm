#![no_std]

use soroban_sdk::{
    contract, contractclient, contracterror, contractimpl, contracttype, Address, Env, Vec,
};

const PRICE_SCALE: i128 = 1_000_000;
const POW_SCALE: i128 = 1_000_000_000_000;

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AdapterMode {
    Amm,
    ConcentratedLiquidity,
}

#[contracttype]
#[derive(Clone)]
enum DataKey {
    Admin,
    Factory,
    Consumer,
    Mode,
    Window,
    MaxAge,
    FeeTiers,
}

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum AdapterError {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    InvalidConfiguration = 3,
    PoolNotFound = 4,
    NoSnapshot = 5,
    StaleSnapshot = 6,
    InvalidPrice = 7,
    ArithmeticOverflow = 8,
    CrossContractCallFailed = 9,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TwapResult {
    pub twap: i128,
    pub window_used: u64,
    pub snapshot_timestamp: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClTwapResult {
    pub average_tick: i64,
    pub window_used: u64,
    pub snapshot_timestamp: u64,
}

// Only the generated `FactoryClient` is used; the trait is its source of truth.
#[allow(dead_code)]
#[contractclient(name = "FactoryClient")]
trait Factory {
    fn get_pool(env: Env, token_a: Address, token_b: Address) -> Option<Address>;
    fn get_cl_pool(env: Env, token_a: Address, token_b: Address, fee_bps: i128) -> Option<Address>;
    fn get_pool_tokens(env: Env, pool: Address) -> Option<(Address, Address)>;
}

// Only the generated `TwapConsumerClient` is used; the trait is its source of truth.
#[allow(dead_code)]
#[contractclient(name = "TwapConsumerClient")]
trait TwapConsumer {
    fn get_twap_price_detailed(env: Env, pool: Address, window_seconds: u64) -> TwapResult;
    fn get_cl_twap_detailed(env: Env, pool: Address, window_seconds: u64) -> ClTwapResult;
    fn get_latest_snapshot_timestamp(env: Env, pool: Address) -> u64;
}

// Only the generated `ClPoolClient` is used; the trait is its source of truth.
#[allow(dead_code)]
#[contractclient(name = "ClPoolClient")]
trait ClPool {
    fn active_liquidity(env: Env) -> i128;
}

#[contract]
pub struct OracleTwapAdapter;

#[contractimpl]
impl OracleTwapAdapter {
    #[allow(clippy::too_many_arguments)]
    pub fn initialize(
        env: Env,
        admin: Address,
        factory: Address,
        consumer: Address,
        mode: AdapterMode,
        window_seconds: u64,
        max_age_seconds: u64,
        fee_tiers: Vec<i128>,
    ) -> Result<(), AdapterError> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(AdapterError::AlreadyInitialized);
        }
        if window_seconds == 0 || max_age_seconds == 0 {
            return Err(AdapterError::InvalidConfiguration);
        }
        if mode == AdapterMode::ConcentratedLiquidity && fee_tiers.is_empty() {
            return Err(AdapterError::InvalidConfiguration);
        }
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Factory, &factory);
        env.storage().instance().set(&DataKey::Consumer, &consumer);
        env.storage().instance().set(&DataKey::Mode, &mode);
        env.storage()
            .instance()
            .set(&DataKey::Window, &window_seconds);
        env.storage()
            .instance()
            .set(&DataKey::MaxAge, &max_age_seconds);
        env.storage().instance().set(&DataKey::FeeTiers, &fee_tiers);
        Ok(())
    }

    pub fn quote(
        env: Env,
        token_a: Address,
        token_b: Address,
    ) -> Result<(i128, u64), AdapterError> {
        let factory: Address = Self::read(&env, DataKey::Factory)?;
        let consumer: Address = Self::read(&env, DataKey::Consumer)?;
        let mode: AdapterMode = Self::read(&env, DataKey::Mode)?;
        let window: u64 = Self::read(&env, DataKey::Window)?;
        let max_age: u64 = Self::read(&env, DataKey::MaxAge)?;
        let factory_client = FactoryClient::new(&env, &factory);

        let pool = match mode {
            AdapterMode::Amm => match factory_client.try_get_pool(&token_a, &token_b) {
                Ok(Ok(pool)) => pool.ok_or(AdapterError::PoolNotFound)?,
                _ => return Err(AdapterError::CrossContractCallFailed),
            },
            AdapterMode::ConcentratedLiquidity => {
                let tiers: Vec<i128> = Self::read(&env, DataKey::FeeTiers)?;
                let mut selected: Option<(Address, i128)> = None;
                for tier in tiers.iter() {
                    let candidate = match factory_client.try_get_cl_pool(&token_a, &token_b, &tier)
                    {
                        Ok(Ok(candidate)) => candidate,
                        _ => return Err(AdapterError::CrossContractCallFailed),
                    };
                    if let Some(candidate) = candidate {
                        let liquidity =
                            match ClPoolClient::new(&env, &candidate).try_active_liquidity() {
                                Ok(Ok(liquidity)) => liquidity,
                                _ => continue,
                            };
                        if selected
                            .as_ref()
                            .map(|(_, l)| liquidity > *l)
                            .unwrap_or(true)
                        {
                            selected = Some((candidate, liquidity));
                        }
                    }
                }
                selected
                    .map(|(pool, _)| pool)
                    .ok_or(AdapterError::PoolNotFound)?
            }
        };

        let consumer_client = TwapConsumerClient::new(&env, &consumer);
        let newest = match consumer_client.try_get_latest_snapshot_timestamp(&pool) {
            Ok(Ok(ts)) => ts,
            _ => return Err(AdapterError::CrossContractCallFailed),
        };
        if newest == 0 {
            return Err(AdapterError::NoSnapshot);
        }
        if env.ledger().timestamp().saturating_sub(newest) > max_age {
            return Err(AdapterError::StaleSnapshot);
        }

        let mut price = match mode {
            AdapterMode::Amm => match consumer_client.try_get_twap_price_detailed(&pool, &window) {
                Ok(Ok(result)) => result.twap,
                _ => return Err(AdapterError::CrossContractCallFailed),
            },
            AdapterMode::ConcentratedLiquidity => {
                let result = match consumer_client.try_get_cl_twap_detailed(&pool, &window) {
                    Ok(Ok(result)) => result,
                    _ => return Err(AdapterError::CrossContractCallFailed),
                };
                Self::tick_to_price(result.average_tick)?
            }
        };
        if price <= 0 {
            return Err(AdapterError::InvalidPrice);
        }

        let pool_tokens = match factory_client.try_get_pool_tokens(&pool) {
            Ok(Ok(tokens)) => tokens.ok_or(AdapterError::PoolNotFound)?,
            _ => return Err(AdapterError::CrossContractCallFailed),
        };
        if pool_tokens.0 != token_a {
            price = PRICE_SCALE
                .checked_mul(PRICE_SCALE)
                .ok_or(AdapterError::ArithmeticOverflow)?
                / price;
        }
        Ok((price, newest))
    }

    pub fn get_config(env: Env) -> Result<(Address, Address, AdapterMode, u64, u64), AdapterError> {
        Ok((
            Self::read(&env, DataKey::Factory)?,
            Self::read(&env, DataKey::Consumer)?,
            Self::read(&env, DataKey::Mode)?,
            Self::read(&env, DataKey::Window)?,
            Self::read(&env, DataKey::MaxAge)?,
        ))
    }

    fn read<T>(env: &Env, key: DataKey) -> Result<T, AdapterError>
    where
        T: soroban_sdk::TryFromVal<Env, soroban_sdk::Val>,
        T::Error: core::fmt::Debug,
    {
        env.storage()
            .instance()
            .get(&key)
            .ok_or(AdapterError::NotInitialized)
    }

    fn tick_to_price(tick: i64) -> Result<i128, AdapterError> {
        let negative = tick < 0;
        let mut exponent = tick.unsigned_abs();
        let mut factor = 1_000_100_000_000_i128;
        let mut ratio = POW_SCALE;
        while exponent > 0 {
            if exponent & 1 == 1 {
                ratio = ratio
                    .checked_mul(factor)
                    .ok_or(AdapterError::ArithmeticOverflow)?
                    / POW_SCALE;
            }
            exponent >>= 1;
            if exponent > 0 {
                factor = factor
                    .checked_mul(factor)
                    .ok_or(AdapterError::ArithmeticOverflow)?
                    / POW_SCALE;
            }
        }
        if negative {
            ratio = POW_SCALE
                .checked_mul(POW_SCALE)
                .ok_or(AdapterError::ArithmeticOverflow)?
                / ratio;
        }
        ratio
            .checked_mul(PRICE_SCALE)
            .ok_or(AdapterError::ArithmeticOverflow)
            .map(|value| value / POW_SCALE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{
        testutils::{Address as _, Ledger},
        vec, Address, Env,
    };

    mod mock_factory {
        use soroban_sdk::{contract, contractimpl, contracttype, Address, Env};

        #[contracttype]
        #[derive(Clone)]
        enum Key {
            AmmPool(Address, Address),
            ClPool(Address, Address, i128),
            PoolTokens(Address),
        }

        #[contract]
        pub struct MockFactory;

        #[contractimpl]
        impl MockFactory {
            pub fn set_pool(env: Env, token_a: Address, token_b: Address, pool: Address) {
                env.storage()
                    .instance()
                    .set(&Key::AmmPool(token_a, token_b), &pool);
            }

            pub fn set_cl_pool(
                env: Env,
                token_a: Address,
                token_b: Address,
                fee_bps: i128,
                pool: Address,
            ) {
                env.storage()
                    .instance()
                    .set(&Key::ClPool(token_a, token_b, fee_bps), &pool);
            }

            pub fn set_pool_tokens(env: Env, pool: Address, token_0: Address, token_1: Address) {
                env.storage()
                    .instance()
                    .set(&Key::PoolTokens(pool), &(token_0, token_1));
            }

            pub fn get_pool(env: Env, token_a: Address, token_b: Address) -> Option<Address> {
                env.storage()
                    .instance()
                    .get(&Key::AmmPool(token_a, token_b))
            }

            pub fn get_cl_pool(
                env: Env,
                token_a: Address,
                token_b: Address,
                fee_bps: i128,
            ) -> Option<Address> {
                env.storage()
                    .instance()
                    .get(&Key::ClPool(token_a, token_b, fee_bps))
            }

            pub fn get_pool_tokens(env: Env, pool: Address) -> Option<(Address, Address)> {
                env.storage().instance().get(&Key::PoolTokens(pool))
            }
        }
    }

    mod mock_consumer {
        use crate::{ClTwapResult, TwapResult};
        use soroban_sdk::{contract, contractimpl, Address, Env};

        #[contract]
        pub struct MockConsumer;

        #[contractimpl]
        impl MockConsumer {
            pub fn get_latest_snapshot_timestamp(_env: Env, _pool: Address) -> u64 {
                1_900
            }

            pub fn get_twap_price_detailed(
                _env: Env,
                _pool: Address,
                window_seconds: u64,
            ) -> TwapResult {
                TwapResult {
                    twap: 2_000_000,
                    window_used: window_seconds,
                    snapshot_timestamp: 1_900,
                }
            }

            pub fn get_cl_twap_detailed(
                _env: Env,
                _pool: Address,
                window_seconds: u64,
            ) -> ClTwapResult {
                ClTwapResult {
                    average_tick: 0,
                    window_used: window_seconds,
                    snapshot_timestamp: 1_900,
                }
            }
        }
    }

    mod healthy_cl_pool {
        use soroban_sdk::{contract, contractimpl, Env};

        #[contract]
        pub struct HealthyClPool;

        #[contractimpl]
        impl HealthyClPool {
            pub fn active_liquidity(_env: Env) -> i128 {
                5_000
            }
        }
    }

    mod panicking_cl_pool {
        use soroban_sdk::{contract, contractimpl, Env};

        #[contract]
        pub struct PanickingClPool;

        #[contractimpl]
        impl PanickingClPool {
            pub fn active_liquidity(_env: Env) -> i128 {
                panic!("broken CL pool");
            }
        }
    }

    mod panicking_factory {
        use soroban_sdk::{contract, contractimpl, Address, Env};

        #[contract]
        pub struct PanickingFactory;

        #[contractimpl]
        impl PanickingFactory {
            pub fn get_pool(_env: Env, _token_a: Address, _token_b: Address) -> Option<Address> {
                panic!("factory get_pool failed");
            }

            pub fn get_cl_pool(
                _env: Env,
                _token_a: Address,
                _token_b: Address,
                _fee_bps: i128,
            ) -> Option<Address> {
                panic!("factory get_cl_pool failed");
            }

            pub fn get_pool_tokens(_env: Env, _pool: Address) -> Option<(Address, Address)> {
                panic!("factory get_pool_tokens failed");
            }
        }
    }

    mod panicking_consumer {
        use crate::{ClTwapResult, TwapResult};
        use soroban_sdk::{contract, contractimpl, Address, Env};

        #[contract]
        pub struct PanickingConsumer;

        #[contractimpl]
        impl PanickingConsumer {
            pub fn get_latest_snapshot_timestamp(_env: Env, _pool: Address) -> u64 {
                panic!("consumer unavailable");
            }

            pub fn get_twap_price_detailed(
                _env: Env,
                _pool: Address,
                _window_seconds: u64,
            ) -> TwapResult {
                panic!("consumer unavailable");
            }

            pub fn get_cl_twap_detailed(
                _env: Env,
                _pool: Address,
                _window_seconds: u64,
            ) -> ClTwapResult {
                panic!("consumer unavailable");
            }
        }
    }

    #[test]
    fn test_cl_quote_skips_panicking_candidate_pool() {
        let env = Env::default();
        env.ledger().set_timestamp(2_000);

        let admin = Address::generate(&env);
        let token_a = Address::generate(&env);
        let token_b = Address::generate(&env);

        let factory_id = env.register_contract(None, mock_factory::MockFactory);
        let factory = mock_factory::MockFactoryClient::new(&env, &factory_id);
        let bad_pool = env.register_contract(None, panicking_cl_pool::PanickingClPool);
        let good_pool = env.register_contract(None, healthy_cl_pool::HealthyClPool);
        factory.set_cl_pool(&token_a, &token_b, &5, &bad_pool);
        factory.set_cl_pool(&token_a, &token_b, &30, &good_pool);
        factory.set_pool_tokens(&good_pool, &token_a, &token_b);

        let consumer_id = env.register_contract(None, mock_consumer::MockConsumer);
        let adapter_id = env.register_contract(None, OracleTwapAdapter);
        let adapter = OracleTwapAdapterClient::new(&env, &adapter_id);

        adapter.initialize(
            &admin,
            &factory_id,
            &consumer_id,
            &AdapterMode::ConcentratedLiquidity,
            &300,
            &600,
            &vec![&env, 5, 30],
        );

        let (price, ts) = adapter.quote(&token_a, &token_b);
        assert_eq!(price, PRICE_SCALE);
        assert_eq!(ts, 1_900);
    }

    #[test]
    fn test_quote_returns_typed_error_on_panicking_factory() {
        let env = Env::default();
        env.ledger().set_timestamp(2_000);

        let admin = Address::generate(&env);
        let token_a = Address::generate(&env);
        let token_b = Address::generate(&env);

        let factory_id = env.register_contract(None, panicking_factory::PanickingFactory);
        let consumer_id = env.register_contract(None, mock_consumer::MockConsumer);
        let adapter_id = env.register_contract(None, OracleTwapAdapter);
        let adapter = OracleTwapAdapterClient::new(&env, &adapter_id);

        adapter.initialize(
            &admin,
            &factory_id,
            &consumer_id,
            &AdapterMode::Amm,
            &300,
            &600,
            &vec![&env],
        );

        let res = adapter.try_quote(&token_a, &token_b);
        assert_eq!(res, Err(Ok(AdapterError::CrossContractCallFailed)));
    }

    #[test]
    fn test_quote_returns_typed_error_on_panicking_consumer() {
        let env = Env::default();
        env.ledger().set_timestamp(2_000);

        let admin = Address::generate(&env);
        let token_a = Address::generate(&env);
        let token_b = Address::generate(&env);

        let pool = Address::generate(&env);
        let factory_id = env.register_contract(None, mock_factory::MockFactory);
        let factory = mock_factory::MockFactoryClient::new(&env, &factory_id);
        factory.set_pool(&token_a, &token_b, &pool);
        factory.set_pool_tokens(&pool, &token_a, &token_b);

        let consumer_id = env.register_contract(None, panicking_consumer::PanickingConsumer);
        let adapter_id = env.register_contract(None, OracleTwapAdapter);
        let adapter = OracleTwapAdapterClient::new(&env, &adapter_id);

        adapter.initialize(
            &admin,
            &factory_id,
            &consumer_id,
            &AdapterMode::Amm,
            &300,
            &600,
            &vec![&env],
        );

        let res = adapter.try_quote(&token_a, &token_b);
        assert_eq!(res, Err(Ok(AdapterError::CrossContractCallFailed)));
    }
}
