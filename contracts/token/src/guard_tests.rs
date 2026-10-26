//! LP Token Input Bound and Over-Locking Regression Tests (#1188).
//!
//! Asserts that:
//! 1. `mint` rejects `amount <= 0` (0, -1, i128::MIN) -> `InvalidAmount`.
//! 2. `burn` rejects `amount <= 0` (0, -1, i128::MIN) -> `InvalidAmount`, and burn of balance + 1 -> `InsufficientBalance`.
//! 3. `lock` rejects `amount <= 0` -> `InvalidAmount`.
//! 4. `lock` rejects locking beyond unlocked balance (`bal - locked < amount`) -> `InsufficientBalance`.
//! 5. `unlock` rejects `amount <= 0` -> `InvalidAmount`.
//! 6. `unlock` rejects unlocking more than locked balance -> `InsufficientLocked`.
//! 7. `migrate_legacy_lock` rejects `amount <= 0` -> `InvalidAmount`.
//! 8. `migrate_legacy_lock` rejects `amount > total_locked` -> `MigrationOverflow`.
//! 9. `propose_admin` rejects caller when caller does not match stored admin -> `Unauthorized`.

extern crate std;

use super::*;
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    Address, Env, String,
};

struct TokenFixture {
    env: Env,
    admin: Address,
    locker: Address,
    user: Address,
    token_addr: Address,
}

impl TokenFixture {
    fn new() -> Self {
        let env = Env::default();
        env.budget().reset_unlimited();
        env.mock_all_auths();
        env.ledger().set_timestamp(1_000);

        let admin = Address::generate(&env);
        let locker = Address::generate(&env);
        let user = Address::generate(&env);

        let token_addr = env.register_contract(None, LpToken);
        let client = LpTokenClient::new(&env, &token_addr);

        client.initialize(
            &admin,
            &String::from_str(&env, "Pool Shares"),
            &String::from_str(&env, "SHARE"),
            &7,
        );
        client.set_locker(&locker);

        Self {
            env,
            admin,
            locker,
            user,
            token_addr,
        }
    }

    fn client(&self) -> LpTokenClient<'_> {
        LpTokenClient::new(&self.env, &self.token_addr)
    }
}

// =========================================================================
// 1. mint: amount <= 0 guard (0, -1, i128::MIN)
// =========================================================================

#[test]
fn test_mint_zero_or_negative_rejected() {
    let f = TokenFixture::new();
    let client = f.client();

    let res0 = client.try_mint(&f.user, &0);
    assert_eq!(res0, Err(Ok(TokenError::InvalidAmount)));

    let res_neg = client.try_mint(&f.user, &-1);
    assert_eq!(res_neg, Err(Ok(TokenError::InvalidAmount)));

    let res_min = client.try_mint(&f.user, &i128::MIN);
    assert_eq!(res_min, Err(Ok(TokenError::InvalidAmount)));

    // Storage invariance
    assert_eq!(client.balance(&f.user), 0);
    assert_eq!(client.total_supply(), 0);
}

// =========================================================================
// 2. burn: amount <= 0 and insufficient balance guards
// =========================================================================

#[test]
fn test_burn_zero_or_negative_rejected() {
    let f = TokenFixture::new();
    let client = f.client();
    client.mint(&f.user, &10_000);

    let res0 = client.try_burn(&f.user, &0);
    assert_eq!(res0, Err(Ok(TokenError::InvalidAmount)));

    let res_neg = client.try_burn(&f.user, &-1);
    assert_eq!(res_neg, Err(Ok(TokenError::InvalidAmount)));

    let res_min = client.try_burn(&f.user, &i128::MIN);
    assert_eq!(res_min, Err(Ok(TokenError::InvalidAmount)));

    // Invariance: balance and supply untouched
    assert_eq!(client.balance(&f.user), 10_000);
    assert_eq!(client.total_supply(), 10_000);
}

#[test]
fn test_burn_insufficient_balance_rejected() {
    let f = TokenFixture::new();
    let client = f.client();
    client.mint(&f.user, &5_000);

    // Exact balance succeeds
    let ok = client.try_burn(&f.user, &5_000);
    assert!(ok.is_ok());
    assert_eq!(client.balance(&f.user), 0);

    // Balance + 1 fails
    let res = client.try_burn(&f.user, &1);
    assert_eq!(res, Err(Ok(TokenError::InsufficientBalance)));
}

// =========================================================================
// 3. lock: amount <= 0 guard
// =========================================================================

