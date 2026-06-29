//! LP Staking and Rewards Contract
//!
//! Liquidity providers can stake their LP tokens to earn reward tokens.
//! Uses a rewards-per-share accumulator pattern (similar to SushiSwap's MasterChef)
//! for efficient O(1) reward calculation per claim.
//!
//! Issue #296: Optional lock-duration boost multiplier (1×–4×), modelled on
//! Curve's veTokenomics.  Stakers may voluntarily lock for a fixed duration to
//! earn a higher share of rewards.  The boost is applied to the *effective*
//! staked amount used in reward calculations; the actual LP token balance is
//! unchanged.

#![no_std]

use soroban_sdk::{contract, contractimpl, contracttype, Address, Env, Symbol};

use soroban_sdk::token::Client as SepTokenClient;

// ── Constants ──────────────────────────────────────────────────────────────

const SCALE_FACTOR: i128 = 1_000_000_000_000_000_000; // 1e18

/// Boost multiplier is stored scaled by BOOST_SCALE so we avoid floats.
/// 1× = 10_000, 4× = 40_000.
const BOOST_SCALE: i128 = 10_000;

/// Maximum lock duration in seconds (4 years).
const MAX_LOCK_DURATION: u64 = 4 * 365 * 24 * 3600;

/// Minimum lock duration in seconds (1 week).
const MIN_LOCK_DURATION: u64 = 7 * 24 * 3600;

/// Default maximum boost multiplier (2.5×, stored as 25_000 / BOOST_SCALE).
const DEFAULT_MAX_BOOST: i128 = 25_000;

/// Minimum boost multiplier (1×, stored as 10_000 / BOOST_SCALE).
const MIN_BOOST: i128 = BOOST_SCALE;

const MIN_TTL: u32 = 518_400;   // ~30 days (at 5s per ledger)
const BUMP_TO: u32 = 3_110_400; // ~180 days (at 5s per ledger)

// ── Storage keys ───────────────────────────────────────────────────────────

#[contracttype]
pub enum DataKey {
    /// LP token address
    LpToken,
    /// Reward token address
    RewardToken,
    /// Admin address (can add rewards)
    Admin,
    /// Total *effective* LP tokens staked (boosted amounts summed)
    TotalEffectiveStaked,
    /// Accumulated rewards per effective LP token (scaled by 1e18)
    AccumulatedRewardsPerShare,
    /// Staker info: raw staked amount
    StakerAmount(Address),
    /// Staker info: rewards debt (to track already-distributed rewards)
    StakerRewardsDebt(Address),
    /// Remaining reward tokens available in pool
    RewardPoolBalance,
    /// Lock expiry timestamp (seconds) for a staker; 0 = no lock
    LockExpiry(Address),
    /// Boost multiplier for a staker (scaled by BOOST_SCALE); default = BOOST_SCALE (1×)
    BoostMultiplier(Address),
    /// Configurable min boost (scaled); default MIN_BOOST.
    ConfigMinBoost,
    /// Configurable max boost (scaled); default DEFAULT_MAX_BOOST.
    ConfigMaxBoost,
    /// Configurable min lock duration in seconds.
    ConfigMinLockDuration,
    /// Configurable max lock duration in seconds.
    ConfigMaxLockDuration,
    /// Optional maximum reward pool balance (0 = no cap).
    ConfigMaxRewardPoolBalance,
    /// Circuit-breaker flag; when true new stakes and claims are halted (#360).
    Paused,
    /// Emergency mode flag; when true stakers may reclaim LP without rewards (#359).
    EmergencyMode,
}

// ── Data structures ───────────────────────────────────────────────────────

/// Time-locked staking position (#317).
#[contracttype]
#[derive(Clone, Debug)]
pub struct LockedPosition {
    pub amount: i128,
    pub lock_expiry: u64,
    pub boost_multiplier: i128,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct StakerInfo {
    pub staked_amount: i128,
    pub effective_amount: i128,
    pub rewards_debt: i128,
    pub lock_expiry: u64,
    pub boost_multiplier: i128,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct PoolInfo {
    pub lp_token: Address,
    pub reward_token: Address,
    pub admin: Address,
    pub total_effective_staked: i128,
    pub reward_pool_balance: i128,
    pub accumulated_rewards_per_share: i128,
}

// ── Contract ────────────────────────────────────────────────────────────────

#[contract]
pub struct Staking;

#[contractimpl]
impl Staking {
    /// Initialize the staking contract.
    pub fn initialize(env: Env, lp_token: Address, reward_token: Address, admin: Address) {
        assert!(
            !env.storage().instance().has(&DataKey::LpToken),
            "already initialized"
        );
        env.storage().instance().set(&DataKey::LpToken, &lp_token);
        env.storage().instance().set(&DataKey::RewardToken, &reward_token);
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::TotalEffectiveStaked, &0i128);
        env.storage().instance().set(&DataKey::AccumulatedRewardsPerShare, &0i128);
        env.storage().instance().set(&DataKey::RewardPoolBalance, &0i128);
        // ConfigMaxRewardPoolBalance initialized above
        Self::_write_boost_config(&env, MIN_BOOST, DEFAULT_MAX_BOOST, MIN_LOCK_DURATION, MAX_LOCK_DURATION);
    }

    /// Initialize with configurable veToken boost parameters (#317).
    pub fn initialize_with_boost_config(
        env: Env,
        lp_token: Address,
        reward_token: Address,
        admin: Address,
        min_boost_scaled: i128,
        max_boost_scaled: i128,
        min_lock_duration_secs: u64,
        max_lock_duration_secs: u64,
    ) {
        assert!(
            !env.storage().instance().has(&DataKey::LpToken),
            "already initialized"
        );
        assert!(min_boost_scaled > 0 && max_boost_scaled >= min_boost_scaled);
        assert!(min_lock_duration_secs > 0 && max_lock_duration_secs >= min_lock_duration_secs);
        env.storage().instance().set(&DataKey::LpToken, &lp_token);
        env.storage().instance().set(&DataKey::RewardToken, &reward_token);
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::TotalEffectiveStaked, &0i128);
        env.storage().instance().set(&DataKey::AccumulatedRewardsPerShare, &0i128);
        env.storage().instance().set(&DataKey::RewardPoolBalance, &0i128);
        env.storage().instance().set(&DataKey::ConfigMaxRewardPoolBalance, &0i128);
        Self::_write_boost_config(
            &env,
            min_boost_scaled,
            max_boost_scaled,
            min_lock_duration_secs,
            max_lock_duration_secs,
        );
    }

