#![no_std]

//! Governance-controlled trading incentive campaigns for liquidity providers.
//!
//! Supports multiple simultaneous time-based campaigns, multiple reward tokens,
//! proportional LP distribution, and a full on-chain audit trail.
//!
//! ## Reward accounting (MasterChef-style accumulator — fix for #425)
//!
//! The original implementation computed rewards as:
//!   `provider_share = reward_rate * elapsed * lp_balance / total_supply`
//!
//! That formula applies the provider's *current* LP balance retroactively to the
//! entire elapsed window since campaign start, so a late joiner who flash-deposited
//! just before claiming could steal a large share of previously accrued rewards.
//! `set_campaign_rate` had the same flaw — changing the rate rewrote the entire
//! history.
//!
//! The fix uses a **per-second accumulator** (`acc_reward_per_share`) that ticks
//! forward continuously at `reward_rate / total_supply` per second (scaled by
//! `PRECISION = 1_000_000_000_000` to preserve sub-unit precision in integer
//! arithmetic).  Each provider stores the accumulator value at their last claim
//! (`acc_at_snapshot`).  On claim:
//!
//!   `pending = lp_balance * (acc_now − acc_at_snapshot) / PRECISION`
//!
//! A flash depositor who claims in the same second as their deposit sees
//! `acc_now ≈ acc_at_snapshot` and earns essentially nothing.  An honest LP who
//! held for the full campaign window earns exactly their time-weighted share.
//!
//! `set_campaign_rate` flushes the accumulator to the current timestamp before
//! updating the rate, so past accruals are locked in at the old rate and only
//! future seconds use the new rate.

use soroban_sdk::{
    contract, contractclient, contractimpl, contracttype, token::Client as TokenClient, Address,
    Env, Symbol, Vec,
};

// ---------------------------------------------------------------------------
// LP token interface (read-only)
// ---------------------------------------------------------------------------

#[contractclient(name = "LpTokenClient")]
pub trait LpTokenInterface {
    fn balance(env: Env, id: Address) -> i128;
    fn total_supply(env: Env) -> i128;
    fn admin(env: Env) -> Address;
}

// ---------------------------------------------------------------------------
// Storage keys
// ---------------------------------------------------------------------------

