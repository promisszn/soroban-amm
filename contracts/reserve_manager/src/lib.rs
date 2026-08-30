//! Liquidity reserve management contract.
//!
//! Tracks protocol-wide minimum liquidity requirements for pool pairs and
//! exposes a `check_reserves` guard that other contracts can call before
//! processing withdrawals or rebalancing operations.
//!
//! Governance is a single address that may update requirements. The address
//! can be a multisig or DAO contract for on-chain governance.
//!
//! Both pool kinds in this workspace are supported: constant-product V2 pools
//! (`contracts/amm`) and concentrated-liquidity pools
//! (`contracts/concentrated_liquidity`). V2 reserves come from the pool's
//! `get_info()`; CL pools have no such function and no scalar reserves, so
//! their reserves are the token balances the pool contract actually holds.
//! `check_reserves` auto-detects which path to take, and governance may record
//! a pool's kind with `set_pool_kind` to skip the probe.
//!
//! Flow:
//!   1. Deploy this contract.
//!   2. Call `initialize` with the governance address and the factory address.
//!   3. Governance calls `set_min_reserve` to configure per-pair requirements.
//!   4. Optionally, governance calls `set_pool_kind` to record a pool's kind.
//!   5. Any caller uses `check_reserves` to verify a pool is compliant.
//!   6. Governance may call `transfer_governance` to hand off control.

#![no_std]

use soroban_sdk::{contract, contractclient, contractimpl, contracttype, Address, Env};

// ── External contract interfaces ─────────────────────────────────────────────

/// Subset of the AMM pool interface needed to read current reserves.
#[contractclient(name = "AmmPoolClient")]
pub trait AmmPoolInterface {
    fn get_info(env: Env) -> PoolInfo;
}

/// Subset of the concentrated-liquidity pool interface needed to identify its
/// token pair. CL pools expose no `get_info`, and their liquidity is spread
/// across ticks rather than held as a pair of scalar reserves, so reserves are
/// derived from the pool's actual token balances instead.
#[contractclient(name = "ClPoolClient")]
pub trait ClPoolInterface {
    fn get_tokens(env: Env) -> (Address, Address);
}

/// Subset of the SEP-41 token interface needed to read a holder's balance.
#[contractclient(name = "TokenBalanceClient")]
pub trait TokenBalanceInterface {
    fn balance(env: Env, id: Address) -> i128;
}

/// Which pool implementation a given address is.
///
/// Recorded per pool address by governance via `set_pool_kind`. When a pool has
/// no recorded kind, `check_reserves` auto-detects it by trying the V2
/// `get_info` path first and falling back to the CL path.
#[contracttype]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PoolKind {
    /// Constant-product V2 pool (`contracts/amm`), exposing `get_info`.
    Amm,
    /// Tick-based concentrated-liquidity pool (`contracts/concentrated_liquidity`).
    ConcentratedLiquidity,
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

// ── Storage keys ─────────────────────────────────────────────────────────────

#[contracttype]
pub enum DataKey {
    Governance,
    Factory,
    /// Normalized (smaller_addr, larger_addr) → ReserveRequirement.
    MinReserve(Address, Address),
    /// Pool address → PoolKind. Optional; absence means "auto-detect".
    PoolKind(Address),
}

// ── Contract ──────────────────────────────────────────────────────────────────

#[contract]
pub struct ReserveManager;

#[contractimpl]
impl ReserveManager {
    // ── Setup ─────────────────────────────────────────────────────────────────

    /// One-time setup. `governance` is the only address permitted to call
    /// `set_min_reserve` and `transfer_governance`.
    pub fn initialize(env: Env, governance: Address, factory: Address) {
        assert!(
            !env.storage().instance().has(&DataKey::Governance),
            "already initialized"
        );
        env.storage().instance().set(&DataKey::Governance, &governance);
        env.storage().instance().set(&DataKey::Factory, &factory);
    }

    // ── Governance ────────────────────────────────────────────────────────────

    /// Transfer governance to a new address. Requires current governance auth.
    pub fn transfer_governance(env: Env, new_governance: Address) {
        let gov: Address = env.storage().instance().get(&DataKey::Governance).unwrap();
        gov.require_auth();
        env.storage().instance().set(&DataKey::Governance, &new_governance);
    }

    // ── Reserve requirements ──────────────────────────────────────────────────