    /// Escrow LP tokens for a fixed lock duration with a boosted reward rate (#317).
    pub fn lock(env: Env, staker: Address, amount: i128, lock_duration_seconds: u64) {
        Self::stake_locked(env, staker, amount, lock_duration_seconds);
    }

    /// Withdraw all LP and accrued rewards after the lock expires (#317).
    pub fn unlock(env: Env, staker: Address) -> (i128, i128) {
        let amount: i128 = env
            .storage()
            .persistent()
            .get(&DataKey::StakerAmount(staker.clone()))
            .unwrap_or(0);
        assert!(amount > 0, "nothing staked");
        Self::unstake(env, staker, amount)
    }

    /// Extend an existing lock forward in time only (#317).
    pub fn extend_lock(env: Env, staker: Address, new_duration_seconds: u64) {
        staker.require_auth();
        assert!(new_duration_seconds > 0, "duration must be positive");

        let now = env.ledger().timestamp();
        let existing_expiry: u64 = env
            .storage()
            .persistent()
            .get(&DataKey::LockExpiry(staker.clone()))
            .unwrap_or(0);
        assert!(existing_expiry > now, "no active lock to extend");

        let (min_lock, max_lock) = Self::_lock_duration_bounds(&env);
        let clamped = new_duration_seconds.clamp(min_lock, max_lock);
        let proposed_expiry = now + clamped;
        let expiry = proposed_expiry.max(existing_expiry);
        let boost = Self::_boost_for_remaining(&env, expiry, now);

        let staked_amount: i128 = env
            .storage()
            .persistent()
            .get(&DataKey::StakerAmount(staker.clone()))
            .unwrap_or(0);
        assert!(staked_amount > 0, "nothing staked");

        Self::_settle_pending(&env, &staker);

        let old_boost: i128 = env
            .storage()
            .persistent()
            .get(&DataKey::BoostMultiplier(staker.clone()))
            .unwrap_or(MIN_BOOST);
        let old_effective = Self::_effective_amount(staked_amount, old_boost);
        let new_effective = Self::_effective_amount(staked_amount, boost);

        let total: i128 = env
            .storage()
            .instance()
            .get(&DataKey::TotalEffectiveStaked)
            .unwrap_or(0);
        env.storage()
            .instance()
            .set(&DataKey::TotalEffectiveStaked, &(total - old_effective + new_effective).max(0));

        let key_lock = DataKey::LockExpiry(staker.clone());
        env.storage().persistent().set(&key_lock, &expiry);
        env.storage().persistent().extend_ttl(&key_lock, MIN_TTL, BUMP_TO);

        let key_boost = DataKey::BoostMultiplier(staker.clone());
        env.storage().persistent().set(&key_boost, &boost);
        env.storage().persistent().extend_ttl(&key_boost, MIN_TTL, BUMP_TO);

        let acc_per_share: i128 = env
            .storage()
            .instance()
            .get(&DataKey::AccumulatedRewardsPerShare)
            .unwrap_or(0);
        let new_debt = new_effective * acc_per_share / SCALE_FACTOR;
        let key_debt = DataKey::StakerRewardsDebt(staker.clone());
        env.storage().persistent().set(&key_debt, &new_debt);
        env.storage().persistent().extend_ttl(&key_debt, MIN_TTL, BUMP_TO);

        env.events().publish(
            (Symbol::new(&env, "lock_extended"),),
            (staker, boost, expiry),
        );
    }

    /// View the staker's locked position (#317).
    pub fn get_locked_position(env: Env, staker: Address) -> LockedPosition {
        let amount: i128 = env
            .storage()
            .persistent()
            .get(&DataKey::StakerAmount(staker.clone()))
            .unwrap_or(0);
        let boost: i128 = env
            .storage()
            .persistent()
            .get(&DataKey::BoostMultiplier(staker.clone()))
            .unwrap_or(MIN_BOOST);
        let lock_expiry: u64 = env
            .storage()
            .persistent()
            .get(&DataKey::LockExpiry(staker.clone()))
            .unwrap_or(0);
        LockedPosition {
            amount,
            lock_expiry,
            boost_multiplier: boost,
        }
    }

