//! Liquidity reserve management contract.
//!
//! Tracks protocol-wide minimum liquidity requirements for pool pairs and
//! exposes a `check_reserves` read-only gate for **off-chain callers**.
//! The contract is **not** wired into `amm::remove_liquidity` /
//! `amm::remove_liquidity_one_sided`; `set_min_reserve` minimums are not
//! enforced on any on-chain withdrawal path. Off-chain dashboards, bots,
//! multisig governance, and migration scripts should invoke
//! `check_reserves(pool)` against any candidate pool before triggering a
//! rebalance or migration; the return value determines whether to proceed,
//! retry, or alert. The on-chain AMM hookup is deferred to a follow-up;
//! see issue #518.
//!
//! Governance is a single address that may update requirements. The address
//! can be a multisig or DAO contract for on-chain governance.
//!
//! Flow:
//!   1. Deploy this contract.
//!   2. Call `initialize` with the governance address and the factory address.
//!   3. Governance calls `set_min_reserve` to configure per-pair requirements.
//!   4. **Off-chain** callers query `check_reserves(pool)` to gate actions
//!      that take liquidity out of the pool (rebalance, migration, ...).
//!      The AMM itself does **not** call this contract on-chain; integrating
//!      pool exits with minimum guards is the responsibility of callers
//!      (off-chain bots, multisig governance, the off-chain router).
//!   5. Governance may call `propose_governance` / `accept_governance` to
//!      securely hand off control.

#![no_std]

use soroban_sdk::{
    contract, contractclient, contracterror, contractimpl, contracttype, symbol_short, Address,
    Env, Symbol, Vec,
};

// ── Storage TTL ──────────────────────────────────────────────────────────────

/// Bump per-pair requirements when their remaining TTL drops below this.
const MIN_PERSISTENT_TTL: u32 = 172_800; // ~10 days at 5s/ledger
/// Target TTL to extend per-pair requirements to on write.
const PERSISTENT_TTL_BUMP_TO: u32 = 259_200; // ~15 days at 5s/ledger

// ── Pagination / batching ────────────────────────────────────────────────────

/// Upper bound on the number of entries a single paginated read or batch health
/// check may touch. Keeps every read path within the per-transaction resource
/// limit no matter how many pairs governance has configured.
pub const MAX_PAGE: u32 = 50;

// ── External contract interfaces ─────────────────────────────────────────────

/// Subset of the AMM pool interface needed to read current reserves.
#[contractclient(name = "AmmPoolClient")]
pub trait AmmPoolInterface {
    fn get_info(env: Env) -> PoolInfo;
}

/// Mirror of the PoolInfo struct exported by the AMM pool contract.
/// Must match the AMM's field list exactly for cross-contract deserialization.
#[contracttype]
#[derive(Debug, Clone, PartialEq)]
pub struct PoolInfo {
    pub token_a: Address,
    pub token_b: Address,
    pub reserve_a: i128,
    pub reserve_b: i128,
    pub total_shares: i128,
    pub fee_bps: i128,
    pub flash_loan_fee_bps: i128,
    pub admin: Address,
    pub fee_recipient: Address,
    pub protocol_fee_bps: i128,
    pub lp_rebate_bps: i128,
}

/// Minimum reserve requirement for a token pair.
#[contracttype]
#[derive(Debug, Clone, PartialEq)]
pub struct ReserveRequirement {
    pub min_reserve_a: i128,
    pub min_reserve_b: i128,
}

/// Structured health report for a single pool.
///
/// Every amount is expressed in the pool's own token order, so `reserve_a` and
/// `min_a` both refer to `token_a` regardless of how the requirement was
/// normalised in storage.
///
/// When a pool could not be read (see `check_reserves_batch`), `token_a` and
/// `token_b` are set to the pool address itself, the reserves and minimums are
/// zero, and `healthy` is `false`.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct ReserveReport {
    pub pool: Address,
    pub token_a: Address,
    pub token_b: Address,
    pub reserve_a: i128,
    pub reserve_b: i128,
    pub min_a: i128,
    pub min_b: i128,
    pub healthy: bool,
    /// Shortfall on each side; 0 when the side is at or above its floor.
    pub shortfall_a: i128,
    pub shortfall_b: i128,
}

// ── Storage keys ─────────────────────────────────────────────────────────────

#[contracttype]
pub enum DataKey {
    Governance,
    /// Pending governance nominee for two-step handover.
    PendingGovernance,
    Factory,
    /// Normalized (smaller_addr, larger_addr) → ReserveRequirement.
    MinReserve(Address, Address),
    /// Insertion-ordered index of every pair that currently has a non-zero
    /// requirement, stored normalised as (smaller_addr, larger_addr).
    ConfiguredPairs,
}

// ── Typed errors ─────────────────────────────────────────────────────────────

