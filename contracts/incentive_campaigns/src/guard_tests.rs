#[cfg(test)]
mod guard_tests {
    extern crate std;

    use super::*;
    use amm::{AmmPool, AmmPoolClient};
    use soroban_sdk::{
        testutils::{storage::Persistent as _, Address as _, Ledger},
        token::StellarAssetClient,
        Address, Env,
    };
    use token::{LpToken, LpTokenClient};

    fn setup() -> (Env, Address, Address, Address, Address, Address, Address) {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set_timestamp(1_000);

        let gov = Address::generate(&env);
        let provider = Address::generate(&env);
        let admin = Address::generate(&env);

        let amm_addr = env.register_contract(None, AmmPool);
        let lp_addr = env.register_contract(None, LpToken);
        let reward_admin = Address::generate(&env);
        let reward_addr = env.register_stellar_asset_contract_v2(reward_admin.clone()).address();

        let token_a_admin = Address::generate(&env);
        let token_b_admin = Address::generate(&env);
        let token_a = env.register_stellar_asset_contract_v2(token_a_admin.clone()).address();
        let token_b = env.register_stellar_asset_contract_v2(token_b_admin.clone()).address();

        LpTokenClient::new(&env, &lp_addr).initialize(
            &admin,
            &amm_addr,
            &soroban_sdk::String::from_str(&env, "LP"),
            &soroban_sdk::String::from_str(&env, "LP"),
        );

        AmmPoolClient::new(&env, &amm_addr).initialize(
            &admin,
            &lp_addr,
            &token_a,
            &token_b,
            &30,
        );

        StellarAssetClient::new(&env, &token_a).mint(&provider, &1_000_000);
        StellarAssetClient::new(&env, &token_b).mint(&provider, &1_000_000);
        AmmPoolClient::new(&env, &amm_addr).deposit(&provider, &1_000_000, &1_000_000, &0, &0);

        let incentives_addr = env.register_contract(None, IncentiveCampaigns);
        let client = IncentiveCampaignsClient::new(&env, &incentives_addr);
        client.initialize(&gov);

        StellarAssetClient::new(&env, &reward_addr).mint(&gov, &10_000_000);

        (env, incentives_addr, amm_addr, lp_addr, reward_addr, provider, gov)
    }

    #[test]
    fn test_recover_leftover_funds_zero_leftover_returns_no_leftover_funds() {
        let (env, incentives, pool, lp, reward, _provider, gov) = setup();
        let client = IncentiveCampaignsClient::new(&env, &incentives);
        let treasury = Address::generate(&env);

        let funding = 500_000;
        let id = client.create_campaign(&gov, &pool, &lp, &reward, &1_000, &2_000, &100, &funding);

        // Advance ledger timestamp beyond end_time + claim_grace_secs
        env.ledger().set_timestamp(3_000);

        // Mutate campaign state so that total_distributed == funding_amount (i.e. leftover == 0)
        let campaign_key = DataKey::Campaign(id);
        let mut campaign: Campaign = env.as_contract(&incentives, || {
            env.storage().persistent().get(&campaign_key).unwrap()
        });
        campaign.total_distributed = campaign.funding_amount;
        env.as_contract(&incentives, || {
            env.storage().persistent().set(&campaign_key, &campaign);
        });

        // Calling recover_leftover_funds must fail with NoLeftoverFunds
        let res = client.try_recover_leftover_funds(&gov, &id, &treasury);
        assert_eq!(res, Err(Ok(IncentiveError::NoLeftoverFunds)));
    }

    #[test]
    fn test_recover_leftover_funds_negative_leftover_returns_no_leftover_funds() {
        let (env, incentives, pool, lp, reward, _provider, gov) = setup();
        let client = IncentiveCampaignsClient::new(&env, &incentives);
        let treasury = Address::generate(&env);

        let funding = 500_000;
        let id = client.create_campaign(&gov, &pool, &lp, &reward, &1_000, &2_000, &100, &funding);

        env.ledger().set_timestamp(3_000);

        let campaign_key = DataKey::Campaign(id);
        let mut campaign: Campaign = env.as_contract(&incentives, || {
            env.storage().persistent().get(&campaign_key).unwrap()
        });
        campaign.total_distributed = campaign.funding_amount + 100;
        env.as_contract(&incentives, || {
            env.storage().persistent().set(&campaign_key, &campaign);
        });

        let res = client.try_recover_leftover_funds(&gov, &id, &treasury);
        assert_eq!(res, Err(Ok(IncentiveError::NoLeftoverFunds)));
    }

    #[test]
    fn test_claim_rewards_zero_lp_supply_returns_no_lp_supply() {
        let (env, incentives, pool, _lp, reward, provider, gov) = setup();
        let client = IncentiveCampaignsClient::new(&env, &incentives);

        // Register a separate LP token with 0 total supply
        let empty_lp = env.register_contract(None, LpToken);
        let admin = Address::generate(&env);
        LpTokenClient::new(&env, &empty_lp).initialize(
            &admin,
            &pool,
            &soroban_sdk::String::from_str(&env, "EMPTY"),
            &soroban_sdk::String::from_str(&env, "EMPTY"),
        );

        let funding = 500_000;
        let id = client.create_campaign(&gov, &pool, &empty_lp, &reward, &1_000, &2_000, &100, &funding);

        // Advance to active campaign window
        env.ledger().set_timestamp(1_500);

        // try_claim_rewards must detect zero total supply on the LP token and revert with NoLpSupply
        let res = client.try_claim_rewards(&provider, &id);
        assert_eq!(res, Err(Ok(IncentiveError::NoLpSupply)));
    }
}