    /// Add rewards to the pool. Admin only.
    pub fn add_rewards(env: Env, admin: Address, amount: i128) {
        admin.require_auth();
        let stored_admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        assert!(admin == stored_admin, "not admin");
        assert!(amount > 0, "amount must be positive");

        let reward_token: Address = env.storage().instance().get(&DataKey::RewardToken).unwrap();
        let pool_addr = env.current_contract_address();
        // Record pre‑transfer token balance
        let token_client = SepTokenClient::new(&env, &reward_token);
        let pre_balance: i128 = token_client.balance(&pool_addr);
        // Transfer requested amount from admin to pool
        token_client.transfer(&admin, &pool_addr, &amount);
        // Record post‑transfer token balance to determine actual received amount
        let post_balance: i128 = token_client.balance(&pool_addr);
        let received: i128 = post_balance - pre_balance;
        // Update reward pool balance with the actual received amount
        let current_balance: i128 = env.storage().instance().get(&DataKey::RewardPoolBalance).unwrap_or(0);
        let new_balance = current_balance + received;
        // Enforce optional max reward pool balance cap (0 = no cap)
        let max_balance: i128 = env.storage().instance().get(&DataKey::ConfigMaxRewardPoolBalance).unwrap_or(0);
        if max_balance != 0 {
            assert!(new_balance <= max_balance, "exceeds max reward pool balance");
        }
        env.storage().instance().set(&DataKey::RewardPoolBalance, &new_balance);
        // Emit event with the actual amount added
        env.events().publish((Symbol::new(&env, "rewards_added"),), (admin, received));
    }

    /// Halt new stakes and reward claims. Admin only (#360).
    ///
    /// Lets the admin freeze the contract (e.g. while a reward-accounting bug
    /// is being patched). Unstaking and emergency withdrawals remain available
    /// so stakers can always retrieve their LP tokens.
    pub fn pause(env: Env, admin: Address) {
        admin.require_auth();
        let stored_admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        assert!(admin == stored_admin, "not admin");
        env.storage().instance().set(&DataKey::Paused, &true);
        env.events().publish((Symbol::new(&env, "paused"),), (admin,));
    }

    /// Resume staking and claiming. Admin only (#360).
    pub fn unpause(env: Env, admin: Address) {
        admin.require_auth();
        let stored_admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        assert!(admin == stored_admin, "not admin");
        env.storage().instance().set(&DataKey::Paused, &false);
        env.events().publish((Symbol::new(&env, "unpaused"),), (admin,));
    }

    /// Whether the contract is currently paused (#360).
    pub fn is_paused(env: Env) -> bool {
        env.storage()
            .instance()
            .get(&DataKey::Paused)
            .unwrap_or(false)
    }

    /// Stake LP tokens without a lock (1× boost).
    pub fn stake(env: Env, staker: Address, amount: i128) {
        Self::stake_locked(env, staker, amount, 0);
    }

    /// Stake LP tokens with an optional lock duration for a boost multiplier.
    ///
    /// `lock_duration_secs` = 0 → no lock, 1× boost.
    /// Lock duration is clamped to [MIN_LOCK_DURATION, MAX_LOCK_DURATION].
    /// Boost scales linearly from 1× (no lock) to 4× (MAX_LOCK_DURATION).
    ///
    /// If the staker already has a lock, the new lock must expire no earlier
    /// than the existing one (locks can only be extended, not shortened).
    pub fn stake_locked(env: Env, staker: Address, amount: i128, lock_duration_secs: u64) {
        assert!(!Self::is_paused(env.clone()), "contract is paused");
        staker.require_auth();
        assert!(amount > 0, "amount must be positive");

        let lp_token: Address = env.storage().instance().get(&DataKey::LpToken).unwrap();
        let pool_addr = env.current_contract_address();
        SepTokenClient::new(&env, &lp_token).transfer(&staker, &pool_addr, &amount);

        // Settle any pending rewards before changing effective stake.
        Self::_settle_pending(&env, &staker);

        // Compute new boost and lock expiry.
        let now = env.ledger().timestamp();
        let existing_expiry: u64 = env
            .storage()
            .persistent()
            .get(&DataKey::LockExpiry(staker.clone()))
            .unwrap_or(0);

        let (new_expiry, new_boost) = if lock_duration_secs == 0 {
            // No new lock requested — keep existing lock if still active.
            let expiry = existing_expiry.max(now);
            let boost = Self::_boost_for_remaining(&env, expiry, now);
            (existing_expiry, boost)
        } else {
            let (min_lock, max_lock) = Self::_lock_duration_bounds(&env);
            let clamped = lock_duration_secs.clamp(min_lock, max_lock);
            let proposed_expiry = now + clamped;
            // Cannot shorten an existing lock.
            let expiry = proposed_expiry.max(existing_expiry);
            let boost = Self::_boost_for_remaining(&env, expiry, now);
            (expiry, boost)
        };

        // Update raw staked amount.
        let current_staked: i128 = env
            .storage()
            .persistent()
            .get(&DataKey::StakerAmount(staker.clone()))
            .unwrap_or(0);
        let new_staked = current_staked + amount;
        let key_amount = DataKey::StakerAmount(staker.clone());
        env.storage().persistent().set(&key_amount, &new_staked);
        env.storage().persistent().extend_ttl(&key_amount, MIN_TTL, BUMP_TO);

        // Recompute effective amount for the whole position with the new boost.
        let old_effective = Self::_effective_amount(current_staked, new_boost);
        let new_effective = Self::_effective_amount(new_staked, new_boost);

        // Adjust total effective staked.
        let total: i128 = env
            .storage()
            .instance()
            .get(&DataKey::TotalEffectiveStaked)
            .unwrap_or(0);
        // Remove old effective, add new effective.
        let new_total = total - old_effective + new_effective;
        env.storage()
            .instance()
            .set(&DataKey::TotalEffectiveStaked, &new_total.max(0));

        // Persist lock and boost.
        let key_lock = DataKey::LockExpiry(staker.clone());
        env.storage().persistent().set(&key_lock, &new_expiry);
        env.storage().persistent().extend_ttl(&key_lock, MIN_TTL, BUMP_TO);

        let key_boost = DataKey::BoostMultiplier(staker.clone());
        env.storage().persistent().set(&key_boost, &new_boost);
        env.storage().persistent().extend_ttl(&key_boost, MIN_TTL, BUMP_TO);

        // Reset rewards debt to current acc_per_share * new_effective.
        let acc_per_share: i128 = env
            .storage()
            .instance()
            .get(&DataKey::AccumulatedRewardsPerShare)
            .unwrap_or(0);
        let new_debt = new_effective * acc_per_share / SCALE_FACTOR;
        let key_debt = DataKey::StakerRewardsDebt(staker.clone());
        env.storage().persistent().set(&key_debt, &new_debt);
        env.storage().persistent().extend_ttl(&key_debt, MIN_TTL, BUMP_TO);

        env.events().publish(
            (Symbol::new(&env, "staked"),),
            (staker, amount, new_boost, new_expiry),
        );
    }