#[contracterror]
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum ReserveManagerError {
    NoPendingGovernance = 1,
    Unauthorized = 2,
    AlreadyInitialized = 3,
    NegativeReserveAmount = 4,
    /// A batch health check was handed more pools than `MAX_PAGE`.
    BatchTooLarge = 5,
}

// ── Contract ──────────────────────────────────────────────────────────────────

#[contract]
pub struct ReserveManager;

#[contractimpl]
impl ReserveManager {
    // ── Setup ─────────────────────────────────────────────────────────────────

    /// One-time setup. `governance` is the only address permitted to call
    /// `set_min_reserve` and `transfer_governance`.
    pub fn initialize(
        env: Env,
        governance: Address,
        factory: Address,
    ) -> Result<(), ReserveManagerError> {
        if env.storage().instance().has(&DataKey::Governance) {
            return Err(ReserveManagerError::AlreadyInitialized);
        }
        env.storage()
            .instance()
            .set(&DataKey::Governance, &governance);
        env.storage().instance().set(&DataKey::Factory, &factory);
        Ok(())
    }

    // ── Governance ────────────────────────────────────────────────────────────

    /// Nominate a new governance address.
    ///
    /// The nominee must call `accept_governance` to complete the two-step
    /// handover. Requires current governance auth.
    pub fn propose_governance(
        env: Env,
        current_governance: Address,
        new_governance: Address,
    ) -> Result<(), ReserveManagerError> {
        let stored: Address = env.storage().instance().get(&DataKey::Governance).unwrap();
        if current_governance != stored {
            return Err(ReserveManagerError::Unauthorized);
        }
        stored.require_auth();
        env.storage()
            .instance()
            .set(&DataKey::PendingGovernance, &Some(new_governance.clone()));
        env.events().publish(
            (Symbol::new(&env, "governance_proposed"),),
            (current_governance, new_governance),
        );
        Ok(())
    }

    /// Accept a pending governance nomination.
    ///
    /// Only the nominated address can call this, and it must authorize the
    /// transaction. On success the stored governance is updated, the pending
    /// nominee is cleared, and a `governance_transferred` event is emitted.
    pub fn accept_governance(env: Env, new_governance: Address) -> Result<(), ReserveManagerError> {
        let pending: Option<Address> = env
            .storage()
            .instance()
            .get(&DataKey::PendingGovernance)
            .unwrap_or(None);
        let nominee = pending.ok_or(ReserveManagerError::NoPendingGovernance)?;
        if new_governance != nominee {
            return Err(ReserveManagerError::Unauthorized);
        }
        new_governance.require_auth();
        env.storage()
            .instance()
            .set(&DataKey::Governance, &new_governance);
        env.storage()
            .instance()
            .set(&DataKey::PendingGovernance, &Option::<Address>::None);
        env.events().publish(
            (Symbol::new(&env, "governance_transferred"),),
            (new_governance,),
        );
        Ok(())
    }

    /// Return the pending governance nominee, if any.
    pub fn get_pending_governance(env: Env) -> Option<Address> {
        env.storage()
            .instance()
            .get(&DataKey::PendingGovernance)
            .unwrap_or(None)
    }

    // ── Reserve requirements ──────────────────────────────────────────────────

    /// Set the minimum reserve amounts for a token pair.
    ///
    /// Requires governance auth. Token order is normalised: the pair is stored
    /// with the lexicographically smaller address first so that lookups are
    /// order-independent.
    ///
    /// Set both values to 0 to remove a requirement.
    ///
    /// Per-pair requirements are held in persistent storage so each pair is an
    /// independent entry with its own TTL, rather than sharing the single
    /// instance-storage blob loaded on every invocation.
    pub fn set_min_reserve(
        env: Env,
        token_a: Address,
        token_b: Address,
        min_reserve_a: i128,
        min_reserve_b: i128,
    ) -> Result<(), ReserveManagerError> {
        let gov: Address = env.storage().instance().get(&DataKey::Governance).unwrap();
        gov.require_auth();
        if min_reserve_a < 0 || min_reserve_b < 0 {
            return Err(ReserveManagerError::NegativeReserveAmount);
        }

        let token_a_is_first = token_a < token_b;
        let (ta, tb) = Self::normalize(token_a, token_b);
        let key = DataKey::MinReserve(ta, tb);

        // Both minimums zero means "no requirement": delete the entry so it does
        // not linger, matching the documented behaviour above, and drop the pair
        // from the enumeration index so it cannot grow without bound.
        if min_reserve_a == 0 && min_reserve_b == 0 {
            env.storage().persistent().remove(&key);
            if let DataKey::MinReserve(ta, tb) = &key {
                Self::deindex_pair(&env, ta, tb);
            }
            return Ok(());
        }

        let (normalized_min_a, normalized_min_b) = if token_a_is_first {
            (min_reserve_a, min_reserve_b)
        } else {
            (min_reserve_b, min_reserve_a)
        };
        let req = ReserveRequirement {
            min_reserve_a: normalized_min_a,
            min_reserve_b: normalized_min_b,
        };
        env.storage().persistent().set(&key, &req);
        env.storage()
            .persistent()
            .extend_ttl(&key, MIN_PERSISTENT_TTL, PERSISTENT_TTL_BUMP_TO);
        if let DataKey::MinReserve(ta, tb) = &key {
            Self::index_pair(&env, ta, tb);
        }
        Ok(())
    }

