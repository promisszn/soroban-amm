//! Guard regression and unauthorized state transition tests for Staking contract (#1192).
//!
//! Asserts that each of the 11 critical authorization, state invariant, and input bounds
//! guards cannot be removed without causing tests to fail.

extern crate std;

use super::*;
use soroban_sdk::{
    testutils::{Address as _, BytesN as _, Ledger as _},
    token::{StellarAssetClient, TokenClient as StellarTokenClient},
    Address, BytesN, Env, Vec,
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

fn setup_staking(env: &Env) -> (Address, Address, Address, StakingClient<'_>) {
    env.mock_all_auths();
    let admin = Address::generate(env);
    let staker = Address::generate(env);
    let stranger = Address::generate(env);
    let staking_addr = env.register_contract(None, Staking);
    let (lp_token, lp_sac) = create_sac(env, &admin);
    let (reward_token, reward_sac) = create_sac(env, &admin);
    let staking = StakingClient::new(env, &staking_addr);

    staking.initialize(&lp_token.address, &reward_token.address, &admin);
    reward_sac.mint(&admin, &10_000_i128);
    staking.add_rewards(&admin, &10_000_i128);
    lp_sac.mint(&staker, &5_000_i128);

    (admin, staker, stranger, staking)
}

// =========================================================================
// 1. unpause: stored-admin check (line 636)
// =========================================================================

#[test]
fn test_unpause_stranger_rejected() {
    let env = Env::default();
    let (admin, _staker, stranger, staking) = setup_staking(&env);

    staking.pause(&admin);
    assert!(staking.is_paused());

    // Calling unpause with stranger's address as admin must fail with Unauthorized
    let res = staking.try_unpause(&stranger);
    assert_eq!(res, Err(Ok(StakingError::Unauthorized)));
}

#[test]
fn test_unpause_stranger_leaves_contract_paused() {
    let env = Env::default();
    let (admin, _staker, stranger, staking) = setup_staking(&env);

    staking.pause(&admin);
    let _ = staking.try_unpause(&stranger);
    // Contract must remain paused
    assert!(staking.is_paused());
}

// =========================================================================
// 2. upgrade: stored-admin check (line 710)
// =========================================================================

#[test]
fn test_upgrade_stranger_rejected() {
    let env = Env::default();
    let (_admin, _staker, stranger, staking) = setup_staking(&env);

    let dummy_hash = BytesN::random(&env);
    let res = staking.try_upgrade(&stranger, &dummy_hash);
    assert_eq!(res, Err(Ok(StakingError::Unauthorized)));
}

#[test]
fn test_upgrade_stranger_leaves_code_unchanged() {
    let env = Env::default();
    let (_admin, _staker, stranger, staking) = setup_staking(&env);

    let dummy_hash = BytesN::random(&env);
    let _ = staking.try_upgrade(&stranger, &dummy_hash);
    // Contract still responds normally to queries
    assert!(!staking.is_paused());
}

// =========================================================================
// 3, 4, 5. initialize_with_boost_config guards (lines 361, 367, 370)
// =========================================================================

#[test]
fn test_initialize_with_boost_config_already_initialized() {
    let env = Env::default();
    let (admin, _staker, _stranger, staking) = setup_staking(&env);

    let lp_token = Address::generate(&env);
    let reward_token = Address::generate(&env);

    // Contract was already initialized in setup; calling boost initializer again must fail
    let res = staking.try_initialize_with_boost_config(
        &lp_token,
        &reward_token,
        &admin,
        &1000,
        &2000,
        &3600,
        &86400,
    );
    assert_eq!(res, Err(Ok(StakingError::AlreadyInitialized)));
}

#[test]
fn test_initialize_with_boost_config_min_boost_zero() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let staking_addr = env.register_contract(None, Staking);
    let (lp_token, _) = create_sac(&env, &admin);
    let (reward_token, _) = create_sac(&env, &admin);
    let staking = StakingClient::new(&env, &staking_addr);

    let res = staking.try_initialize_with_boost_config(
        &lp_token.address,
        &reward_token.address,
        &admin,
        &0, // invalid zero min boost
        &2000,
        &3600,
        &86400,
    );
    assert_eq!(res, Err(Ok(StakingError::InvalidBoostConfig)));
}

#[test]
fn test_initialize_with_boost_config_min_boost_negative() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let staking_addr = env.register_contract(None, Staking);
    let (lp_token, _) = create_sac(&env, &admin);
    let (reward_token, _) = create_sac(&env, &admin);
    let staking = StakingClient::new(&env, &staking_addr);

    let res = staking.try_initialize_with_boost_config(
        &lp_token.address,
        &reward_token.address,
        &admin,
        &-100, // negative min boost
        &2000,
        &3600,
        &86400,
    );
    assert_eq!(res, Err(Ok(StakingError::InvalidBoostConfig)));
}