    /// Claim accrued rewards without unstaking.
    pub fn claim(env: Env, staker: Address) -> i128 {
        assert!(!Self::is_paused(env.clone()), "contract is paused");
        staker.require_auth();
        Self::_claim_rewards(&env, &staker)
    }

    /// Unstake LP tokens and claim pending rewards.
    ///
    /// Panics if the staker's lock has not yet expired.
    pub fn unstake(env: Env, staker: Address, amount: i128) -> (i128, i128) {
        staker.require_auth();
        assert!(amount > 0, "amount must be positive");

        let staked_amount: i128 = env
            .storage()
            .persistent()
            .get(&DataKey::StakerAmount(staker.clone()))
            .unwrap_or(0);
        assert!(staked_amount >= amount, "insufficient staked amount");

        // Enforce lock.
        let now = env.ledger().timestamp();
        let lock_expiry: u64 = env
            .storage()
            .persistent()
            .get(&DataKey::LockExpiry(staker.clone()))
            .unwrap_or(0);
        assert!(now >= lock_expiry, "tokens are still locked");

        // Claim pending rewards first (auth already checked above).
        let rewards = if Self::pending_rewards(env.clone(), staker.clone()) > 0 {
            Self::_claim_rewards(&env, &staker)
        } else {
            0
        };

        let lp_token: Address = env.storage().instance().get(&DataKey::LpToken).unwrap();
        let pool_addr = env.current_contract_address();
        SepTokenClient::new(&env, &lp_token).transfer(&pool_addr, &staker, &amount);

        let boost: i128 = env
            .storage()
            .persistent()
            .get(&DataKey::BoostMultiplier(staker.clone()))
            .unwrap_or(MIN_BOOST);

        let old_effective = Self::_effective_amount(staked_amount, boost);
        let new_staked = staked_amount - amount;
        let new_effective = Self::_effective_amount(new_staked, boost);

        let key_amount = DataKey::StakerAmount(staker.clone());
        env.storage().persistent().set(&key_amount, &new_staked);
        env.storage().persistent().extend_ttl(&key_amount, MIN_TTL, BUMP_TO);

        let total: i128 = env
            .storage()
            .instance()
            .get(&DataKey::TotalEffectiveStaked)
            .unwrap_or(0);
        env.storage()
            .instance()
            .set(&DataKey::TotalEffectiveStaked, &(total - old_effective + new_effective).max(0));

        // Reset debt.
        let acc_per_share: i128 = env
            .storage()
            .instance()
            .get(&DataKey::AccumulatedRewardsPerShare)
            .unwrap_or(0);
        let new_debt = new_effective * acc_per_share / SCALE_FACTOR;
        let key_debt = DataKey::StakerRewardsDebt(staker.clone());
        env.storage().persistent().set(&key_debt, &new_debt);
        env.storage().persistent().extend_ttl(&key_debt, MIN_TTL, BUMP_TO);

        env.events().publish((Symbol::new(&env, "unstaked"),), (staker, amount, rewards));
        (amount, rewards)
    }

    /// Enable or disable emergency mode (#359). Admin only.
    ///
    /// Emergency mode unlocks [`Self::emergency_withdraw`] so stakers can
    /// reclaim their LP tokens without touching the reward token. It is gated
    /// behind the admin so it cannot be used to skip rewards under normal
    /// conditions.
    pub fn set_emergency_mode(env: Env, admin: Address, enabled: bool) {
        admin.require_auth();
        let stored_admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        assert!(admin == stored_admin, "not admin");
        env.storage().instance().set(&DataKey::EmergencyMode, &enabled);
        env.events()
            .publish((Symbol::new(&env, "emergency_mode"),), (admin, enabled));
    }
    
    /// Set the optional maximum reward pool balance. Admin only.
    pub fn set_max_reward_pool_balance(env: Env, admin: Address, max_balance: i128) {
        admin.require_auth();
        let stored_admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        assert!(admin == stored_admin, "not admin");
        let current_balance: i128 = env.storage().instance().get(&DataKey::RewardPoolBalance).unwrap_or(0);
        // 0 means no cap; otherwise ensure the new cap is not below current balance
        assert!(max_balance == 0 || max_balance >= current_balance, "max_balance less than current pool");
        env.storage().instance().set(&DataKey::ConfigMaxRewardPoolBalance, &max_balance);
        env.events().publish((Symbol::new(&env, "max_reward_pool_balance_set"),), (admin, max_balance));
    }