    // -- Enumeration ----------------------------------------------------------

    /// Number of pairs that currently have a non-zero requirement configured.
    pub fn get_configured_pair_count(env: Env) -> u32 {
        Self::configured_pairs(&env).len()
    }

    /// Page through the configured pairs in the order they were first written.
    ///
    /// `limit` is clamped to [`MAX_PAGE`]; an `offset` at or beyond the current
    /// count yields an empty `Vec` rather than panicking. Pairs are returned
    /// normalised, i.e. the lexicographically smaller address comes first.
    pub fn list_configured_pairs(env: Env, offset: u32, limit: u32) -> Vec<(Address, Address)> {
        let pairs = Self::configured_pairs(&env);
        let count = pairs.len();
        let mut page: Vec<(Address, Address)> = Vec::new(&env);
        if offset >= count || limit == 0 {
            return page;
        }
        let end = offset.saturating_add(limit.min(MAX_PAGE)).min(count);
        for i in offset..end {
            page.push_back(pairs.get(i).unwrap());
        }
        page
    }

    /// Return the minimum reserve requirement for a pair, or (0, 0) if none.
    pub fn get_min_reserve(env: Env, token_a: Address, token_b: Address) -> ReserveRequirement {
        let (ta, tb) = Self::normalize(token_a, token_b);
        env.storage()
            .persistent()
            .get(&DataKey::MinReserve(ta, tb))
            .unwrap_or(ReserveRequirement {
                min_reserve_a: 0,
                min_reserve_b: 0,
            })
    }

    // ── Compliance checks ─────────────────────────────────────────────────────

    /// Check whether a pool's current reserves satisfy the registered minimums.
    ///
    /// Returns `true` if the pool meets or exceeds its requirements, or if no
    /// requirement has been set for that pair. Returns `false` otherwise.
    ///
    /// Does not modify any state.
    pub fn check_reserves(env: Env, pool: Address) -> bool {
        let info = AmmPoolClient::new(&env, &pool).get_info();
        let token_a_is_first = info.token_a < info.token_b;
        let (ta, tb) = if token_a_is_first {
            (info.token_a, info.token_b)
        } else {
            (info.token_b, info.token_a)
        };
        let (reserve_a, reserve_b) = if token_a_is_first {
            (info.reserve_a, info.reserve_b)
        } else {
            (info.reserve_b, info.reserve_a)
        };

        let req: ReserveRequirement = env
            .storage()
            .persistent()
            .get(&DataKey::MinReserve(ta, tb))
            .unwrap_or(ReserveRequirement {
                min_reserve_a: 0,
                min_reserve_b: 0,
            });

        reserve_a >= req.min_reserve_a && reserve_b >= req.min_reserve_b
    }

    /// Structured version of [`ReserveManager::check_reserves`] that reports the
    /// actual numbers instead of a bare boolean.
    ///
    /// `healthy` always agrees with `check_reserves` for the same pool. Like
    /// `check_reserves`, this call propagates a failure of the pool's
    /// `get_info()`; use [`ReserveManager::check_reserves_batch`] for
    /// fault-isolated reads.
    ///
    /// Does not modify any state.
    pub fn check_reserves_detailed(env: Env, pool: Address) -> ReserveReport {
        let info = AmmPoolClient::new(&env, &pool).get_info();
        Self::build_report(&env, &pool, &info)
    }

    /// Health-check up to [`MAX_PAGE`] pools in one call.
    ///
    /// The read of each pool is fault-isolated: a pool whose `get_info()` call
    /// fails (not an AMM pool, archived, panicking) is reported with
    /// `healthy: false` and zeroed amounts instead of aborting the whole batch.
    ///
    /// When at least one pool is unhealthy a `res_warn` event is emitted
    /// carrying the offending pool addresses, so keepers can subscribe rather
    /// than poll.
    ///
    /// Returns [`ReserveManagerError::BatchTooLarge`] when `pools.len()` exceeds
    /// `MAX_PAGE`; truncating silently would hide pools from a health check.
    pub fn check_reserves_batch(
        env: Env,
        pools: Vec<Address>,
    ) -> Result<Vec<ReserveReport>, ReserveManagerError> {
        if pools.len() > MAX_PAGE {
            return Err(ReserveManagerError::BatchTooLarge);
        }

        let mut reports: Vec<ReserveReport> = Vec::new(&env);
        let mut unhealthy: Vec<Address> = Vec::new(&env);

        for pool in pools.iter() {
            let report = match AmmPoolClient::new(&env, &pool).try_get_info() {
                Ok(Ok(info)) => Self::build_report(&env, &pool, &info),
                _ => Self::unreadable_report(&pool),
            };
            if !report.healthy {
                unhealthy.push_back(pool.clone());
            }
            reports.push_back(report);
        }

        if !unhealthy.is_empty() {
            env.events()
                .publish((symbol_short!("res_warn"),), (unhealthy,));
        }

        Ok(reports)
    }