    /// Set the minimum reserve amounts for a token pair.
    ///
    /// Requires governance auth. Token order is normalised: the pair is stored
    /// with the lexicographically smaller address first so that lookups are
    /// order-independent.
    ///
    /// The requirement is keyed by token pair, not by pool address, and applies
    /// uniformly to both pool kinds. For a V2 pool the minimums are compared
    /// against `get_info()`'s reserves; for a concentrated-liquidity pool they
    /// are compared against the token balances the pool actually holds. A pair
    /// configured here therefore constrains every pool trading it, whichever
    /// implementation the pool uses.
    ///
    /// Set both values to 0 to remove a requirement.
    pub fn set_min_reserve(
        env: Env,
        token_a: Address,
        token_b: Address,
        min_reserve_a: i128,
        min_reserve_b: i128,
    ) {
        let gov: Address = env.storage().instance().get(&DataKey::Governance).unwrap();
        gov.require_auth();
        assert!(min_reserve_a >= 0, "min_reserve_a must be non-negative");
        assert!(min_reserve_b >= 0, "min_reserve_b must be non-negative");

        let (ta, tb) = Self::normalize(token_a, token_b);
        let req = ReserveRequirement { min_reserve_a, min_reserve_b };
        env.storage()
            .instance()
            .set(&DataKey::MinReserve(ta, tb), &req);
    }

    /// Return the minimum reserve requirement for a pair, or (0, 0) if none.
    ///
    /// The value is keyed by token pair and is independent of which pool kind
    /// will eventually be checked against it — see [`Self::set_min_reserve`].
    pub fn get_min_reserve(
        env: Env,
        token_a: Address,
        token_b: Address,
    ) -> ReserveRequirement {
        let (ta, tb) = Self::normalize(token_a, token_b);
        env.storage()
            .instance()
            .get(&DataKey::MinReserve(ta, tb))
            .unwrap_or(ReserveRequirement {
                min_reserve_a: 0,
                min_reserve_b: 0,
            })
    }

    // ── Compliance checks ─────────────────────────────────────────────────────

    /// Check whether a pool's current reserves satisfy the registered minimums.
    ///
    /// Works for both pool kinds:
    ///
    /// * **V2 (`PoolKind::Amm`)** — reserves are read from the pool's
    ///   `get_info()`, exactly as before.
    /// * **Concentrated liquidity (`PoolKind::ConcentratedLiquidity`)** — CL
    ///   pools have no `get_info` and no scalar reserves (liquidity is spread
    ///   across ticks, and only the in-range slice backs the current price), so
    ///   "reserves" are defined as the token balances the pool contract
    ///   actually holds: the SEP-41 `balance()` of each token in
    ///   `get_tokens()`, queried against the pool's own address.
    ///
    /// When the pool has no recorded kind, the V2 path is tried first and the
    /// CL path is used if it fails, so callers need not register a kind for
    /// `check_reserves` to work. Registering one via `set_pool_kind` skips the
    /// failed probe and its wasted cross-contract call.
    ///
    /// Returns `true` if the pool meets or exceeds its requirements, or if no
    /// requirement has been set for that pair. Returns `false` otherwise.
    ///
    /// Does not modify any state.
    pub fn check_reserves(env: Env, pool: Address) -> bool {
        let (token_a, token_b, reserve_a, reserve_b) = match Self::pool_kind_of(&env, &pool) {
            Some(PoolKind::Amm) => Self::read_amm_reserves(&env, &pool),
            Some(PoolKind::ConcentratedLiquidity) => Self::read_balance_reserves(&env, &pool),
            // Unregistered: probe the V2 shape, fall back to token balances.
            None => match AmmPoolClient::new(&env, &pool).try_get_info() {
                Ok(Ok(info)) => (info.token_a, info.token_b, info.reserve_a, info.reserve_b),
                _ => Self::read_balance_reserves(&env, &pool),
            },
        };

        let (ta, tb) = Self::normalize(token_a.clone(), token_b.clone());
        let req: ReserveRequirement = env
            .storage()
            .instance()
            .get(&DataKey::MinReserve(ta.clone(), tb))
            .unwrap_or(ReserveRequirement {
                min_reserve_a: 0,
                min_reserve_b: 0,
            });

        // The requirement is stored against the normalized pair, so align the
        // observed reserves with that same ordering before comparing.
        let (min_a, min_b) = if ta == token_a {
            (req.min_reserve_a, req.min_reserve_b)
        } else {
            (req.min_reserve_b, req.min_reserve_a)
        };

        reserve_a >= min_a && reserve_b >= min_b
    }