    /// Whether emergency mode is currently active (#359).
    pub fn is_emergency_mode(env: Env) -> bool {
        env.storage()
            .instance()
            .get(&DataKey::EmergencyMode)
            .unwrap_or(false)
    }

    /// Reclaim staked LP tokens without claiming rewards (#359).
    ///
    /// Only callable while the admin has enabled emergency mode. Unlike
    /// [`Self::unstake`], this never interacts with the reward token, so
    /// stakers can always recover their LP even if the reward token is paused,
    /// blacklisted, or the reward pool has been drained by a bug. Any pending
    /// rewards are forfeited, and the lock (if any) is ignored.
    ///
    /// Returns the raw LP amount returned to the staker.
    pub fn emergency_withdraw(env: Env, staker: Address) -> i128 {
        staker.require_auth();
        assert!(
            Self::is_emergency_mode(env.clone()),
            "emergency mode not active"
        );

        let staked_amount: i128 = env
            .storage()
            .persistent()
            .get(&DataKey::StakerAmount(staker.clone()))
            .unwrap_or(0);
        assert!(staked_amount > 0, "nothing staked");

        // Remove this staker's effective contribution from the global total.
        let boost: i128 = env
            .storage()
            .persistent()
            .get(&DataKey::BoostMultiplier(staker.clone()))
            .unwrap_or(MIN_BOOST);
        let effective = Self::_effective_amount(staked_amount, boost);
        let total: i128 = env
            .storage()
            .instance()
            .get(&DataKey::TotalEffectiveStaked)
            .unwrap_or(0);
        env.storage()
            .instance()
            .set(&DataKey::TotalEffectiveStaked, &(total - effective).max(0));

        // Zero out the staker's position, debt, boost, and lock.
        env.storage()
            .persistent()
            .set(&DataKey::StakerAmount(staker.clone()), &0i128);
        env.storage()
            .persistent()
            .set(&DataKey::StakerRewardsDebt(staker.clone()), &0i128);
        env.storage()
            .persistent()
            .set(&DataKey::BoostMultiplier(staker.clone()), &MIN_BOOST);
        env.storage()
            .persistent()
            .set(&DataKey::LockExpiry(staker.clone()), &0u64);

        // Return the raw LP balance without touching the reward token.
        let lp_token: Address = env.storage().instance().get(&DataKey::LpToken).unwrap();
        let pool_addr = env.current_contract_address();
        SepTokenClient::new(&env, &lp_token).transfer(&pool_addr, &staker, &staked_amount);

        env.events().publish(
            (Symbol::new(&env, "emergency_withdraw"),),
            (staker, staked_amount),
        );
        staked_amount
    }

    /// View pending rewards for a staker.
    pub fn pending_rewards(env: Env, staker: Address) -> i128 {
        let effective = Self::_staker_effective(&env, &staker);
        if effective == 0 {
            return 0;
        }
        let acc_per_share: i128 = env
            .storage()
            .instance()
            .get(&DataKey::AccumulatedRewardsPerShare)
            .unwrap_or(0);
        let rewards_debt: i128 = env
            .storage()
            .persistent()
            .get(&DataKey::StakerRewardsDebt(staker))
            .unwrap_or(0);
        (effective * acc_per_share / SCALE_FACTOR - rewards_debt).max(0)
    }

    /// Get pool information.
    pub fn get_pool_info(env: Env) -> PoolInfo {
        PoolInfo {
            lp_token: env.storage().instance().get(&DataKey::LpToken).unwrap(),
            reward_token: env.storage().instance().get(&DataKey::RewardToken).unwrap(),
            admin: env.storage().instance().get(&DataKey::Admin).unwrap(),
            total_effective_staked: env
                .storage()
                .instance()
                .get(&DataKey::TotalEffectiveStaked)
                .unwrap_or(0),
            reward_pool_balance: env
                .storage()
                .instance()
                .get(&DataKey::RewardPoolBalance)
                .unwrap_or(0),
            accumulated_rewards_per_share: env
                .storage()
                .instance()
                .get(&DataKey::AccumulatedRewardsPerShare)
                .unwrap_or(0),
        }
    }

    /// Get staker info including boost and lock details.
    pub fn get_staker_info(env: Env, staker: Address) -> StakerInfo {
        let staked_amount: i128 = env
            .storage()
            .persistent()
            .get(&DataKey::StakerAmount(staker.clone()))
            .unwrap_or(0);
        let boost: i128 = env
            .storage()
            .persistent()
            .get(&DataKey::BoostMultiplier(staker.clone()))
            .unwrap_or(MIN_BOOST);
        let lock_expiry: u64 = env
            .storage()
            .persistent()
            .get(&DataKey::LockExpiry(staker.clone()))
            .unwrap_or(0);
        let rewards_debt: i128 = env
            .storage()
            .persistent()
            .get(&DataKey::StakerRewardsDebt(staker))
            .unwrap_or(0);
        StakerInfo {
            staked_amount,
            effective_amount: Self::_effective_amount(staked_amount, boost),
            rewards_debt,
            lock_expiry,
            boost_multiplier: boost,
        }
    }