#[test]
fn test_lock_zero_or_negative_rejected() {
    let f = TokenFixture::new();
    let client = f.client();
    client.mint(&f.user, &10_000);

    let res0 = client.try_lock(&f.user, &0);
    assert_eq!(res0, Err(Ok(TokenError::InvalidAmount)));

    let res_neg = client.try_lock(&f.user, &-1);
    assert_eq!(res_neg, Err(Ok(TokenError::InvalidAmount)));

    let res_min = client.try_lock(&f.user, &i128::MIN);
    assert_eq!(res_min, Err(Ok(TokenError::InvalidAmount)));

    assert_eq!(client.locked_balance(&f.user), 0);
}

// =========================================================================
// 4. lock: over-locking beyond unlocked balance (bal - locked < amount)
// =========================================================================

#[test]
fn test_lock_beyond_unlocked_balance_rejected() {
    let f = TokenFixture::new();
    let client = f.client();
    client.mint(&f.user, &10_000);

    // Lock 6,000 of 10,000 -> succeeds
    client.lock(&f.user, &6_000);
    assert_eq!(client.locked_balance(&f.user), 6_000);

    // Remaining unlocked is 4,000. Locking 4,001 fails
    let res = client.try_lock(&f.user, &4_001);
    assert_eq!(res, Err(Ok(TokenError::InsufficientBalance)));

    // Locking exact 4,000 succeeds
    let ok = client.try_lock(&f.user, &4_000);
    assert!(ok.is_ok());
    assert_eq!(client.locked_balance(&f.user), 10_000);
}

// =========================================================================
// 5. unlock: amount <= 0 guard
// =========================================================================

#[test]
fn test_unlock_zero_or_negative_rejected() {
    let f = TokenFixture::new();
    let client = f.client();
    client.mint(&f.user, &10_000);
    client.lock(&f.user, &5_000);

    let res0 = client.try_unlock(&f.user, &f.locker, &0);
    assert_eq!(res0, Err(Ok(TokenError::InvalidAmount)));

    let res_neg = client.try_unlock(&f.user, &f.locker, &-1);
    assert_eq!(res_neg, Err(Ok(TokenError::InvalidAmount)));

    let res_min = client.try_unlock(&f.user, &f.locker, &i128::MIN);
    assert_eq!(res_min, Err(Ok(TokenError::InvalidAmount)));

    assert_eq!(client.locked_balance(&f.user), 5_000);
}

// =========================================================================
// 6. unlock: InsufficientLocked (more than locked balance)
// =========================================================================

#[test]
fn test_unlock_more_than_locked_rejected() {
    let f = TokenFixture::new();
    let client = f.client();
    client.mint(&f.user, &10_000);
    client.lock(&f.user, &5_000);

    // Unlocking 5,001 fails
    let res = client.try_unlock(&f.user, &f.locker, &5_001);
    assert_eq!(res, Err(Ok(TokenError::InsufficientLocked)));

    // Exact 5,000 succeeds
    let ok = client.try_unlock(&f.user, &f.locker, &5_000);
    assert!(ok.is_ok());
    assert_eq!(client.locked_balance(&f.user), 0);
}

// =========================================================================
// 7. migrate_legacy_lock: amount <= 0 guard
// =========================================================================

#[test]
fn test_migrate_legacy_lock_zero_or_negative_rejected() {
    let f = TokenFixture::new();
    let client = f.client();

    let res0 = client.try_migrate_legacy_lock(&f.user, &f.locker, &0);
    assert_eq!(res0, Err(Ok(TokenError::InvalidAmount)));

    let res_neg = client.try_migrate_legacy_lock(&f.user, &f.locker, &-1);
    assert_eq!(res_neg, Err(Ok(TokenError::InvalidAmount)));

    let res_min = client.try_migrate_legacy_lock(&f.user, &f.locker, &i128::MIN);
    assert_eq!(res_min, Err(Ok(TokenError::InvalidAmount)));
}

// =========================================================================
// 8. migrate_legacy_lock: MigrationOverflow guard (amount > total_locked)
// =========================================================================

#[test]
fn test_migrate_legacy_lock_overflow_rejected() {
    let f = TokenFixture::new();
    let client = f.client();

    // Total locked is initially 0
    let res = client.try_migrate_legacy_lock(&f.user, &f.locker, &100);
    assert_eq!(res, Err(Ok(TokenError::MigrationOverflow)));
}

// =========================================================================
// 9. propose_admin: stored-admin check (line 720)
// =========================================================================

#[test]
fn test_propose_admin_non_admin_caller_rejected() {
    let f = TokenFixture::new();
    let client = f.client();

    let stranger = Address::generate(&f.env);
    let nominee = Address::generate(&f.env);

    // Caller is stranger, not stored admin -> Unauthorized
    let res = client.try_propose_admin(&stranger, &nominee);
    assert_eq!(res, Err(Ok(TokenError::Unauthorized)));

    // True admin succeeds
    let ok = client.try_propose_admin(&f.admin, &nominee);
    assert!(ok.is_ok());
}