    /// Record which implementation `pool` is, so `check_reserves` can dispatch
    /// without probing. Requires governance auth.
    ///
    /// This is optional: `check_reserves` auto-detects unregistered pools.
    /// Registering a kind only avoids the cost of a failed `get_info` probe.
    pub fn set_pool_kind(env: Env, pool: Address, kind: PoolKind) {
        let gov: Address = env.storage().instance().get(&DataKey::Governance).unwrap();
        gov.require_auth();
        env.storage()
            .instance()
            .set(&DataKey::PoolKind(pool), &kind);
    }

    /// Return the recorded kind for `pool`, or `None` if it is auto-detected.
    pub fn get_pool_kind(env: Env, pool: Address) -> Option<PoolKind> {
        Self::pool_kind_of(&env, &pool)
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
        if a < b { (a, b) } else { (b, a) }
    }

    /// Recorded kind for `pool`, or `None` when it should be auto-detected.
    fn pool_kind_of(env: &Env, pool: &Address) -> Option<PoolKind> {
        env.storage()
            .instance()
            .get(&DataKey::PoolKind(pool.clone()))
    }

    /// Read `(token_a, token_b, reserve_a, reserve_b)` from a V2 pool's
    /// `get_info()`.
    fn read_amm_reserves(env: &Env, pool: &Address) -> (Address, Address, i128, i128) {
        let info = AmmPoolClient::new(env, pool).get_info();
        (info.token_a, info.token_b, info.reserve_a, info.reserve_b)
    }