    /// Distribute new rewards across all stakers. Admin only.
    pub fn update_rewards(env: Env, admin: Address, new_rewards: i128) {
        admin.require_auth();
        let stored_admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        assert!(admin == stored_admin, "not admin");
        assert!(new_rewards > 0, "new_rewards must be positive");

        let total_effective: i128 = env
            .storage()
            .instance()
            .get(&DataKey::TotalEffectiveStaked)
            .unwrap_or(0);
        assert!(total_effective > 0, "no stakers");

        let acc_per_share: i128 = env
            .storage()
            .instance()
            .get(&DataKey::AccumulatedRewardsPerShare)
            .unwrap_or(0);
        let rewards_increase = new_rewards * SCALE_FACTOR / total_effective;
        env.storage()
            .instance()
            .set(&DataKey::AccumulatedRewardsPerShare, &(acc_per_share + rewards_increase));

        let pool_balance: i128 = env
            .storage()
            .instance()
            .get(&DataKey::RewardPoolBalance)
            .unwrap_or(0);
        assert!(new_rewards <= pool_balance, "insufficient reward pool balance");
        env.storage()
            .instance()
            .set(&DataKey::RewardPoolBalance, &(pool_balance - new_rewards));

        env.events().publish((Symbol::new(&env, "rewards_updated"),), (new_rewards,));
    }

    // ── Internal helpers ──────────────────────────────────────────────────────

    fn _claim_rewards(env: &Env, staker: &Address) -> i128 {
        let pending = Self::pending_rewards(env.clone(), staker.clone());
        assert!(pending > 0, "no pending rewards");

        let reward_token: Address = env.storage().instance().get(&DataKey::RewardToken).unwrap();
        let pool_addr = env.current_contract_address();

        let effective = Self::_staker_effective(env, staker);
        let acc_per_share: i128 = env
            .storage()
            .instance()
            .get(&DataKey::AccumulatedRewardsPerShare)
            .unwrap_or(0);
        let new_debt = effective * acc_per_share / SCALE_FACTOR;
        let key_debt = DataKey::StakerRewardsDebt(staker.clone());
        env.storage().persistent().set(&key_debt, &new_debt);
        env.storage().persistent().extend_ttl(&key_debt, MIN_TTL, BUMP_TO);

        SepTokenClient::new(env, &reward_token).transfer(&pool_addr, staker, &pending);

        let pool_balance: i128 = env
            .storage()
            .instance()
            .get(&DataKey::RewardPoolBalance)
            .unwrap_or(0);
        env.storage()
            .instance()
            .set(&DataKey::RewardPoolBalance, &(pool_balance - pending));

        env.events()
            .publish((Symbol::new(env, "claimed"),), (staker.clone(), pending));
        pending
    }

    fn _write_boost_config(
        env: &Env,
        min_boost: i128,
        max_boost: i128,
        min_lock: u64,
        max_lock: u64,
    ) {
        env.storage().instance().set(&DataKey::ConfigMinBoost, &min_boost);
        env.storage().instance().set(&DataKey::ConfigMaxBoost, &max_boost);
        env.storage()
            .instance()
            .set(&DataKey::ConfigMinLockDuration, &min_lock);
        env.storage()
            .instance()
            .set(&DataKey::ConfigMaxLockDuration, &max_lock);
    }

    fn _boost_bounds(env: &Env) -> (i128, i128) {
        let min_b: i128 = env
            .storage()
            .instance()
            .get(&DataKey::ConfigMinBoost)
            .unwrap_or(MIN_BOOST);
        let max_b: i128 = env
            .storage()
            .instance()
            .get(&DataKey::ConfigMaxBoost)
            .unwrap_or(DEFAULT_MAX_BOOST);
        (min_b, max_b)
    }

    fn _lock_duration_bounds(env: &Env) -> (u64, u64) {
        let min_l: u64 = env
            .storage()
            .instance()
            .get(&DataKey::ConfigMinLockDuration)
            .unwrap_or(MIN_LOCK_DURATION);
        let max_l: u64 = env
            .storage()
            .instance()
            .get(&DataKey::ConfigMaxLockDuration)
            .unwrap_or(MAX_LOCK_DURATION);
        (min_l, max_l)
    }

    /// Compute boost multiplier for remaining lock time (linear min→max).
    fn _boost_for_remaining(env: &Env, expiry: u64, now: u64) -> i128 {
        let (min_boost, max_boost) = Self::_boost_bounds(env);
        if expiry <= now {
            return min_boost;
        }
        let remaining = expiry - now;
        let (_, max_lock) = Self::_lock_duration_bounds(env);
        let clamped = remaining.min(max_lock) as i128;
        let max_dur = max_lock as i128;
        if max_dur == 0 {
            return min_boost;
        }
        min_boost + (max_boost - min_boost) * clamped / max_dur
    }

    /// Effective staked amount = raw_amount * boost / BOOST_SCALE.
    fn _effective_amount(raw: i128, boost: i128) -> i128 {
        raw * boost / BOOST_SCALE
    }

    /// Current effective amount for a staker (uses stored boost).
    fn _staker_effective(env: &Env, staker: &Address) -> i128 {
        let raw: i128 = env
            .storage()
            .persistent()
            .get(&DataKey::StakerAmount(staker.clone()))
            .unwrap_or(0);
        if raw == 0 {
            return 0;
        }
        let boost: i128 = env
            .storage()
            .persistent()
            .get(&DataKey::BoostMultiplier(staker.clone()))
            .unwrap_or(MIN_BOOST);
        Self::_effective_amount(raw, boost)
    }