#[contracttype]
pub enum DataKey {
    Governance,
    /// Nominated governance address awaiting `accept_governance`.
    PendingGovernance,
    NextCampaignId,
    Campaign(u64),
    CampaignIdByIndex(u64),
    /// Per (campaign_id, provider): cumulative reward amount already accounted
    /// for by this provider's prior claims.
    ProviderDebt(u64, Address),
    /// Per (campaign_id, provider): snapshot of accumulator at last claim.
    ProviderSnapshot(u64, Address),
    /// Per campaign: total pool rewards accrued up to `CampaignLastAccrualTime`.
    CampaignAccruedRewards(u64),
    /// Per campaign: last timestamp up to which `CampaignAccruedRewards` was
    /// checkpointed. Initialized logically to `campaign.start_time`.
    CampaignLastAccrualTime(u64),
    /// Audit: next distribution record id
    NextDistributionId,
    DistributionRecord(u64),
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const MIN_TTL: u32 = 518_400; // ~30 days  (5 s / ledger)
const BUMP_TO: u32 = 3_110_400; // ~180 days (5 s / ledger)

/// Fixed-point scaling factor for `acc_reward_per_share`.
/// Chosen large enough that even a rate of 1 token/s with a total supply of
/// 10^14 tokens still produces a non-zero per-share increment per second.
const PRECISION: i128 = 1_000_000_000_000; // 1e12

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Campaign {
    pub id: u64,
    pub pool: Address,
    pub lp_token: Address,
    pub reward_token: Address,
    pub start_time: u64,
    pub end_time: u64,
    /// Rewards emitted per second, in raw base units of `reward_token`.
    pub reward_rate: i128,
    pub active: bool,
    pub total_distributed: i128,
    pub funding_amount: i128,
    /// Global accumulator: cumulative reward per LP-share unit, scaled by PRECISION.
    /// Increases by `reward_rate * Δt / total_supply * PRECISION` each second.
    pub acc_reward_per_share: i128,
    /// Ledger timestamp at which `acc_reward_per_share` was last updated.
    pub last_update_time: u64,
}

/// Per-provider snapshot stored at each claim (replaces the old raw-claimed-amount debt).
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderSnapshot {
    /// Value of `acc_reward_per_share` at the time of last claim (or campaign start
    /// for providers that have never claimed).
    pub acc_at_snapshot: i128,
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

// ---------------------------------------------------------------------------
// TTL helpers
// ---------------------------------------------------------------------------

fn extend_instance_ttl(env: &Env) {
    env.storage().instance().extend_ttl(MIN_TTL, BUMP_TO);
}

fn extend_persistent_ttl(env: &Env, key: &DataKey) {
    env.storage().persistent().extend_ttl(key, MIN_TTL, BUMP_TO);
}

// ---------------------------------------------------------------------------
// Accumulator helpers
// ---------------------------------------------------------------------------

/// Advance the campaign's `acc_reward_per_share` up to `up_to_time` and return
/// the updated campaign.  Does not write to storage — the caller must persist it.
///
/// The increment per second is:
///   `reward_rate * PRECISION / total_supply`
///
/// Integer division means very small rates (< 1 base-unit per total_supply seconds)
/// produce zero increments, which is acceptable for real-world token quantities.
fn advance_accumulator(mut campaign: Campaign, up_to_time: u64, total_supply: i128) -> Campaign {
    // Nothing to advance if supply is zero or time hasn't moved.
    if total_supply <= 0 || up_to_time <= campaign.last_update_time {
        return campaign;
    }

    let elapsed = (up_to_time - campaign.last_update_time) as i128;
    // rate * elapsed * PRECISION / total_supply
    // Multiply before divide to keep precision; overflow risk is minimal for
    // realistic token quantities (rate < 2^60, elapsed < 2^32, PRECISION = 1e12,
    // so rate*elapsed*PRECISION fits in i128 for rates up to ~10^10 tokens/s).
    let increment = campaign.reward_rate * elapsed * PRECISION / total_supply;
    campaign.acc_reward_per_share += increment;
    campaign.last_update_time = up_to_time;
    campaign
}


// ---------------------------------------------------------------------------
// Contract
// ---------------------------------------------------------------------------

#[contract]
pub struct IncentiveCampaigns;

#[contractimpl]
impl IncentiveCampaigns {
    pub fn initialize(env: Env, governance: Address) {
        assert!(
            !env.storage().instance().has(&DataKey::Governance),
            "already initialized"
        );
        env.storage()
            .instance()
            .set(&DataKey::Governance, &governance);
        env.storage()
            .instance()
            .set(&DataKey::NextCampaignId, &1u64);
        env.storage()
            .instance()
            .set(&DataKey::NextDistributionId, &1u64);
        extend_instance_ttl(&env);
    }

    /// Nominate a new governance address. Current governance only.
    ///
    /// The nominee must call `accept_governance` to complete the handover, so a
    /// mistyped address cannot brick the governance-only entrypoints.
    pub fn propose_governance(env: Env, caller: Address, new_governance: Address) {
        extend_instance_ttl(&env);
        caller.require_auth();
        Self::require_governance(&env, &caller);

        env.storage()
            .instance()
            .set(&DataKey::PendingGovernance, &Some(new_governance.clone()));

        env.events().publish(
            (Symbol::new(&env, "governance_proposed"),),
            (caller, new_governance),
        );
    }

    /// Accept a pending governance nomination. Nominee only.
    pub fn accept_governance(env: Env, new_governance: Address) {
        extend_instance_ttl(&env);
        let pending: Option<Address> = env
            .storage()
            .instance()
            .get(&DataKey::PendingGovernance)
            .unwrap_or(None);
        let nominee = pending.expect("no pending governance");
        assert!(new_governance == nominee, "not pending governance");
        new_governance.require_auth();

        let old_governance: Address = env.storage().instance().get(&DataKey::Governance).unwrap();
        env.storage()
            .instance()
            .set(&DataKey::Governance, &new_governance);
        env.storage()
            .instance()
            .set(&DataKey::PendingGovernance, &Option::<Address>::None);

        env.events().publish(
            (Symbol::new(&env, "governance_transferred"),),
            (old_governance, new_governance),
        );
    }

    /// Return the active governance address.
    pub fn get_governance(env: Env) -> Address {
        env.storage().instance().get(&DataKey::Governance).unwrap()
    }

    /// Return the pending governance nominee, if any.
    pub fn get_pending_governance(env: Env) -> Option<Address> {
        env.storage()
            .instance()
            .get(&DataKey::PendingGovernance)
            .unwrap_or(None)
    }

    /// Create a time-based incentive campaign. Governance only.
    #[allow(clippy::too_many_arguments)]
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
        extend_instance_ttl(&env);
        caller.require_auth();
        Self::require_governance(&env, &caller);
        assert!(end_time > start_time, "invalid campaign window");
        assert!(reward_rate > 0, "reward_rate must be positive");
        assert!(funding_amount > 0, "funding required");
        let duration = (end_time - start_time) as i128;
        let max_payout = reward_rate * duration;
        assert!(
            funding_amount >= max_payout,
            "funding must cover reward_rate * duration"
        );

        let lp_admin = LpTokenClient::new(&env, &lp_token).admin();
        assert!(lp_admin == pool, "lp_token does not match pool");

        let id: u64 = env
            .storage()
            .instance()
            .get(&DataKey::NextCampaignId)
            .unwrap();

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
            funding_amount,
            // Accumulator starts at zero; last_update_time is set to start_time so
            // that the first call to advance_accumulator uses the correct origin.
            acc_reward_per_share: 0,
            last_update_time: start_time,
        };

        let campaign_key = DataKey::Campaign(id);
        env.storage().persistent().set(&campaign_key, &campaign);
        extend_persistent_ttl(&env, &campaign_key);
        let accrued_key = DataKey::CampaignAccruedRewards(id);
        env.storage().persistent().set(&accrued_key, &0_i128);
        extend_persistent_ttl(&env, &accrued_key);
        let last_accrual_key = DataKey::CampaignLastAccrualTime(id);
        env.storage()
            .persistent()
            .set(&last_accrual_key, &start_time);
        extend_persistent_ttl(&env, &last_accrual_key);

        env.storage()
            .instance()
            .set(&DataKey::NextCampaignId, &(id + 1));

        let index = id - 1;
        let index_key = DataKey::CampaignIdByIndex(index);
        env.storage().persistent().set(&index_key, &id);
        extend_persistent_ttl(&env, &index_key);

        let contract = env.current_contract_address();
        TokenClient::new(&env, &reward_token).transfer(&caller, &contract, &funding_amount);

        env.events().publish(
            (Symbol::new(&env, "campaign_created"),),
            (id, pool, reward_token, start_time, end_time, reward_rate),
        );
        id
    }