    /// Return the governance address.
    pub fn get_governance(env: Env) -> Address {
        env.storage().instance().get(&DataKey::Governance).unwrap()
    }

    /// Return the factory address.
    pub fn get_factory(env: Env) -> Address {
        env.storage().instance().get(&DataKey::Factory).unwrap()
    }

    // ── Internals ─────────────────────────────────────────────────────────────

    fn normalize(a: Address, b: Address) -> (Address, Address) {
        if a < b {
            (a, b)
        } else {
            (b, a)
        }
    }

    /// Load the pair index, or an empty vector when nothing is configured yet.
    fn configured_pairs(env: &Env) -> Vec<(Address, Address)> {
        env.storage()
            .persistent()
            .get(&DataKey::ConfiguredPairs)
            .unwrap_or_else(|| Vec::new(env))
    }

    fn save_configured_pairs(env: &Env, pairs: &Vec<(Address, Address)>) {
        env.storage()
            .persistent()
            .set(&DataKey::ConfiguredPairs, pairs);
        env.storage().persistent().extend_ttl(
            &DataKey::ConfiguredPairs,
            MIN_PERSISTENT_TTL,
            PERSISTENT_TTL_BUMP_TO,
        );
    }

    /// Append a normalised pair to the index on its first write. Re-writing an
    /// existing pair is a no-op, so the index never holds duplicates.
    fn index_pair(env: &Env, token_a: &Address, token_b: &Address) {
        let mut pairs = Self::configured_pairs(env);
        for i in 0..pairs.len() {
            let (a, b) = pairs.get(i).unwrap();
            if a == *token_a && b == *token_b {
                // Already indexed: still refresh the TTL so the index does not
                // expire while the entries it points at are being kept alive.
                Self::save_configured_pairs(env, &pairs);
                return;
            }
        }
        pairs.push_back((token_a.clone(), token_b.clone()));
        Self::save_configured_pairs(env, &pairs);
    }

    /// Drop a normalised pair from the index, preserving the order of the rest.
    fn deindex_pair(env: &Env, token_a: &Address, token_b: &Address) {
        let pairs = Self::configured_pairs(env);
        for i in 0..pairs.len() {
            let (a, b) = pairs.get(i).unwrap();
            if a == *token_a && b == *token_b {
                let mut remaining = pairs.clone();
                remaining.remove(i);
                Self::save_configured_pairs(env, &remaining);
                return;
            }
        }
    }

    /// Build a report from a pool's own `PoolInfo`, expressed in the pool's
    /// token order.
    fn build_report(env: &Env, pool: &Address, info: &PoolInfo) -> ReserveReport {
        let token_a_is_first = info.token_a < info.token_b;
        let (ta, tb) = if token_a_is_first {
            (info.token_a.clone(), info.token_b.clone())
        } else {
            (info.token_b.clone(), info.token_a.clone())
        };

        let req: ReserveRequirement = env
            .storage()
            .persistent()
            .get(&DataKey::MinReserve(ta, tb))
            .unwrap_or(ReserveRequirement {
                min_reserve_a: 0,
                min_reserve_b: 0,
            });

        // Requirements are stored under the normalised pair; map them back onto
        // the pool's own token order so `min_a` always describes `token_a`.
        let (min_a, min_b) = if token_a_is_first {
            (req.min_reserve_a, req.min_reserve_b)
        } else {
            (req.min_reserve_b, req.min_reserve_a)
        };

        let shortfall_a = (min_a - info.reserve_a).max(0);
        let shortfall_b = (min_b - info.reserve_b).max(0);

        ReserveReport {
            pool: pool.clone(),
            token_a: info.token_a.clone(),
            token_b: info.token_b.clone(),
            reserve_a: info.reserve_a,
            reserve_b: info.reserve_b,
            min_a,
            min_b,
            healthy: shortfall_a == 0 && shortfall_b == 0,
            shortfall_a,
            shortfall_b,
        }
    }

