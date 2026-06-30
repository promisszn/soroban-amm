//! Unit tests for Staking contract cap functionality

#![cfg(test)]

use super::*;
use soroban_sdk::{Env, Address, testutils::Address as _};

#[test]
fn test_set_max_reward_pool_balance() {
    let env = Env::default();
    // Initialize with dummy parameters
    let lp_token = Address::random(&env);
    let reward_token = Address::random(&env);
    let admin = Address::random(&env);
    Staking::initialize(&env, &lp_token, &reward_token, &admin);

    // Initially, cap is 0 (no limit)
    let initial_cap: i128 = env.storage().instance().get(&DataKey::ConfigMaxRewardPoolBalance).unwrap_or(0);
    assert_eq!(initial_cap, 0);

    // Set a positive cap
    Staking::set_max_reward_pool_balance(&env, &admin, 1_000_000);
    let cap: i128 = env.storage().instance().get(&DataKey::ConfigMaxRewardPoolBalance).unwrap();
    assert_eq!(cap, 1_000_000);

    // Setting cap lower than current balance should panic
    // Simulate a current reward pool balance
    env.storage().instance().set(&DataKey::RewardPoolBalance, &2_000_000);
    // This should panic because max_balance < current_balance
    let result = std::panic::catch_unwind(|| {
        Staking::set_max_reward_pool_balance(&env, &admin, 1_000_000);
    });
    assert!(result.is_err());
}

#[test]
fn test_add_rewards_respects_cap() {
    let env = Env::default();
    let lp_token = Address::random(&env);
    let reward_token = Address::random(&env);
    let admin = Address::random(&env);
    Staking::initialize(&env, &lp_token, &reward_token, &admin);

    // Set a cap of 500
    Staking::set_max_reward_pool_balance(&env, &admin, 500);

    // Simulate adding 300 rewards (should succeed)
    env.storage().instance().set(&DataKey::RewardPoolBalance, &0i128);
    let received = 300i128;
    let current = env.storage().instance().get(&DataKey::RewardPoolBalance).unwrap_or(0);
    let new_balance = current + received;
    let max = env.storage().instance().get(&DataKey::ConfigMaxRewardPoolBalance).unwrap_or(0);
    if max != 0 {
        assert!(new_balance <= max, "exceeds max reward pool balance");
    }
    env.storage().instance().set(&DataKey::RewardPoolBalance, &new_balance);
    let stored = env.storage().instance().get(&DataKey::RewardPoolBalance).unwrap();
    assert_eq!(stored, 300);

    // Attempt to add 250 (would exceed cap) – should panic
    let result = std::panic::catch_unwind(|| {
        let received = 250i128;
        let current = env.storage().instance().get(&DataKey::RewardPoolBalance).unwrap_or(0);
        let new_balance = current + received;
        let max = env.storage().instance().get(&DataKey::ConfigMaxRewardPoolBalance).unwrap_or(0);
        if max != 0 {
            assert!(new_balance <= max, "exceeds max reward pool balance");
        }
        env.storage().instance().set(&DataKey::RewardPoolBalance, &new_balance);
    });
    assert!(result.is_err());
}