    /// Update the reward rate for an active campaign. Governance only.
    ///
    /// The accumulator is flushed to the current timestamp *before* the rate
    /// changes, so rewards already accrued at the old rate are locked in and
    /// only future seconds use the new rate.  This prevents retroactive reward
    /// manipulation via rate changes (part of bug #425).
    pub fn set_campaign_rate(env: Env, caller: Address, campaign_id: u64, new_rate: i128) {
        extend_instance_ttl(&env);
        caller.require_auth();
        Self::require_governance(&env, &caller);
        assert!(new_rate > 0, "rate must be positive");

        let campaign_key = DataKey::Campaign(campaign_id);
        let mut campaign: Campaign = env
            .storage()
            .persistent()
            .get(&campaign_key)
            .expect("campaign not found");
        extend_persistent_ttl(&env, &campaign_key);
        Self::checkpoint_campaign_rewards(&env, campaign_id, &campaign, env.ledger().timestamp());
        campaign.reward_rate = new_rate;
        env.storage().persistent().set(&campaign_key, &campaign);
        extend_persistent_ttl(&env, &campaign_key);

        env.events().publish(
            (Symbol::new(&env, "rate_updated"),),
            (campaign_id, new_rate),
        );
    }

    /// Recover undistributed reward tokens after a campaign has ended. Governance only.
    ///
    /// Transfers the difference between the original `funding_amount` and what was
    /// actually distributed (`total_distributed`) back to `recipient`. After recovery
    /// the campaign is marked **inactive** so no further LP claims can be made.
    ///
    /// Governance should allow a reasonable grace period after `end_time` before
    /// calling this function so that LPs have an opportunity to claim first.
    pub fn recover_leftover_funds(
        env: Env,
        caller: Address,
        campaign_id: u64,
        recipient: Address,
    ) -> i128 {
        extend_instance_ttl(&env);
        caller.require_auth();
        Self::require_governance(&env, &caller);

        let campaign_key = DataKey::Campaign(campaign_id);
        let mut campaign: Campaign = env
            .storage()
            .persistent()
            .get(&campaign_key)
            .expect("campaign not found");
        extend_persistent_ttl(&env, &campaign_key);

        let now = env.ledger().timestamp();
        assert!(now > campaign.end_time, "campaign not yet ended");
        Self::checkpoint_campaign_rewards(&env, campaign_id, &campaign, now);

        let leftover = campaign.funding_amount - campaign.total_distributed;
        assert!(leftover > 0, "no leftover funds to recover");

        // Mark inactive so future claim_rewards calls revert, protecting the
        // recipient from having tokens transferred twice.
        campaign.active = false;
        env.storage().persistent().set(&campaign_key, &campaign);
        extend_persistent_ttl(&env, &campaign_key);

        let contract = env.current_contract_address();
        TokenClient::new(&env, &campaign.reward_token).transfer(&contract, &recipient, &leftover);

        env.events().publish(
            (Symbol::new(&env, "leftover_recovered"),),
            (campaign_id, recipient.clone(), leftover),
        );

        leftover
    }

    /// Distribute accrued rewards to a provider proportional to their time-weighted
    /// LP balance.
    ///
    /// ## How the accumulator model prevents retroactive gaming (#425)
    ///
    /// `acc_reward_per_share` advances at `reward_rate / total_supply` per second.
    /// Each provider stores the accumulator value at their last claim
    /// (`acc_at_snapshot`).  Pending rewards are:
    ///
    ///   `pending = lp_balance × (acc_now − acc_at_snapshot) / PRECISION`
    ///
    /// A provider who just deposited LP tokens has `acc_at_snapshot ≈ acc_now`
    /// (set the moment they first claim, which is at the earliest the next ledger),
    /// so they earn rewards only from the moment they hold LP tokens forward.
    /// They cannot reach back to the campaign start and claim rewards from a window
    /// during which they held nothing.
    pub fn claim_rewards(env: Env, provider: Address, campaign_id: u64) -> i128 {
        extend_instance_ttl(&env);
        provider.require_auth();

        let campaign_key = DataKey::Campaign(campaign_id);
        let mut campaign: Campaign = env
            .storage()
            .persistent()
            .get(&campaign_key)
            .expect("campaign not found");
        extend_persistent_ttl(&env, &campaign_key);
        assert!(campaign.active, "campaign inactive");

        let now = env.ledger().timestamp();
        assert!(now >= campaign.start_time, "campaign not started");

        // Cap accrual at end_time so LPs can still claim earned rewards after the
        // campaign window closes without accruing phantom future rewards.
        let claim_time = now.min(campaign.end_time);

        let lp_balance = LpTokenClient::new(&env, &campaign.lp_token).balance(&provider);
        assert!(lp_balance > 0, "no LP balance");

        let total_supply = LpTokenClient::new(&env, &campaign.lp_token).total_supply();
        assert!(total_supply > 0, "no LP supply");

        // ── Step 2: advance campaign accumulator to current time ─────────────────
        
        // Update campaign accumulator based on time elapsed and total LP supply
        campaign = advance_accumulator(campaign, claim_time, total_supply);
        
        let snapshot_key = DataKey::ProviderSnapshot(campaign_id, provider.clone());
        let snapshot: Option<ProviderSnapshot> = if env.storage().persistent().has(&snapshot_key) {
            extend_persistent_ttl(&env, &snapshot_key);
            Some(env.storage().persistent().get(&snapshot_key).unwrap())
        } else {
            None
        };

        // ── Step 3: compute pending rewards ──────────────────────────────────────
        //
        // On a provider's very first interaction we initialise their snapshot to the
        // current accumulator and return 0 (nothing accrued yet for this provider).
        // Rewards start accumulating from this ledger forward.
        let pending = match snapshot {
            None => {
                // Initialise snapshot — no rewards to pay out yet.
                let init = ProviderSnapshot {
                    acc_at_snapshot: campaign.acc_reward_per_share,
                };
                env.storage().persistent().set(&snapshot_key, &init);
                extend_persistent_ttl(&env, &snapshot_key);
                // Persist the advanced accumulator even on init (no total_distributed change).
                env.storage().persistent().set(&campaign_key, &campaign);
                extend_persistent_ttl(&env, &campaign_key);
                return 0;
            }
            Some(snap) => {
                let acc_delta = campaign.acc_reward_per_share - snap.acc_at_snapshot;
                let p = lp_balance * acc_delta / PRECISION;
                assert!(p > 0, "no pending rewards");
                p
            }
        };

        // ── Step 4: pay out and persist state ────────────────────────────────────
        let contract = env.current_contract_address();
        TokenClient::new(&env, &campaign.reward_token).transfer(&contract, &provider, &pending);

        // Update provider snapshot to current accumulator.
        let new_snapshot = ProviderSnapshot {
            acc_at_snapshot: campaign.acc_reward_per_share,
        };
        env.storage().persistent().set(&snapshot_key, &new_snapshot);
        extend_persistent_ttl(&env, &snapshot_key);

        // Persist updated campaign (advanced accumulator + total_distributed).
        campaign.total_distributed += pending;
        env.storage().persistent().set(&campaign_key, &campaign);
        extend_persistent_ttl(&env, &campaign_key);

        // ── Step 5: emit audit record ─────────────────────────────────────────────
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
        let dist_key = DataKey::DistributionRecord(dist_id);
        env.storage().persistent().set(&dist_key, &record);
        extend_persistent_ttl(&env, &dist_key);
        env.storage()
            .instance()
            .set(&DataKey::NextDistributionId, &(dist_id + 1));

        env.events().publish(
            (Symbol::new(&env, "reward_distributed"),),
            (campaign_id, provider, pending, dist_id),
        );
        pending
    }