    /// Placeholder report for a pool whose `get_info()` could not be read.
    ///
    /// The pool address stands in for the unknown token pair so the struct stays
    /// a plain `#[contracttype]` without optional fields.
    fn unreadable_report(pool: &Address) -> ReserveReport {
        ReserveReport {
            pool: pool.clone(),
            token_a: pool.clone(),
            token_b: pool.clone(),
            reserve_a: 0,
            reserve_b: 0,
            min_a: 0,
            min_b: 0,
            healthy: false,
            shortfall_a: 0,
            shortfall_b: 0,
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use amm::AmmPool;
    use soroban_sdk::{
        testutils::{Address as _, Events as _},
        token::StellarAssetClient,
        Env, IntoVal, String,
    };
    use token::{LpToken, LpTokenClient};

    struct Setup {
        env: Env,
        rm_addr: Address,
        pool: Address,
        ta: Address,
        tb: Address,
        governance: Address,
    }

    fn setup() -> Setup {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let governance = Address::generate(&env);

        // Deploy token pair.
        let ta = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();
        let tb = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();

        // Deploy AMM pool directly (native — avoids WASM serialization mismatches).
        let lp_addr = env.register_contract(None, LpToken);
        let pool_addr = env.register_contract(None, AmmPool);
        LpTokenClient::new(&env, &lp_addr).initialize(
            &pool_addr,
            &String::from_str(&env, "LP"),
            &String::from_str(&env, "LP"),
            &7u32,
        );
        amm::AmmPoolClient::new(&env, &pool_addr)
            .initialize(&admin, &ta, &tb, &lp_addr, &30_i128, &admin, &0_i128);

        let provider = Address::generate(&env);
        StellarAssetClient::new(&env, &ta).mint(&provider, &1_000_000_i128);
        StellarAssetClient::new(&env, &tb).mint(&provider, &1_000_000_i128);
        amm::AmmPoolClient::new(&env, &pool_addr).add_liquidity(
            &provider,
            &1_000_000_i128,
            &1_000_000_i128,
            &0_i128,
            &u64::MAX,
        );

        // factory_addr is not used in check_reserves, just needed for initialize.
        let factory_addr = Address::generate(&env);
        let rm_addr = env.register_contract(None, ReserveManager);
        ReserveManagerClient::new(&env, &rm_addr).initialize(&governance, &factory_addr);

        Setup {
            env,
            rm_addr,
            pool: pool_addr,
            ta,
            tb,
            governance,
        }
    }

    #[test]
    fn test_initialize_stores_governance_and_factory() {
        let env = Env::default();
        env.mock_all_auths();
        let gov = Address::generate(&env);
        let factory = Address::generate(&env);
        let rm_addr = env.register_contract(None, ReserveManager);
        let rm = ReserveManagerClient::new(&env, &rm_addr);
        rm.initialize(&gov, &factory);
        assert_eq!(rm.get_governance(), gov);
        assert_eq!(rm.get_factory(), factory);
    }

    #[test]
    fn test_initialize_twice_panics() {
        let env = Env::default();
        env.mock_all_auths();
        let gov = Address::generate(&env);
        let factory = Address::generate(&env);
        let rm_addr = env.register_contract(None, ReserveManager);
        let rm = ReserveManagerClient::new(&env, &rm_addr);
        rm.initialize(&gov, &factory);
        assert_eq!(
            rm.try_initialize(&gov, &factory),
            Err(Ok(ReserveManagerError::AlreadyInitialized))
        );
    }

    #[test]
    fn test_set_and_get_min_reserve() {
        let s = setup();
        let rm = ReserveManagerClient::new(&s.env, &s.rm_addr);

        rm.set_min_reserve(&s.ta, &s.tb, &500_000_i128, &300_000_i128);

        // Order-independent lookup
        let req_ab = rm.get_min_reserve(&s.ta, &s.tb);
        let req_ba = rm.get_min_reserve(&s.tb, &s.ta);

        // Reserves are stored normalised; values correspond to the normalised order
        assert_eq!(req_ab.min_reserve_a, req_ba.min_reserve_a);
        assert_eq!(req_ab.min_reserve_b, req_ba.min_reserve_b);
    }

    #[test]
    fn test_check_reserves_passes_when_above_minimum() {
        let s = setup();
        let rm = ReserveManagerClient::new(&s.env, &s.rm_addr);

        // Pool has 1_000_000 of each; set minimum below that
        rm.set_min_reserve(&s.ta, &s.tb, &500_000_i128, &500_000_i128);
        assert!(rm.check_reserves(&s.pool));
    }

    #[test]
    fn test_check_reserves_fails_when_below_minimum() {
        let s = setup();
        let rm = ReserveManagerClient::new(&s.env, &s.rm_addr);

        // Set minimum above current reserves (1_000_000)
        rm.set_min_reserve(&s.ta, &s.tb, &2_000_000_i128, &2_000_000_i128);
        assert!(!rm.check_reserves(&s.pool));
    }

    #[test]
    fn test_set_min_reserve_preserves_amounts_for_reversed_token_args() {
        let s = setup();
        let rm = ReserveManagerClient::new(&s.env, &s.rm_addr);

        let (larger_token, smaller_token) = if s.ta < s.tb {
            (&s.tb, &s.ta)
        } else {
            (&s.ta, &s.tb)
        };

        // The AMM pool reserves are 1_000_000 for both tokens. This call
        // intentionally passes the larger token first, so set_min_reserve must
        // swap the amounts before storing them under the normalized key.
        rm.set_min_reserve(larger_token, smaller_token, &2_000_000_i128, &500_000_i128);

        let req = rm.get_min_reserve(&s.ta, &s.tb);
        assert_eq!(req.min_reserve_a, 500_000);
        assert_eq!(req.min_reserve_b, 2_000_000);
        assert!(!rm.check_reserves(&s.pool));
    }

    #[test]
    fn test_check_reserves_passes_with_no_requirement() {
        let s = setup();
        let rm = ReserveManagerClient::new(&s.env, &s.rm_addr);

        // No requirement set — should pass by default
        assert!(rm.check_reserves(&s.pool));
    }

    #[test]
    fn test_propose_and_accept_governance() {
        let s = setup();
        let rm = ReserveManagerClient::new(&s.env, &s.rm_addr);
        let new_gov = Address::generate(&s.env);

        rm.propose_governance(&s.governance, &new_gov);
        assert_eq!(rm.get_pending_governance(), Some(new_gov.clone()));

        rm.accept_governance(&new_gov);
        assert_eq!(rm.get_governance(), new_gov);
        assert_eq!(rm.get_pending_governance(), None);
    }

    #[test]
    fn test_propose_governance_requires_current_governance() {
        let s = setup();
        let rm = ReserveManagerClient::new(&s.env, &s.rm_addr);
        let rando = Address::generate(&s.env);
        let new_gov = Address::generate(&s.env);

        assert!(rm.try_propose_governance(&rando, &new_gov).is_err());
    }

    #[test]
    fn test_accept_governance_requires_pending_nomination() {
        let s = setup();
        let rm = ReserveManagerClient::new(&s.env, &s.rm_addr);
        let rando = Address::generate(&s.env);

        assert!(rm.try_accept_governance(&rando).is_err());
    }

    #[test]
    fn test_accept_governance_requires_nominee() {
        let s = setup();
        let rm = ReserveManagerClient::new(&s.env, &s.rm_addr);
        let new_gov = Address::generate(&s.env);
        let other = Address::generate(&s.env);

        rm.propose_governance(&s.governance, &new_gov);
        assert!(rm.try_accept_governance(&other).is_err());
    }

    #[test]
    fn test_set_min_reserve_to_zero_removes_constraint() {
        let s = setup();
        let rm = ReserveManagerClient::new(&s.env, &s.rm_addr);

        rm.set_min_reserve(&s.ta, &s.tb, &2_000_000_i128, &2_000_000_i128);
        assert!(!rm.check_reserves(&s.pool));

        rm.set_min_reserve(&s.ta, &s.tb, &0_i128, &0_i128);
        assert!(rm.check_reserves(&s.pool));
    }

    #[test]
    fn test_set_min_reserve_to_zero_deletes_persistent_entry() {
        let s = setup();
        let rm = ReserveManagerClient::new(&s.env, &s.rm_addr);

        rm.set_min_reserve(&s.ta, &s.tb, &2_000_000_i128, &2_000_000_i128);
        let (ta, tb) = ReserveManager::normalize(s.ta.clone(), s.tb.clone());
        let key = DataKey::MinReserve(ta, tb);

        // The entry exists while a non-zero requirement is set.
        assert!(s
            .env
            .as_contract(&s.rm_addr, || s.env.storage().persistent().has(&key)));

        // Setting both minimums to zero must delete the key, not store (0, 0).
        rm.set_min_reserve(&s.ta, &s.tb, &0_i128, &0_i128);
        assert!(!s
            .env
            .as_contract(&s.rm_addr, || s.env.storage().persistent().has(&key)));
    }

    #[test]
    fn test_negative_min_reserve_panics() {
        let s = setup();
        let rm = ReserveManagerClient::new(&s.env, &s.rm_addr);
        assert_eq!(
            rm.try_set_min_reserve(&s.ta, &s.tb, &-1_i128, &0_i128),
            Err(Ok(ReserveManagerError::NegativeReserveAmount))
        );
    }

    // -- #682: pair indexing, pagination, detailed / batch reporting ----------

    /// Normalised (smaller, larger) ordering of a pair, matching storage.
    fn norm(a: &Address, b: &Address) -> (Address, Address) {
        if a < b {
            (a.clone(), b.clone())
        } else {
            (b.clone(), a.clone())
        }
    }

    #[test]
    fn test_configured_pairs_indexed_in_insertion_order_without_duplicates() {
        let s = setup();
        let rm = ReserveManagerClient::new(&s.env, &s.rm_addr);
        let tc = Address::generate(&s.env);
        let td = Address::generate(&s.env);

        rm.set_min_reserve(&s.ta, &s.tb, &1_i128, &1_i128);
        rm.set_min_reserve(&tc, &td, &2_i128, &2_i128);
        // Re-writing an existing pair (in either token order) must not duplicate.
        rm.set_min_reserve(&s.tb, &s.ta, &5_i128, &5_i128);

        assert_eq!(rm.get_configured_pair_count(), 2);
        let pairs = rm.list_configured_pairs(&0, &10);
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs.get(0).unwrap(), norm(&s.ta, &s.tb));
        assert_eq!(pairs.get(1).unwrap(), norm(&tc, &td));
    }

    #[test]
    fn test_list_configured_pairs_pagination_edges() {
        let s = setup();
        let rm = ReserveManagerClient::new(&s.env, &s.rm_addr);

        let mut expected: soroban_sdk::Vec<(Address, Address)> = soroban_sdk::Vec::new(&s.env);
        for _ in 0..5 {
            let a = Address::generate(&s.env);
            let b = Address::generate(&s.env);
            rm.set_min_reserve(&a, &b, &1_i128, &1_i128);
            expected.push_back(norm(&a, &b));
        }
        assert_eq!(rm.get_configured_pair_count(), 5);

        // Mid-range page.
        let page = rm.list_configured_pairs(&1, &2);
        assert_eq!(page.len(), 2);
        assert_eq!(page.get(0).unwrap(), expected.get(1).unwrap());
        assert_eq!(page.get(1).unwrap(), expected.get(2).unwrap());

        // offset == count and offset > count yield an empty page, not a panic.
        assert_eq!(rm.list_configured_pairs(&5, &10).len(), 0);
        assert_eq!(rm.list_configured_pairs(&99, &10).len(), 0);

        // limit == 0 yields an empty page.
        assert_eq!(rm.list_configured_pairs(&0, &0).len(), 0);

        // limit > MAX_PAGE is clamped, not rejected; only 5 pairs exist so the
        // whole set comes back.
        let all = rm.list_configured_pairs(&0, &(MAX_PAGE + 1_000));
        assert_eq!(all.len(), 5);

        // A partial trailing page is truncated to the remaining entries.
        assert_eq!(rm.list_configured_pairs(&4, &10).len(), 1);
    }

    #[test]
    fn test_setting_zero_requirement_deindexes_pair() {
        let s = setup();
        let rm = ReserveManagerClient::new(&s.env, &s.rm_addr);
        let tc = Address::generate(&s.env);
        let td = Address::generate(&s.env);

        rm.set_min_reserve(&s.ta, &s.tb, &1_i128, &1_i128);
        rm.set_min_reserve(&tc, &td, &2_i128, &2_i128);
        assert_eq!(rm.get_configured_pair_count(), 2);

        rm.set_min_reserve(&s.ta, &s.tb, &0_i128, &0_i128);
        assert_eq!(rm.get_configured_pair_count(), 1);
        let pairs = rm.list_configured_pairs(&0, &10);
        assert_eq!(pairs.get(0).unwrap(), norm(&tc, &td));

        // De-indexing an already-removed pair is a no-op.
        rm.set_min_reserve(&s.ta, &s.tb, &0_i128, &0_i128);
        assert_eq!(rm.get_configured_pair_count(), 1);
    }

    #[test]
    fn test_check_reserves_detailed_healthy_has_zero_shortfalls() {
        let s = setup();
        let rm = ReserveManagerClient::new(&s.env, &s.rm_addr);
        rm.set_min_reserve(&s.ta, &s.tb, &500_000_i128, &500_000_i128);

        let report = rm.check_reserves_detailed(&s.pool);
        assert!(report.healthy);
        assert_eq!(report.healthy, rm.check_reserves(&s.pool));
        assert_eq!(report.pool, s.pool);
        assert_eq!(report.reserve_a, 1_000_000);
        assert_eq!(report.reserve_b, 1_000_000);
        assert_eq!(report.min_a, 500_000);
        assert_eq!(report.min_b, 500_000);
        assert_eq!(report.shortfall_a, 0);
        assert_eq!(report.shortfall_b, 0);
    }

    #[test]
    fn test_check_reserves_detailed_reports_shortfall_arithmetic() {
        let s = setup();
        let rm = ReserveManagerClient::new(&s.env, &s.rm_addr);

        // Below the floor on token_a only; token_b stays comfortably above it.
        let (smaller, larger) = norm(&s.ta, &s.tb);
        rm.set_min_reserve(&smaller, &larger, &1_500_000_i128, &400_000_i128);

        let report = rm.check_reserves_detailed(&s.pool);
        assert!(!report.healthy);
        assert_eq!(report.healthy, rm.check_reserves(&s.pool));

        // The report is expressed in the pool's own token order.
        let pool_info = amm::AmmPoolClient::new(&s.env, &s.pool).get_info();
        assert_eq!(report.token_a, pool_info.token_a);
        assert_eq!(report.token_b, pool_info.token_b);
        let (exp_a, exp_b) = if pool_info.token_a == smaller {
            (1_500_000_i128, 400_000_i128)
        } else {
            (400_000_i128, 1_500_000_i128)
        };
        assert_eq!(report.min_a, exp_a);
        assert_eq!(report.min_b, exp_b);
        assert_eq!(report.shortfall_a, (exp_a - 1_000_000).max(0));
        assert_eq!(report.shortfall_b, (exp_b - 1_000_000).max(0));
    }

    #[test]
    fn test_check_reserves_detailed_without_requirement_is_healthy() {
        let s = setup();
        let rm = ReserveManagerClient::new(&s.env, &s.rm_addr);
        let report = rm.check_reserves_detailed(&s.pool);
        assert!(report.healthy);
        assert_eq!(report.min_a, 0);
        assert_eq!(report.min_b, 0);
        assert_eq!(report.shortfall_a, 0);
        assert_eq!(report.shortfall_b, 0);
    }

    #[test]
    fn test_check_reserves_batch_is_fault_isolated() {
        let s = setup();
        let rm = ReserveManagerClient::new(&s.env, &s.rm_addr);
        rm.set_min_reserve(&s.ta, &s.tb, &500_000_i128, &500_000_i128);

        // The middle entry is a plain account address, not an AMM pool.
        let not_a_pool = Address::generate(&s.env);
        let pools = soroban_sdk::vec![&s.env, s.pool.clone(), not_a_pool.clone(), s.pool.clone()];

        let reports = rm.check_reserves_batch(&pools);
        assert_eq!(reports.len(), 3);

        let bad = reports.get(1).unwrap();
        assert_eq!(bad.pool, not_a_pool);
        assert!(!bad.healthy);
        assert_eq!(bad.reserve_a, 0);
        assert_eq!(bad.reserve_b, 0);

        // Every other pool in the batch still gets a real report.
        assert!(reports.get(0).unwrap().healthy);
        assert!(reports.get(2).unwrap().healthy);
    }

    #[test]
    fn test_check_reserves_batch_emits_res_warn_with_unhealthy_pools() {
        let s = setup();
        let rm = ReserveManagerClient::new(&s.env, &s.rm_addr);
        rm.set_min_reserve(&s.ta, &s.tb, &2_000_000_i128, &2_000_000_i128);

        let pools = soroban_sdk::vec![&s.env, s.pool.clone()];
        let reports = rm.check_reserves_batch(&pools);
        assert!(!reports.get(0).unwrap().healthy);

        let events = s.env.events().all();
        let (contract, topics, data) = events.last().unwrap();
        assert_eq!(contract, s.rm_addr);
        assert_eq!(
            topics,
            soroban_sdk::vec![&s.env, symbol_short!("res_warn").into_val(&s.env)]
        );
        let (unhealthy,): (soroban_sdk::Vec<Address>,) = data.into_val(&s.env);
        assert_eq!(unhealthy, soroban_sdk::vec![&s.env, s.pool.clone()]);
    }

    #[test]
    fn test_check_reserves_batch_stays_silent_when_all_pools_healthy() {
        let s = setup();
        let rm = ReserveManagerClient::new(&s.env, &s.rm_addr);
        rm.set_min_reserve(&s.ta, &s.tb, &1_000_i128, &1_000_i128);

        let before = s.env.events().all().len();
        let reports = rm.check_reserves_batch(&soroban_sdk::vec![&s.env, s.pool.clone()]);
        assert!(reports.get(0).unwrap().healthy);
        assert_eq!(s.env.events().all().len(), before);
    }

    #[test]
    fn test_check_reserves_batch_rejects_oversized_batch() {
        let s = setup();
        let rm = ReserveManagerClient::new(&s.env, &s.rm_addr);

        let mut pools: soroban_sdk::Vec<Address> = soroban_sdk::Vec::new(&s.env);
        for _ in 0..(MAX_PAGE + 1) {
            pools.push_back(s.pool.clone());
        }
        assert_eq!(
            rm.try_check_reserves_batch(&pools),
            Err(Ok(ReserveManagerError::BatchTooLarge))
        );
    }

    #[test]
    fn test_check_reserves_batch_on_empty_input() {
        let s = setup();
        let rm = ReserveManagerClient::new(&s.env, &s.rm_addr);
        let reports = rm.check_reserves_batch(&soroban_sdk::Vec::new(&s.env));
        assert_eq!(reports.len(), 0);
    }
}