#[test]
fn test_initialize_with_boost_config_max_boost_less_than_min() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let staking_addr = env.register_contract(None, Staking);
    let (lp_token, _) = create_sac(&env, &admin);
    let (reward_token, _) = create_sac(&env, &admin);
    let staking = StakingClient::new(&env, &staking_addr);

    let res = staking.try_initialize_with_boost_config(
        &lp_token.address,
        &reward_token.address,
        &admin,
        &2000, // min > max
        &1000,
        &3600,
        &86400,
    );
    assert_eq!(res, Err(Ok(StakingError::InvalidBoostConfig)));
}

#[test]
fn test_initialize_with_boost_config_boost_exact_equal_accepted() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let staking_addr = env.register_contract(None, Staking);
    let (lp_token, _) = create_sac(&env, &admin);
    let (reward_token, _) = create_sac(&env, &admin);
    let staking = StakingClient::new(&env, &staking_addr);

    // Exact equal bounds min == max must be accepted
    let res = staking.try_initialize_with_boost_config(
        &lp_token.address,
        &reward_token.address,
        &admin,
        &1500,
        &1500,
        &3600,
        &86400,
    );
    assert!(res.is_ok());
}

#[test]
fn test_initialize_with_boost_config_min_lock_zero() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let staking_addr = env.register_contract(None, Staking);
    let (lp_token, _) = create_sac(&env, &admin);
    let (reward_token, _) = create_sac(&env, &admin);
    let staking = StakingClient::new(&env, &staking_addr);

    let res = staking.try_initialize_with_boost_config(
        &lp_token.address,
        &reward_token.address,
        &admin,
        &1000,
        &2000,
        &0, // invalid zero lock duration
        &86400,
    );
    assert_eq!(res, Err(Ok(StakingError::InvalidLockDuration)));
}

#[test]
fn test_initialize_with_boost_config_max_lock_less_than_min() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let staking_addr = env.register_contract(None, Staking);
    let (lp_token, _) = create_sac(&env, &admin);
    let (reward_token, _) = create_sac(&env, &admin);
    let staking = StakingClient::new(&env, &staking_addr);

    let res = staking.try_initialize_with_boost_config(
        &lp_token.address,
        &reward_token.address,
        &admin,
        &1000,
        &2000,
        &86400,
        &3600, // max < min
    );
    assert_eq!(res, Err(Ok(StakingError::InvalidLockDuration)));
}

#[test]
fn test_initialize_with_boost_config_lock_exact_equal_accepted() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let staking_addr = env.register_contract(None, Staking);
    let (lp_token, _) = create_sac(&env, &admin);
    let (reward_token, _) = create_sac(&env, &admin);
    let staking = StakingClient::new(&env, &staking_addr);

    // Exact equal lock bounds min == max must be accepted
    let res = staking.try_initialize_with_boost_config(
        &lp_token.address,
        &reward_token.address,
        &admin,
        &1000,
        &2000,
        &3600,
        &3600,
    );
    assert!(res.is_ok());
}

// =========================================================================
// 6. extend_lock: staked_amount <= 0 -> NothingStaked (line 472)
// =========================================================================

#[test]
fn test_extend_lock_nothing_staked_rejected() {
    let env = Env::default();
    let (_admin, _staker, stranger, staking) = setup_staking(&env);

    // Stranger has an active lock timestamp artificially set but 0 staked amount
    env.as_contract(&staking.address, || {
        env.storage()
            .persistent()
            .set(&DataKey::LockExpiry(stranger.clone()), &(env.ledger().timestamp() + 10000));
    });

    let res = staking.try_extend_lock(&stranger, &5000);
    assert_eq!(res, Err(Ok(StakingError::NothingStaked)));
}

#[test]
fn test_extend_lock_after_full_unstake_rejected() {
    let env = Env::default();
    let (_admin, staker, _stranger, staking) = setup_staking(&env);

    // Staker stakes with lock then unstakes fully
    staking.stake_locked(&staker, &1000, &10000);
    // Fast-forward past expiry
    env.ledger().with_mut(|l| l.timestamp += 10001);
    staking.unlock(&staker);

    // Re-set lock expiry in future without staking LP
    env.as_contract(&staking.address, || {
        env.storage()
            .persistent()
            .set(&DataKey::LockExpiry(staker.clone()), &(env.ledger().timestamp() + 10000));
    });

    let res = staking.try_extend_lock(&staker, &5000);
    assert_eq!(res, Err(Ok(StakingError::NothingStaked)));
}

