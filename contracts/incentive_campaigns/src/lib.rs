#![no_std]

//! Governance-controlled trading incentive campaigns for liquidity providers.
//!
//! Supports multiple simultaneous time-based campaigns, multiple reward tokens,
//! proportional LP distribution, and a full on-chain audit trail.

use soroban_sdk::{
    contract, contractclient, contractimpl, contracttype, token::Client as TokenClient, Address,
    Env, Symbol, Vec,
};

#[contractclient(name = "LpTokenClient")]
pub trait LpTokenInterface {
    fn balance(env: Env, id: Address) -> i128;
    fn total_supply(env: Env) -> i128;
}

#[contracttype]
pub enum DataKey {
    Governance,
    NextCampaignId,
    Campaign(u64),
    CampaignIds,
    /// Per (campaign_id, provider): cumulative reward index at last claim
    ProviderDebt(u64, Address),
    /// Audit: next distribution record id
    NextDistributionId,
    DistributionRecord(u64),
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Campaign {
    pub id: u64,
    pub pool: Address,
    pub lp_token: Address,
    pub reward_token: Address,
    pub start_time: u64,
    pub end_time: u64,
    /// Rewards per second, scaled by REWARD_SCALE
    pub reward_rate: i128,
    pub active: bool,
    pub total_distributed: i128,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DistributionRecord {
    pub id: u64,
    pub campaign_id: u64,
    pub provider: Address,
    pub reward_token: Address,
    pub amount: i128,
    pub timestamp: u64,
}

#[contract]
pub struct IncentiveCampaigns;

#[contractimpl]
impl IncentiveCampaigns {
    pub const REWARD_SCALE: i128 = 1_000_000_000_000_000_000; // 1e18

    pub fn initialize(env: Env, governance: Address) {
        assert!(
            !env.storage().instance().has(&DataKey::Governance),
            "already initialized"
        );
        env.storage()
            .instance()
            .set(&DataKey::Governance, &governance);
        env.storage().instance().set(&DataKey::NextCampaignId, &1u64);
        env.storage()
            .instance()
            .set(&DataKey::NextDistributionId, &1u64);
        let empty: Vec<u64> = Vec::new(&env);
        env.storage().instance().set(&DataKey::CampaignIds, &empty);
    }

    /// Create a time-based incentive campaign. Governance only.
    pub fn create_campaign(
        env: Env,
        caller: Address,
        pool: Address,
        lp_token: Address,
        reward_token: Address,
        start_time: u64,
        end_time: u64,
        reward_rate: i128,
        funding_amount: i128,
    ) -> u64 {
        caller.require_auth();
        Self::require_governance(&env, &caller);
        assert!(end_time > start_time, "invalid campaign window");
        assert!(reward_rate > 0, "reward_rate must be positive");
        assert!(funding_amount > 0, "funding required");

        let id: u64 = env.storage().instance().get(&DataKey::NextCampaignId).unwrap();
        let campaign = Campaign {
            id,
            pool: pool.clone(),
            lp_token: lp_token.clone(),
            reward_token: reward_token.clone(),
            start_time,
            end_time,
            reward_rate,
            active: true,
            total_distributed: 0,
        };
        env.storage().persistent().set(&DataKey::Campaign(id), &campaign);
        env.storage()
            .instance()
            .set(&DataKey::NextCampaignId, &(id + 1));

        let mut ids: Vec<u64> = env
            .storage()
            .instance()
            .get(&DataKey::CampaignIds)
            .unwrap_or_else(|| Vec::new(&env));
        ids.push_back(id);
        env.storage().instance().set(&DataKey::CampaignIds, &ids);

        let contract = env.current_contract_address();
        TokenClient::new(&env, &reward_token).transfer(&caller, &contract, &funding_amount);

        env.events().publish(
            (Symbol::new(&env, "campaign_created"),),
            (id, pool, reward_token, start_time, end_time, reward_rate),
        );
        id
    }

    /// Update reward rate for an active campaign. Governance only.
    pub fn set_campaign_rate(env: Env, caller: Address, campaign_id: u64, new_rate: i128) {
        caller.require_auth();
        Self::require_governance(&env, &caller);
        assert!(new_rate > 0, "rate must be positive");
        let mut campaign: Campaign = env
            .storage()
            .persistent()
            .get(&DataKey::Campaign(campaign_id))
            .expect("campaign not found");
        campaign.reward_rate = new_rate;
        env.storage()
            .persistent()
            .set(&DataKey::Campaign(campaign_id), &campaign);
        env.events().publish(
            (Symbol::new(&env, "rate_updated"),),
            (campaign_id, new_rate),
        );
    }

    /// Recover undistributed reward tokens after a campaign has ended. Governance only.
    ///
    /// Computes the maximum total distributable amount (`reward_rate × campaign_duration`)
    /// and transfers any portion that was never claimed by providers back to `recipient`.
    /// The campaign is marked **inactive** upon recovery so no further LP claims can be
    /// made against the already-withdrawn balance.
    ///
    /// Governance should allow a reasonable grace period after `end_time` before calling
    /// this function so that LPs have an opportunity to claim their earned rewards first.
    pub fn recover_leftover_funds(
        env: Env,
        caller: Address,
        campaign_id: u64,
        recipient: Address,
    ) -> i128 {
        caller.require_auth();
        Self::require_governance(&env, &caller);

        let mut campaign: Campaign = env
            .storage()
            .persistent()
            .get(&DataKey::Campaign(campaign_id))
            .expect("campaign not found");

        let now = env.ledger().timestamp();
        assert!(now > campaign.end_time, "campaign not yet ended");

        // Maximum that could ever be distributed = rate × full duration.
        let campaign_duration = (campaign.end_time - campaign.start_time) as i128;
        let max_distributable = campaign.reward_rate * campaign_duration;
        let leftover = max_distributable - campaign.total_distributed;

        assert!(leftover > 0, "no leftover funds to recover");

        // Mark inactive so future claim_rewards calls revert, protecting the
        // recipient from having tokens transferred twice.
        campaign.active = false;
        env.storage()
            .persistent()
            .set(&DataKey::Campaign(campaign_id), &campaign);

        let contract = env.current_contract_address();
        TokenClient::new(&env, &campaign.reward_token).transfer(
            &contract,
            &recipient,
            &leftover,
        );

        env.events().publish(
            (Symbol::new(&env, "leftover_recovered"),),
            (campaign_id, recipient.clone(), leftover),
        );

        leftover
    }

    /// Distribute accrued rewards to a provider proportional to LP balance.
    pub fn claim_rewards(env: Env, provider: Address, campaign_id: u64) -> i128 {
        provider.require_auth();
        let campaign: Campaign = env
            .storage()
            .persistent()
            .get(&DataKey::Campaign(campaign_id))
            .expect("campaign not found");
        assert!(campaign.active, "campaign inactive");

        let now = env.ledger().timestamp();
        assert!(now >= campaign.start_time, "campaign not started");

        // Cap accrual at end_time so LPs can claim earned rewards even after the campaign
        // has finished. Without this cap any unclaimed balance would be permanently locked.
        let claim_time = if now > campaign.end_time {
            campaign.end_time
        } else {
            now
        };

        let lp_balance =
            LpTokenClient::new(&env, &campaign.lp_token).balance(&provider);
        assert!(lp_balance > 0, "no LP balance");

        let total_supply = LpTokenClient::new(&env, &campaign.lp_token).total_supply();
        assert!(total_supply > 0, "no LP supply");

        // Proportional share of rewards since campaign start, capped at end_time.
        let elapsed = (claim_time - campaign.start_time) as i128;
        let pool_rewards = campaign.reward_rate * elapsed;
        let provider_share = pool_rewards * lp_balance / total_supply;

        let debt_key = DataKey::ProviderDebt(campaign_id, provider.clone());
        let already_claimed: i128 = env
            .storage()
            .persistent()
            .get(&debt_key)
            .unwrap_or(0);
        let pending = provider_share - already_claimed;
        assert!(pending > 0, "no pending rewards");

        let contract = env.current_contract_address();
        TokenClient::new(&env, &campaign.reward_token).transfer(&contract, &provider, &pending);

        env.storage()
            .persistent()
            .set(&debt_key, &provider_share);

        let mut updated = campaign.clone();
        updated.total_distributed += pending;
        env.storage()
            .persistent()
            .set(&DataKey::Campaign(campaign_id), &updated);

        let dist_id: u64 = env
            .storage()
            .instance()
            .get(&DataKey::NextDistributionId)
            .unwrap();
        let record = DistributionRecord {
            id: dist_id,
            campaign_id,
            provider: provider.clone(),
            reward_token: campaign.reward_token.clone(),
            amount: pending,
            timestamp: now,
        };
        env.storage()
            .persistent()
            .set(&DataKey::DistributionRecord(dist_id), &record);
        env.storage()
            .instance()
            .set(&DataKey::NextDistributionId, &(dist_id + 1));

        env.events().publish(
            (Symbol::new(&env, "reward_distributed"),),
            (campaign_id, provider, pending, dist_id),
        );
        pending
    }

    pub fn get_campaign(env: Env, campaign_id: u64) -> Campaign {
        env.storage()
            .persistent()
            .get(&DataKey::Campaign(campaign_id))
            .expect("campaign not found")
    }

    pub fn list_campaigns(env: Env) -> Vec<u64> {
        env.storage()
            .instance()
            .get(&DataKey::CampaignIds)
            .unwrap_or_else(|| Vec::new(&env))
    }

    pub fn get_distribution_record(env: Env, record_id: u64) -> DistributionRecord {
        env.storage()
            .persistent()
            .get(&DataKey::DistributionRecord(record_id))
            .expect("record not found")
    }

    pub fn get_active_campaigns(env: Env) -> Vec<Campaign> {
        let ids = Self::list_campaigns(env.clone());
        let now = env.ledger().timestamp();
        let mut active: Vec<Campaign> = Vec::new(&env);
        for i in 0..ids.len() {
            let id = ids.get(i).unwrap();
            let c: Campaign = env
                .storage()
                .persistent()
                .get(&DataKey::Campaign(id))
                .unwrap();
            if c.active && now >= c.start_time && now <= c.end_time {
                active.push_back(c);
            }
        }
        active
    }

    fn require_governance(env: &Env, caller: &Address) {
        let gov: Address = env.storage().instance().get(&DataKey::Governance).unwrap();
        assert!(caller == &gov, "not governance");
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
        LpTokenClient::new(&env, &lp_addr).initialize(
            &amm_addr,
            &soroban_sdk::String::from_str(&env, "LP"),
            &soroban_sdk::String::from_str(&env, "LP"),
            &7u32,
        );

        let ta = env.register_stellar_asset_contract_v2(admin.clone());
        let tb = env.register_stellar_asset_contract_v2(admin.clone());
        let reward = env.register_stellar_asset_contract_v2(admin.clone());

        AmmPoolClient::new(&env, &amm_addr).initialize(
            &admin,
            &ta.address(),
            &tb.address(),
            &lp_addr,
            &30_i128,
            &admin,
            &0_i128,
        );

        StellarAssetClient::new(&env, &ta.address()).mint(&provider, &1_000_000);
        StellarAssetClient::new(&env, &tb.address()).mint(&provider, &1_000_000);
        AmmPoolClient::new(&env, &amm_addr).add_liquidity(
            &provider,
            &1_000_000,
            &1_000_000,
            &0,
            &u64::MAX,
        );

        StellarAssetClient::new(&env, &reward.address()).mint(&gov, &10_000_000);

        let incentives = env.register_contract(None, IncentiveCampaigns);
        IncentiveCampaignsClient::new(&env, &incentives).initialize(&gov);

        (
            env,
            incentives,
            amm_addr,
            lp_addr,
            reward.address(),
            provider,
            gov,
        )
    }

    #[test]
    fn test_multiple_campaigns_and_distribution_audit() {
        let (env, incentives, pool, lp, reward, provider, gov_addr) = setup();
        let client = IncentiveCampaignsClient::new(&env, &incentives);
        let id1 = client.create_campaign(
            &gov_addr,
            &pool,
            &lp,
            &reward,
            &1_000,
            &10_000,
            &100,
            &1_000_000,
        );
        let id2 = client.create_campaign(
            &gov_addr,
            &pool,
            &lp,
            &reward,
            &1_000,
            &20_000,
            &50,
            &500_000,
        );
        assert_eq!(id1, 1);
        assert_eq!(id2, 2);
        assert_eq!(client.list_campaigns().len(), 2);

        env.ledger().with_mut(|l| l.timestamp = 2_000);
        let claimed = client.claim_rewards(&provider, &id1);
        assert!(claimed > 0);

        let record = client.get_distribution_record(&1);
        assert_eq!(record.campaign_id, id1);
        assert_eq!(record.provider, provider);
    }

    /// Regression test for bug #424: claim_rewards must succeed after end_time,
    /// with rewards capped at end_time rather than panicking.
    #[test]
    fn test_claim_after_end_time() {
        let (env, incentives, pool, lp, reward, provider, gov_addr) = setup();
        let client = IncentiveCampaignsClient::new(&env, &incentives);

        // Campaign runs from t=1_000 to t=5_000, rate=100.
        let id = client.create_campaign(
            &gov_addr,
            &pool,
            &lp,
            &reward,
            &1_000,
            &5_000,
            &100,
            &1_000_000,
        );

        // ── Case 1: first claim arrives after end_time ────────────────────────────
        // Advance to t=8_000 (3_000 seconds past end_time=5_000).
        env.ledger().with_mut(|l| l.timestamp = 8_000);

        // Must NOT panic. Rewards should reflect exactly end_time - start_time = 4_000 s.
        let claimed_after_end = client.claim_rewards(&provider, &id);
        assert!(claimed_after_end > 0, "expected non-zero rewards after end_time");

        // Verify accrual was capped at end_time: elapsed = 5_000 - 1_000 = 4_000.
        // provider_share = rate * elapsed * lp_balance / total_supply.
        // The AMM permanently locks MINIMUM_LIQUIDITY (1_000) on the first deposit, so the
        // provider holds 999_000 / 1_000_000 of supply: 100 * 4_000 * 999_000 / 1_000_000 = 399_600.
        assert_eq!(claimed_after_end, 399_600, "rewards must be capped at end_time");

        // ── Case 2: duplicate claim after campaign has fully paid out ─────────────
        // All accrued rewards were just claimed; a second call must not double-pay.
        assert!(
            client.try_claim_rewards(&provider, &id).is_err(),
            "second claim should fail with 'no pending rewards'"
        );

        // ── Case 3: partial claim during campaign, remainder claimed after end ────
        let id2 = client.create_campaign(
            &gov_addr,
            &pool,
            &lp,
            &reward,
            &1_000,
            &5_000,
            &100,
            &1_000_000,
        );

        // First claim at t=3_000 (2_000 s into campaign).
        env.ledger().with_mut(|l| l.timestamp = 3_000);
        let partial = client.claim_rewards(&provider, &id2);
        // 100 * 2_000 * 999_000 / 1_000_000 = 199_800 (provider's 99.9% share of supply).
        assert_eq!(partial, 199_800, "partial claim should cover t=1_000..3_000");

        // Second claim at t=9_000 (well after end_time=5_000); should yield remaining 2_000 s.
        env.ledger().with_mut(|l| l.timestamp = 9_000);
        let remainder = client.claim_rewards(&provider, &id2);
        // Cumulative share at end_time is 399_600; 199_800 already claimed, leaving 199_800.
        assert_eq!(remainder, 199_800, "remainder should cover t=3_000..5_000");

        // Third call: nothing left to claim.
        assert!(
            client.try_claim_rewards(&provider, &id2).is_err(),
            "third claim should fail with 'no pending rewards'"
        );
    }

    /// Regression test for the leftover-funds issue: governance must be able to recover
    /// undistributed reward tokens after a campaign ends.
    #[test]
    fn test_recover_leftover_funds() {
        let (env, incentives, pool, lp, reward, provider, gov_addr) = setup();
        let client = IncentiveCampaignsClient::new(&env, &incentives);
        let treasury = Address::generate(&env);

        // Campaign: t=1_000..5_000, rate=100 → max_distributable = 100 * 4_000 = 400_000.
        let id = client.create_campaign(
            &gov_addr,
            &pool,
            &lp,
            &reward,
            &1_000,
            &5_000,
            &100,
            &1_000_000,
        );

        // ── Case 1: partial claim during campaign, then governance recovers the rest ──
        // LP claims at t=2_000 (1_000 s in). The AMM locks MINIMUM_LIQUIDITY on the first
        // deposit, so the provider's share is 100 * 1_000 * 999_000 / 1_000_000 = 99_900.
        env.ledger().with_mut(|l| l.timestamp = 2_000);
        let claimed = client.claim_rewards(&provider, &id);
        assert_eq!(claimed, 99_900);

        // Advance past end_time.
        env.ledger().with_mut(|l| l.timestamp = 8_000);

        // Governance recovers the unclaimed remainder: max_distributable - total_distributed
        // = 400_000 - 99_900 = 300_100.
        let recovered = client.recover_leftover_funds(&gov_addr, &id, &treasury);
        assert_eq!(recovered, 300_100, "should recover unclaimed rewards");

        // After recovery the campaign is inactive; LP cannot claim any more.
        assert!(
            client.try_claim_rewards(&provider, &id).is_err(),
            "claim after recovery must fail (campaign inactive)"
        );

        // ── Case 2: no claims at all → governance recovers the full budget ──────────
        let id2 = client.create_campaign(
            &gov_addr,
            &pool,
            &lp,
            &reward,
            &1_000,
            &5_000,
            &100,
            &1_000_000,
        );

        env.ledger().with_mut(|l| l.timestamp = 9_000);
        let full_recovery = client.recover_leftover_funds(&gov_addr, &id2, &treasury);
        assert_eq!(full_recovery, 100 * 4_000, "full budget should be recoverable when no claims made");

        // ── Case 3: recovery before end_time must be rejected ────────────────────────
        let id3 = client.create_campaign(
            &gov_addr,
            &pool,
            &lp,
            &reward,
            &1_000,
            &5_000,
            &100,
            &1_000_000,
        );

        env.ledger().with_mut(|l| l.timestamp = 3_000); // still inside campaign
        assert!(
            client
                .try_recover_leftover_funds(&gov_addr, &id3, &treasury)
                .is_err(),
            "recovery before end_time must be rejected"
        );
    }
}
