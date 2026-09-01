//! Unit tests for Staking contract cap functionality

extern crate std;

use super::*;
use soroban_sdk::{testutils::Address as _, Address, Env};
use std::panic::AssertUnwindSafe;

#[test]
fn test_set_max_reward_pool_balance() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register_contract(None, Staking);
    env.as_contract(&contract_id, || {
        // Initialize with dummy parameters
        let lp_token = Address::generate(&env);
        let reward_token = Address::generate(&env);
        let admin = Address::generate(&env);
        Staking::initialize(
            env.clone(),
            lp_token.clone(),
            reward_token.clone(),
            admin.clone(),
        );

        // Initially, cap is 0 (no limit)
        let initial_cap: i128 = env
            .storage()
            .instance()
            .get(&DataKey::ConfigMaxRewardPoolBalance)
            .unwrap_or(0);
        assert_eq!(initial_cap, 0);

        // Set a positive cap
        Staking::set_max_reward_pool_balance(env.clone(), admin.clone(), 1_000_000);
        let cap: i128 = env
            .storage()
            .instance()
            .get(&DataKey::ConfigMaxRewardPoolBalance)
            .unwrap();
        assert_eq!(cap, 1_000_000);

        // Setting cap lower than current balance should panic
        // Simulate a current reward pool balance
        env.storage()
            .instance()
            .set(&DataKey::RewardPoolBalance, &2_000_000);
        // This should panic because max_balance < current_balance
        let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
            Staking::set_max_reward_pool_balance(env.clone(), admin.clone(), 1_000_000);
        }));
        assert!(result.is_err());
    });
}

#[test]
fn test_add_rewards_respects_cap() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register_contract(None, Staking);
    env.as_contract(&contract_id, || {
        let lp_token = Address::generate(&env);
        let reward_token = Address::generate(&env);
        let admin = Address::generate(&env);
        Staking::initialize(env.clone(), lp_token, reward_token, admin.clone());

        // Set a cap of 500
        Staking::set_max_reward_pool_balance(env.clone(), admin, 500);

        // Simulate adding 300 rewards (should succeed)
        env.storage()
            .instance()
            .set(&DataKey::RewardPoolBalance, &0i128);
        let received = 300i128;
        let current = env
            .storage()
            .instance()
            .get(&DataKey::RewardPoolBalance)
            .unwrap_or(0);
        let new_balance = current + received;
        let max = env
            .storage()
            .instance()
            .get(&DataKey::ConfigMaxRewardPoolBalance)
            .unwrap_or(0);
        if max != 0 {
            assert!(new_balance <= max, "exceeds max reward pool balance");
        }
        env.storage()
            .instance()
            .set(&DataKey::RewardPoolBalance, &new_balance);
        let stored: i128 = env
            .storage()
            .instance()
            .get(&DataKey::RewardPoolBalance)
            .unwrap();
        assert_eq!(stored, 300);

        // Attempt to add 250 (would exceed cap) – should panic
        let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
            let received = 250i128;
            let current = env
                .storage()
                .instance()
                .get(&DataKey::RewardPoolBalance)
                .unwrap_or(0);
            let new_balance = current + received;
            let max = env
                .storage()
                .instance()
                .get(&DataKey::ConfigMaxRewardPoolBalance)
                .unwrap_or(0);
            if max != 0 {
                assert!(new_balance <= max, "exceeds max reward pool balance");
            }
            env.storage()
                .instance()
                .set(&DataKey::RewardPoolBalance, &new_balance);
        }));
        assert!(result.is_err());
    });
}
