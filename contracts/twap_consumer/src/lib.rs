#![no_std]

use soroban_sdk::{contract, contractclient, contractimpl, contracttype, Address, Env};

#[contractclient(name = "AmmPoolOracleClient")]
pub trait AmmPoolOracle {
    fn get_price_cumulative(env: Env) -> (i128, i128, u64);
}

#[contracttype]
pub enum DataKey {
    Snapshot(Address, u64),
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PriceSnapshot {
    pub cum_a: i128,
    pub cum_b: i128,
    pub pool_ts: u64,
}

#[contract]
pub struct TwapConsumer;

#[contractimpl]
impl TwapConsumer {
    /// Stores a pool cumulative-price snapshot keyed by the pool timestamp.
    pub fn save_snapshot(env: Env, pool: Address) {
        let (cum_a, cum_b, pool_ts) = AmmPoolOracleClient::new(&env, &pool).get_price_cumulative();
        let snapshot = PriceSnapshot {
            cum_a,
            cum_b,
            pool_ts,
        };
        env.storage()
            .persistent()
            .set(&DataKey::Snapshot(pool, pool_ts), &snapshot);
    }

    /// Computes TWAP for token A in terms of token B over `window_seconds`.
    ///
    /// Returns: (cum_a_now - cum_a_then) / window_seconds.
    pub fn get_twap_price(env: Env, pool: Address, window_seconds: u64) -> i128 {
        assert!(window_seconds > 0, "window_seconds must be > 0");

        let (cum_a_now, _cum_b_now, now_ts) =
            AmmPoolOracleClient::new(&env, &pool).get_price_cumulative();
        assert!(
            now_ts >= window_seconds,
            "pool timestamp is smaller than requested window"
        );

        let then_ts = now_ts - window_seconds;
        let snapshot: PriceSnapshot = env
            .storage()
            .persistent()
            .get(&DataKey::Snapshot(pool, then_ts))
            .unwrap_or_else(|| panic!("missing snapshot at target timestamp"));

        (cum_a_now - snapshot.cum_a) / (window_seconds as i128)
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

    /// Register a Stellar Asset Contract and return (TokenClient, StellarAssetClient).
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
        amm.add_liquidity(&provider, &2_000_000_i128, &2_000_000_i128, &0_i128, &10_000_u64);

        let consumer = TwapConsumerClient::new(&env, &consumer_addr);
        consumer.save_snapshot(&amm_addr);

        // Let 60s pass at the pre-trade price, then execute a large trade that moves spot.
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
}
