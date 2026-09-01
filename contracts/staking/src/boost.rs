//! Lock-boost decay helpers (issue #699).
//!
//! Pure, storage-free functions so the decay rule is defined exactly once and
//! is trivially unit-testable without spinning up a contract `Env`. Every
//! read and write path in `lib.rs` that needs "what boost does this staker
//! actually earn right now" goes through [`current_boost`] rather than
//! reading `DataKey::BoostMultiplier` directly.

/// The boost a staker earns at time `now`, given their stored (peak) boost
/// and lock expiry.
///
/// `lock_expiry == 0` means "never locked" (always at `min_boost`, matching
/// the contract's existing convention for unlocked stakers). Otherwise, once
/// `now >= lock_expiry` the lock has expired and the boost has fully decayed
/// to `min_boost` — this is the settle-on-expiry cliff Option A chooses
/// instead of Option B's continuous linear decay (see the module-level
/// tradeoff note in `lib.rs`).
pub fn current_boost(stored_boost: i128, lock_expiry: u64, now: u64, min_boost: i128) -> i128 {
    if lock_expiry == 0 || now >= lock_expiry {
        min_boost
    } else {
        stored_boost
    }
}

/// Effective (boosted) staked amount for a raw LP balance.
///
/// Scaled the same way as the rest of the contract: `boost` is expressed in
/// `boost_scale` units (e.g. `boost_scale = 10_000` means `1.0x == 10_000`).
pub fn effective_amount(raw: i128, boost: i128, boost_scale: i128) -> i128 {
    raw * boost / boost_scale
}

/// Whether a stored boost is stale and eligible for `settle_boost`: the lock
/// has genuinely expired *and* the stored value hasn't already been settled
/// down to `min_boost`. Used to keep `settle_boost`/`settle_boost_batch`
/// permissionless-but-harmless: calling it on a live lock, or on an
/// already-settled one, is always a no-op rather than an error, so it can
/// never be used to hurt the staker it targets.
pub fn is_expired_and_stale(
    stored_boost: i128,
    lock_expiry: u64,
    now: u64,
    min_boost: i128,
) -> bool {
    lock_expiry != 0 && now >= lock_expiry && stored_boost > min_boost
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOOST_SCALE: i128 = 10_000;
    const MIN_BOOST: i128 = BOOST_SCALE;
    const MAX_BOOST: i128 = 25_000;

    #[test]
    fn never_locked_is_always_min_boost() {
        assert_eq!(current_boost(MAX_BOOST, 0, 1_000, MIN_BOOST), MIN_BOOST);
        assert_eq!(current_boost(MAX_BOOST, 0, 0, MIN_BOOST), MIN_BOOST);
    }

    #[test]
    fn active_lock_keeps_stored_boost() {
        assert_eq!(
            current_boost(MAX_BOOST, 1_000, 500, MIN_BOOST),
            MAX_BOOST,
            "now < lock_expiry must return the stored boost unchanged"
        );
    }

    #[test]
    fn boundary_at_exactly_expiry_is_decayed() {
        // now == lock_expiry is the documented cliff: "if now >= lock_expiry"
        // uses >=, so the boost is already gone in the same instant it expires.
        assert_eq!(current_boost(MAX_BOOST, 1_000, 1_000, MIN_BOOST), MIN_BOOST);
    }

    #[test]
    fn one_second_past_expiry_is_decayed() {
        assert_eq!(current_boost(MAX_BOOST, 1_000, 1_001, MIN_BOOST), MIN_BOOST);
    }

    #[test]
    fn one_second_before_expiry_is_not_decayed() {
        assert_eq!(current_boost(MAX_BOOST, 1_000, 999, MIN_BOOST), MAX_BOOST);
    }

    #[test]
    fn effective_amount_matches_existing_scaling() {
        assert_eq!(effective_amount(1_000, MAX_BOOST, BOOST_SCALE), 2_500);
        assert_eq!(effective_amount(1_000, MIN_BOOST, BOOST_SCALE), 1_000);
        assert_eq!(effective_amount(0, MAX_BOOST, BOOST_SCALE), 0);
    }

    #[test]
    fn is_expired_and_stale_only_true_for_genuinely_expired_unsettled_locks() {
        // Never locked: nothing to settle.
        assert!(!is_expired_and_stale(MIN_BOOST, 0, 1_000, MIN_BOOST));
        // Active lock: nothing to settle yet.
        assert!(!is_expired_and_stale(MAX_BOOST, 1_000, 500, MIN_BOOST));
        // Expired but already settled to min_boost: nothing left to do.
        assert!(!is_expired_and_stale(MIN_BOOST, 1_000, 2_000, MIN_BOOST));
        // Expired and still carrying the peak boost: this is the case
        // settle_boost must act on.
        assert!(is_expired_and_stale(MAX_BOOST, 1_000, 2_000, MIN_BOOST));
        // Exactly at the expiry boundary counts as expired.
        assert!(is_expired_and_stale(MAX_BOOST, 1_000, 1_000, MIN_BOOST));
    }
}