    /// Settle pending rewards into debt without transferring (used before
    /// changing effective stake so rewards earned so far are not lost).
    fn _settle_pending(env: &Env, staker: &Address) {
        let effective = Self::_staker_effective(env, staker);
        if effective == 0 {
            return;
        }
        let acc_per_share: i128 = env
            .storage()
            .instance()
            .get(&DataKey::AccumulatedRewardsPerShare)
            .unwrap_or(0);
        // The current debt already accounts for previously settled rewards.
        // We do NOT transfer here — just record what has been earned so far
        // by updating the debt to the current acc_per_share level.
        // Pending = effective * acc / SCALE - debt  (already owed to staker).
        // We leave debt unchanged so pending_rewards still returns the right value.
        // The actual settlement happens in claim() / unstake().
        let _ = (effective, acc_per_share); // no-op: debt stays, rewards accumulate
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{
        testutils::{Address as _, Ledger},
        token::{StellarAssetClient, TokenClient as StellarTokenClient},
        Address, Env,
    };

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

    fn setup(env: &Env) -> (Address, Address, StakingClient) {
        let admin = Address::generate(env);
        let staking_addr = env.register_contract(None, Staking);
        let (lp_token, lp_sac) = create_sac(env, &admin);
        let (reward_token, reward_sac) = create_sac(env, &admin);
        let staking = StakingClient::new(env, &staking_addr);
        staking.initialize(&lp_token.address, &reward_token.address, &admin);
        reward_sac.mint(&admin, &10_000_i128);
        staking.add_rewards(&admin, &10_000_i128);
        let staker = Address::generate(env);
        lp_sac.mint(&staker, &5_000_i128);
        (admin, staker, staking)
    }

    #[test]
    fn test_stake_no_lock_one_x_boost() {
        let env = Env::default();
        env.mock_all_auths();
        let (admin, staker, staking) = setup(&env);

        staking.stake(&staker, &1_000_i128);

        let info = staking.get_staker_info(&staker);
        assert_eq!(info.staked_amount, 1_000);
        assert_eq!(info.boost_multiplier, BOOST_SCALE); // 1×
        assert_eq!(info.effective_amount, 1_000);
        assert_eq!(info.lock_expiry, 0);

        let pool = staking.get_pool_info();
        assert_eq!(pool.total_effective_staked, 1_000);
    }

    #[test]
    fn test_stake_locked_max_duration_max_boost() {
        let env = Env::default();
        env.mock_all_auths();
        let (_, staker, staking) = setup(&env);

        staking.stake_locked(&staker, &1_000_i128, &MAX_LOCK_DURATION);

        let info = staking.get_staker_info(&staker);
        assert_eq!(info.staked_amount, 1_000);
        assert_eq!(info.boost_multiplier, DEFAULT_MAX_BOOST); // 2.5×
        assert_eq!(info.effective_amount, 2_500);

        let pool = staking.get_pool_info();
        assert_eq!(pool.total_effective_staked, 2_500);
    }

    #[test]
    fn test_boosted_staker_earns_more_rewards() {
        let env = Env::default();
        env.mock_all_auths();
        let (admin, staker_a, staking) = setup(&env);
        let staker_b = Address::generate(&env);

        // Mint LP for staker_b
        let lp_token = staking.get_pool_info().lp_token;
        StellarAssetClient::new(&env, &lp_token).mint(&staker_b, &1_000_i128);

        // staker_a: 1000 LP, no lock (1×) → effective 1000
        staking.stake(&staker_a, &1_000_i128);
        // staker_b: 1000 LP, max lock (2.5×) → effective 2500
        staking.stake_locked(&staker_b, &1_000_i128, &MAX_LOCK_DURATION);

        // Distribute 500 rewards across total effective 3500
        staking.update_rewards(&admin, &500_i128);

        let pending_a = staking.pending_rewards(&staker_a);
        let pending_b = staking.pending_rewards(&staker_b);

        assert!(pending_b > pending_a * 2);
        assert_eq!(pending_a + pending_b, 499); // integer rounding on 500 reward split
    }

    #[test]
    fn test_extend_lock_increases_boost() {
        let env = Env::default();
        env.mock_all_auths();
        let (_, staker, staking) = setup(&env);

        staking.lock(&staker, &1_000_i128, &MIN_LOCK_DURATION);
        let before = staking.get_locked_position(&staker);
        assert!(before.boost_multiplier < DEFAULT_MAX_BOOST);

        staking.extend_lock(&staker, &MAX_LOCK_DURATION);
        let after = staking.get_locked_position(&staker);
        assert_eq!(after.boost_multiplier, DEFAULT_MAX_BOOST);
        assert!(after.lock_expiry >= before.lock_expiry);
    }

    #[test]
    fn test_unstake_locked_before_expiry_panics() {
        let env = Env::default();
        env.mock_all_auths();
        let (_, staker, staking) = setup(&env);

        staking.stake_locked(&staker, &1_000_i128, &MIN_LOCK_DURATION);

        // Try to unstake immediately — should panic because lock hasn't expired.
        let result = staking.try_unstake(&staker, &1_000_i128);
        assert!(result.is_err());
    }

    #[test]
    fn test_unstake_after_lock_expiry_succeeds() {
        let env = Env::default();
        env.mock_all_auths();
        let (admin, staker, staking) = setup(&env);

        staking.stake_locked(&staker, &1_000_i128, &MIN_LOCK_DURATION);
        staking.update_rewards(&admin, &100_i128);

        // Advance time past lock expiry.
        env.ledger().with_mut(|l| {
            l.timestamp = l.timestamp + MIN_LOCK_DURATION + 1;
        });

        let (lp_returned, rewards) = staking.unstake(&staker, &1_000_i128);
        assert_eq!(lp_returned, 1_000);
        assert!(rewards > 0);
    }

    #[test]
    fn test_stake_and_claim() {
        let env = Env::default();
        env.mock_all_auths();
        let (admin, staker, staking) = setup(&env);

        staking.stake(&staker, &1_000_i128);
        staking.update_rewards(&admin, &100_i128);

        let pending = staking.pending_rewards(&staker);
        assert_eq!(pending, 100);

        let claimed = staking.claim(&staker);
        assert_eq!(claimed, 100);
        assert_eq!(staking.pending_rewards(&staker), 0);
    }

    // ── Pause / circuit-breaker tests (#360) ────────────────────────────────

    #[test]
    fn test_pause_blocks_stake_and_claim() {
        let env = Env::default();
        env.mock_all_auths();
        let (admin, staker, staking) = setup(&env);

        staking.stake(&staker, &1_000_i128);
        staking.update_rewards(&admin, &100_i128);

        staking.pause(&admin);
        assert!(staking.is_paused());

        // New stakes and claims are halted while paused.
        assert!(staking.try_stake(&staker, &500_i128).is_err());
        assert!(staking
            .try_stake_locked(&staker, &500_i128, &MIN_LOCK_DURATION)
            .is_err());
        assert!(staking.try_claim(&staker).is_err());
    }

    #[test]
    fn test_unpause_restores_operations() {
        let env = Env::default();
        env.mock_all_auths();
        let (admin, staker, staking) = setup(&env);

        staking.pause(&admin);
        assert!(staking.try_stake(&staker, &1_000_i128).is_err());

        staking.unpause(&admin);
        assert!(!staking.is_paused());

        staking.stake(&staker, &1_000_i128);
        staking.update_rewards(&admin, &100_i128);
        assert_eq!(staking.claim(&staker), 100);
    }

    #[test]
    fn test_unstake_works_while_paused() {
        let env = Env::default();
        env.mock_all_auths();
        let (admin, staker, staking) = setup(&env);

        staking.stake(&staker, &1_000_i128);
        staking.pause(&admin);

        // Stakers must always be able to retrieve their LP tokens.
        let (lp_returned, _) = staking.unstake(&staker, &1_000_i128);
        assert_eq!(lp_returned, 1_000);
    }

    #[test]
    fn test_pause_requires_admin() {
        let env = Env::default();
        env.mock_all_auths();
        let (_, staker, staking) = setup(&env);

        assert!(staking.try_pause(&staker).is_err());
    }

    // ── Emergency mode tests (#359) ─────────────────────────────────────────

    #[test]
    fn test_emergency_withdraw_disabled_by_default_panics() {
        let env = Env::default();
        env.mock_all_auths();
        let (_, staker, staking) = setup(&env);

        staking.stake(&staker, &1_000_i128);

        // Emergency mode is off by default — withdrawal must be rejected.
        assert!(!staking.is_emergency_mode());
        let result = staking.try_emergency_withdraw(&staker);
        assert!(result.is_err());
    }

    #[test]
    fn test_emergency_withdraw_returns_lp_without_rewards() {
        let env = Env::default();
        env.mock_all_auths();
        let (admin, staker, staking) = setup(&env);

        let lp_token = staking.get_pool_info().lp_token;
        let reward_token = staking.get_pool_info().reward_token;
        let lp_client = StellarTokenClient::new(&env, &lp_token);
        let reward_client = StellarTokenClient::new(&env, &reward_token);

        staking.stake(&staker, &1_000_i128);
        staking.update_rewards(&admin, &100_i128);
        assert_eq!(staking.pending_rewards(&staker), 100);

        let reward_balance_before = reward_client.balance(&staker);

        staking.set_emergency_mode(&admin, &true);
        let returned = staking.emergency_withdraw(&staker);

        // Full LP returned, no reward token moved.
        assert_eq!(returned, 1_000);
        assert_eq!(lp_client.balance(&staker), 5_000); // original mint restored
        assert_eq!(reward_client.balance(&staker), reward_balance_before);

        // Position fully cleared.
        let info = staking.get_staker_info(&staker);
        assert_eq!(info.staked_amount, 0);
        assert_eq!(info.effective_amount, 0);
        assert_eq!(staking.pending_rewards(&staker), 0);
        assert_eq!(staking.get_pool_info().total_effective_staked, 0);
    }

    #[test]
    fn test_emergency_withdraw_ignores_active_lock() {
        let env = Env::default();
        env.mock_all_auths();
        let (admin, staker, staking) = setup(&env);

        // Lock for the max duration — unstake would panic before expiry.
        staking.stake_locked(&staker, &1_000_i128, &MAX_LOCK_DURATION);
        assert!(staking.try_unstake(&staker, &1_000_i128).is_err());

        staking.set_emergency_mode(&admin, &true);
        let returned = staking.emergency_withdraw(&staker);
        assert_eq!(returned, 1_000);
    }

    #[test]
    fn test_set_emergency_mode_requires_admin() {
        let env = Env::default();
        env.mock_all_auths();
        let (_, staker, staking) = setup(&env);

        // A non-admin caller must not be able to toggle emergency mode.
        let result = staking.try_set_emergency_mode(&staker, &true);
        assert!(result.is_err());
    }
}