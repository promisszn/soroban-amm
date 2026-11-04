#[cfg(test)]
mod auth_tests {
    extern crate std;
    use super::*;
    use soroban_sdk::{
        testutils::Address as _,
        Address, Env,
    };

    fn setup_vesting(env: &Env) -> (PolVestingClient<'_>, Address, Address) {
        let gov = Address::generate(env);
        let treasury = Address::generate(env);
        let vesting_addr = env.register_contract(None, PolVestingContract);
        let client = PolVestingClient::new(env, &vesting_addr);
        client.initialize(&gov, &treasury);
        (client, gov, treasury)
    }

    #[test]
    fn test_propose_governance_requires_current_governance_auth() {
        let env = Env::default();
        let (client, _gov, _treasury) = setup_vesting(&env);
        let new_gov = Address::generate(&env);

        assert!(client.try_propose_governance(&new_gov).is_err());
    }

    #[test]
    fn test_claim_governance_requires_new_governance_auth() {
        let env = Env::default();
        let (client, _gov, _treasury) = setup_vesting(&env);

        assert!(client.try_claim_governance().is_err());
    }

    #[test]
    fn test_propose_treasury_requires_governance_auth() {
        let env = Env::default();
        let (client, _gov, _treasury) = setup_vesting(&env);
        let new_treasury = Address::generate(&env);

        assert!(client.try_propose_treasury(&new_treasury).is_err());
    }

    #[test]
    fn test_claim_treasury_requires_new_treasury_auth() {
        let env = Env::default();
        let (client, _gov, _treasury) = setup_vesting(&env);

        assert!(client.try_claim_treasury().is_err());
    }

    #[test]
    fn test_create_schedule_requires_governance_auth() {
        let env = Env::default();
        let (client, _gov, _treasury) = setup_vesting(&env);
        let beneficiary = Address::generate(&env);

        assert!(client
            .try_create_schedule(&beneficiary, &100, &200, &300, &1_000)
            .is_err());
    }

    #[test]
    fn test_cancel_schedule_requires_governance_auth() {
        let env = Env::default();
        let (client, _gov, _treasury) = setup_vesting(&env);
        let beneficiary = Address::generate(&env);

        assert!(client.try_cancel_schedule(&beneficiary, &1).is_err());
    }

    #[test]
    fn test_transfer_schedule_requires_governance_auth() {
        let env = Env::default();
        let (client, _gov, _treasury) = setup_vesting(&env);
        let old_b = Address::generate(&env);
        let new_b = Address::generate(&env);

        assert!(client.try_transfer_schedule(&old_b, &1, &new_b).is_err());
    }

    #[test]
    fn test_upgrade_requires_governance_auth() {
        let env = Env::default();
        let (client, _gov, _treasury) = setup_vesting(&env);
        let dummy_hash = soroban_sdk::BytesN::from_array(&env, &[0u8; 32]);

        assert!(client.try_upgrade(&dummy_hash).is_err());
    }
}