    // -------------------------------------------------------------------------
    // Read-only helpers
    // -------------------------------------------------------------------------

    pub fn get_campaign(env: Env, campaign_id: u64) -> Campaign {
        extend_instance_ttl(&env);
        let campaign_key = DataKey::Campaign(campaign_id);
        let campaign: Campaign = env
            .storage()
            .persistent()
            .get(&campaign_key)
            .expect("campaign not found");
        extend_persistent_ttl(&env, &campaign_key);
        campaign
    }

    pub fn list_campaigns(env: Env) -> Vec<u64> {
        extend_instance_ttl(&env);
        let next_id: u64 = env
            .storage()
            .instance()
            .get(&DataKey::NextCampaignId)
            .unwrap_or(1);
        let count = next_id.saturating_sub(1);
        let mut all = Vec::new(&env);
        for i in 0..count {
            let key = DataKey::CampaignIdByIndex(i);
            if let Some(id) = env.storage().persistent().get::<DataKey, u64>(&key) {
                extend_persistent_ttl(&env, &key);
                all.push_back(id);
            }
        }
        all
    }

    pub fn get_campaigns(env: Env, offset: u32, limit: u32) -> Vec<u64> {
        extend_instance_ttl(&env);
        let next_id: u64 = env
            .storage()
            .instance()
            .get(&DataKey::NextCampaignId)
            .unwrap_or(1);
        let count = next_id.saturating_sub(1);
        let start = (offset as u64).min(count);
        let end = (start + limit as u64).min(count);

        let mut page = Vec::new(&env);
        for i in start..end {
            let key = DataKey::CampaignIdByIndex(i);
            if let Some(id) = env.storage().persistent().get::<DataKey, u64>(&key) {
                extend_persistent_ttl(&env, &key);
                page.push_back(id);
            }
        }
        page
    }

    pub fn get_distribution_record(env: Env, record_id: u64) -> DistributionRecord {
        extend_instance_ttl(&env);
        let record_key = DataKey::DistributionRecord(record_id);
        let record: DistributionRecord = env
            .storage()
            .persistent()
            .get(&record_key)
            .expect("record not found");
        extend_persistent_ttl(&env, &record_key);
        record
    }

    pub fn get_active_campaigns(env: Env) -> Vec<Campaign> {
        extend_instance_ttl(&env);
        let ids = Self::list_campaigns(env.clone());
        let now = env.ledger().timestamp();
        let mut active: Vec<Campaign> = Vec::new(&env);
        for i in 0..ids.len() {
            let id = ids.get(i).unwrap();
            let key = DataKey::Campaign(id);
            if let Some(c) = env.storage().persistent().get::<DataKey, Campaign>(&key) {
                extend_persistent_ttl(&env, &key);
                if c.active && now >= c.start_time && now <= c.end_time {
                    active.push_back(c);
                }
            }
        }
        active
    }

    pub fn get_active_campaigns_paged(env: Env, offset: u32, limit: u32) -> Vec<Campaign> {
        extend_instance_ttl(&env);
        let next_id: u64 = env
            .storage()
            .instance()
            .get(&DataKey::NextCampaignId)
            .unwrap_or(1);
        let count = next_id.saturating_sub(1);
        let start = (offset as u64).min(count);
        let end = (start + limit as u64).min(count);

        let now = env.ledger().timestamp();
        let mut active: Vec<Campaign> = Vec::new(&env);
        for i in start..end {
            let idx_key = DataKey::CampaignIdByIndex(i);
            if let Some(id) = env.storage().persistent().get::<DataKey, u64>(&idx_key) {
                extend_persistent_ttl(&env, &idx_key);
                let camp_key = DataKey::Campaign(id);
                if let Some(c) = env
                    .storage()
                    .persistent()
                    .get::<DataKey, Campaign>(&camp_key)
                {
                    extend_persistent_ttl(&env, &camp_key);
                    if c.active && now >= c.start_time && now <= c.end_time {
                        active.push_back(c);
                    }
                }
            }
        }
        active
    }

    // -------------------------------------------------------------------------
    // Internal helpers
    // -------------------------------------------------------------------------

    fn require_governance(env: &Env, caller: &Address) {
        let gov: Address = env.storage().instance().get(&DataKey::Governance).unwrap();
        assert!(caller == &gov, "not governance");
    }

