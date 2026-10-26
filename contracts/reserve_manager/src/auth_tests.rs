#[cfg(test)]
mod auth_tests {
    extern crate std;
    use super::*;
    use soroban_sdk::{
        testutils::Address as _,
        Address, Env,
    };

    fn setup_reserve_manager(env: &Env) -> (ReserveManagerClient<'_>, Address) {
        let gov = Address::generate(env);
        let rm_addr = env.register_contract(None, ReserveManager);
        let client = ReserveManagerClient::new(env, &rm_addr);
        client.initialize(&gov);
        (client, gov)
    }

    #[test]
    fn test_pause_requires_governance_auth() {
        let env = Env::default();
        let (rm, _gov) = setup_reserve_manager(&env);

        // Invoking pause without mocked governance auth must fail
        assert!(rm.try_pause().is_err());
    }

    #[test]
    fn test_unpause_requires_governance_auth() {
        let env = Env::default();
        let (rm, _gov) = setup_reserve_manager(&env);

        assert!(rm.try_unpause().is_err());
    }

    #[test]
    fn test_propose_governance_requires_governance_auth() {
        let env = Env::default();
        let (rm, _gov) = setup_reserve_manager(&env);
        let new_gov = Address::generate(&env);

        assert!(rm.try_propose_governance(&new_gov).is_err());
    }

    #[test]
    fn test_claim_governance_requires_new_governance_auth() {
        let env = Env::default();
        let (rm, _gov) = setup_reserve_manager(&env);

        assert!(rm.try_claim_governance().is_err());
    }

    #[test]
    fn test_set_reserve_floor_requires_governance_auth() {
        let env = Env::default();
        let (rm, _gov) = setup_reserve_manager(&env);
        let pool = Address::generate(&env);

        assert!(rm.try_set_reserve_floor(&pool, &1_000, &1_000).is_err());
    }

    #[test]
    fn test_set_pool_kind_requires_governance_auth() {
        let env = Env::default();
        let (rm, _gov) = setup_reserve_manager(&env);
        let pool = Address::generate(&env);

        assert!(rm.try_set_pool_kind(&pool, &PoolKind::StandardAmm).is_err());
    }
}