// =========================================================================
// 7. set_max_reward_pool_balance: stored-admin check (line 1057)
// =========================================================================

#[test]
fn test_set_max_reward_pool_balance_stranger_rejected() {
    let env = Env::default();
    let (_admin, _staker, stranger, staking) = setup_staking(&env);

    let res = staking.try_set_max_reward_pool_balance(&stranger, &500_000);
    assert_eq!(res, Err(Ok(StakingError::Unauthorized)));
}

#[test]
fn test_set_max_reward_pool_balance_stranger_leaves_cap_unchanged() {
    let env = Env::default();
    let (admin, _staker, stranger, staking) = setup_staking(&env);

    staking.set_max_reward_pool_balance(&admin, &500_000);
    let _ = staking.try_set_max_reward_pool_balance(&stranger, &1_000_000);

    // Cap must remain 500_000
    let cap: i128 = env.as_contract(&staking.address, || {
        env.storage()
            .instance()
            .get(&DataKey::ConfigMaxRewardPoolBalance)
            .unwrap_or(0)
    });
    assert_eq!(cap, 500_000);
}

// =========================================================================
// 8, 9. update_rewards: stored-admin (line 1803) and new_rewards <= 0 (line 1806)
// =========================================================================

#[test]
fn test_update_rewards_stranger_rejected() {
    let env = Env::default();
    let (_admin, _staker, stranger, staking) = setup_staking(&env);

    let res = staking.try_update_rewards(&stranger, &1000);
    assert_eq!(res, Err(Ok(StakingError::Unauthorized)));
}

#[test]
fn test_update_rewards_zero_rejected() {
    let env = Env::default();
    let (admin, _staker, _stranger, staking) = setup_staking(&env);

    let res = staking.try_update_rewards(&admin, &0);
    assert_eq!(res, Err(Ok(StakingError::InvalidAmount)));
}

#[test]
fn test_update_rewards_negative_rejected() {
    let env = Env::default();
    let (admin, _staker, _stranger, staking) = setup_staking(&env);

    let res = staking.try_update_rewards(&admin, &-500);
    assert_eq!(res, Err(Ok(StakingError::InvalidAmount)));
}

// =========================================================================
// 10. emergency_withdraw: staked_amount <= 0 -> NothingStaked (line 1106)
// =========================================================================

#[test]
fn test_emergency_withdraw_nothing_staked_rejected() {
    let env = Env::default();
    let (admin, _staker, stranger, staking) = setup_staking(&env);

    staking.set_emergency_mode(&admin, &true);
    assert!(staking.is_emergency_mode());

    // Stranger has never staked; must be rejected with NothingStaked
    let res = staking.try_emergency_withdraw(&stranger);
    assert_eq!(res, Err(Ok(StakingError::NothingStaked)));
}

#[test]
fn test_emergency_withdraw_zero_staked_rejected() {
    let env = Env::default();
    let (admin, staker, _stranger, staking) = setup_staking(&env);

    // Staker stakes and unstakes completely
    staking.stake(&staker, &1000);
    staking.unstake(&staker, &1000);

    staking.set_emergency_mode(&admin, &true);
    let res = staking.try_emergency_withdraw(&staker);
    assert_eq!(res, Err(Ok(StakingError::NothingStaked)));
}

// =========================================================================
// 11. register_existing_stakers: batch > MAX_BATCH_SIZE -> BatchTooLarge (line 1547)
// =========================================================================

#[test]
fn test_register_existing_stakers_over_max_batch_rejected() {
    let env = Env::default();
    let (_admin, _staker, _stranger, staking) = setup_staking(&env);

    let mut batch = Vec::new(&env);
    for _ in 0..51 {
        batch.push_back(Address::generate(&env));
    }

    let res = staking.try_register_existing_stakers(&batch);
    assert_eq!(res, Err(Ok(StakingError::BatchTooLarge)));
}

#[test]
fn test_register_existing_stakers_max_batch_accepted() {
    let env = Env::default();
    let (_admin, _staker, _stranger, staking) = setup_staking(&env);

    let mut batch = Vec::new(&env);
    for _ in 0..50 {
        batch.push_back(Address::generate(&env));
    }

    let res = staking.try_register_existing_stakers(&batch);
    assert!(res.is_ok());
}

#[test]
fn test_register_existing_stakers_over_batch_registers_nothing() {
    let env = Env::default();
    let (_admin, _staker, _stranger, staking) = setup_staking(&env);

    let mut batch = Vec::new(&env);
    for _ in 0..51 {
        batch.push_back(Address::generate(&env));
    }

    let _ = staking.try_register_existing_stakers(&batch);
    assert_eq!(staking.get_staker_index_len(), 0);
}