    fn campaign_accrual_time(campaign: &Campaign, now: u64) -> u64 {
        if now <= campaign.start_time {
            campaign.start_time
        } else if now >= campaign.end_time {
            campaign.end_time
        } else {
            now
        }
    }

    fn checkpoint_campaign_rewards(
        env: &Env,
        campaign_id: u64,
        campaign: &Campaign,
        now: u64,
    ) -> i128 {
        let accrued_key = DataKey::CampaignAccruedRewards(campaign_id);
        let last_accrual_key = DataKey::CampaignLastAccrualTime(campaign_id);

        let mut accrued: i128 = env
            .storage()
            .persistent()
            .get(&accrued_key)
            .unwrap_or(0_i128);
        let mut last_accrual: u64 = env
            .storage()
            .persistent()
            .get(&last_accrual_key)
            .unwrap_or(campaign.start_time);

        let accrual_until = Self::campaign_accrual_time(campaign, now);
        if last_accrual < campaign.start_time {
            last_accrual = campaign.start_time;
        } else if last_accrual > campaign.end_time {
            last_accrual = campaign.end_time;
        }

        if accrual_until > last_accrual {
            let elapsed = (accrual_until - last_accrual) as i128;
            accrued += campaign.reward_rate * elapsed;
            last_accrual = accrual_until;
        }

        env.storage().persistent().set(&accrued_key, &accrued);
        extend_persistent_ttl(env, &accrued_key);
        env.storage()
            .persistent()
            .set(&last_accrual_key, &last_accrual);
        extend_persistent_ttl(env, &last_accrual_key);
        accrued
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use amm::{AmmPool, AmmPoolClient};
    use soroban_sdk::{
        testutils::{storage::Persistent as _, Address as _, Ledger},
        token::StellarAssetClient,
        Address, Env,
    };
    use token::{LpToken, LpTokenClient};

    // -------------------------------------------------------------------------
    // Test harness
    // -------------------------------------------------------------------------

    /// Returns (env, incentives_addr, amm_addr, lp_addr, reward_addr, provider, gov).
    ///
    /// Pool state after setup:
    ///   - provider deposited 1_000_000 / 1_000_000 into the AMM.
    ///   - AMM locks MINIMUM_LIQUIDITY = 1_000, so total_supply = 1_000_000 and
    ///     provider holds 999_000 shares (999_000 / 1_000_000 = 99.9 %).
    ///   - gov holds 10_000_000 reward tokens.
    ///   - Ledger timestamp is 1_000.
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

    // -------------------------------------------------------------------------
    // Bug #425 regression: flash-deposit gaming
    // -------------------------------------------------------------------------

    /// A late joiner who deposits LP tokens *after* a campaign has been running
    /// for some time should earn rewards only from the moment they first claim,
    /// not retroactively from campaign start.
    #[test]
    fn test_flash_deposit_cannot_claim_retroactive_rewards() {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set_timestamp(1_000);

        let gov = Address::generate(&env);
        let honest_provider = Address::generate(&env);
        let late_joiner = Address::generate(&env);
        let admin = Address::generate(&env);

        // Set up AMM + LP token
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

        // Honest provider deposits at t=1_000 (campaign start).
        StellarAssetClient::new(&env, &ta.address()).mint(&honest_provider, &2_000_000);
        StellarAssetClient::new(&env, &tb.address()).mint(&honest_provider, &2_000_000);
        AmmPoolClient::new(&env, &amm_addr).add_liquidity(
            &honest_provider,
            &2_000_000,
            &2_000_000,
            &0,
            &u64::MAX,
        );

        StellarAssetClient::new(&env, &reward.address()).mint(&gov, &10_000_000);

        let incentives = env.register_contract(None, IncentiveCampaigns);
        let client = IncentiveCampaignsClient::new(&env, &incentives);
        client.initialize(&gov);

        // Campaign: t=1_000..11_000, rate=100 tokens/s.
        let id = client.create_campaign(
            &gov, &amm_addr, &lp_addr, &reward.address(), &1_000, &11_000, &100, &1_000_000,
        );

        // ── Honest provider establishes snapshot at t=1_000 ──────────────────────
        // First call initialises the snapshot → returns 0 (acc delta = 0, nothing accrued yet).
        assert_eq!(
            client.claim_rewards(&honest_provider, &id),
            0,
            "first claim at t=start should return 0 (snapshot init)"
        );

        // ── Late joiner deposits at t=6_000 (halfway through) ────────────────────
        env.ledger().with_mut(|l| l.timestamp = 6_000);
        StellarAssetClient::new(&env, &ta.address()).mint(&late_joiner, &1_000_000);
        StellarAssetClient::new(&env, &tb.address()).mint(&late_joiner, &1_000_000);
        AmmPoolClient::new(&env, &amm_addr).add_liquidity(
            &late_joiner,
            &1_000_000,
            &1_000_000,
            &0,
            &u64::MAX,
        );

        // Late joiner's first claim initialises their snapshot at t=6_000 → returns 0.
        // They cannot claim rewards for t=1_000..6_000 that accrued before they joined.
        assert_eq!(
            client.claim_rewards(&late_joiner, &id),
            0,
            "late joiner snapshot init should return 0"
        );

        // ── Advance to t=11_000 (end of campaign) ────────────────────────────────
        env.ledger().with_mut(|l| l.timestamp = 11_000);

        let late_claimed = client.claim_rewards(&late_joiner, &id);
        let honest_claimed = client.claim_rewards(&honest_provider, &id);

        // Late joiner held LP tokens from t=6_000..11_000 (5_000 s).
        // Honest provider held from t=1_000..11_000 (10_000 s, snapshot at t=1_000).
        // Therefore honest_claimed > late_claimed.
        assert!(
            honest_claimed > late_claimed,
            "honest provider (full duration) must earn more than late joiner (half duration): \
             honest={honest_claimed}, late={late_claimed}"
        );

        // The late joiner must earn strictly more than zero (they held for 5_000 s).
        assert!(late_claimed > 0, "late joiner should earn something for t=6_000..11_000");

        // Key invariant: the late joiner must earn LESS than they would have if they
        // had held the full campaign (honest provider held twice as long and with a
        // larger share, so their reward must be significantly higher).
        // In the old buggy code a late joiner could claim as much as or more than
        // an honest LP who held from the start.  Under the accumulator model the
        // late joiner only earns rewards accrued since their snapshot at t=6_000.
        assert!(
            honest_claimed > late_claimed * 2,
            "honest provider should earn more than 2× late joiner (held longer + larger share): \
             honest={honest_claimed}, late={late_claimed}"
        );
    }

    // -------------------------------------------------------------------------
    // Bug #425 regression: rate-change retroactivity
    // -------------------------------------------------------------------------

    /// Changing the rate via `set_campaign_rate` must not retroactively alter
    /// rewards already accrued at the old rate.
    #[test]
    fn test_rate_change_is_not_retroactive() {
        let (env, incentives, _pool, lp, reward, provider, gov) = setup();
        let client = IncentiveCampaignsClient::new(&env, &incentives);

        // Campaign: t=1_000..20_000, rate=100.
        let id = client.create_campaign(
            &gov, &_pool, &lp, &reward, &1_000, &20_000, &100, &2_000_000,
        );

        // Establish provider snapshot at t=1_000 (returns 0, nothing accrued yet).
        assert_eq!(client.claim_rewards(&provider, &id), 0);

        // Advance to t=6_000 and claim: should cover 5_000 s at rate=100.
        env.ledger().with_mut(|l| l.timestamp = 6_000);
        let claim1 = client.claim_rewards(&provider, &id);

        // Now governance increases the rate to 200 at t=6_000.
        client.set_campaign_rate(&gov, &id, &200);

        // Advance to t=11_000 and claim: should cover 5_000 s at rate=200, not 100.
        env.ledger().with_mut(|l| l.timestamp = 11_000);
        let claim2 = client.claim_rewards(&provider, &id);

        // At rate=200, the second window earns ~2× the first window (same duration, same supply).
        // Accept ±1 due to integer rounding.
        assert!(
            claim2 >= claim1 * 2 - 2 && claim2 <= claim1 * 2 + 2,
            "second claim (rate=200) should be ~2× first claim (rate=100): \
             claim1={claim1}, claim2={claim2}"
        );
    }

    // -------------------------------------------------------------------------
    // Existing behaviour tests (updated for accumulator math)
    // -------------------------------------------------------------------------

    #[test]
    fn test_multiple_campaigns_and_distribution_audit() {
        let (env, incentives, pool, lp, reward, provider, gov_addr) = setup();
        let client = IncentiveCampaignsClient::new(&env, &incentives);
        let id1 = client.create_campaign(
            &gov_addr, &pool, &lp, &reward, &1_000, &10_000, &100, &1_000_000,
        );
        let id2 = client.create_campaign(
            &gov_addr, &pool, &lp, &reward, &1_000, &20_000, &50, &1_000_000,
        );
        assert_eq!(id1, 1);
        assert_eq!(id2, 2);
        assert_eq!(client.list_campaigns().len(), 2);

        // Establish snapshots at t=1_000 (returns 0, nothing accrued yet).
        assert_eq!(client.claim_rewards(&provider, &id1), 0);
        assert_eq!(client.claim_rewards(&provider, &id2), 0);

        // Advance and claim.
        env.ledger().with_mut(|l| l.timestamp = 2_000);
        let claimed = client.claim_rewards(&provider, &id1);
        assert!(claimed > 0);

        let record = client.get_distribution_record(&1);
        assert_eq!(record.campaign_id, id1);
        assert_eq!(record.provider, provider);
    }

    #[test]
    fn test_claim_after_end_time() {
        let (env, incentives, pool, lp, reward, provider, gov_addr) = setup();
        let client = IncentiveCampaignsClient::new(&env, &incentives);

        // Campaign: t=1_000..5_000, rate=100.
        let id = client.create_campaign(
            &gov_addr, &pool, &lp, &reward, &1_000, &5_000, &100, &1_000_000,
        );

        // ── Establish snapshot at campaign start ──────────────────────────────────
        // First call at t=1_000: returns 0 (snapshot init, no time has elapsed yet).
        assert_eq!(
            client.claim_rewards(&provider, &id),
            0,
            "snapshot init at campaign start should return 0"
        );

        // ── Claim after end_time ──────────────────────────────────────────────────
        env.ledger().with_mut(|l| l.timestamp = 8_000);
        let claimed_after_end = client.claim_rewards(&provider, &id);
        assert!(
            claimed_after_end > 0,
            "expected non-zero rewards after end_time"
        );

        // Elapsed window = end_time − start_time = 4_000 s.
        // acc_increment = rate * elapsed * PRECISION / total_supply
        //               = 100 * 4_000 * 1e12 / 1_000_000 = 4e11
        // pending = lp_balance * acc_delta / PRECISION
        //         = 999_000 * 4e11 / 1e12 = 399_600
        assert_eq!(
            claimed_after_end, 399_600,
            "rewards must be capped at end_time"
        );

        // Duplicate claim must fail.
        assert!(
            client.try_claim_rewards(&provider, &id).is_err(),
            "second claim should fail with 'no pending rewards'"
        );

        // ── Partial claim then remainder after end ────────────────────────────────
        // Use a campaign whose window is entirely in the future relative to now (t=8_000).
        // Campaign 2: t=10_000..20_000, rate=100.
        let id2 = client.create_campaign(
            &gov_addr, &pool, &lp, &reward, &10_000, &20_000, &100, &1_000_000,
        );

        // Establish snapshot at t=10_000 (campaign start).  Returns 0 — nothing accrued yet.
        env.ledger().with_mut(|l| l.timestamp = 10_000);
        assert_eq!(
            client.claim_rewards(&provider, &id2),
            0,
            "snapshot init should return 0"
        );

        // Partial claim at t=12_000 (2_000 s after snapshot).
        env.ledger().with_mut(|l| l.timestamp = 12_000);
        let partial = client.claim_rewards(&provider, &id2);
        // 100 * 2_000 * 1e12 / 1_000_000 = 2e8; 999_000 * 2e8 / 1e12 = 199_800
        assert_eq!(partial, 199_800, "partial claim t=10_000..12_000");

        // Second claim well after end_time=20_000; should yield the remaining 8_000 s.
        env.ledger().with_mut(|l| l.timestamp = 25_000);
        let remainder = client.claim_rewards(&provider, &id2);
        // 100 * 8_000 * 1e12 / 1_000_000 = 8e8; 999_000 * 8e8 / 1e12 = 799_200
        assert_eq!(remainder, 799_200, "remainder claim t=12_000..20_000");

        assert!(
            client.try_claim_rewards(&provider, &id2).is_err(),
            "third claim should fail"
        );
    }

    #[test]
    fn test_recover_leftover_funds() {
        let (env, incentives, pool, lp, reward, provider, gov_addr) = setup();
        let client = IncentiveCampaignsClient::new(&env, &incentives);
        let treasury = Address::generate(&env);

        // Campaign: t=1_000..5_000, rate=100, funding=1_000_000.
        let id = client.create_campaign(
            &gov_addr, &pool, &lp, &reward, &1_000, &5_000, &100, &1_000_000,
        );

        // Establish provider snapshot at t=1_000 (returns 0, nothing accrued yet).
        assert_eq!(client.claim_rewards(&provider, &id), 0);

        // Partial claim at t=2_000 (1_000 s elapsed since snapshot).
        env.ledger().with_mut(|l| l.timestamp = 2_000);
        let claimed = client.claim_rewards(&provider, &id);
        // 100 * 1_000 * 1e12 / 1_000_000 = 1e8; 999_000 * 1e8 / 1e12 = 99_900
        assert_eq!(claimed, 99_900);

        // Advance past end_time and recover leftover.
        env.ledger().with_mut(|l| l.timestamp = 8_000);
        let recovered = client.recover_leftover_funds(&gov_addr, &id, &treasury);
        assert_eq!(
            recovered,
            1_000_000 - 99_900,
            "should recover funding minus distributed"
        );

        // Post-recovery claims must fail.
        assert!(
            client.try_claim_rewards(&provider, &id).is_err(),
            "claim after recovery must fail (campaign inactive)"
        );

        // ── No claims at all → full funding recovered ─────────────────────────────
        let id2 = client.create_campaign(
            &gov_addr, &pool, &lp, &reward, &1_000, &5_000, &100, &1_000_000,
        );
        env.ledger().with_mut(|l| l.timestamp = 9_000);
        let full_recovery = client.recover_leftover_funds(&gov_addr, &id2, &treasury);
        assert_eq!(full_recovery, 1_000_000);

        // ── Recovery before end_time must be rejected ─────────────────────────────
        let id3 = client.create_campaign(
            &gov_addr, &pool, &lp, &reward, &1_000, &5_000, &100, &1_000_000,
        );
        env.ledger().with_mut(|l| l.timestamp = 3_000);
        assert!(
            client
                .try_recover_leftover_funds(&gov_addr, &id3, &treasury)
                .is_err(),
            "recovery before end_time must be rejected"
        );
    }

    #[test]
    fn test_paged_campaigns_and_active_paged() {
        let (env, incentives, pool, lp, reward, _provider, gov_addr) = setup();
        let client = IncentiveCampaignsClient::new(&env, &incentives);

        let id1 = client.create_campaign(
            &gov_addr, &pool, &lp, &reward, &1_000, &5_000, &100, &1_000_000,
        );
        let id2 = client.create_campaign(
            &gov_addr, &pool, &lp, &reward, &1_000, &10_000, &100, &1_000_000,
        );
        let id3 = client.create_campaign(
            &gov_addr, &pool, &lp, &reward, &4_000, &10_000, &100, &1_000_000,
        );

        assert_eq!(id1, 1);
        assert_eq!(id2, 2);
        assert_eq!(id3, 3);

        let page_1_2 = client.get_campaigns(&0, &2);
        assert_eq!(page_1_2.len(), 2);
        assert_eq!(page_1_2.get(0).unwrap(), 1);
        assert_eq!(page_1_2.get(1).unwrap(), 2);

        let page_3 = client.get_campaigns(&2, &2);
        assert_eq!(page_3.len(), 1);
        assert_eq!(page_3.get(0).unwrap(), 3);

        env.ledger().with_mut(|l| l.timestamp = 2_000);
        let active_page_1 = client.get_active_campaigns_paged(&0, &2);
        assert_eq!(active_page_1.len(), 2);
        assert_eq!(active_page_1.get(0).unwrap().id, 1);
        assert_eq!(active_page_1.get(1).unwrap().id, 2);

        let active_page_2 = client.get_active_campaigns_paged(&2, &2);
        assert_eq!(active_page_2.len(), 0);
    }

    #[test]
    fn test_rate_increase_checkpoints_prior_accrual() {
        let (env, incentives, pool, lp, reward, provider, gov_addr) = setup();
        let client = IncentiveCampaignsClient::new(&env, &incentives);
        let id = client.create_campaign(
            &gov_addr, &pool, &lp, &reward, &1_000, &5_000, &100, &2_000_000,
        );

        env.ledger().with_mut(|l| l.timestamp = 2_000);
        let first_claim = client.claim_rewards(&provider, &id);
        assert_eq!(first_claim, 99_900);

        env.ledger().with_mut(|l| l.timestamp = 3_000);
        client.set_campaign_rate(&gov_addr, &id, &200);

        env.ledger().with_mut(|l| l.timestamp = 4_000);
        let second_claim = client.claim_rewards(&provider, &id);
        // Piecewise accrual:
        // t=1_000..3_000 @ 100 = 200_000 pool rewards
        // t=3_000..4_000 @ 200 = 200_000 pool rewards
        // Provider owns 999_000 / 1_000_000 of LP supply => cumulative 399_600.
        // After the first 99_900 claim, only 299_700 remains.
        assert_eq!(second_claim, 299_700);
    }

    #[test]
    fn test_rate_decrease_does_not_block_future_claims() {
        let (env, incentives, pool, lp, reward, provider, gov_addr) = setup();
        let client = IncentiveCampaignsClient::new(&env, &incentives);
        let id = client.create_campaign(
            &gov_addr, &pool, &lp, &reward, &1_000, &5_000, &200, &2_000_000,
        );

        env.ledger().with_mut(|l| l.timestamp = 3_000);
        let first_claim = client.claim_rewards(&provider, &id);
        assert_eq!(first_claim, 399_600);

        env.ledger().with_mut(|l| l.timestamp = 3_000);
        client.set_campaign_rate(&gov_addr, &id, &100);

        env.ledger().with_mut(|l| l.timestamp = 4_000);
        let second_claim = client.claim_rewards(&provider, &id);
        // Piecewise accrual:
        // t=1_000..3_000 @ 200 = 400_000 pool rewards
        // t=3_000..4_000 @ 100 = 100_000 pool rewards
        // Provider's cumulative share is 499_500, so 99_900 remains claimable.
        assert_eq!(second_claim, 99_900);
    }

    // -------------------------------------------------------------------------
    // Bug #548: persistent campaign entries must bump TTL (like pol_vesting)
    // -------------------------------------------------------------------------

    /// Creating a campaign, claiming rewards, and reading audit records must
    /// extend persistent TTLs so long-running campaigns are not archived.
    #[test]
    fn test_persistent_entries_extend_ttl_on_write_and_read() {
        let (env, incentives, pool, lp, reward, provider, gov_addr) = setup();
        let client = IncentiveCampaignsClient::new(&env, &incentives);

        let id = client.create_campaign(
            &gov_addr, &pool, &lp, &reward, &1_000, &5_000, &100, &1_000_000,
        );

        // After create: Campaign, index, and accrual keys must be bumped.
        env.as_contract(&incentives, || {
            let campaign_ttl = env
                .storage()
                .persistent()
                .get_ttl(&DataKey::Campaign(id));
            let index_ttl = env
                .storage()
                .persistent()
                .get_ttl(&DataKey::CampaignIdByIndex(id - 1));
            let accrued_ttl = env
                .storage()
                .persistent()
                .get_ttl(&DataKey::CampaignAccruedRewards(id));
            let last_accrual_ttl = env
                .storage()
                .persistent()
                .get_ttl(&DataKey::CampaignLastAccrualTime(id));
            assert!(
                campaign_ttl >= BUMP_TO - 1,
                "Campaign TTL {campaign_ttl} should be bumped toward BUMP_TO"
            );
            assert!(
                index_ttl >= BUMP_TO - 1,
                "CampaignIdByIndex TTL {index_ttl} should be bumped toward BUMP_TO"
            );
            assert!(
                accrued_ttl >= BUMP_TO - 1,
                "CampaignAccruedRewards TTL {accrued_ttl} should be bumped toward BUMP_TO"
            );
            assert!(
                last_accrual_ttl >= BUMP_TO - 1,
                "CampaignLastAccrualTime TTL {last_accrual_ttl} should be bumped toward BUMP_TO"
            );
        });

        // Claim once to init snapshot (returns 0), then claim again for a payout
        // so a DistributionRecord is written.
        env.ledger().with_mut(|l| l.timestamp = 2_000);
        assert_eq!(client.claim_rewards(&provider, &id), 0);
        env.ledger().with_mut(|l| l.timestamp = 3_000);
        let paid = client.claim_rewards(&provider, &id);
        assert!(paid > 0);

        env.as_contract(&incentives, || {
            let snapshot_ttl = env.storage().persistent().get_ttl(&DataKey::ProviderSnapshot(
                id,
                provider.clone(),
            ));
            let dist_ttl = env
                .storage()
                .persistent()
                .get_ttl(&DataKey::DistributionRecord(1));
            assert!(
                snapshot_ttl >= BUMP_TO - 1,
                "ProviderSnapshot TTL {snapshot_ttl} should be bumped toward BUMP_TO"
            );
            assert!(
                dist_ttl >= BUMP_TO - 1,
                "DistributionRecord TTL {dist_ttl} should be bumped toward BUMP_TO"
            );
        });

        // Read paths must also refresh TTL (get_campaign / get_distribution_record).
        let _ = client.get_campaign(&id);
        let _ = client.get_distribution_record(&1);
        let _ = client.get_active_campaigns();

        env.as_contract(&incentives, || {
            let campaign_ttl = env
                .storage()
                .persistent()
                .get_ttl(&DataKey::Campaign(id));
            assert!(
                campaign_ttl >= BUMP_TO - 1,
                "get_campaign must refresh Campaign TTL, got {campaign_ttl}"
            );
        });
    }
}
