#[cfg(test)]
mod auth_tests {
    extern crate std;
    use super::*;
    use soroban_sdk::{
        testutils::Address as _,
        Address, Env,
    };

    #[test]
    fn test_save_snapshot_requires_keeper_auth() {
        let env = Env::default();
        let keeper = Address::generate(&env);
        let pool = Address::generate(&env);
        let consumer = TwapConsumerClient::new(&env, &env.register_contract(None, TwapConsumer));
        consumer.initialize(&keeper);

        // Without keeper auth, save_snapshot must fail
        assert!(consumer.try_save_snapshot(&pool).is_err());
    }

    #[test]
    fn test_save_snapshot_with_cumulatives_requires_keeper_auth() {
        let env = Env::default();
        let keeper = Address::generate(&env);
        let pool = Address::generate(&env);
        let consumer = TwapConsumerClient::new(&env, &env.register_contract(None, TwapConsumer));
        consumer.initialize(&keeper);

        assert!(consumer
            .try_save_snapshot_with_cumulatives(&pool, &1_000, &1_000, &100)
            .is_err());
    }

    #[test]
    fn test_save_cl_snapshot_requires_keeper_auth() {
        let env = Env::default();
        let keeper = Address::generate(&env);
        let pool = Address::generate(&env);
        let consumer = TwapConsumerClient::new(&env, &env.register_contract(None, TwapConsumer));
        consumer.initialize(&keeper);

        assert!(consumer.try_save_cl_snapshot(&pool).is_err());
    }

    #[test]
    fn test_set_min_snapshot_interval_requires_keeper_auth() {
        let env = Env::default();
        let keeper = Address::generate(&env);
        let consumer = TwapConsumerClient::new(&env, &env.register_contract(None, TwapConsumer));
        consumer.initialize(&keeper);

        assert!(consumer.try_set_min_snapshot_interval(&300).is_err());
    }

    #[test]
    fn test_set_retention_policy_requires_keeper_auth() {
        let env = Env::default();
        let keeper = Address::generate(&env);
        let pool = Address::generate(&env);
        let consumer = TwapConsumerClient::new(&env, &env.register_contract(None, TwapConsumer));
        consumer.initialize(&keeper);

        let policy = RetentionPolicy {
            max_snapshots: 10,
            max_age_seconds: 3600,
        };
        assert!(consumer.try_set_retention_policy(&pool, &policy).is_err());
    }

    #[test]
    fn test_track_pool_requires_keeper_auth() {
        let env = Env::default();
        let keeper = Address::generate(&env);
        let pool = Address::generate(&env);
        let consumer = TwapConsumerClient::new(&env, &env.register_contract(None, TwapConsumer));
        consumer.initialize(&keeper);

        assert!(consumer.try_track_pool(&pool, &PoolType::Amm).is_err());
    }

    #[test]
    fn test_untrack_pool_requires_keeper_auth() {
        let env = Env::default();
        let keeper = Address::generate(&env);
        let pool = Address::generate(&env);
        let consumer = TwapConsumerClient::new(&env, &env.register_contract(None, TwapConsumer));
        consumer.initialize(&keeper);

        assert!(consumer.try_untrack_pool(&pool).is_err());
    }

    #[test]
    fn test_upgrade_requires_keeper_auth() {
        let env = Env::default();
        let keeper = Address::generate(&env);
        let consumer = TwapConsumerClient::new(&env, &env.register_contract(None, TwapConsumer));
        consumer.initialize(&keeper);

        let dummy_hash = soroban_sdk::BytesN::from_array(&env, &[0u8; 32]);
        assert!(consumer.try_upgrade(&dummy_hash).is_err());
    }
}