    /// Read `(token_a, token_b, reserve_a, reserve_b)` from a CL pool by
    /// querying the SEP-41 balance of each token held by the pool itself.
    ///
    /// This sidesteps mirroring CL's internal tick/liquidity accounting and
    /// gives a signal that is meaningful for both pool kinds: how much of each
    /// token the pool can actually pay out.
    fn read_balance_reserves(env: &Env, pool: &Address) -> (Address, Address, i128, i128) {
        let (token_a, token_b) = ClPoolClient::new(env, pool).get_tokens();
        let reserve_a = TokenBalanceClient::new(env, &token_a).balance(pool);
        let reserve_b = TokenBalanceClient::new(env, &token_b).balance(pool);
        (token_a, token_b, reserve_a, reserve_b)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use amm::AmmPool;
    use soroban_sdk::{
        testutils::Address as _,
        token::StellarAssetClient,
        Env, String,
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
        let ta = env.register_stellar_asset_contract_v2(admin.clone()).address();
        let tb = env.register_stellar_asset_contract_v2(admin.clone()).address();

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
        amm::AmmPoolClient::new(&env, &pool_addr)
            .add_liquidity(&provider, &1_000_000_i128, &1_000_000_i128, &0_i128, &u64::MAX);

        // factory_addr is not used in check_reserves, just needed for initialize.
        let factory_addr = Address::generate(&env);
        let rm_addr = env.register_contract(None, ReserveManager);
        ReserveManagerClient::new(&env, &rm_addr)
            .initialize(&governance, &factory_addr);

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
        assert!(rm.try_initialize(&gov, &factory).is_err());
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
    fn test_check_reserves_passes_with_no_requirement() {
        let s = setup();
        let rm = ReserveManagerClient::new(&s.env, &s.rm_addr);

        // No requirement set — should pass by default
        assert!(rm.check_reserves(&s.pool));
    }

    // ── Concentrated-liquidity pools (Issue #829) ────────────────────────────

    /// Deploy a CL pool on the same token pair and seed it with `amount` of
    /// each token, so its held balances stand in for "reserves".
    fn deploy_cl_pool(s: &Setup, amount: i128) -> Address {
        let admin = Address::generate(&s.env);
        // Fully qualified: the bare name would shadow `PoolKind::ConcentratedLiquidity`.
        let cl_addr = s
            .env
            .register_contract(None, concentrated_liquidity::ConcentratedLiquidity);
        concentrated_liquidity::ConcentratedLiquidityClient::new(&s.env, &cl_addr)
            .initialize(&admin, &s.ta, &s.tb, &30_i128, &0_i32, &1_i32);
        if amount > 0 {
            StellarAssetClient::new(&s.env, &s.ta).mint(&cl_addr, &amount);
            StellarAssetClient::new(&s.env, &s.tb).mint(&cl_addr, &amount);
        }
        cl_addr
    }

    #[test]
    fn test_check_reserves_cl_pool_does_not_trap() {
        // Before the fix this trapped: CL pools have no `get_info`.
        let s = setup();
        let rm = ReserveManagerClient::new(&s.env, &s.rm_addr);
        let cl = deploy_cl_pool(&s, 1_000_000);

        rm.set_min_reserve(&s.ta, &s.tb, &500_000_i128, &500_000_i128);
        assert!(rm.check_reserves(&cl));
    }

    #[test]
    fn test_check_reserves_cl_pool_below_minimum_returns_false() {
        let s = setup();
        let rm = ReserveManagerClient::new(&s.env, &s.rm_addr);
        let cl = deploy_cl_pool(&s, 100_000);

        rm.set_min_reserve(&s.ta, &s.tb, &500_000_i128, &500_000_i128);
        assert!(!rm.check_reserves(&cl));
    }

    #[test]
    fn test_check_reserves_cl_pool_exactly_at_minimum_returns_true() {
        // Mirrors the V2 semantics: the comparison is `>=`, not `>`.
        let s = setup();
        let rm = ReserveManagerClient::new(&s.env, &s.rm_addr);
        let cl = deploy_cl_pool(&s, 500_000);

        rm.set_min_reserve(&s.ta, &s.tb, &500_000_i128, &500_000_i128);
        assert!(rm.check_reserves(&cl));
    }

    #[test]
    fn test_check_reserves_cl_pool_with_no_requirement_passes() {
        let s = setup();
        let rm = ReserveManagerClient::new(&s.env, &s.rm_addr);
        let cl = deploy_cl_pool(&s, 0);

        assert!(rm.check_reserves(&cl));
    }

    #[test]
    fn test_check_reserves_respects_recorded_cl_pool_kind() {
        let s = setup();
        let rm = ReserveManagerClient::new(&s.env, &s.rm_addr);
        let cl = deploy_cl_pool(&s, 1_000_000);

        rm.set_pool_kind(&cl, &PoolKind::ConcentratedLiquidity);
        assert_eq!(rm.get_pool_kind(&cl), Some(PoolKind::ConcentratedLiquidity));

        rm.set_min_reserve(&s.ta, &s.tb, &500_000_i128, &500_000_i128);
        assert!(rm.check_reserves(&cl));

        rm.set_min_reserve(&s.ta, &s.tb, &2_000_000_i128, &2_000_000_i128);
        assert!(!rm.check_reserves(&cl));
    }

    #[test]
    fn test_check_reserves_respects_recorded_amm_pool_kind() {
        let s = setup();
        let rm = ReserveManagerClient::new(&s.env, &s.rm_addr);

        rm.set_pool_kind(&s.pool, &PoolKind::Amm);
        assert_eq!(rm.get_pool_kind(&s.pool), Some(PoolKind::Amm));

        rm.set_min_reserve(&s.ta, &s.tb, &500_000_i128, &500_000_i128);
        assert!(rm.check_reserves(&s.pool));
    }

    #[test]
    fn test_pool_kind_defaults_to_auto_detect() {
        let s = setup();
        let rm = ReserveManagerClient::new(&s.env, &s.rm_addr);
        assert_eq!(rm.get_pool_kind(&s.pool), None);
    }

    #[test]
    fn test_both_pool_kinds_share_one_pair_requirement() {
        // The requirement is keyed by token pair, not pool address, so a single
        // `set_min_reserve` constrains both pools trading that pair.
        let s = setup();
        let rm = ReserveManagerClient::new(&s.env, &s.rm_addr);
        let cl = deploy_cl_pool(&s, 1_000_000);

        rm.set_min_reserve(&s.ta, &s.tb, &500_000_i128, &500_000_i128);
        assert!(rm.check_reserves(&s.pool));
        assert!(rm.check_reserves(&cl));

        rm.set_min_reserve(&s.ta, &s.tb, &1_500_000_i128, &1_500_000_i128);
        assert!(!rm.check_reserves(&s.pool));
        assert!(!rm.check_reserves(&cl));
    }

    #[test]
    fn test_transfer_governance() {
        let s = setup();
        let rm = ReserveManagerClient::new(&s.env, &s.rm_addr);
        let new_gov = Address::generate(&s.env);

        rm.transfer_governance(&new_gov);
        assert_eq!(rm.get_governance(), new_gov);
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
    fn test_negative_min_reserve_panics() {
        let s = setup();
        let rm = ReserveManagerClient::new(&s.env, &s.rm_addr);
        assert!(rm.try_set_min_reserve(&s.ta, &s.tb, &-1_i128, &0_i128).is_err());
    }
}
